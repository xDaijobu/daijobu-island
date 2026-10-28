//! Finds the shell's current folder in its output: `OSC 9;9;<path>` (ConPTY forwards it; what
//! Windows Terminal's PowerShell hook prints) and `OSC 7;file://<host>/<path>`.
//!
//! The scanner only watches bytes that were already read; it never changes or holds them back.
//! It buffers only while inside an OSC 7 or OSC 9;9 payload, and drops a payload that is too
//! long, so memory stays bounded.

const MAX_PAYLOAD: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Ground,
    /// Saw ESC.
    Esc,
    /// Inside an OSC payload. `skip`: not one we want (or too long); wait for the terminator.
    Osc {
        skip: bool,
    },
    /// Inside an OSC payload and saw ESC (maybe the start of `ESC \`).
    OscEsc {
        skip: bool,
    },
}

#[derive(Debug)]
pub(crate) struct CwdScanner {
    state: State,
    buf: Vec<u8>,
}

impl Default for CwdScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl CwdScanner {
    pub(crate) fn new() -> Self {
        CwdScanner {
            state: State::Ground,
            buf: Vec::new(),
        }
    }

    /// Scans a chunk; returns the last complete folder found in it. A sequence may be split
    /// across calls at any byte.
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Option<String> {
        let mut found = None;
        for &b in bytes {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => self.after_esc(b),
                State::Osc { skip } => match b {
                    0x07 => found = self.finish(skip).or(found),
                    0x1b => self.state = State::OscEsc { skip },
                    _ if skip => {}
                    _ => self.push(b),
                },
                State::OscEsc { skip } => {
                    if b == b'\\' {
                        found = self.finish(skip).or(found);
                    } else {
                        // A stray ESC aborts the payload; the byte is read as after an ESC.
                        self.buf.clear();
                        self.after_esc(b);
                    }
                }
            }
        }
        found
    }

    fn after_esc(&mut self, b: u8) {
        self.state = match b {
            b']' => {
                self.buf.clear();
                State::Osc { skip: false }
            }
            0x1b => State::Esc,
            _ => State::Ground,
        };
    }

    fn push(&mut self, b: u8) {
        self.buf.push(b);
        if self.buf.len() > MAX_PAYLOAD || !wanted_prefix(&self.buf) {
            self.buf.clear();
            self.state = State::Osc { skip: true };
        }
    }

    fn finish(&mut self, skip: bool) -> Option<String> {
        self.state = State::Ground;
        let payload = std::mem::take(&mut self.buf);
        let result = if skip { None } else { parse(&payload) };
        // Keep the allocation for the next payload.
        self.buf = payload;
        self.buf.clear();
        result
    }
}

/// Whether `buf` can still turn into `7;...` or `9;9;...`.
fn wanted_prefix(buf: &[u8]) -> bool {
    match buf.first() {
        Some(b'7') => buf.len() < 2 || buf[1] == b';',
        Some(b'9') => {
            let n = buf.len().min(4);
            buf[..n] == b"9;9;"[..n]
        }
        _ => false,
    }
}

fn parse(payload: &[u8]) -> Option<String> {
    let path = if let Some(rest) = payload.strip_prefix(b"9;9;") {
        let s = std::str::from_utf8(rest).ok()?;
        let s = s.strip_prefix('"').unwrap_or(s);
        let s = s.strip_suffix('"').unwrap_or(s);
        s.to_string()
    } else {
        let rest = payload.strip_prefix(b"7;file://")?;
        let slash = rest.iter().position(|&b| b == b'/')?;
        file_path(&rest[slash..])?
    };
    (!path.is_empty()).then_some(path)
}

