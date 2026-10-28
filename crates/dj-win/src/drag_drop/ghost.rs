use super::Ghost;
use std::mem::size_of;
use windows::core::IUnknown;
use windows::Win32::Foundation::{COLORREF, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, IDragSourceHelper, SHDRAGIMAGE};

const MAX_EDGE: u32 = 512;

pub(super) fn validate(image: &Ghost, count: usize) -> Result<(), String> {
    if image.width == 0 || image.height == 0 || image.width > MAX_EDGE || image.height > MAX_EDGE {
        return Err("Drag thumbnail dimensions must be between 1 and 512 pixels".into());
    }
    let expected = (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(image.bgra.len()) {
        return Err("Drag thumbnail byte length does not match its dimensions".into());
    }
    if count == 0 || image.count != count {
        return Err("Drag thumbnail count does not match the file list".into());
    }
    Ok(())
}

struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: this handle is owned and has never been selected into a DC. Successful
        // helper initialization transfers ownership and forgets this guard instead.
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

pub(super) fn initialize(
    object: &IDataObject,
    image: &Ghost,
    count: usize,
) -> Result<IDragSourceHelper, String> {
    validate(image, count)?;
    // SAFETY: run initialized OLE on the UI STA; no aggregation is requested.
    let helper: IDragSourceHelper = unsafe {
        CoCreateInstance(
            &CLSID_DragDropHelper,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(|e| format!("Create Shell drag helper: {e}"))?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: image.width as i32,
            biHeight: -(image.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: image.bgra.len() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: bounded, validated dimensions describe a 32-bit top-down DIB. No desktop
    // pixels are read: this allocates a new, unselected memory bitmap and writable bits.
    let bitmap = Bitmap(
        unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
            .map_err(|e| format!("Create drag thumbnail: {e}"))?,
    );
    if bits.is_null() {
        return Err("Drag thumbnail has no writable pixels".into());
    }
    // SAFETY: CreateDIBSection allocated exactly the validated pixel extent. Source and
    // destination are disjoint, tightly packed BGRA; the helper applies premultiplication.
    unsafe { std::ptr::copy_nonoverlapping(image.bgra.as_ptr(), bits.cast(), image.bgra.len()) };
    let drag_image = SHDRAGIMAGE {
        sizeDragImage: SIZE {
            cx: image.width as i32,
            cy: image.height as i32,
        },
        ptOffset: POINT {
            x: (image.width / 2) as i32,
            y: 8.min(image.height - 1) as i32,
        },
        hbmpDragImage: bitmap.0,
        crColorKey: COLORREF(u32::MAX), // use the alpha channel, not a keyed pixel colour
    };
    // SAFETY: bitmap/object live through initialization. The Shell writes private formats
    // using SetData; on success it owns the HBITMAP, on failure our guard deletes it.
    unsafe { helper.InitializeFromBitmap(&drag_image, object) }
        .map_err(|e| format!("Initialize Shell drag thumbnail: {e}"))?;
    std::mem::forget(bitmap);
    Ok(helper)
}
