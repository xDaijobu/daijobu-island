//! D3D12 device, composition swap chain, DirectComposition tree and Skia wiring.
//!
//! The swap chain is created once at the largest state's size; the pill animates
//! inside it, so there is never a `ResizeBuffers`. A DPI change rebuilds everything.
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
            Ok(Self {
                ctx,
                queue,
                _device: device,
                factory,
            })
        }
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
                let resource = swap_chain
                    .GetBuffer(i as u32)
                    .map_err(setup("swap chain buffer"))?;
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
                )
                .ok_or_else(|| RenderError::Setup("wrap swap chain buffer".into()))?;
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
    gpu: Gpu,
    chain: Chain,
    width: u32,
    height: u32,
}

impl Renderer {
    /// `width`/`height` are the window's client size in device pixels.
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, RenderError> {
        let mut gpu = Gpu::new()?;
        let chain = Chain::new(&mut gpu, hwnd, width, height)?;
        let painter = Painter::new().map_err(RenderError::Setup)?;
        Ok(Self {
            painter,
            gpu,
            chain,
            width,
            height,
        })
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

    /// Draws one frame and presents it (`Present(1)` paces animation to vsync).
    pub fn render(&mut self, frame: &Frame) -> Result<(), RenderError> {
        let canvas = self.chain.back_buffer().canvas();
        canvas.clear(Color::TRANSPARENT);
        self.painter.paint(canvas, frame);
        self.chain.present(&mut self.gpu.ctx)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // Wait for in-flight GPU work, then release the wrapped buffers before the
        // swap chain goes away.
        self.chain.surfaces.clear();
        self.gpu.ctx.flush_submit_and_sync_cpu();
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