/// `/C:/x%20y` → `C:\x y`; other paths are only decoded.
fn file_path(raw: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            let h = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    let b = s.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        let rest = &s[1..];
        if rest.len() == 2 {
            return Some(format!("{rest}\\"));
        }
        if rest.as_bytes()[2] == b'/' {
            return Some(rest.replace('/', "\\"));
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(bytes: &[u8]) -> Option<String> {
        CwdScanner::new().feed(bytes)
    }

    #[test]
    fn osc_9_9_with_both_terminators() {
        assert_eq!(
            scan(b"\x1b]9;9;C:\\Windows\x07"),
            Some("C:\\Windows".into())
        );
        assert_eq!(
            scan(b"\x1b]9;9;C:\\Windows\x1b\\"),
            Some("C:\\Windows".into())
        );
    }

    #[test]
    fn osc_9_9_quoted_and_unquoted() {
        assert_eq!(
            scan(b"\x1b]9;9;\"C:\\Program Files\"\x07"),
            Some("C:\\Program Files".into())
        );
        assert_eq!(
            scan(b"\x1b]9;9;C:\\Program Files\x07"),
            Some("C:\\Program Files".into())
        );
    }

    #[test]
    fn osc_7_decodes_the_url() {
        assert_eq!(
            scan(b"\x1b]7;file://HOST/C:/Users/me/a%20b\x07"),
            Some("C:\\Users\\me\\a b".into())
        );
        assert_eq!(
            scan(b"\x1b]7;file:///C:/x/y\x1b\\"),
            Some("C:\\x\\y".into())
        );
        assert_eq!(scan(b"\x1b]7;file:///C:\x07"), Some("C:\\".into()));
        assert_eq!(
            scan("\x1b]7;file:///C:/caf%C3%A9\x07".as_bytes()),
            Some("C:\\café".into())
        );
        assert_eq!(scan(b"\x1b]7;file:///home/me\x07"), Some("/home/me".into()));
    }

    #[test]
    fn osc_7_that_is_not_a_file_url_is_ignored() {
        assert_eq!(scan(b"\x1b]7;http://x/C:/a\x07"), None);
        assert_eq!(scan(b"\x1b]7;C:\\a\x07"), None);
        assert_eq!(scan(b"\x1b]7;file://hostonly\x07"), None);
    }

    #[test]
    fn invalid_utf8_bad_percent_and_empty_are_ignored() {
        assert_eq!(scan(b"\x1b]9;9;\xff\xfe\x07"), None);
        assert_eq!(scan(b"\x1b]7;file:///C:/%ff\x07"), None);
        assert_eq!(scan(b"\x1b]7;file:///C:/%zz\x07"), None);
        assert_eq!(scan(b"\x1b]7;file:///C:/%4\x07"), None);
        assert_eq!(scan(b"\x1b]9;9;\x07"), None);
        assert_eq!(scan(b"\x1b]9;9;\"\"\x07"), None);
    }

    #[test]
    fn split_at_every_byte_position() {
        for seq in [
            &b"\x1b]9;9;C:\\Windows\x07"[..],
            b"\x1b]9;9;\"C:\\a b\"\x1b\\",
            b"\x1b]7;file://h/C:/a%20b\x1b\\",
        ] {
            let whole = scan(seq).expect("whole");
            for cut in 0..=seq.len() {
                let mut s = CwdScanner::new();
                let a = s.feed(&seq[..cut]);
                let b = s.feed(&seq[cut..]);
                assert_eq!(a.or(b), Some(whole.clone()), "cut at {cut}");
            }
            // One byte at a time.
            let mut s = CwdScanner::new();
            let mut got = None;
            for &byte in seq {
                got = s.feed(&[byte]).or(got);
            }
            assert_eq!(got, Some(whole));
        }
    }

    #[test]
    fn the_last_sequence_in_a_chunk_wins() {
        let bytes = b"\x1b]9;9;C:\\one\x07text\x1b]7;file:///C:/two\x07\x1b]9;9;C:\\three\x1b\\ok";
        assert_eq!(scan(bytes), Some("C:\\three".into()));
    }

    #[test]
    fn a_result_is_reported_once() {
        let mut s = CwdScanner::new();
        assert!(s.feed(b"\x1b]9;9;C:\\a\x07").is_some());
        assert_eq!(s.feed(b"more output"), None);
    }

    #[test]
    fn other_sequences_are_ignored() {
        assert_eq!(scan(b"\x1b]0;C:\\title\x07"), None);
        assert_eq!(scan(b"\x1b]2;9;9;C:\\x\x07"), None);
        assert_eq!(scan(b"\x1b]9;4;1;50\x07"), None);
        assert_eq!(scan(b"\x1b]9;9\x07"), None);
        assert_eq!(scan(b"\x1b]9;8;C:\\x\x07"), None);
        assert_eq!(scan(b"\x1b]77;file:///C:/x\x07"), None);
        assert_eq!(scan(b"\x1b[31mred\x1b[0m"), None);
        // A wanted one after an unwanted one still works.
        assert_eq!(
            scan(b"\x1b]0;t\x07\x1b]9;4;0\x07\x1b]9;9;C:\\ok\x07"),
            Some("C:\\ok".into())
        );
    }

    #[test]
    fn plain_text_is_never_a_sequence() {
        assert_eq!(scan(b"]9;9;C:\\x\x07"), None);
        assert_eq!(scan(b"echo ]7;file:///C:/x\x07"), None);
        let mut s = CwdScanner::new();
        for _ in 0..1000 {
            assert_eq!(s.feed(b"hello ]9;9;C:\\x world\r\n"), None);
        }
        assert_eq!(s.buf.capacity(), 0, "ordinary output never allocates");
    }

    #[test]
    fn an_oversized_payload_is_dropped_and_the_scanner_recovers() {
        let mut big = b"\x1b]9;9;C:\\".to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD + 10));
        big.push(0x07);
        let mut s = CwdScanner::new();
        assert_eq!(s.feed(&big), None);
        assert!(s.buf.len() <= MAX_PAYLOAD + 1);
        assert_eq!(s.feed(b"\x1b]9;9;C:\\ok\x07"), Some("C:\\ok".into()));
        // Just under the limit is fine.
        let mut ok = b"\x1b]9;9;".to_vec();
        ok.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD - 4));
        ok.push(0x07);
        assert!(s.feed(&ok).is_some());
    }

    #[test]
    fn a_stray_escape_aborts_the_payload() {
        // ESC followed by something other than `\` abandons the OSC.
        assert_eq!(scan(b"\x1b]9;9;C:\\a\x1bXrest\x07"), None);
        // ... and that ESC may itself start a new sequence.
        assert_eq!(
            scan(b"\x1b]9;9;C:\\a\x1b]9;9;C:\\b\x07"),
            Some("C:\\b".into())
        );
        assert_eq!(scan(b"\x1b\x1b]9;9;C:\\c\x07"), Some("C:\\c".into()));
        // After the abort, the later BEL does nothing.
        let mut s = CwdScanner::new();
        assert_eq!(s.feed(b"\x1b]9;9;C:\\a\x1b[0m\x07"), None);
        assert_eq!(s.feed(b"\x1b]9;9;C:\\z\x07"), Some("C:\\z".into()));
    }

    #[test]
    fn garbage_does_not_panic() {
        let mut s = CwdScanner::new();
        let mut x: u32 = 12345;
        for _ in 0..200 {
            let chunk: Vec<u8> = (0..64)
                .map(|_| {
                    x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                    [0x1b, b']', b'7', b'9', b';', 0x07, b'\\', b'/', b'%', b'a']
                        [(x >> 24) as usize % 10]
                })
                .collect();
            let _ = s.feed(&chunk);
        }
    }
}
