//! Bounded, platform-free clipboard bitmap codecs.
//!
//! Layout: https://learn.microsoft.com/windows/win32/api/wingdi/ns-wingdi-bitmapv5header
use dj_core::clipboard_types::{
    ImageData, Thumbnail, MAX_IMAGE_EDGE, MAX_PIXELS_BYTES, MAX_PNG_BYTES, MAX_RAW_BYTES,
};
use std::io::{Cursor, Write};

fn fail() -> String {
    "Invalid clipboard image".into()
}

fn pixels_len(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 || width > MAX_IMAGE_EDGE || height > MAX_IMAGE_EDGE {
        return Err("Clipboard image dimensions exceed limit".into());
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= MAX_PIXELS_BYTES)
        .ok_or_else(|| "Clipboard image pixels exceed limit".into())
}

fn buffer(len: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| fail())?;
    bytes.resize(len, 0);
    Ok(bytes)
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, String> {
    let b = bytes.get(at..at + 4).ok_or_else(fail)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, String> {
    let b = bytes.get(at..at + 2).ok_or_else(fail)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

/// Decode ordinary PNG, including palette, grayscale and 16-bit input.
/// APNG is unsupported rather than silently discarding its other frames.
pub fn decode_png(bytes: &[u8]) -> Result<ImageData, String> {
    if bytes.len() > MAX_PNG_BYTES {
        return Err("Clipboard image encoding exceeds limit".into());
    }
    if !complete_png(bytes) {
        return Err(fail());
    }
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: MAX_PIXELS_BYTES,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let header = decoder.read_header_info().map_err(|_| fail())?;
    let len = pixels_len(header.width, header.height)?;
    let mut reader = decoder.read_info().map_err(|_| fail())?;
    if reader.info().animation_control.is_some() {
        return Err("Unsupported clipboard image encoding".into());
    }
    let size = reader
        .output_buffer_size()
        .filter(|&n| n <= len)
        .ok_or_else(fail)?;
    let mut bgra = buffer(size)?;
    let info = reader.next_frame(&mut bgra).map_err(|_| fail())?;
    reader.finish().map_err(|_| fail())?;
    if info.bit_depth != png::BitDepth::Eight || info.buffer_size() != size {
        return Err(fail());
    }
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return Err(fail()),
    };
    if size != len / 4 * channels {
        return Err(fail());
    }
    bgra.try_reserve_exact(len - size).map_err(|_| fail())?;
    bgra.resize(len, 0);
    for i in (0..len / 4).rev() {
        let at = i * channels;
        let (r, g, b, a) = match channels {
            4 => (bgra[at], bgra[at + 1], bgra[at + 2], bgra[at + 3]),
            3 => (bgra[at], bgra[at + 1], bgra[at + 2], 255),
            2 => (bgra[at], bgra[at], bgra[at], bgra[at + 1]),
            _ => (bgra[at], bgra[at], bgra[at], 255),
        };
        bgra[i * 4..i * 4 + 4].copy_from_slice(&[b, g, r, a]);
    }
    let thumbnail = thumbnail(info.width, info.height, &bgra)?;
    Ok(ImageData {
        width: info.width,
        height: info.height,
        bgra,
        png: bytes.to_vec(),
        thumbnail,
    })
}

// Require a complete chunk envelope, including IEND, even if the decoder can
// recover pixels from a truncated stream. Native allocation padding may be zero.
// Chunk CRCs and ordering are checked by png's decoder, with checksums enabled.
fn complete_png(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut cursor = 8usize;
    while let Some(header) = bytes.get(cursor..).and_then(|tail| tail.get(..8)) {
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let Some(end) = cursor
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .filter(|&n| n <= bytes.len())
        else {
            return false;
        };
        if cursor == 8 && (&header[4..8] != b"IHDR" || length != 13) {
            return false;
        }
        if &header[4..8] == b"IEND" {
            return length == 0 && bytes[end..].iter().all(|&b| b == 0);
        }
        cursor = end;
    }
    false
}

fn valid_mask(mask: u32) -> bool {
    mask != 0
        && (mask >> mask.trailing_zeros())
            .wrapping_add(1)
            .is_power_of_two()
}

fn component(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 255;
    }
    let max = mask >> mask.trailing_zeros();
    let value = (pixel & mask) >> mask.trailing_zeros();
    ((u64::from(value) * 255 + u64::from(max) / 2) / u64::from(max)) as u8
}

