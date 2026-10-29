//! D3D12 device, composition swap chain, DirectComposition tree and Skia wiring.
//!
//! The swap chain is created once at the largest state's size; the pill animates
//! inside it, so there is never a `ResizeBuffers`. A DPI change rebuilds the chain.
//! The Snap overlay (`snap::SnapOverlay`) builds its own device and chain with the same
//! helpers, per snap.

use crate::paint::{Frame, Painter};
use dj_core::color::Rgb;
use dj_core::snap::Bitmap;
use skia_safe::gpu::{
    self, backend_render_targets,
    d3d::{BackendContext, TextureResourceInfo},
    surfaces, FlushInfo, Protected, SurfaceOrigin,
};
use skia_safe::{Color, ColorType, Surface};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use windows::core::Interface;
use windows::Win32::Foundation::{DXGI_STATUS_OCCLUDED, HWND};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device, D3D12_COMMAND_LIST_TYPE_DIRECT,
    D3D12_COMMAND_QUEUE_DESC, D3D12_RESOURCE_STATE_PRESENT,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionTarget,
    IDCompositionVisual2,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory4, IDXGISwapChain3, DXGI_ADAPTER_FLAG,
    DXGI_ADAPTER_FLAG_NONE, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_DEVICE_REMOVED,
    DXGI_ERROR_DEVICE_RESET, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

const BUFFERS: usize = 2;

type SharedGpu = Rc<RefCell<Gpu>>;

thread_local! {
    static SHARED_GPU: RefCell<Weak<RefCell<Gpu>>> = const { RefCell::new(Weak::new()) };
    static LIVE_CONTEXTS: Cell<usize> = const { Cell::new(0) };
}

fn acquire_shared<T>(
    slot: &RefCell<Weak<T>>,
    valid: impl FnOnce(&T) -> bool,
    create: impl FnOnce() -> Result<T, RenderError>,
) -> Result<Rc<T>, RenderError> {
    if let Some(owner) = slot.borrow().upgrade().filter(|owner| valid(owner)) {
        return Ok(owner);
    }
    let owner = Rc::new(create()?);
    *slot.borrow_mut() = Rc::downgrade(&owner);
    Ok(owner)
}

pub(crate) fn shared_gpu() -> Result<SharedGpu, RenderError> {
    SHARED_GPU.with(|slot| {
        acquire_shared(
            slot,
            |gpu| gpu.borrow().is_valid(),
            || Gpu::new().map(RefCell::new),
        )
    })
}

pub fn graphics_memory_counters() -> Vec<dj_core::dev_memory::Row> {
    use dj_core::dev_memory::{bytes, Row};
    let mut rows = vec![Row::resource(
        "Unique graphics contexts",
        format!("{} contexts", LIVE_CONTEXTS.with(Cell::get)),
        "Live contexts on UI thread, including independent capture/tile/preview/pins; count, not RAM.",
    )];
    SHARED_GPU.with(|slot| {
        if let Some(gpu) = slot.borrow().upgrade() {
            let usage = gpu.borrow().ctx.resource_cache_usage();
            rows.push(Row::resource(
                "Shared island/popup graphics cache",
                format!("{} GPU", bytes(Some(usage.resource_bytes as u64))),
                format!(
                    "1 shared context, {} renderers, {} cached resources; counted once. Excludes driver/upload pools; not process RAM.",
                    Rc::strong_count(&gpu) - 1,
                    usage.resource_count
                ),
            ));
        }
    });
    rows
}

