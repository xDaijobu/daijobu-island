//! Pure normalization of owned clipboard snapshots. Error messages are categories
//! only: never include clipboard bytes or decoded values in a diagnostic.
use crate::{image, RawFormat, RawSnapshot};
use dj_core::clipboard_types::{
    Content, Payload, MAX_PATHS, MAX_PATH_BYTES, MAX_PNG_BYTES, MAX_RAW_BYTES, MAX_RICH_BYTES,
    MAX_TEXT_BYTES,
};

fn invalid() -> String {
    "Invalid clipboard format".into()
}

pub fn normalize(snapshot: &RawSnapshot) -> Result<Payload, String> {
    let mut total = 0usize;
    let mut seen = Vec::new();
    for part in &snapshot.parts {
        total = total.checked_add(part.bytes.len()).ok_or_else(invalid)?;
        if total > MAX_RAW_BYTES {
            return Err("Clipboard snapshot exceeds limit".into());
        }
        if seen.contains(&part.format) {
            return Err(invalid());
        }
        seen.push(part.format);
        let limit = match part.format {
            RawFormat::Unicode => MAX_TEXT_BYTES * 2 + 2,
            RawFormat::Html | RawFormat::Rtf => MAX_RICH_BYTES,
            RawFormat::Png => MAX_PNG_BYTES,
            RawFormat::DibV5 | RawFormat::Dib => MAX_RAW_BYTES,
            RawFormat::Files => MAX_PATH_BYTES * 2 + 22,
        };
        if part.bytes.len() > limit {
            return Err("Clipboard format exceeds limit".into());
        }
    }
    let get = |format| {
        snapshot
            .parts
            .iter()
            .find(|part| part.format == format)
            .map(|part| part.bytes.as_slice())
    };
    let content = if let Some(bytes) = get(RawFormat::Files) {
        Content::Files(decode_files(bytes)?)
    } else if [RawFormat::Png, RawFormat::DibV5, RawFormat::Dib]
        .iter()
        .any(|&format| get(format).is_some())
    {
        let mut result = Err("Unsupported clipboard image".into());
        for format in [RawFormat::Png, RawFormat::DibV5, RawFormat::Dib] {
            if let Some(bytes) = get(format) {
                result = if format == RawFormat::Png {
                    image::decode_png(bytes)
                } else {
                    image::decode_dib(bytes)
                };
                if result.is_ok() {
                    break;
                }
            }
        }
        Content::Image(result?)
    } else {
        let bytes =
            get(RawFormat::Unicode).ok_or_else(|| "Unsupported clipboard content".to_string())?;
        let text = unicode_text(bytes)?;
        // Rich formats are never parsed for display, executed or fetched. Invalid
        // alternatives are omitted while the safe Unicode fallback is retained.
        let html = get(RawFormat::Html)
            .filter(|bytes| valid_html(bytes))
            .map(<[u8]>::to_vec);
        let rtf = get(RawFormat::Rtf)
            .filter(|bytes| valid_rtf(bytes))
            .map(<[u8]>::to_vec);
        Content::Text { text, html, rtf }
    };
    Ok(Payload {
        content,
        no_cloud: snapshot.no_cloud,
    })
}

fn unicode_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return Err(invalid());
    }
    let words: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let end = words.iter().position(|&w| w == 0).ok_or_else(invalid)?;
    if end == 0 || words[end..].iter().any(|&w| w != 0) {
        return Err(invalid());
    }
    let text = String::from_utf16(&words[..end]).map_err(|_| invalid())?;
    if text.len() > MAX_TEXT_BYTES {
        return Err("Clipboard text exceeds limit".into());
    }
    Ok(text)
}