/// Decode packed CF_DIB/CF_DIBV5, without a BITMAPFILEHEADER. BI_RGB 24/32
/// and BI_BITFIELDS 32 are supported. BI_RGB's high byte is reserved, not alpha.
pub fn decode_dib(bytes: &[u8]) -> Result<ImageData, String> {
    if bytes.len() > MAX_RAW_BYTES {
        return Err("Clipboard image encoding exceeds limit".into());
    }
    let header = u32_at(bytes, 0)? as usize;
    if !matches!(header, 40 | 52 | 56 | 108 | 124) || bytes.len() < header {
        return Err(fail());
    }
    let width = u32_at(bytes, 4)? as i32;
    let height = u32_at(bytes, 8)? as i32;
    if width <= 0 || height == i32::MIN || height == 0 || u16_at(bytes, 12)? != 1 {
        return Err(fail());
    }
    let (width, rows) = (width as u32, height.unsigned_abs());
    let len = pixels_len(width, rows)?;
    let bpp = u16_at(bytes, 14)?;
    let compression = u32_at(bytes, 16)?;
    if !matches!((bpp, compression), (24 | 32, 0) | (32, 3)) {
        return Err("Unsupported clipboard image encoding".into());
    }
    // Embedded/linked profiles require a colour-management pipeline. Refusing
    // profiles prevents interpreting a profile blob as pixels or opening a file.
    if header == 124 && (u32_at(bytes, 112)? != 0 || u32_at(bytes, 116)? != 0) {
        return Err("Unsupported clipboard image profile".into());
    }
    let mut offset = header;
    let mut masks = [0x00ff0000, 0x0000ff00, 0x000000ff, 0];
    if compression == 3 {
        for (i, mask) in masks[..3].iter_mut().enumerate() {
            *mask = u32_at(bytes, 40 + i * 4)?;
        }
        if header == 40 {
            offset += 12;
        } else if header >= 56 {
            masks[3] = u32_at(bytes, 52)?;
        }
        for (i, &mask) in masks.iter().enumerate() {
            if (i < 3 || mask != 0) && !valid_mask(mask) {
                return Err(fail());
            }
            if masks[..i].iter().any(|&other| mask & other != 0) {
                return Err(fail());
            }
        }
    }
    let colours = u32_at(bytes, 32)? as usize;
    offset = colours
        .checked_mul(4)
        .and_then(|n| offset.checked_add(n))
        .ok_or_else(fail)?;
    let stride = (width as usize * usize::from(bpp)).div_ceil(32) * 4;
    let size = stride.checked_mul(rows as usize).ok_or_else(fail)?;
    let end = offset.checked_add(size).ok_or_else(fail)?;
    let declared = u32_at(bytes, 20)? as usize;
    if end > bytes.len()
        || (declared != 0 && (declared < size || declared > bytes.len().saturating_sub(offset)))
    {
        return Err(fail());
    }
    let mut bgra = buffer(len)?;
    for y in 0..rows as usize {
        let source_y = if height < 0 { y } else { rows as usize - 1 - y };
        for x in 0..width as usize {
            let source = offset + source_y * stride + x * usize::from(bpp / 8);
            let colour = if bpp == 24 {
                [bytes[source], bytes[source + 1], bytes[source + 2], 255]
            } else {
                let pixel = u32_at(bytes, source)?;
                [
                    component(pixel, masks[2]),
                    component(pixel, masks[1]),
                    component(pixel, masks[0]),
                    component(pixel, masks[3]),
                ]
            };
            let target = (y * width as usize + x) * 4;
            bgra[target..target + 4].copy_from_slice(&colour);
        }
    }
    let png = encode_png(width, rows, &bgra)?;
    let thumbnail = thumbnail(width, rows, &bgra)?;
    Ok(ImageData {
        width,
        height: rows,
        bgra,
        png,
        thumbnail,
    })
}

