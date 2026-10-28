//! Clipboard classification and bounded presentation helpers. No content is executed.
pub use crate::clipboard_types::*;

pub const MAX_PREVIEW_BYTES: usize = 4096;

/// Truncate display metadata only, at a UTF-8 boundary. Payloads are never truncated.
pub fn bounded_text(text: &str, max_bytes: usize) -> String {
    let mut result = String::with_capacity(text.len().min(max_bytes));
    for c in text.chars() {
        let c = if c.is_control() { ' ' } else { c };
        if result.len() + c.len_utf8() > max_bytes {
            break;
        }
        result.push(c);
    }
    result
}

/// Standalone hexadecimal CSS colour; the returned channels are RGBA.
pub fn colour_rgba(text: &str) -> Option<[u8; 4]> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 3 | 4 | 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let nibble = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
    let mut out = [255; 4];
    if digits.len() <= 4 {
        for (i, b) in digits.bytes().enumerate() {
            out[i] = nibble(b)? * 17;
        }
    } else {
        for (i, pair) in digits.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            out[i] = nibble(pair[0])? * 16 + nibble(pair[1])?;
        }
    }
    Some(out)
}

/// Extract a bounded authority for display, without resolving or opening the URL.
pub fn link_domain(text: &str) -> Option<String> {
    let (scheme, rest) = text.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || text.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    if host.is_empty() || host.contains('\\') {
        return None;
    }
    let domain = if host.starts_with('[') {
        let end = host.find(']')?;
        if end <= 1 {
            return None;
        }
        host[1..end].parse::<std::net::Ipv6Addr>().ok()?;
        let suffix = &host[end + 1..];
        if !suffix.is_empty() && !valid_port(suffix.strip_prefix(':')?) {
            return None;
        }
        &host[..=end]
    } else {
        let mut parts = host.split(':');
        let domain = parts.next()?;
        if let Some(port) = parts.next() {
            if !valid_port(port) {
                return None;
            }
        }
        if parts.next().is_some()
            || domain.is_empty()
            || domain.starts_with('.')
            || domain.ends_with('.')
            || domain.contains("..")
        {
            return None;
        }
        if domain
            .chars()
            .any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-')))
        {
            return None;
        }
        domain
    };
    Some(bounded_text(domain, 256))
}

fn valid_port(port: &str) -> bool {
    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok()
}

impl Payload {
    pub fn kind(&self) -> Kind {
        match &self.content {
            Content::Text { text, .. } if colour_rgba(text).is_some() => Kind::Colour,
            Content::Text { text, .. } if link_domain(text).is_some() => Kind::Link,
            Content::Text { .. } => Kind::Text,
            Content::Image(_) => Kind::Image,
            Content::Files(_) => Kind::Files,
        }
    }

    pub fn retained_bytes(&self) -> usize {
        match &self.content {
            Content::Text { text, html, rtf } => text
                .len()
                .saturating_add(html.as_ref().map_or(0, Vec::len))
                .saturating_add(rtf.as_ref().map_or(0, Vec::len)),
            Content::Image(image) => image
                .bgra
                .len()
                .saturating_add(image.png.len())
                .saturating_add(image.thumbnail.bgra.len()),
            Content::Files(paths) => paths
                .iter()
                .fold(0usize, |sum, path| sum.saturating_add(path.len())),
        }
    }

    /// Owned exact text for explicit plain paste. Paths retain order, without file I/O.
    pub fn plain_text(&self) -> Option<String> {
        match &self.content {
            Content::Text { text, .. } => Some(text.clone()),
            Content::Files(paths) => Some(paths.join("\r\n")),
            Content::Image(_) => None,
        }
    }

    pub fn summary(&self) -> String {
        match &self.content {
            Content::Text { text, .. } => bounded_text(text, MAX_PREVIEW_BYTES),
            Content::Image(image) => format!("Image · {} × {}", image.width, image.height),
            Content::Files(paths) => bounded_text(
                &format!(
                    "{} item(s) · {}",
                    paths.len(),
                    paths
                        .first()
                        .map_or_else(String::new, |path| bounded_text(path, MAX_PREVIEW_BYTES))
                ),
                MAX_PREVIEW_BYTES,
            ),
        }
    }