// CF_HTML offsets count UTF-8 bytes, not characters. Header syntax supports CR,
// LF and CRLF and the specified context-free StartHTML/EndHTML == -1 form.
// https://learn.microsoft.com/windows/win32/dataxchg/html-clipboard-format
fn valid_html(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > MAX_RICH_BYTES {
        return false;
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    if bytes[end..].iter().any(|&b| b != 0) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(&bytes[..end]) else {
        return false;
    };
    let Some((fields, cursor)) = html_header(&bytes[..end]) else {
        return false;
    };
    let [Some(start), Some(finish), Some(fragment), Some(fragment_end), selection, selection_end] =
        fields
    else {
        return false;
    };
    let range = |a: i64, b: i64| {
        a >= cursor as i64
            && b >= a
            && b <= end as i64
            && text.is_char_boundary(a as usize)
            && text.is_char_boundary(b as usize)
    };
    if !range(fragment, fragment_end) {
        return false;
    }
    if !((start == -1 && finish == -1)
        || (range(start, finish) && start <= fragment && fragment_end <= finish))
    {
        return false;
    }
    match (selection, selection_end) {
        (None, None) => true,
        (Some(a), Some(b)) => range(a, b) && fragment <= a && b <= fragment_end,
        _ => false,
    }
}

// Only the bounded ASCII header is inspected before native allocation padding
// is removed. The complete UTF-8 and offset checks still run on the logical data.
fn html_header(bytes: &[u8]) -> Option<([Option<i64>; 6], usize)> {
    let bytes = &bytes[..bytes.len().min(16 * 1024)];
    let mut fields: [Option<i64>; 6] = [None; 6];
    let keys = [
        "StartHTML",
        "EndHTML",
        "StartFragment",
        "EndFragment",
        "StartSelection",
        "EndSelection",
    ];
    let mut cursor = 0;
    let mut version = false;
    while cursor < bytes.len() {
        if bytes[cursor] == b'<' {
            break;
        }
        if fields[2].is_some_and(|n| n >= 0 && cursor >= n as usize) {
            break;
        }
        if fields[0].is_some_and(|n| n >= 0 && cursor >= n as usize) {
            break;
        }
        let length = bytes[cursor..]
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')?;
        let line_end = cursor + length;
        let line = std::str::from_utf8(&bytes[cursor..line_end]).ok()?;
        if !line.is_ascii() {
            return None;
        }
        let (key, value) = line.split_once(':')?;
        if key == "Version" {
            if version || !matches!(value.trim(), "0.9" | "1.0") {
                return None;
            }
            version = true;
        } else if let Some(index) = keys.iter().position(|&k| k == key) {
            let value = value.trim();
            if fields[index].is_some()
                || (value != "-1"
                    && (value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit())))
            {
                return None;
            }
            let value = value.parse::<i64>().ok()?;
            fields[index] = Some(value);
        }
        cursor = line_end + 1;
        if bytes[line_end] == b'\r' && bytes.get(cursor) == Some(&b'\n') {
            cursor += 1;
        }
    }
    if !version || cursor >= 16 * 1024 {
        return None;
    }
    Some((fields, cursor))
}

/// Native HGLOBAL boundary only: discard allocation bytes beyond the declared
/// HTML end. Context-free HTML has no enclosing context, so keep through the
/// fragment end; full validation rejects ends inside the header or UTF-8 units.
pub(crate) fn html_logical_end(bytes: &[u8]) -> Option<usize> {
    let (fields, _) = html_header(bytes)?;
    let end = match (fields[0]?, fields[1]?) {
        (-1, -1) => fields[3]?,
        (_, end) if end >= 0 => end,
        _ => return None,
    };
    let end = usize::try_from(end).ok()?;
    if end > MAX_RICH_BYTES {
        return None;
    }
    valid_html(bytes.get(..end)?).then_some(end)
}

/// Structural validation only; rich bytes are opaque replay alternatives. Scan
/// escaped braces and bounded binary runs without recursion or interpreting RTF.
/// https://learn.microsoft.com/previous-versions/office/developer/office2000/aa140284(v=office.10)
fn valid_rtf(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_RICH_BYTES
        && rtf_logical_end(bytes).is_some_and(|end| {
            bytes[end..]
                .iter()
                .all(|b| *b == 0 || b.is_ascii_whitespace())
        })
}