#[derive(Debug)]
pub enum RenderError {
    /// The GPU device was removed or reset; rebuild the `Renderer`.
    DeviceLost,
    Setup(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceLost => write!(f, "GPU device lost"),
            Self::Setup(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for RenderError {}

pub(crate) fn setup<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> RenderError {
    move |e| RenderError::Setup(format!("{what}: {e}"))
}

/// A D3D12 device, its direct queue and a Skia context on it. The context is the first
/// field, so it drops first.
pub(crate) struct Gpu {
    pub ctx: gpu::DirectContext,
    pub queue: ID3D12CommandQueue,
    /// Kept alive for the queue and the context.
    _device: ID3D12Device,
    pub factory: IDXGIFactory4,
}

impl Gpu {
    pub fn new() -> Result<Self, RenderError> {
        // SAFETY: plain COM creation calls; every out-parameter and descriptor lives for
        // the duration of its call and all interfaces are ref-counted by the `windows`
        // crate.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory1().map_err(setup("DXGI factory"))?;
            let (adapter, device) = pick_adapter(&factory)?;
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .map_err(setup("command queue"))?;
            let backend = BackendContext {
                adapter,
                device: device.clone(),
                queue: queue.clone(),
                memory_allocator: None,
                protected_context: Protected::No,
            };
            let ctx = gpu::direct_contexts::make_d3d(&backend, None)
                .ok_or_else(|| RenderError::Setup("Skia D3D context".into()))?;
            LIVE_CONTEXTS.with(|count| count.set(count.get() + 1));
            Ok(Self {
                ctx,
                queue,
                _device: device,
                factory,
            })
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        // SAFETY: device remains owned by self throughout this read-only COM call.
        unsafe { self._device.GetDeviceRemovedReason().is_ok() }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        LIVE_CONTEXTS.with(|count| count.set(count.get() - 1));
    }
}

/// A composition swap chain on a window, its DirectComposition tree, and a Skia surface
/// per buffer. The surfaces must go before the context they were made with: owners
/// clear `surfaces` (and sync the GPU) in their `Drop` before any field drops.
pub(crate) struct Chain {
    pub surfaces: Vec<Surface>,
    pub swap_chain: IDXGISwapChain3,
    _visual: IDCompositionVisual2,
    _target: IDCompositionTarget,
    _dcomp: IDCompositionDesktopDevice,
}

impl Chain {
    /// `width`/`height` are the window's client size in device pixels.
    pub fn new(gpu: &mut Gpu, hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError> {
        let hwnd = HWND(hwnd as *mut _);
        // SAFETY: as in `Gpu::new`; the window handle belongs to this thread.
        unsafe {
            let swap_chain: IDXGISwapChain3 = gpu
                .factory
                .CreateSwapChainForComposition(
                    &gpu.queue,
                    &DXGI_SWAP_CHAIN_DESC1 {
                        Width: width,
                        Height: height,
                        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        Stereo: false.into(),
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                        BufferCount: BUFFERS as u32,
                        Scaling: DXGI_SCALING_STRETCH,
                        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                        Flags: 0,
                    },
                    None,
                )
                .map_err(setup("composition swap chain"))?
                .cast()
                .map_err(setup("IDXGISwapChain3"))?;

            let dcomp: IDCompositionDesktopDevice =
                DCompositionCreateDevice2(None::<&windows::core::IUnknown>)
                    .map_err(setup("DirectComposition device"))?;
            let target = dcomp
                .CreateTargetForHwnd(hwnd, true)
                .map_err(setup("DComp target"))?;
            let visual = dcomp.CreateVisual().map_err(setup("DComp visual"))?;
            visual
                .SetContent(&swap_chain)
                .map_err(setup("DComp content"))?;
            target.SetRoot(&visual).map_err(setup("DComp root"))?;
            dcomp.Commit().map_err(setup("DComp commit"))?;

            let mut surfaces = Vec::with_capacity(BUFFERS);
            for i in 0..BUFFERS {
                let resource = match swap_chain.GetBuffer(i as u32) {
                    Ok(resource) => resource,
                    Err(error) => {
                        surfaces.clear();
                        gpu.ctx.flush_submit_and_sync_cpu();
                        return Err(setup("swap chain buffer")(error));
                    }
                };
                let target = backend_render_targets::make_d3d(
                    (width as i32, height as i32),
                    &TextureResourceInfo {
                        resource,
                        alloc: None,
                        resource_state: D3D12_RESOURCE_STATE_PRESENT,
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        sample_count: 1,
                        level_count: 1,
                        sample_quality_pattern: 0,
                        protected: Protected::No,
                    },
                );
                let surface = surfaces::wrap_backend_render_target(
                    &mut gpu.ctx,
                    &target,
                    SurfaceOrigin::TopLeft,
                    ColorType::BGRA8888,
                    None,
                    None,
                );
                let Some(surface) = surface else {
                    surfaces.clear();
                    gpu.ctx.flush_submit_and_sync_cpu();
                    return Err(RenderError::Setup("wrap swap chain buffer".into()));
                };
                surfaces.push(surface);
            }
            Ok(Self {
                surfaces,
                swap_chain,
                _visual: visual,
                _target: target,
                _dcomp: dcomp,
            })
        }
    }

    /// The surface to draw the next frame on.
    pub fn back_buffer(&mut self) -> &mut Surface {
        // SAFETY: the swap chain is alive for `self`'s lifetime.
        let index = unsafe { self.swap_chain.GetCurrentBackBufferIndex() } as usize;
        &mut self.surfaces[index]
    }

    /// Flushes the back buffer and presents it (`Present(1)`: paced to vsync).
    pub fn present(&mut self, ctx: &mut gpu::DirectContext) -> Result<(), RenderError> {
        let surface = self.back_buffer();
        ctx.flush_surface_with_access(
            surface,
            skia_safe::surface::BackendSurfaceAccess::Present,
            &FlushInfo::default(),
        );
        ctx.submit(gpu::SubmitInfo::default());
        // SAFETY: as above.
        let hr = unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) };
        if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
            return Err(RenderError::DeviceLost);
        }
        if hr.is_err() && hr != DXGI_STATUS_OCCLUDED {
            return Err(RenderError::Setup(format!("Present failed: {hr:?}")));
        }
        Ok(())
    }
}

pub struct Renderer {
    // Field order is drop order: Skia objects go before the D3D objects they use, and
    // `Drop` clears the chain's surfaces before any of them.
    painter: Painter,
    chain: Chain,
    gpu: SharedGpu,
    width: u32,
    height: u32,
}

impl Renderer {
    /// `width`/`height` are the window's client size in device pixels.
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError> {
        let painter = Painter::new().map_err(RenderError::Setup)?;
        let gpu = shared_gpu()?;
        let chain = Chain::new(&mut gpu.borrow_mut(), hwnd, width, height)?;
        Ok(Self {
            painter,
            gpu,
            chain,
            width,
            height,
        })
    }

