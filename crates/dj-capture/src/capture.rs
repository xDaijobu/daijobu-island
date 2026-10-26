//! Freezes every monitor once: DXGI Desktop Duplication per output (a D3D11 device per
//! adapter, made for this snap and dropped after it), and GDI `BitBlt` from the screen
//! DC for any monitor that duplication can't give (no output, a rotated output, a
//! non-BGRA format, a timeout or access denied, e.g. on the secure desktop).
//!
//! Pixels come out as opaque BGRA, top-down, in physical px. The process is
//! Per-Monitor-V2 aware, so monitor rects are physical desktop coordinates.

use dj_core::snap::{Bitmap, IRect, MonitorFrame};
use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{HMODULE, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CAPTUREBLT, DIB_RGB_COLORS, HDC, HGDIOBJ, HMONITOR, MONITORINFO, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

/// How long one `AcquireNextFrame` waits, and how many times it is tried. The first
/// frame after `DuplicateOutput` is normally there at once.
const ACQUIRE_MS: u32 = 60;
const ACQUIRE_TRIES: u32 = 4;

/// A monitor as Windows lists it.
#[derive(Clone, Copy, Debug)]
struct Monitor {
    handle: HMONITOR,
    rect: IRect,
    dpi: u32,
}

/// What went into a freeze, for the app log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FreezeReport {
    pub dxgi: usize,
    pub gdi: usize,
    /// Why duplication was skipped, if it was (one line).
    pub note: Option<String>,
}

/// Captures every monitor once. Fails only when there are no monitors or every
/// capture path failed.
pub fn freeze() -> Result<(Vec<MonitorFrame>, FreezeReport), String> {
    let monitors = monitors();
    if monitors.is_empty() {
        return Err("no monitors".into());
    }
    let mut report = FreezeReport::default();
    let mut got: Vec<Option<Bitmap>> = match duplicate(&monitors) {
        Ok(v) => v,
        Err(e) => {
            report.note = Some(e);
            vec![None; monitors.len()]
        }
    };
    let mut frames = Vec::with_capacity(monitors.len());
    for (m, bitmap) in monitors.iter().zip(got.iter_mut()) {
        let bitmap = match bitmap.take() {
            Some(b) => {
                report.dxgi += 1;
                b
            }
            None => match gdi(m.rect) {
                Some(b) => {
                    report.gdi += 1;
                    b
                }
                None => continue,
            },
        };
        frames.push(MonitorFrame {
            rect: m.rect,
            dpi: m.dpi,
            bitmap,
        });
    }
    if frames.is_empty() {
        return Err("every capture failed".into());
    }
    Ok((frames, report))
}