/// Publish top-down V5 bitfields so straight alpha is explicit and round-trips.
pub fn encode_dib(image: &ImageData) -> Result<Vec<u8>, String> {
    let len = pixels_len(image.width, image.height)?;
    if image.bgra.len() != len {
        return Err("Invalid clipboard image pixels".into());
    }
    let mut bytes = buffer(124 + len)?;
    for (at, value) in [
        (0, 124),
        (4, image.width),
        (8, (-(image.height as i32)) as u32),
        (16, 3),
        (20, len as u32),
        (40, 0x00ff0000),
        (44, 0x0000ff00),
        (48, 0x000000ff),
        (52, 0xff000000),
        (56, 0x73524742),
        (108, 4),
    ] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
    bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
    bytes[124..].copy_from_slice(&image.bgra);
    Ok(bytes)
}

struct BoundedPng(Vec<u8>);
impl Write for BoundedPng {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_PNG_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("Image encoding limit"));
        }
        self.0
            .try_reserve(bytes.len())
            .map_err(|_| std::io::Error::other("Image allocation limit"))?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = BoundedPng(Vec::new());
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|_| fail())?;
        let mut stream = writer.stream_writer().map_err(|_| fail())?;
        let mut row = buffer(width as usize * 4)?;
        for source in bgra.chunks_exact(row.len()) {
            for (source, target) in source
                .as_chunks::<4>()
                .0
                .iter()
                .zip(row.as_chunks_mut::<4>().0.iter_mut())
            {
                target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
            }
            stream.write_all(&row).map_err(|_| fail())?;
        }
        stream.finish().map_err(|_| fail())?;
        // StreamWriter::finish flushes pixels; Writer::finish also checks IEND.
        writer.finish().map_err(|_| fail())?;
    }
    Ok(output.0)
}