/// Locate the outer group's close without treating escaped or binary braces
/// (including NULs inside binary data) as syntax. Callers own trailer policy.
pub(crate) fn rtf_logical_end(bytes: &[u8]) -> Option<usize> {
    let bytes = &bytes[..bytes.len().min(MAX_RICH_BYTES)];
    if !bytes.starts_with(b"{\\rtf1")
        || !bytes
            .get(6)
            .is_some_and(|b| !b.is_ascii_alphanumeric() && *b != b'-')
    {
        return None;
    }
    let mut cursor = 0;
    let mut depth = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' => {
                depth += 1;
                if depth > 1024 {
                    return None;
                }
                cursor += 1;
            }
            b'}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            b'\\' => {
                cursor += 1;
                let &next = bytes.get(cursor)?;
                if next == 0 {
                    return None;
                }
                if next == b'\'' {
                    if !bytes
                        .get(cursor + 1..cursor + 3)
                        .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
                    {
                        return None;
                    }
                    cursor += 3;
                } else if next.is_ascii_alphabetic() {
                    let start = cursor;
                    while bytes.get(cursor).is_some_and(u8::is_ascii_alphabetic) {
                        cursor += 1;
                    }
                    if cursor - start > 32 {
                        return None;
                    }
                    let binary = &bytes[start..cursor] == b"bin";
                    let number = cursor;
                    if bytes.get(cursor) == Some(&b'-') {
                        cursor += 1;
                    }
                    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                        cursor += 1;
                    }
                    let numeric_end = cursor;
                    if bytes.get(cursor) == Some(&b' ') {
                        cursor += 1;
                    }
                    if binary {
                        let number = std::str::from_utf8(&bytes[number..numeric_end])
                            .ok()?
                            .parse::<usize>()
                            .ok()?;
                        let end = cursor
                            .checked_add(number)
                            .filter(|&end| end <= bytes.len())?;
                        cursor = end;
                    }
                } else {
                    cursor += 1;
                }
            }
            0 => return None,
            _ => cursor += 1,
        }
    }
    None
}

