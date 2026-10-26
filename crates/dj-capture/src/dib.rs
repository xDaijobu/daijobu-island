//! `CF_DIB` bytes: a `BITMAPINFOHEADER` followed by 32-bit pixels, bottom-up. Pure byte
//! work, so it is tested without the clipboard.

use dj_core::snap::Bitmap;

/// `sizeof(BITMAPINFOHEADER)`.
pub const HEADER_LEN: usize = 40;
/// `BI_RGB`: no compression, no colour masks after the header.
const BI_RGB: u32 = 0;

/// Packs BGRA rows (top-down, `src_stride` bytes apart, at least `width * 4`) as a
/// 32-bit bottom-up DIB. A 32-bit row is always DWORD-aligned, so the DIB stride is
/// `width * 4` whatever the source stride. None if `pixels` is too short.
pub fn from_bgra(width: u32, height: u32, src_stride: usize, pixels: &[u8]) -> Option<Vec<u8>> {
    let row = width as usize * 4;
    let h = height as usize;
    if src_stride < row || (h > 0 && pixels.len() < src_stride * (h - 1) + row) {
        return None;
    }
    let image_len = row * h;
    let mut out = Vec::with_capacity(HEADER_LEN + image_len);
    let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    put(HEADER_LEN as u32); // biSize
    put(width); // biWidth
    put(height); // biHeight: positive, so bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    put(BI_RGB); // biCompression
    put(image_len as u32); // biSizeImage
    put(0); // biXPelsPerMeter
    put(0); // biYPelsPerMeter
    put(0); // biClrUsed
    put(0); // biClrImportant
    for y in (0..h).rev() {
        let s = y * src_stride;
        out.extend_from_slice(&pixels[s..s + row]);
    }
    Some(out)
}

/// The DIB for a whole bitmap.
pub fn from_bitmap(b: &Bitmap) -> Vec<u8> {
    from_bgra(b.width, b.height, b.stride(), &b.pixels).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
    }

    fn i32_at(b: &[u8], i: usize) -> i32 {
        i32::from_le_bytes(b[i..i + 4].try_into().unwrap())
    }

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes(b[i..i + 2].try_into().unwrap())
    }

    /// A 3 x 2 bitmap whose pixel (x, y) is [x, y, 7, 255].
    fn bitmap() -> Bitmap {
        let mut b = Bitmap::black(3, 2);
        for y in 0..2 {
            for x in 0..3 {
                let i = (y * 3 + x) * 4;
                b.pixels[i] = x as u8;
                b.pixels[i + 1] = y as u8;
                b.pixels[i + 2] = 7;
            }
        }
        b
    }

    #[test]
    fn header_fields() {
        let d = from_bitmap(&bitmap());
        assert_eq!(d.len(), HEADER_LEN + 3 * 2 * 4);
        assert_eq!(u32_at(&d, 0), 40);
        assert_eq!(i32_at(&d, 4), 3);
        assert_eq!(i32_at(&d, 8), 2, "positive height: bottom-up");
        assert_eq!(u16_at(&d, 12), 1);
        assert_eq!(u16_at(&d, 14), 32);
        assert_eq!(u32_at(&d, 16), BI_RGB);
        assert_eq!(u32_at(&d, 20), 24);
        assert!(d[24..40].iter().all(|&b| b == 0));
    }

    #[test]
    fn rows_are_flipped_and_pixels_kept() {
        let d = from_bitmap(&bitmap());
        let px = &d[HEADER_LEN..];
        // The first DIB row is the bottom row (y = 1) of the bitmap.
        assert_eq!(&px[0..4], &[0, 1, 7, 255]);
        assert_eq!(&px[8..12], &[2, 1, 7, 255]);
        assert_eq!(&px[12..16], &[0, 0, 7, 255]);
        assert_eq!(&px[20..24], &[2, 0, 7, 255]);
    }

    #[test]
    fn a_padded_source_stride_is_dropped() {
        // 2 x 2 with 4 bytes of padding per row (like a mapped GPU texture).
        let src = [
            1, 1, 1, 255, 2, 2, 2, 255, 0xEE, 0xEE, 0xEE, 0xEE, //
            3, 3, 3, 255, 4, 4, 4, 255, 0xEE, 0xEE, 0xEE, 0xEE,
        ];
        let d = from_bgra(2, 2, 12, &src).unwrap();
        assert_eq!(d.len(), HEADER_LEN + 16);
        assert_eq!(u32_at(&d, 20), 16);
        assert_eq!(
            &d[HEADER_LEN..],
            &[3, 3, 3, 255, 4, 4, 4, 255, 1, 1, 1, 255, 2, 2, 2, 255]
        );
        // The last row needs no padding after it.
        assert!(from_bgra(2, 2, 12, &src[..20]).is_some());
    }

    #[test]
    fn short_input_or_a_narrow_stride_is_refused() {
        assert_eq!(from_bgra(2, 2, 8, &[0; 15]), None);
        assert_eq!(from_bgra(2, 2, 4, &[0; 64]), None);
        // An empty bitmap is just the header.
        assert_eq!(from_bgra(0, 0, 0, &[]).map(|d| d.len()), Some(HEADER_LEN));
    }
}