    /// Structural and resource validation; native format parsers validate format syntax.
    /// Error strings contain categories only, never payload data.
    pub fn validate(&self) -> Result<(), String> {
        match &self.content {
            Content::Text { text, html, rtf } => {
                if text.len() > MAX_TEXT_BYTES {
                    return Err("Text exceeds item limit".into());
                }
                if text.is_empty() || text.contains('\0') {
                    return Err("Invalid Unicode text".into());
                }
                if html.as_ref().is_some_and(|v| v.len() > MAX_RICH_BYTES)
                    || rtf.as_ref().is_some_and(|v| v.len() > MAX_RICH_BYTES)
                {
                    return Err("Rich text exceeds item limit".into());
                }
            }
            Content::Image(image) => {
                if !valid_pixels(
                    image.width,
                    image.height,
                    &image.bgra,
                    MAX_IMAGE_EDGE,
                    MAX_IMAGE_EDGE,
                    MAX_PIXELS_BYTES,
                ) || image.png.is_empty()
                    || image.png.len() > MAX_PNG_BYTES
                {
                    return Err("Invalid image or image exceeds item limit".into());
                }
                if !valid_thumbnail(&image.thumbnail) {
                    return Err("Invalid image thumbnail".into());
                }
            }
            Content::Files(paths) => {
                if paths.is_empty() || paths.len() > MAX_PATHS {
                    return Err("Invalid path count".into());
                }
                // UTF-8 serialized path text, with one delimiter per path;
                // the native adapter separately bounds its UTF-16 envelope.
                let mut bytes = 0usize;
                for path in paths {
                    bytes = bytes.saturating_add(path.len().saturating_add(1));
                    if bytes > MAX_PATH_BYTES {
                        return Err("Paths exceed item limit".into());
                    }
                    if !valid_path(path) {
                        return Err("Invalid file path".into());
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn valid_thumbnail(image: &Thumbnail) -> bool {
    valid_pixels(
        image.width,
        image.height,
        &image.bgra,
        160,
        90,
        160 * 90 * 4,
    )
}

fn valid_pixels(
    width: u32,
    height: u32,
    bytes: &[u8],
    max_w: u32,
    max_h: u32,
    budget: usize,
) -> bool {
    width > 0
        && height > 0
        && width <= max_w
        && height <= max_h
        && (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .is_some_and(|n| n <= budget && n == bytes.len())
}

fn valid_path(path: &str) -> bool {
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '<' | '>' | '|' | '?' | '*'))
    {
        return false;
    }
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let tail = if drive {
        &path[3..]
    } else if let Some(tail) = path.strip_prefix("\\\\") {
        let mut components = tail.split(['\\', '/']);
        if !components.next().is_some_and(|p| !p.is_empty())
            || !components.next().is_some_and(|p| !p.is_empty())
        {
            return false;
        }
        tail
    } else {
        return false;
    };
    if tail.contains(':') {
        return false;
    }
    tail.split(['\\', '/']).all(|part| {
        if part.is_empty() {
            return true;
        }
        if part.ends_with(['.', ' ']) {
            return false;
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) {
            return false;
        }
        !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(s: &str) -> Payload {
        Payload {
            content: Content::Text {
                text: s.into(),
                html: None,
                rtf: None,
            },
            no_cloud: false,
        }
    }

    #[test]
    fn classification_is_standalone_and_never_resolves() {
        for (value, kind) in [
            ("https://example.invalid/a", Kind::Link),
            ("HTTP://localhost:80", Kind::Link),
            ("#AbC", Kind::Colour),
            ("#1234", Kind::Colour),
            ("#12345678", Kind::Colour),
            (" #fff", Kind::Text),
            ("#12345", Kind::Text),
            ("https://", Kind::Text),
            ("https://bad host", Kind::Text),
            ("https://[::1]/", Kind::Link),
            ("https://[invalid]/", Kind::Text),
            ("ftp://example.invalid", Kind::Text),
        ] {
            assert_eq!(text(value).kind(), kind);
        }
        assert_eq!(colour_rgba("#1234"), Some([17, 34, 51, 68]));
        assert_eq!(
            link_domain("https://user@example.invalid:443/a"),
            Some("example.invalid".into())
        );
    }

    #[test]
    fn exact_payloads_preserve_rich_alternatives_and_privacy() {
        let original = text("synthetic 😀\n日本語");
        let mut rich = original.clone();
        if let Content::Text { html, .. } = &mut rich.content {
            *html = Some(b"<b>synthetic</b>".to_vec());
        }
        assert_ne!(original, rich);
        assert_eq!(original.plain_text(), rich.plain_text());
        rich.no_cloud = true;
        assert_ne!(original, rich);
        assert!(rich.validate().is_ok());
    }

    #[test]
    fn bounds_reject_without_truncating_and_summaries_are_utf8_safe() {
        let oversized = text(&"😀".repeat(MAX_TEXT_BYTES / 4 + 1));
        assert!(oversized.validate().is_err());
        assert_eq!(oversized.plain_text().unwrap().len(), MAX_TEXT_BYTES + 4);
        assert_eq!(oversized.summary().len(), MAX_PREVIEW_BYTES);
        assert!(text(&"x".repeat(MAX_TEXT_BYTES)).validate().is_ok());
        let mut rich = text("valid");
        if let Content::Text { rtf, .. } = &mut rich.content {
            *rtf = Some(vec![0; MAX_RICH_BYTES + 1]);
        }
        assert!(rich.validate().is_err());
    }

    #[test]
    fn image_dimensions_and_thumbnails_are_checked() {
        let mut payload = Payload {
            content: Content::Image(ImageData {
                width: 1,
                height: 1,
                bgra: vec![0; 4],
                png: vec![1],
                thumbnail: Thumbnail {
                    width: 1,
                    height: 1,
                    bgra: vec![0; 4],
                },
            }),
            no_cloud: false,
        };
        assert!(payload.validate().is_ok());
        assert_eq!(payload.plain_text(), None);
        if let Content::Image(image) = &mut payload.content {
            image.width = u32::MAX;
        }
        assert!(payload.validate().is_err());
    }

    #[test]
    fn file_lists_preserve_order_and_validate_without_access() {
        let mut payload = Payload {
            content: Content::Files(vec![
                r"C:\synthetic\b".into(),
                r"\\synthetic\share\a".into(),
            ]),
            no_cloud: true,
        };
        assert!(payload.validate().is_ok());
        assert_eq!(
            payload.plain_text().unwrap(),
            "C:\\synthetic\\b\r\n\\\\synthetic\\share\\a"
        );
        payload.content = Content::Files(vec!["relative.txt".into()]);
        assert!(payload.validate().is_err());
    }

    #[test]
    fn path_limits_and_unicode_are_consistent_with_serialized_text() {
        for path in [
            r"C:\synthetic\éé",
            r"C:\synthetic\日本語",
            r"\\synthetic\share\éé",
        ] {
            let payload = Payload {
                content: Content::Files(vec![path.into()]),
                no_cloud: false,
            };
            assert!(payload.validate().is_ok());
        }
        for path in [
            r"C:\NUL.txt",
            r"C:\COM1",
            r"C:\synthetic:stream",
            r"\\.\synthetic",
            r"C:\..\synthetic",
            r"C:\trailing.",
            "C:\\synthetic\0hidden",
        ] {
            let payload = Payload {
                content: Content::Files(vec![path.into()]),
                no_cloud: false,
            };
            assert!(payload.validate().is_err());
        }
        let prefix = r"C:\synthetic\";
        let path = format!("{prefix}{}", "a".repeat(MAX_PATH_BYTES - prefix.len() - 1));
        let mut payload = Payload {
            content: Content::Files(vec![path]),
            no_cloud: false,
        };
        assert!(payload.validate().is_ok());
        if let Content::Files(paths) = &mut payload.content {
            paths[0].push('a');
        }
        assert!(payload.validate().is_err());
    }
}
