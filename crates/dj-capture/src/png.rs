//! PNG through WIC (`IWICImagingFactory`, part of Windows). Encoding writes into an
//! `HGLOBAL` stream and copies the bytes out; decoding (used by the tests and kept for
//! later stages) reads from memory into 32-bit BGRA.

use dj_core::snap::Bitmap;
use std::cell::Cell;
use windows::core::GUID;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    IWICBitmapFrameEncode, IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapEncoderNoCache,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IStream, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    STATFLAG_NONAME, STATSTG,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

/// Makes sure COM is initialised on this thread (once). The UI thread already is; test
/// threads are not.
fn ensure_com() {
    if COM_READY.with(Cell::get) {
        return;
    }
    // SAFETY: initialises the calling thread's apartment. S_FALSE (already initialised)
    // and RPC_E_CHANGED_MODE (initialised in another mode) both leave COM usable.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    COM_READY.with(|c| c.set(true));
}

fn err(what: &'static str) -> impl FnOnce(windows::core::Error) -> String {
    move |e| format!("{what}: {e}")
}

fn factory() -> Result<IWICImagingFactory, String> {
    ensure_com();
    // SAFETY: plain in-process COM creation of the system WIC factory.
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
        .map_err(err("WIC factory"))
}

/// Encodes an opaque BGRA bitmap as PNG.
pub fn encode(b: &Bitmap) -> Result<Vec<u8>, String> {
    if b.width == 0 || b.height == 0 || b.pixels.len() < b.stride() * b.height as usize {
        return Err("empty or short bitmap".into());
    }
    let wic = factory()?;
    // SAFETY: every interface is ref-counted by the `windows` crate; the pixel slice
    // outlives `WritePixels`; the HGLOBAL belongs to the stream (deleted on release),
    // and it is only read between GlobalLock and GlobalUnlock while the stream is alive.
    unsafe {
        let stream: IStream =
            CreateStreamOnHGlobal(HGLOBAL::default(), true).map_err(err("memory stream"))?;
        let encoder = wic
            .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
            .map_err(err("PNG encoder"))?;
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .map_err(err("encoder init"))?;
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut props = None;
        encoder
            .CreateNewFrame(&mut frame, &mut props)
            .map_err(err("PNG frame"))?;
        let frame = frame.ok_or("PNG frame")?;
        frame
            .Initialize(props.as_ref())
            .map_err(err("frame init"))?;
        frame
            .SetSize(b.width, b.height)
            .map_err(err("frame size"))?;
        let mut format: GUID = GUID_WICPixelFormat32bppBGRA;
        frame
            .SetPixelFormat(&mut format)
            .map_err(err("pixel format"))?;
        if format != GUID_WICPixelFormat32bppBGRA {
            return Err("the PNG encoder refused 32-bit BGRA".into());
        }
        let len = b.stride() * b.height as usize;
        frame
            .WritePixels(b.height, b.stride() as u32, &b.pixels[..len])
            .map_err(err("write pixels"))?;
        frame.Commit().map_err(err("frame commit"))?;
        encoder.Commit().map_err(err("encoder commit"))?;

        let mut stat = STATSTG::default();
        stream
            .Stat(&mut stat, STATFLAG_NONAME)
            .map_err(err("stream size"))?;
        let size = stat.cbSize as usize;
        let hglobal = GetHGlobalFromStream(&stream).map_err(err("stream memory"))?;
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            return Err("cannot lock the PNG bytes".into());
        }
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(hglobal);
        Ok(bytes)
    }
}

/// Decodes any image WIC reads into 32-bit BGRA.
pub fn decode(bytes: &[u8]) -> Result<Bitmap, String> {
    let wic = factory()?;
    // SAFETY: `bytes` outlives the stream and the decoder, which are dropped at the end
    // of this block; the output buffer is sized for the frame at the stride passed.
    unsafe {
        let stream = wic.CreateStream().map_err(err("WIC stream"))?;
        stream
            .InitializeFromMemory(bytes)
            .map_err(err("stream init"))?;
        let decoder = wic
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(err("decoder"))?;
        let frame = decoder.GetFrame(0).map_err(err("frame"))?;
        let conv = wic.CreateFormatConverter().map_err(err("converter"))?;
        conv.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .map_err(err("convert"))?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).map_err(err("size"))?;
        let mut out = Bitmap {
            width: w,
            height: h,
            pixels: vec![0; w as usize * h as usize * 4],
        };
        let stride = out.stride() as u32;
        conv.CopyPixels(std::ptr::null(), stride, &mut out.pixels)
            .map_err(err("copy pixels"))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic opaque gradient: no screen involved.
    fn gradient(w: u32, h: u32) -> Bitmap {
        let mut b = Bitmap::black(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                b.pixels[i] = (x * 7) as u8;
                b.pixels[i + 1] = (y * 13) as u8;
                b.pixels[i + 2] = (x ^ y) as u8;
            }
        }
        b
    }

    #[test]
    fn png_round_trip_keeps_every_pixel() {
        let b = gradient(37, 23);
        let png = encode(&b).expect("encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let back = decode(&png).expect("decode");
        assert_eq!((back.width, back.height), (37, 23));
        assert_eq!(back.pixels, b.pixels);
    }

    #[test]
    fn a_one_pixel_bitmap_round_trips() {
        let mut b = Bitmap::black(1, 1);
        b.pixels.copy_from_slice(&[0x58, 0xA4, 0xE0, 255]);
        let back = decode(&encode(&b).unwrap()).unwrap();
        assert_eq!(back.pixels, [0x58, 0xA4, 0xE0, 255]);
    }

    #[test]
    fn empty_or_short_bitmaps_are_refused() {
        assert!(encode(&Bitmap::default()).is_err());
        let short = Bitmap {
            width: 4,
            height: 4,
            pixels: vec![0; 10],
        };
        assert!(encode(&short).is_err());
        assert!(decode(b"not a png").is_err());
    }
}