fn thumbnail(width: u32, height: u32, pixels: &[u8]) -> Result<Thumbnail, String> {
    let scale = (160.0 / f64::from(width))
        .min(90.0 / f64::from(height))
        .min(1.0);
    let tw = (f64::from(width) * scale).floor().max(1.0) as u32;
    let th = (f64::from(height) * scale).floor().max(1.0) as u32;
    let mut bgra = buffer(tw as usize * th as usize * 4)?;
    for y in 0..th {
        for x in 0..tw {
            let sx = u64::from(x) * u64::from(width) / u64::from(tw);
            let sy = u64::from(y) * u64::from(height) / u64::from(th);
            let source = (sy as usize * width as usize + sx as usize) * 4;
            let target = (y as usize * tw as usize + x as usize) * 4;
            bgra[target..target + 4].copy_from_slice(&pixels[source..source + 4]);
        }
    }
    Ok(Thumbnail {
        width: tw,
        height: th,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn dib24(height: i32) -> Vec<u8> {
        let mut bytes = vec![0; 48];
        set(&mut bytes, 0, 40);
        set(&mut bytes, 4, 1);
        set(&mut bytes, 8, height as u32);
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&24u16.to_le_bytes());
        bytes[40..].copy_from_slice(&[1, 2, 3, 99, 4, 5, 6, 99]);
        bytes
    }

    fn data(width: u32, height: u32, bgra: Vec<u8>) -> ImageData {
        ImageData {
            width,
            height,
            bgra,
            png: vec![],
            thumbnail: Thumbnail {
                width: 0,
                height: 0,
                bgra: vec![],
            },
        }
    }

    fn png_fixture(
        colour: png::ColorType,
        depth: png::BitDepth,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut output, width, height);
            encoder.set_color(colour);
            encoder.set_depth(depth);
            if colour == png::ColorType::Indexed {
                encoder.set_palette(vec![10, 20, 30, 40, 50, 60]);
                encoder.set_trns(vec![0, 128]);
            }
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(bytes).unwrap();
        }
        output
    }

    #[test]
    fn dib24_stride_and_both_orientations() {
        let bottom = decode_dib(&dib24(2)).unwrap();
        assert_eq!(bottom.bgra, [4, 5, 6, 255, 1, 2, 3, 255]);
        let top = decode_dib(&dib24(-2)).unwrap();
        assert_eq!(top.bgra, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(decode_png(&top.png).unwrap().bgra, top.bgra);
    }

    #[test]
    fn v5_round_trip_preserves_straight_alpha_and_top_down_pixels() {
        let original = data(
            2,
            2,
            vec![0, 0, 255, 0, 10, 20, 200, 64, 255, 1, 2, 128, 4, 5, 6, 255],
        );
        let dib = encode_dib(&original).unwrap();
        assert_eq!(u32_at(&dib, 8).unwrap() as i32, -2);
        let decoded = decode_dib(&dib).unwrap();
        assert_eq!(decoded.bgra, original.bgra);
        assert_eq!(decode_png(&decoded.png).unwrap().bgra, original.bgra);
    }

    #[test]
    fn rgb32_reserved_byte_is_opaque() {
        let mut bytes = dib24(-2);
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        bytes[43] = 0;
        bytes[47] = 127;
        assert_eq!(
            decode_dib(&bytes).unwrap().bgra,
            [1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    #[test]
    fn external_and_internal_bitfields_scale_channels() {
        for header in [40, 52, 56, 108, 124] {
            let offset = if header == 40 { 52 } else { header };
            let mut bytes = vec![0; offset + 4];
            set(&mut bytes, 0, header as u32);
            set(&mut bytes, 4, 1);
            set(&mut bytes, 8, 1);
            bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
            bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
            set(&mut bytes, 16, 3);
            set(&mut bytes, 40, 0x3ff00000);
            set(&mut bytes, 44, 0x000ffc00);
            set(&mut bytes, 48, 0x000003ff);
            if header >= 56 {
                set(&mut bytes, 52, 0xc0000000);
            }
            set(&mut bytes, offset, 0x80000000 | 0x3ff00000 | 512);
            let decoded = decode_dib(&bytes).unwrap();
            assert_eq!(
                decoded.bgra,
                [128, 0, 255, if header >= 56 { 170 } else { 255 }]
            );
        }
    }

    #[test]
    fn masks_must_be_nonzero_contiguous_and_disjoint() {
        let valid = encode_dib(&data(1, 1, vec![1, 2, 3, 4])).unwrap();
        for (at, mask) in [
            (40, 0),
            (40, 0x00f50000),
            (40, 0x0000ff00),
            (52, 0x000000ff),
            (52, 0xa0000000),
        ] {
            let mut bytes = valid.clone();
            set(&mut bytes, at, mask);
            assert!(decode_dib(&bytes).is_err());
        }
    }

    #[test]
    fn short_headers_and_pixels_are_rejected_without_panics() {
        let bytes = encode_dib(&data(1, 1, vec![1, 2, 3, 4])).unwrap();
        for end in 0..bytes.len() {
            assert!(decode_dib(&bytes[..end]).is_err());
        }
        let mut header = vec![0; 40];
        set(&mut header, 0, u32::MAX);
        assert!(decode_dib(&header).is_err());
    }

    #[test]
    fn dimension_palette_profile_and_declared_size_attacks() {
        let valid = encode_dib(&data(1, 1, vec![1, 2, 3, 4])).unwrap();
        for (at, value) in [
            (4, 0),
            (4, u32::MAX),
            (4, 16_385),
            (8, 0),
            (8, i32::MIN as u32),
            (8, i32::MAX as u32),
            (32, u32::MAX),
            (20, 3),
            (20, u32::MAX),
            (112, u32::MAX),
            (116, 1),
            (16, 1),
        ] {
            let mut bytes = valid.clone();
            set(&mut bytes, at, value);
            assert!(decode_dib(&bytes).is_err());
        }
        assert_eq!(pixels_len(MAX_IMAGE_EDGE, 1024).unwrap(), MAX_PIXELS_BYTES);
        assert!(pixels_len(MAX_IMAGE_EDGE, 1025).is_err());
        assert!(pixels_len(u32::MAX, u32::MAX).is_err());
        assert!(pixels_len(MAX_IMAGE_EDGE + 1, 1).is_err());
    }

    #[test]
    fn palette_offset_is_checked_and_not_interpreted_as_pixels() {
        let mut bytes = dib24(-2);
        set(&mut bytes, 32, 1);
        bytes.splice(40..40, [90, 91, 92, 93]);
        assert_eq!(
            decode_dib(&bytes).unwrap().bgra,
            [1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    #[test]
    fn png_palette_gray_rgb_and_16bit_expansion() {
        for (colour, depth, raw, expected) in [
            (
                png::ColorType::Indexed,
                png::BitDepth::One,
                vec![0x40],
                vec![30, 20, 10, 0, 60, 50, 40, 128],
            ),
            (
                png::ColorType::Grayscale,
                png::BitDepth::Eight,
                vec![10, 20],
                vec![10, 10, 10, 255, 20, 20, 20, 255],
            ),
            (
                png::ColorType::GrayscaleAlpha,
                png::BitDepth::Eight,
                vec![10, 0, 20, 128],
                vec![10, 10, 10, 0, 20, 20, 20, 128],
            ),
            (
                png::ColorType::Rgb,
                png::BitDepth::Eight,
                vec![10, 20, 30, 40, 50, 60],
                vec![30, 20, 10, 255, 60, 50, 40, 255],
            ),
            (
                png::ColorType::Rgba,
                png::BitDepth::Sixteen,
                vec![10, 1, 20, 2, 30, 3, 40, 4, 50, 5, 60, 6, 70, 7, 80, 8],
                vec![30, 20, 10, 40, 70, 60, 50, 80],
            ),
        ] {
            let png = png_fixture(colour, depth, 2, 1, &raw);
            let image = decode_png(&png).unwrap();
            assert_eq!(image.bgra, expected);
            assert_eq!(image.png, png);
        }
    }

    #[test]
    fn png_requires_complete_checksums_and_iend() {
        let valid = png_fixture(
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            1,
            1,
            &[1, 2, 3, 4],
        );
        for end in 0..valid.len() {
            assert!(decode_png(&valid[..end]).is_err());
        }
        let mut broken = valid;
        broken[29] ^= 1;
        assert!(decode_png(&broken).is_err());
        assert!(decode_png(&vec![0; MAX_PNG_BYTES + 1]).is_err());
    }

    #[test]
    fn oversized_png_rejected_from_header_before_output_allocation() {
        let bytes = png_fixture(
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            MAX_IMAGE_EDGE + 1,
            1,
            &vec![0; (MAX_IMAGE_EDGE as usize + 1) * 4],
        );
        assert!(decode_png(&bytes).is_err());
    }

    #[test]
    fn thumbnail_is_bounded_contained_and_keeps_alpha() {
        for (width, height, tw, th) in [
            (320, 180, 160, 90),
            (10, 1000, 1, 90),
            (1000, 10, 160, 1),
            (2, 3, 2, 3),
        ] {
            let bytes = [10, 20, 30, 64].repeat(width as usize * height as usize);
            let thumb = thumbnail(width, height, &bytes).unwrap();
            assert_eq!((thumb.width, thumb.height), (tw, th));
            assert_eq!(
                thumb.bgra,
                [10, 20, 30, 64].repeat(tw as usize * th as usize)
            );
        }
    }

    #[test]
    fn encode_checks_pixels_and_bounded_png_writer_checks_limit() {
        assert!(encode_dib(&data(2, 2, vec![0; 15])).is_err());
        assert!(encode_dib(&data(0, 2, vec![])).is_err());
        let mut writer = BoundedPng(vec![0; MAX_PNG_BYTES - 1]);
        assert!(writer.write_all(&[1]).is_ok());
        assert!(writer.write_all(&[2]).is_err());
    }
}