// DROPFILES uses a 20-byte header and a double-NUL path list. No filesystem,
// environment expansion or network operations belong in this parser.
// https://learn.microsoft.com/windows/win32/api/shlobj_core/ns-shlobj_core-dropfiles
fn decode_files(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 22 {
        return Err(invalid());
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let offset = word(0) as usize;
    let wide = word(16) != 0;
    if offset < 20 || offset >= bytes.len() || (wide && !offset.is_multiple_of(2)) {
        return Err(invalid());
    }
    let raw = &bytes[offset..];
    let words: Vec<u16> = if wide {
        if !raw.len().is_multiple_of(2) {
            return Err(invalid());
        }
        raw.as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect()
    } else {
        // ANSI bytes have no codepage tag. ASCII is codepage-invariant; guessing
        // an ACP for non-ASCII could silently paste a different path.
        if !raw.is_ascii() {
            return Err("Unsupported clipboard path encoding".into());
        }
        raw.iter().map(|&b| u16::from(b)).collect()
    };
    let mut paths = Vec::new();
    let mut cursor = 0;
    let mut size = 0usize;
    loop {
        let length = words
            .get(cursor..)
            .and_then(|tail| tail.iter().position(|&w| w == 0))
            .ok_or_else(invalid)?;
        if length == 0 {
            if paths.is_empty() || words[cursor..].iter().any(|&w| w != 0) {
                return Err(invalid());
            }
            return Ok(paths);
        }
        if paths.len() == MAX_PATHS {
            return Err("Clipboard path count exceeds limit".into());
        }
        let path = String::from_utf16(&words[cursor..cursor + length]).map_err(|_| invalid())?;
        if !absolute_path(&path) {
            return Err("Invalid clipboard path".into());
        }
        size = size
            .checked_add(path.len())
            .and_then(|n| n.checked_add(1))
            .ok_or_else(invalid)?;
        if size > MAX_PATH_BYTES {
            return Err("Clipboard paths exceed limit".into());
        }
        paths.push(path);
        cursor += length + 1;
    }
}

fn absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '<' | '>' | '|' | '?' | '*'))
    {
        return false;
    }
    let tail = if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        &path[3..]
    } else if let Some(unc) = path.strip_prefix("\\\\") {
        let mut parts = unc.split(['\\', '/']);
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        if server.is_empty()
            || share.is_empty()
            || matches!(server, "." | "..")
            || matches!(share, "." | "..")
        {
            return false;
        }
        unc
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
        if matches!(part, "." | "..") || part.ends_with(['.', ' ']) {
            return false;
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawPart;

    fn wide(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn snapshot(parts: Vec<(RawFormat, Vec<u8>)>) -> RawSnapshot {
        RawSnapshot {
            sequence: 1,
            generation: 1,
            at_ms: 0,
            source: None,
            no_cloud: true,
            parts: parts
                .into_iter()
                .map(|(format, bytes)| RawPart { format, bytes })
                .collect(),
        }
    }

    fn files(paths: &[&str], wide: bool) -> Vec<u8> {
        let mut bytes = vec![0; 20];
        bytes[..4].copy_from_slice(&20u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&u32::from(wide).to_le_bytes());
        for path in paths {
            if wide {
                bytes.extend(super::tests::wide(path));
            } else {
                bytes.extend_from_slice(path.as_bytes());
                bytes.push(0);
            }
        }
        bytes.extend(if wide { vec![0, 0] } else { vec![0] });
        bytes
    }

    fn html(fragment: &str) -> Vec<u8> {
        let context = format!("<html><!--StartFragment-->{fragment}<!--EndFragment--></html>");
        let placeholder = "Version:1.0\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
        let start = placeholder.len();
        let fragment_start = start + "<html><!--StartFragment-->".len();
        format!("Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{:010}\r\nStartFragment:{fragment_start:010}\r\nEndFragment:{:010}\r\n{context}", start + context.len(), fragment_start + fragment.len()).into_bytes()
    }

    #[test]
    fn unicode_exact_and_no_cloud_are_preserved() {
        let payload = normalize(&snapshot(vec![(
            RawFormat::Unicode,
            wide("fixture 🙂 e\u{301}\r\n"),
        )]))
        .unwrap();
        assert!(payload.no_cloud);
        assert!(
            matches!(payload.content, Content::Text { text, html: None, rtf: None } if text == "fixture 🙂 e\u{301}\r\n")
        );
    }

    #[test]
    fn unicode_requires_terminator_valid_surrogates_and_even_length() {
        for bytes in [
            vec![],
            vec![1],
            vec![65, 0],
            vec![0, 216, 0, 0],
            vec![0, 220, 0, 0],
            vec![65, 0, 0, 0, 66, 0],
            vec![0, 0],
        ] {
            assert!(normalize(&snapshot(vec![(RawFormat::Unicode, bytes)])).is_err());
        }
    }

    #[test]
    fn unicode_byte_budget_checks_utf8_and_native_bytes() {
        assert!(normalize(&snapshot(vec![(
            RawFormat::Unicode,
            wide(&"a".repeat(MAX_TEXT_BYTES))
        )]))
        .is_ok());
        assert!(normalize(&snapshot(vec![(
            RawFormat::Unicode,
            wide(&"a".repeat(MAX_TEXT_BYTES + 1))
        )]))
        .is_err());
        assert!(normalize(&snapshot(vec![(
            RawFormat::Unicode,
            wide(&"界".repeat(MAX_TEXT_BYTES / 3 + 1))
        )]))
        .is_err());
    }

    #[test]
    fn valid_rich_alternatives_retained_exactly() {
        let html = html("<b>fixture🙂</b>");
        let rtf = br"{\rtf1\ansi fixture\{x\} \'e9}".to_vec();
        let payload = normalize(&snapshot(vec![
            (RawFormat::Unicode, wide("fixture")),
            (RawFormat::Html, html.clone()),
            (RawFormat::Rtf, rtf.clone()),
        ]))
        .unwrap();
        assert!(
            matches!(payload.content, Content::Text { html: Some(h), rtf: Some(r), .. } if h == html && r == rtf)
        );
    }

    #[test]
    fn malformed_rich_uses_unicode_fallback_and_rich_only_is_unsupported() {
        let parts = vec![
            (RawFormat::Html, b"<html>fixture</html>".to_vec()),
            (RawFormat::Rtf, br"{\rtf1 unclosed".to_vec()),
        ];
        assert!(normalize(&snapshot(parts.clone())).is_err());
        let mut with_text = parts;
        with_text.push((RawFormat::Unicode, wide("fixture")));
        assert!(matches!(
            normalize(&snapshot(with_text)).unwrap().content,
            Content::Text {
                html: None,
                rtf: None,
                ..
            }
        ));
    }

    #[test]
    fn html_offsets_are_bytes_and_cannot_split_utf8() {
        let valid = html("é");
        assert!(valid_html(&valid));
        let text = String::from_utf8(valid.clone()).unwrap();
        let at = text.find("é").unwrap();
        let invalid = text.replace(
            &format!("EndFragment:{:010}", at + 2),
            &format!("EndFragment:{:010}", at + 1),
        );
        assert!(!valid_html(invalid.as_bytes()));
        for invalid in [
            text.replace("StartHTML:", "StartHTML:99999999999999999999999"),
            text.replace("EndHTML:", "EndHTML:-"),
            text.replace("Version:1.0", "Version:1.0\r\nStartHTML:0"),
        ] {
            assert!(!valid_html(invalid.as_bytes()));
        }
        let mut invalid = valid;
        invalid.extend_from_slice(b"\0hidden");
        assert!(!valid_html(&invalid));
    }

    #[test]
    fn html_context_free_and_optional_selection_offsets() {
        let header = "Version:1.0\rStartHTML:-1\rEndHTML:-1\rStartFragment:0000000000\rEndFragment:0000000000\r";
        let start = header.len();
        let valid = format!(
            "Version:1.0\rStartHTML:-1\rEndHTML:-1\rStartFragment:{start:010}\rEndFragment:{:010}\r<b>fixture</b>",
            start + 14
        );
        assert!(valid_html(valid.as_bytes()));
        assert!(!valid_html(
            valid.replace("EndHTML:-1", "EndHTML:0").as_bytes()
        ));
        let extra = "StartSelection:0000000000\r";
        assert!(!valid_html(
            valid
                .replacen("StartFragment:", &format!("{extra}StartFragment:"), 1)
                .as_bytes()
        ));
    }

    #[test]
    fn rtf_binary_escapes_and_bounds_are_structural() {
        assert!(valid_rtf(br"{\rtf1\bin3 {\}x}"));
        assert!(valid_rtf(b"{\\rtf1\\bin4 {\0}\\tail}"));
        for bytes in [
            br"{\rtf1\bin99999999999999999999999 x}".as_slice(),
            br"{\rtf1\bin-1 x}",
            br"{\rtf1\bin9 x}",
            br"{\rtf1 \'xz}",
            br"{\rtf1 fixture}trailer",
            br"{\rtf10 fixture}",
            b"{\\rtf1 fixture}\0dirty",
            b"{\\rtf1 fixture\0}",
        ] {
            assert!(!valid_rtf(bytes));
        }
        let nested = format!("{{\\rtf1 {}{}", "{".repeat(1024), "}".repeat(1025));
        assert!(!valid_rtf(nested.as_bytes()));
        assert!(!valid_rtf(&vec![b'x'; MAX_RICH_BYTES + 1]));
    }

    #[test]
    fn unicode_and_ascii_dropfiles_preserve_order_without_probing() {
        for wide in [true, false] {
            let raw = files(
                &[r"C:\synthetic\one.txt", r"\\synthetic-host\share\two"],
                wide,
            );
            assert_eq!(
                decode_files(&raw).unwrap(),
                [r"C:\synthetic\one.txt", r"\\synthetic-host\share\two"]
            );
        }
        assert_eq!(
            decode_files(&files(&["C:\\fixture\\🙂"], true)).unwrap(),
            ["C:\\fixture\\🙂"]
        );
        assert!(decode_files(&files(&["C:\\fixture\\é"], false)).is_err());
    }

    #[test]
    fn dropfiles_rejects_short_offsets_unpaired_utf16_and_missing_double_nul() {
        let valid = files(&[r"C:\fixture"], true);
        for n in 0..valid.len() {
            assert!(decode_files(&valid[..n]).is_err());
        }
        for offset in [0, 19, 21, u32::MAX] {
            let mut invalid = valid.clone();
            invalid[..4].copy_from_slice(&offset.to_le_bytes());
            assert!(decode_files(&invalid).is_err());
        }
        let mut invalid = valid.clone();
        invalid[20..22].copy_from_slice(&0xd800u16.to_le_bytes());
        assert!(decode_files(&invalid).is_err());
        let mut invalid = valid;
        invalid.extend_from_slice(&[1, 0]);
        assert!(decode_files(&invalid).is_err());
        assert!(decode_files(&files(&[], true)).is_err());
    }

    #[test]
    fn paths_require_unambiguous_absolute_windows_names() {
        for path in [
            "relative",
            r"C:relative",
            r"\rooted",
            "https://synthetic.invalid",
            r"\\host",
            r"\\.\pipe\fixture",
            r"\\?\C:\fixture",
            r"C:\fixture:stream",
            r"C:\..\fixture",
            r"C:\NUL.txt",
            r"C:\COM1",
            "C:\\fixture. ",
            "C:\\a\n",
        ] {
            assert!(!absolute_path(path));
        }
        for path in [
            r"C:\",
            "D:/fixture",
            r"\\host\share",
            r"C:\fixture\COM10.txt",
        ] {
            assert!(absolute_path(path));
        }
    }

    #[test]
    fn file_count_and_serialized_budget_are_enforced() {
        assert!(decode_files(&files(&vec![r"C:\fixture"; MAX_PATHS], true)).is_ok());
        assert!(decode_files(&files(&vec![r"C:\fixture"; MAX_PATHS + 1], true)).is_err());
        let path = format!("C:\\{}", "x".repeat(MAX_PATH_BYTES - 4));
        assert!(decode_files(&files(&[&path], true)).is_ok());
        assert!(decode_files(&files(&[&(path + "x")], true)).is_err());
    }

    #[test]
    fn precedence_and_invalid_primary_do_not_capture_unrelated_text() {
        let payload = normalize(&snapshot(vec![
            (RawFormat::Unicode, wide("fixture")),
            (RawFormat::Png, vec![]),
            (RawFormat::Files, files(&[r"C:\fixture"], true)),
        ]))
        .unwrap();
        assert!(matches!(payload.content, Content::Files(_)));
        assert!(normalize(&snapshot(vec![
            (RawFormat::Unicode, wide("fixture")),
            (RawFormat::Png, vec![])
        ]))
        .is_err());
        assert!(normalize(&snapshot(vec![
            (RawFormat::Unicode, wide("fixture")),
            (RawFormat::Files, vec![])
        ]))
        .is_err());
    }

    #[test]
    fn image_beats_text_and_dib_is_a_safe_fallback_for_bad_png() {
        use dj_core::clipboard_types::{ImageData, Thumbnail};
        let source = ImageData {
            width: 1,
            height: 1,
            bgra: vec![10, 20, 30, 40],
            png: vec![],
            thumbnail: Thumbnail {
                width: 0,
                height: 0,
                bgra: vec![],
            },
        };
        let dib = image::encode_dib(&source).unwrap();
        let payload = normalize(&snapshot(vec![
            (RawFormat::Unicode, wide("fixture")),
            (RawFormat::Png, vec![]),
            (RawFormat::Dib, dib),
        ]))
        .unwrap();
        assert!(matches!(payload.content, Content::Image(image) if image.bgra == source.bgra));
    }

    #[test]
    fn arbitrary_short_format_inputs_never_panic() {
        let mut seed = 1u64;
        for length in 0..256 {
            let bytes: Vec<u8> = (0..length)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (seed >> 32) as u8
                })
                .collect();
            let _ = unicode_text(&bytes);
            let _ = decode_files(&bytes);
            let _ = valid_html(&bytes);
            let _ = valid_rtf(&bytes);
            let _ = image::decode_dib(&bytes);
            let _ = image::decode_png(&bytes);
        }
    }

    #[test]
    fn duplicate_and_oversize_parts_are_rejected_before_parsing() {
        assert!(normalize(&snapshot(vec![
            (RawFormat::Unicode, wide("a")),
            (RawFormat::Unicode, wide("b"))
        ]))
        .is_err());
        assert!(normalize(&snapshot(vec![
            (RawFormat::Unicode, wide("fixture")),
            (RawFormat::Html, vec![0; MAX_RICH_BYTES + 1])
        ]))
        .is_err());
        assert!(normalize(&snapshot(vec![(
            RawFormat::Dib,
            vec![0; MAX_RAW_BYTES + 1]
        )]))
        .is_err());
    }
}