fn irect(r: RECT) -> IRect {
    IRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// Every monitor on the desktop, with its rect and effective DPI.
fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<Monitor>` passed below, alive for the whole
        // enumeration; the monitor handle is valid during the callback.
        unsafe {
            let list = &mut *(data.0 as *mut Vec<Monitor>);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(h, &mut info).as_bool() {
                let (mut dx, mut dy) = (96u32, 96u32);
                let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                list.push(Monitor {
                    handle: h,
                    rect: irect(info.rcMonitor),
                    dpi: dx.max(96),
                });
            }
        }
        true.into()
    }
    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only writes to `list`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(each),
            LPARAM(&mut list as *mut Vec<Monitor> as isize),
        );
    }
    list.retain(|m| !m.rect.is_empty());
    list
}

/// One D3D11 device for an adapter, made on first use.
struct Device {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
}

/// Desktop Duplication for every output it can serve, as a bitmap per monitor index.
fn duplicate(monitors: &[Monitor]) -> Result<Vec<Option<Bitmap>>, String> {
    let mut out: Vec<Option<Bitmap>> = vec![None; monitors.len()];
    // SAFETY: plain COM enumeration; every interface is ref-counted by the `windows`
    // crate and dropped at the end of its scope (the devices with this function).
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| format!("DXGI: {e}"))?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let mut dev: Option<Device> = None;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let Ok(desc) = output.GetDesc() else {
                    continue;
                };
                let rotated = desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
                    && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED;
                if !desc.AttachedToDesktop.as_bool() || rotated {
                    continue;
                }
                let Some(i) = monitors.iter().position(|m| m.handle == desc.Monitor) else {
                    continue;
                };
                if out[i].is_some() {
                    continue;
                }
                if dev.is_none() {
                    let mut device = None;
                    let mut ctx = None;
                    if D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut ctx),
                    )
                    .is_err()
                    {
                        break;
                    }
                    match (device, ctx) {
                        (Some(device), Some(ctx)) => dev = Some(Device { device, ctx }),
                        _ => break,
                    }
                }
                let Some(d) = dev.as_ref() else { break };
                let Ok(output1) = output.cast::<IDXGIOutput1>() else {
                    continue;
                };
                let Ok(dupl) = output1.DuplicateOutput(&d.device) else {
                    continue;
                };
                out[i] = grab(&dupl, d, monitors[i].rect);
            }
        }
    }
    Ok(out)
}

/// Takes one desktop image from `dupl` into a CPU bitmap of `rect`'s size.
///
/// # Safety
/// `dupl` must have been made with `d.device`.
unsafe fn grab(dupl: &IDXGIOutputDuplication, d: &Device, rect: IRect) -> Option<Bitmap> {
    // SAFETY: the acquired frame is released on every path after the copy; the staging
    // texture is mapped only between Map and Unmap, and rows are read within the
    // mapped `RowPitch * Height` bytes.
    unsafe {
        for _ in 0..ACQUIRE_TRIES {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match dupl.AcquireNextFrame(ACQUIRE_MS, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(_) => return None,
            }
            // Only a pointer update so far: no desktop image in this frame yet.
            if info.LastPresentTime == 0 {
                let _ = dupl.ReleaseFrame();
                continue;
            }
            let copied = res
                .and_then(|r| r.cast::<ID3D11Texture2D>().ok())
                .and_then(|tex| stage(d, &tex, rect));
            let _ = dupl.ReleaseFrame();
            return read(d, &copied?, rect);
        }
        None
    }
}

/// Copies a BGRA desktop texture of `rect`'s size into a new CPU-readable texture.
///
/// # Safety
/// `tex` must belong to `d.device`.
unsafe fn stage(d: &Device, tex: &ID3D11Texture2D, rect: IRect) -> Option<ID3D11Texture2D> {
    // SAFETY: valid descriptors and out-pointers for the duration of each call.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
            || desc.Width as i32 != rect.w
            || desc.Height as i32 != rect.h
        {
            return None;
        }
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        d.device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .ok()?;
        let staging = staging?;
        d.ctx.CopyResource(&staging, tex);
        Some(staging)
    }
}

/// Maps a staging texture and copies it out as an opaque bitmap.
///
/// # Safety
/// `staging` must be a CPU-readable texture of `rect`'s size on `d.device`.
unsafe fn read(d: &Device, staging: &ID3D11Texture2D, rect: IRect) -> Option<Bitmap> {
    // SAFETY: the mapping covers `RowPitch * rect.h` bytes until Unmap; each row read is
    // `rect.w * 4 <= RowPitch` bytes long.
    unsafe {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        d.ctx
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .ok()?;
        let mut b = Bitmap::black(rect.w as u32, rect.h as u32);
        let row = b.stride();
        let pitch = mapped.RowPitch as usize;
        if pitch >= row && !mapped.pData.is_null() {
            let src = mapped.pData as *const u8;
            for y in 0..rect.h as usize {
                let line = std::slice::from_raw_parts(src.add(y * pitch), row);
                b.pixels[y * row..(y + 1) * row].copy_from_slice(line);
            }
        }
        d.ctx.Unmap(staging, 0);
        if pitch < row {
            return None;
        }
        opaque(&mut b);
        Some(b)
    }
}

/// GDI fallback: `BitBlt` from the screen DC (with `CAPTUREBLT`, so layered windows are
/// included) into a top-down 32-bit DIB section.
fn gdi(rect: IRect) -> Option<Bitmap> {
    // SAFETY: every GDI object made here is selected out and deleted before returning;
    // the DIB section's bits are read only while it exists, within `w * 4 * h` bytes.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: rect.w,
                biHeight: -rect.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let out = match CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) if !bits.is_null() => {
                let old = SelectObject(mem, HGDIOBJ(dib.0));
                let ok = BitBlt(
                    mem,
                    0,
                    0,
                    rect.w,
                    rect.h,
                    Some(screen),
                    rect.x,
                    rect.y,
                    SRCCOPY | CAPTUREBLT,
                )
                .is_ok();
                let out = ok.then(|| {
                    let len = rect.w as usize * rect.h as usize * 4;
                    let mut b = Bitmap {
                        width: rect.w as u32,
                        height: rect.h as u32,
                        pixels: std::slice::from_raw_parts(bits as *const u8, len).to_vec(),
                    };
                    opaque(&mut b);
                    b
                });
                SelectObject(mem, old);
                let _ = DeleteObject(HGDIOBJ(dib.0));
                out
            }
            Ok(dib) => {
                let _ = DeleteObject(HGDIOBJ(dib.0));
                None
            }
            Err(_) => None,
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

/// Screen pixels have no meaningful alpha (GDI leaves it 0): make them opaque.
fn opaque(b: &mut Bitmap) {
    for px in b.pixels.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_sets_only_alpha() {
        let mut b = Bitmap {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 0, 4, 5, 6, 9],
        };
        opaque(&mut b);
        assert_eq!(b.pixels, [1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