    pub fn memory_counters(&self) -> Vec<dj_core::dev_memory::Row> {
        use dj_core::dev_memory::{bytes, Row};
        vec![
            Row::resource(
                "Island renderer",
                format!(
                    "~{} GPU",
                    bytes(Some(
                        u64::from(self.width)
                            * u64::from(self.height)
                            * 4
                            * self.chain.surfaces.len() as u64
                    ))
                ),
                format!(
                    "{} display buffers draw island; pixel estimate excludes alignment.",
                    self.chain.surfaces.len()
                ),
            ),
            self.painter.fonts.memory_counters(),
            Row::resource(
                "Music artwork",
                format!(
                    "~{} pixels",
                    bytes(Some(
                        self.painter
                            .art
                            .as_ref()
                            .map_or(0, |i| i.width() as u64 * i.height() as u64 * 4)
                    ))
                ),
                "Album cover shown in player; estimate excludes decoded copies/cache.",
            ),
            Row::resource(
                "Screenshot thumbnail",
                format!(
                    "~{} pixels",
                    bytes(Some(
                        self.painter
                            .snap_shot
                            .as_ref()
                            .map_or(0, |i| i.width() as u64 * i.height() as u64 * 4)
                    ))
                ),
                "Small last-capture preview; pixel estimate, not allocation.",
            ),
        ]
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Decodes cover art for the Music views. Returns the cover's average colour.
    pub fn set_art(&mut self, encoded: &[u8]) -> Option<Rgb> {
        self.painter.set_art(encoded)
    }

    pub fn clear_art(&mut self) {
        self.painter.clear_art();
    }

    /// Makes the snap thumb's small copy of a shot, once. False if Skia refused it.
    pub fn set_snap_shot(&mut self, shot: &Bitmap) -> bool {
        self.painter.set_snap_shot(shot)
    }

    /// Drops the thumb's copy (after the thumb has faded out).
    pub fn clear_snap_shot(&mut self) {
        self.painter.clear_snap_shot();
    }

    pub fn has_snap_shot(&self) -> bool {
        self.painter.snap_shot.is_some()
    }

    /// The terminal's cell size in whole device px for `font` at `size` logical px × `scale`
    /// (the grid's cols and rows are fitted to it). `font` falls back to Geist Mono when
    /// no installed font has the name.
    pub fn terminal_cell(&mut self, font: &str, size: f32, scale: f32) -> (u16, u16) {
        crate::terminal::cell_size(&mut self.painter.fonts, font, size, scale)
    }

    /// Draws one frame and presents it (`Present(1)` paces animation to vsync).
    pub fn render(&mut self, frame: &Frame) -> Result<(), RenderError> {
        let mut gpu = self.gpu.borrow_mut();
        if !gpu.is_valid() {
            return Err(RenderError::DeviceLost);
        }
        let canvas = self.chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        self.painter.paint(canvas, frame);
        self.chain.present(&mut gpu.ctx)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // Wait for in-flight GPU work, then release the wrapped buffers before the
        // swap chain goes away.
        let mut gpu = self.gpu.borrow_mut();
        self.chain.surfaces.clear();
        gpu.ctx.flush_submit_and_sync_cpu();
    }
}

fn pick_adapter(factory: &IDXGIFactory4) -> Result<(IDXGIAdapter1, ID3D12Device), RenderError> {
    // SAFETY: enumeration with valid out-pointers; adapters are ref-counted.
    unsafe {
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if (DXGI_ADAPTER_FLAG(desc.Flags as _) & DXGI_ADAPTER_FLAG_SOFTWARE)
                != DXGI_ADAPTER_FLAG_NONE
            {
                continue;
            }
            let mut device: Option<ID3D12Device> = None;
            if D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device).is_ok() {
                if let Some(device) = device {
                    return Ok((adapter, device));
                }
            }
        }
        // No hardware adapter (VM / RDP): fall back to WARP.
        let warp: IDXGIAdapter1 = factory.EnumWarpAdapter().map_err(setup("WARP adapter"))?;
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut device)
            .map_err(setup("D3D12 device"))?;
        device
            .map(|d| (warp, d))
            .ok_or_else(|| RenderError::Setup("D3D12 device".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_owner_reuses_live_context_without_pinning_last_owner() {
        let slot = RefCell::new(Weak::new());
        let first = acquire_shared(&slot, |_| true, || Ok(RefCell::new(7))).unwrap();
        let second = acquire_shared(&slot, |_| true, || panic!("unexpected recreation")).unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert!(first.try_borrow_mut().is_ok());
        let borrow = first.borrow_mut();
        assert!(second.try_borrow_mut().is_err());
        drop(borrow);
        drop(first);
        assert_eq!(*second.borrow(), 7);
        drop(second);
        assert!(slot.borrow().upgrade().is_none());
        let reopened = acquire_shared(&slot, |_| true, || Ok(RefCell::new(9))).unwrap();
        assert_eq!(*reopened.borrow(), 9);
    }

    #[test]
    fn invalid_context_replaced_without_invalidating_old_surface_owner() {
        let slot = RefCell::new(Weak::new());
        let old = acquire_shared(&slot, |_| true, || Ok(1)).unwrap();
        let replacement = acquire_shared(&slot, |_| false, || Ok(2)).unwrap();
        assert!(!Rc::ptr_eq(&old, &replacement));
        assert_eq!(*old, 1);
        let popup = acquire_shared(&slot, |_| true, || panic!("unexpected recreation")).unwrap();
        assert!(Rc::ptr_eq(&replacement, &popup));
        drop(old);
        assert_eq!(*popup, 2);
    }

    #[test]
    fn failed_context_replacement_preserves_existing_owner() {
        let slot = RefCell::new(Weak::new());
        let old = acquire_shared(&slot, |_| true, || Ok(1)).unwrap();
        let failed = acquire_shared(&slot, |_| false, || Err(RenderError::DeviceLost));
        assert!(matches!(failed, Err(RenderError::DeviceLost)));
        assert!(Rc::ptr_eq(&old, &slot.borrow().upgrade().unwrap()));
    }
}
