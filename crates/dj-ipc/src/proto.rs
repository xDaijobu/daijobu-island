//! The `dj` wire format and the command line. Pure: no Win32, no I/O.
//!
//! One request and one reply travel per pipe connection. Each is a single JSON object,
//! UTF-8, ended by `\n`, at most [`MAX_FRAME`] bytes:
//!
//! ```text
//! -> {"v":1,"cmd":"progress","percent":40,"label":"deploy"}
//! <- {"v":1,"ok":true}
//! <- {"v":1,"ok":false,"error":"unknown theme 'nope'"}
//! ```
//!
//! Text is cleaned on the way in ([`clean_text`]): nothing a script sends can carry
//! control characters, a line break or more than [`MAX_LABEL`] / [`MAX_MESSAGE`] characters.
//! No request names a file, a program or an address; the island only ever shows text.

use serde::{Deserialize, Serialize};

/// Protocol version carried in every frame as `"v"`.
pub const VERSION: u32 = 1;
/// A frame (request or reply, including the final `\n`) is at most this many bytes.
pub const MAX_FRAME: usize = 4096;
/// Longest progress label, in `char`s.
pub const MAX_LABEL: usize = 48;
/// Longest done / notify message, in `char`s.
pub const MAX_MESSAGE: usize = 120;

/// What a script asks the island to do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    /// Show or update the progress pill. `percent: None` is an open-ended progress.
    Progress {
        #[serde(skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
        label: String,
    },
    /// Show a check mark and `message` for a moment.
    Done { message: String },
    /// Show a notice pill.
    Notify { message: String },
    /// Switch to the theme `id`, or list the themes when `None`.
    Theme {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Start a snap, as PrtSc does.
    Snap,
}

/// The island's answer.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Reply {
    pub ok: bool,
    /// Something to print on stdout (a theme list, for example).
    pub text: Option<String>,
    /// Why the request failed (`ok` is false).
    pub error: Option<String>,
}

impl Reply {
    /// Success without output.
    pub fn ok() -> Self {
        Reply {
            ok: true,
            text: None,
            error: None,
        }
    }

    /// Success with output for stdout.
    pub fn text(t: impl Into<String>) -> Self {
        Reply {
            ok: true,
            text: Some(t.into()),
            error: None,
        }
    }

    /// Failure with a one-line reason.
    pub fn error(e: impl Into<String>) -> Self {
        Reply {
            ok: false,
            text: None,
            error: Some(e.into()),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Text cleaning

/// Trims, collapses every run of whitespace to one space, drops control characters and
/// keeps at most `max` `char`s (never a half character; a multi-scalar emoji counts as
/// several). The result never starts or ends with a space.
pub fn clean_text(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            // A line break or tab between words is a space, not nothing.
            pending_space = true;
        } else if c.is_control() {
            continue;
        } else {
            let needed = if pending_space && !out.is_empty() {
                2
            } else {
                1
            };
            if count + needed > max {
                break;
            }
            if needed == 2 {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
            count += needed;
        }
    }
    out
}

/// The text a finished job shows when the caller gave none.
const DEFAULT_DONE: &str = "done";

fn done_message(raw: &str) -> String {
    let m = clean_text(raw, MAX_MESSAGE);
    if m.is_empty() {
        DEFAULT_DONE.to_string()
    } else {
        m
    }
}

/// Cleans every text field the way the island expects to receive it.
fn normalize(r: Request) -> Request {
    match r {
        Request::Progress { percent, label } => Request::Progress {
            percent,
            label: clean_text(&label, MAX_LABEL),
        },
        Request::Done { message } => Request::Done {
            message: done_message(&message),
        },
        Request::Notify { message } => Request::Notify {
            message: clean_text(&message, MAX_MESSAGE),
        },
        Request::Theme { id } => Request::Theme {
            id: id
                .map(|i| clean_text(&i, MAX_LABEL))
                .filter(|i| !i.is_empty()),
        },
        Request::Snap => Request::Snap,
    }
}

// ---------------------------------------------------------------------------------------
// Frames

#[derive(Serialize)]
struct RequestOut<'a> {
    v: u32,
    #[serde(flatten)]
    body: &'a Request,
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
enum RequestIn {
    Progress {
        #[serde(default)]
        percent: Option<i64>,
        #[serde(default)]
        label: Option<String>,
    },
    Done {
        #[serde(default)]
        message: Option<String>,
    },
    Notify {
        #[serde(default)]
        message: Option<String>,
    },
    Theme {
        #[serde(default)]
        id: Option<String>,
    },
    Snap,
}

#[derive(Serialize)]
struct ReplyOut<'a> {
    v: u32,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

#[derive(Deserialize)]
struct ReplyIn {
    ok: bool,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `{"v":1,"cmd":"progress","percent":40,"label":"deploy"}\n`. The text fields are
/// cleaned first, so what is sent always fits a frame and passes [`decode_request`]
/// (except an empty notify message, which the island rejects).
pub fn encode_request(r: &Request) -> Vec<u8> {
    let r = normalize(r.clone());
    let mut bytes = serde_json::to_vec(&RequestOut {
        v: VERSION,
        body: &r,
    })
    .unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    bytes
}

/// The JSON text of a frame: size, UTF-8 and the line terminator checked and removed.
fn frame_text(frame: &[u8]) -> Result<&str, String> {
    if frame.len() > MAX_FRAME {
        return Err(format!("frame is larger than {MAX_FRAME} bytes"));
    }
    let text = std::str::from_utf8(frame).map_err(|_| "frame is not valid UTF-8".to_string())?;
    let text = text.trim();
    if text.is_empty() {
        return Err("empty frame".to_string());
    }
    Ok(text)
}

/// Parses a frame to a JSON object and checks its `"v"`.
fn frame_object(frame: &[u8]) -> Result<serde_json::Value, String> {
    let text = frame_text(frame)?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("bad JSON: {e}"))?;
    if !value.is_object() {
        return Err("a frame must be a JSON object".to_string());
    }
    match value.get("v") {
        None => Err("missing protocol version \"v\"".to_string()),
        Some(v) if v.as_u64() == Some(u64::from(VERSION)) => Ok(value),
        Some(v) => Err(format!(
            "unsupported protocol version {v} (this island speaks {VERSION})"
        )),
    }
}

/// Parses and validates one request frame. Rejects: more than [`MAX_FRAME`] bytes, not
/// UTF-8, bad JSON, another protocol version, an unknown `cmd`, a percent outside 0-100
/// and an empty notify message. Cleans the text fields ([`clean_text`]); an empty done
/// message becomes "done".
pub fn decode_request(frame: &[u8]) -> Result<Request, String> {
    let value = frame_object(frame)?;
    let raw: RequestIn = serde_json::from_value(value).map_err(|e| format!("bad request: {e}"))?;
    let request = match raw {
        RequestIn::Progress { percent, label } => {
            let percent = match percent {
                None => None,
                Some(p) if (0..=100).contains(&p) => Some(p as u8),
                Some(p) => return Err(format!("percent must be 0-100, got {p}")),
            };
            Request::Progress {
                percent,
                label: label.unwrap_or_default(),
            }
        }
        RequestIn::Done { message } => Request::Done {
            message: message.unwrap_or_default(),
        },
        RequestIn::Notify { message } => Request::Notify {
            message: message.unwrap_or_default(),
        },
        RequestIn::Theme { id } => Request::Theme { id },
        RequestIn::Snap => Request::Snap,
    };
    let request = normalize(request);
    if matches!(&request, Request::Notify { message } if message.is_empty()) {
        return Err("notify needs a message".to_string());
    }
    Ok(request)
}

fn reply_frame(ok: bool, text: Option<&str>, error: Option<&str>) -> Vec<u8> {
    let out = ReplyOut {
        v: VERSION,
        ok,
        text,
        error,
    };
    let mut bytes = serde_json::to_vec(&out).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    bytes
}

/// Byte offset of the `n`-th char of `s` (or its length).
fn byte_index(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// `{"v":1,"ok":true,"text":"..."}\n`. A reply never exceeds [`MAX_FRAME`]: if its text
/// (or error) does not fit one frame, the longer of the two is cut to the longest
/// prefix that does.
pub fn encode_reply(r: &Reply) -> Vec<u8> {
    let mut text = r.text.clone();
    let mut error = r.error.clone();
    let mut bytes = reply_frame(r.ok, text.as_deref(), error.as_deref());
    let chars = |s: &Option<String>| s.as_ref().map_or(0, |s| s.chars().count());
    let text_first = chars(&text) >= chars(&error);
    for pass in 0..2 {
        if bytes.len() <= MAX_FRAME {
            break;
        }
        let cut_text = text_first == (pass == 0);
        let (cut, other) = if cut_text {
            (&mut text, &error)
        } else {
            (&mut error, &text)
        };
        let Some(full) = cut.clone() else { continue };
        let probe = |n: usize| {
            let part = &full[..byte_index(&full, n)];
            if cut_text {
                reply_frame(r.ok, Some(part), other.as_deref())
            } else {
                reply_frame(r.ok, other.as_deref(), Some(part))
            }
        };
        // Binary search for the longest prefix (in chars) that fits.
        let (mut lo, mut hi) = (0, full.chars().count());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if probe(mid).len() <= MAX_FRAME {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        bytes = probe(lo);
        *cut = Some(full[..byte_index(&full, lo)].to_string());
    }
    bytes
}

/// Parses one reply frame. Control characters other than `\n` and `\t` are dropped from
/// the text, so a reply can never smuggle terminal escape sequences into `dj`'s output.
pub fn decode_reply(frame: &[u8]) -> Result<Reply, String> {
    let value = frame_object(frame)?;
    let raw: ReplyIn = serde_json::from_value(value).map_err(|e| format!("bad reply: {e}"))?;
    let strip = |s: String| -> String {
        s.chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect()
    };
    Ok(Reply {
        ok: raw.ok,
        text: raw.text.map(strip),
        error: raw.error.map(strip),
    })
}

// ---------------------------------------------------------------------------------------
// Command line

/// What `dj <args>` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cli {
    /// Send this request to the island.
    Send(Request),
    Help,
    Version,
    /// `dj scene <name>`: answered locally, scenes are not built yet.
    Scene(String),
}

/// The `--help` text.
pub const USAGE: &str = "\
dj - show activity on Daijobu Island from a terminal or a script

USAGE
  dj progress <0-100|n%> [label]   progress pill at that percent
  dj progress [label]              open-ended progress pill (no number)
  dj done [message]                check mark and message for 4 s (default \"done\")
  dj notify <message>              notice pill for 5 s (click it to dismiss)
  dj theme [id]                    list the themes, or switch to one
  dj snap                          start a snap, like PrtSc
  dj scene <name>                  coming soon
  dj --help | --version

EXAMPLES
  dj progress 40 deploy
  dj progress 80% deploy
  cargo build --release && dj done \"build ok\"
  dj notify \"tests failed\"
  dj theme paper-zine

Labels are cut at 48 characters, messages at 120.

EXIT CODES
  0  ok
  1  usage error, or the island reported an error
  2  Daijobu Island is not running";

/// True for a token that is meant as a number (`40`, `40%`, `-1`, `4.5`), so a bad
/// percent is reported instead of becoming a label.
fn looks_numeric(tok: &str) -> bool {
    let t = tok.strip_suffix('%').unwrap_or(tok);
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    let mut digit = false;
    let mut dot = false;
    for c in t.chars() {
        match c {
            '0'..='9' => digit = true,
            '.' if !dot => dot = true,
            _ => return false,
        }
    }
    digit
}

/// `40` or `40%`: a whole number from 0 to 100.
fn parse_percent(tok: &str) -> Result<u8, String> {
    let digits = tok.strip_suffix('%').unwrap_or(tok);
    let bad = || format!("progress must be a whole number from 0 to 100, got '{tok}'");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    match digits.parse::<u32>() {
        Ok(n) if n <= 100 => Ok(n as u8),
        _ => Err(bad()),
    }
}

fn words(args: &[String]) -> String {
    args.join(" ")
}

/// Parses `dj`'s arguments (without the program name). `Err` is a one-line usage error.
pub fn parse_args(args: &[String]) -> Result<Cli, String> {
    let Some(first) = args.first() else {
        return Ok(Cli::Help);
    };
    let rest = &args[1..];
    match first.to_lowercase().as_str() {
        "-h" | "--help" | "help" => Ok(Cli::Help),
        "-v" | "--version" | "version" => Ok(Cli::Version),
        "progress" => {
            let (percent, label) = match rest.first() {
                Some(tok) if looks_numeric(tok) => (Some(parse_percent(tok)?), &rest[1..]),
                _ => (None, rest),
            };
            Ok(Cli::Send(Request::Progress {
                percent,
                label: clean_text(&words(label), MAX_LABEL),
            }))
        }
        "done" => Ok(Cli::Send(Request::Done {
            message: done_message(&words(rest)),
        })),
        "notify" => {
            let message = clean_text(&words(rest), MAX_MESSAGE);
            if message.is_empty() {
                return Err("notify needs a message: dj notify <message>".to_string());
            }
            Ok(Cli::Send(Request::Notify { message }))
        }
        "theme" => match rest {
            [] => Ok(Cli::Send(Request::Theme { id: None })),
            [id] => Ok(Cli::Send(Request::Theme {
                id: Some(clean_text(id, MAX_LABEL)).filter(|i| !i.is_empty()),
            })),
            _ => Err("theme takes one id: dj theme [id]".to_string()),
        },
        "snap" => {
            if rest.is_empty() {
                Ok(Cli::Send(Request::Snap))
            } else {
                Err("snap takes no arguments".to_string())
            }
        }
        "scene" => {
            let name = clean_text(&words(rest), MAX_LABEL);
            if name.is_empty() {
                Err("scene needs a name: dj scene <name>".to_string())
            } else {
                Ok(Cli::Scene(name))
            }
        }
        _ => Err(format!(
            "unknown command '{}'",
            clean_text(first, MAX_LABEL)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn text(frame: &[u8]) -> &str {
        std::str::from_utf8(frame).unwrap()
    }

    fn all_requests() -> Vec<Request> {
        vec![
            Request::Progress {
                percent: Some(40),
                label: "deploy".into(),
            },
            Request::Progress {
                percent: Some(0),
                label: String::new(),
            },
            Request::Progress {
                percent: Some(100),
                label: "x".into(),
            },
            Request::Progress {
                percent: None,
                label: "open ended".into(),
            },
            Request::Progress {
                percent: None,
                label: String::new(),
            },
            Request::Done {
                message: "build ok".into(),
            },
            Request::Notify {
                message: "tests failed".into(),
            },
            Request::Theme {
                id: Some("paper-zine".into()),
            },
            Request::Theme { id: None },
            Request::Snap,
        ]
    }

    // ---- frames: requests ----

    #[test]
    fn request_round_trips() {
        for r in all_requests() {
            let frame = encode_request(&r);
            assert!(frame.ends_with(b"\n"), "{r:?}");
            assert!(frame.len() <= MAX_FRAME);
            assert_eq!(decode_request(&frame), Ok(r.clone()), "{r:?}");
        }
    }

    #[test]
    fn request_wire_format_is_stable() {
        let enc = |r: Request| String::from_utf8(encode_request(&r)).unwrap();
        assert_eq!(
            enc(Request::Progress {
                percent: Some(40),
                label: "deploy".into()
            }),
            "{\"v\":1,\"cmd\":\"progress\",\"percent\":40,\"label\":\"deploy\"}\n"
        );
        assert_eq!(
            enc(Request::Progress {
                percent: None,
                label: "x".into()
            }),
            "{\"v\":1,\"cmd\":\"progress\",\"label\":\"x\"}\n"
        );
        assert_eq!(
            enc(Request::Done {
                message: "ok".into()
            }),
            "{\"v\":1,\"cmd\":\"done\",\"message\":\"ok\"}\n"
        );
        assert_eq!(
            enc(Request::Notify {
                message: "hi".into()
            }),
            "{\"v\":1,\"cmd\":\"notify\",\"message\":\"hi\"}\n"
        );
        assert_eq!(
            enc(Request::Theme { id: None }),
            "{\"v\":1,\"cmd\":\"theme\"}\n"
        );
        assert_eq!(
            enc(Request::Theme {
                id: Some("crt".into())
            }),
            "{\"v\":1,\"cmd\":\"theme\",\"id\":\"crt\"}\n"
        );
        assert_eq!(enc(Request::Snap), "{\"v\":1,\"cmd\":\"snap\"}\n");
    }

    #[test]
    fn encode_cleans_and_bounds_text() {
        let frame = encode_request(&Request::Notify {
            message: format!("  a\tb\u{7}c \n d {}", "x".repeat(500)),
        });
        assert!(frame.len() <= MAX_FRAME);
        match decode_request(&frame).unwrap() {
            Request::Notify { message } => {
                assert!(message.starts_with("a bc d x"));
                assert_eq!(message.chars().count(), MAX_MESSAGE);
            }
            other => panic!("{other:?}"),
        }
        // Even the worst case (4-byte chars) fits one frame.
        let wide = "\u{1F600}".repeat(1000);
        let frame = encode_request(&Request::Progress {
            percent: Some(1),
            label: wide.clone(),
        });
        assert!(frame.len() <= MAX_FRAME);
        let frame = encode_request(&Request::Notify { message: wide });
        assert!(frame.len() <= MAX_FRAME);
    }

    #[test]
    fn decode_accepts_the_frame_limit_and_loose_line_ends() {
        let base = b"{\"v\":1,\"cmd\":\"snap\"}";
        for tail in [&b""[..], b"\n", b"\r\n", b"  \n"] {
            let mut f = base.to_vec();
            f.extend_from_slice(tail);
            assert_eq!(decode_request(&f), Ok(Request::Snap), "{tail:?}");
        }
        // Exactly MAX_FRAME bytes is fine, one more is not.
        let mut f = base.to_vec();
        f.resize(MAX_FRAME - 1, b' ');
        f.push(b'\n');
        assert_eq!(f.len(), MAX_FRAME);
        assert_eq!(decode_request(&f), Ok(Request::Snap));
        f.push(b'\n');
        assert!(decode_request(&f).unwrap_err().contains("larger"));
    }

    #[test]
    fn decode_rejects_bad_frames() {
        let err = |f: &[u8]| decode_request(f).unwrap_err();
        // over the limit, not UTF-8, empty, not JSON, not an object
        assert!(err(&vec![b'x'; MAX_FRAME + 1]).contains("larger"));
        assert!(err(b"{\"v\":1,\"cmd\":\"snap\",\"x\":\"\xff\"}\n").contains("UTF-8"));
        assert!(err(b"").contains("empty"));
        assert!(err(b"\n").contains("empty"));
        assert!(err(b"not json\n").contains("bad JSON"));
        assert!(err(b"[1,2]\n").contains("object"));
        assert!(err(b"\"snap\"\n").contains("object"));
        // two documents are not one frame
        assert!(
            err(b"{\"v\":1,\"cmd\":\"snap\"}\n{\"v\":1,\"cmd\":\"snap\"}\n").contains("bad JSON")
        );
        // version
        assert!(err(b"{\"cmd\":\"snap\"}\n").contains("version"));
        assert!(err(b"{\"v\":2,\"cmd\":\"snap\"}\n").contains("version 2"));
        assert!(err(b"{\"v\":\"1\",\"cmd\":\"snap\"}\n").contains("version"));
        assert!(err(b"{\"v\":1.5,\"cmd\":\"snap\"}\n").contains("version"));
        // command
        assert!(err(b"{\"v\":1}\n").contains("cmd"));
        assert!(err(b"{\"v\":1,\"cmd\":\"reboot\"}\n").contains("reboot"));
        assert!(err(b"{\"v\":1,\"cmd\":\"Snap\"}\n").contains("Snap"));
        assert!(err(b"{\"v\":1,\"cmd\":7}\n").contains("bad request"));
        // percent
        assert!(err(b"{\"v\":1,\"cmd\":\"progress\",\"percent\":101}\n").contains("0-100"));
        assert!(err(b"{\"v\":1,\"cmd\":\"progress\",\"percent\":-1}\n").contains("0-100"));
        assert!(err(b"{\"v\":1,\"cmd\":\"progress\",\"percent\":1000000}\n").contains("0-100"));
        assert!(err(b"{\"v\":1,\"cmd\":\"progress\",\"percent\":4.5}\n").contains("bad request"));
        assert!(err(b"{\"v\":1,\"cmd\":\"progress\",\"percent\":\"40\"}\n").contains("bad request"));
        // notify needs a message
        assert!(err(b"{\"v\":1,\"cmd\":\"notify\"}\n").contains("message"));
        assert!(err(b"{\"v\":1,\"cmd\":\"notify\",\"message\":\"\"}\n").contains("message"));
        assert!(
            err(b"{\"v\":1,\"cmd\":\"notify\",\"message\":\" \\t\\n \"}\n").contains("message")
        );
        assert!(err(b"{\"v\":1,\"cmd\":\"notify\",\"message\":\"\\u0007\"}\n").contains("message"));
        // wrong field types
        assert!(err(b"{\"v\":1,\"cmd\":\"done\",\"message\":5}\n").contains("bad request"));
        assert!(err(b"{\"v\":1,\"cmd\":\"theme\",\"id\":[1]}\n").contains("bad request"));
    }

    #[test]
    fn decode_cleans_fields_and_fills_defaults() {
        let dec = |f: &str| decode_request(f.as_bytes()).unwrap();
        assert_eq!(
            dec("{\"v\":1,\"cmd\":\"progress\",\"percent\":5,\"label\":\"  a\\tb\\u0000  c \"}"),
            Request::Progress {
                percent: Some(5),
                label: "a b c".into()
            }
        );
        let long = "y".repeat(200);
        match dec(&format!(
            "{{\"v\":1,\"cmd\":\"progress\",\"label\":\"{long}\"}}"
        )) {
            Request::Progress { percent, label } => {
                assert_eq!(percent, None);
                assert_eq!(label.chars().count(), MAX_LABEL);
            }
            other => panic!("{other:?}"),
        }
        // a missing or null label is the empty label
        for f in [
            "{\"v\":1,\"cmd\":\"progress\",\"percent\":3}",
            "{\"v\":1,\"cmd\":\"progress\",\"percent\":3,\"label\":null}",
        ] {
            assert_eq!(
                dec(f),
                Request::Progress {
                    percent: Some(3),
                    label: String::new()
                }
            );
        }
        // an empty done message becomes "done"
        for f in [
            "{\"v\":1,\"cmd\":\"done\"}",
            "{\"v\":1,\"cmd\":\"done\",\"message\":\"\"}",
            "{\"v\":1,\"cmd\":\"done\",\"message\":\"  \\n \"}",
        ] {
            assert_eq!(
                dec(f),
                Request::Done {
                    message: "done".into()
                }
            );
        }
        let long = "z".repeat(300);
        match dec(&format!(
            "{{\"v\":1,\"cmd\":\"notify\",\"message\":\"{long}\"}}"
        )) {
            Request::Notify { message } => assert_eq!(message.chars().count(), MAX_MESSAGE),
            other => panic!("{other:?}"),
        }
        // an empty theme id is "list the themes"
        assert_eq!(
            dec("{\"v\":1,\"cmd\":\"theme\",\"id\":\"  \"}"),
            Request::Theme { id: None }
        );
        assert_eq!(
            dec("{\"v\":1,\"cmd\":\"theme\",\"id\":null}"),
            Request::Theme { id: None }
        );
        // unknown fields are ignored
        assert_eq!(
            dec("{\"v\":1,\"cmd\":\"snap\",\"later\":true}"),
            Request::Snap
        );
        // percent bounds
        assert_eq!(
            dec("{\"v\":1,\"cmd\":\"progress\",\"percent\":100}"),
            Request::Progress {
                percent: Some(100),
                label: String::new()
            }
        );
        assert_eq!(
            dec("{\"v\":1,\"cmd\":\"progress\",\"percent\":0}"),
            Request::Progress {
                percent: Some(0),
                label: String::new()
            }
        );
    }

    // ---- frames: replies ----

    #[test]
    fn reply_constructors() {
        assert_eq!(
            Reply::ok(),
            Reply {
                ok: true,
                text: None,
                error: None
            }
        );
        assert_eq!(
            Reply::text("hi"),
            Reply {
                ok: true,
                text: Some("hi".into()),
                error: None
            }
        );
        assert_eq!(
            Reply::error("no"),
            Reply {
                ok: false,
                text: None,
                error: Some("no".into())
            }
        );
        assert_eq!(
            Reply::default(),
            Reply {
                ok: false,
                text: None,
                error: None
            }
        );
    }

    #[test]
    fn reply_round_trips_and_wire_format() {
        for r in [
            Reply::ok(),
            Reply::text("one"),
            Reply::text("a\nb\n* c"),
            Reply::error("unknown theme 'nope'"),
            Reply::default(),
        ] {
            let frame = encode_reply(&r);
            assert!(frame.ends_with(b"\n"));
            assert_eq!(decode_reply(&frame), Ok(r.clone()), "{r:?}");
        }
        assert_eq!(text(&encode_reply(&Reply::ok())), "{\"v\":1,\"ok\":true}\n");
        assert_eq!(
            text(&encode_reply(&Reply::error("busy"))),
            "{\"v\":1,\"ok\":false,\"error\":\"busy\"}\n"
        );
        // A text with line breaks is still one line on the wire.
        let frame = encode_reply(&Reply::text("a\nb"));
        assert_eq!(frame.iter().filter(|&&b| b == b'\n').count(), 1);
    }

    #[test]
    fn reply_is_bounded_to_one_frame() {
        for r in [
            Reply::text("t".repeat(10_000)),
            Reply::text("\u{1F600}\n\"\\".repeat(3_000)),
            Reply::error("e".repeat(9_000)),
        ] {
            let frame = encode_reply(&r);
            assert!(frame.len() <= MAX_FRAME, "{}", frame.len());
            let back = decode_reply(&frame).unwrap();
            assert_eq!(back.ok, r.ok);
            assert_eq!(back.text.is_some(), r.text.is_some());
            assert_eq!(back.error.is_some(), r.error.is_some());
            // the cut keeps as much as fits, not just a stub
            let kept = back.text.or(back.error).unwrap().chars().count();
            assert!(kept > 1000, "{kept}");
        }
    }

    #[test]
    fn decode_reply_rejects_and_sanitizes() {
        let err = |f: &[u8]| decode_reply(f).unwrap_err();
        assert!(err(&vec![b' '; MAX_FRAME + 1]).contains("larger"));
        assert!(err(b"{\"v\":1,\"ok\":true,\"text\":\"\xff\"}\n").contains("UTF-8"));
        assert!(err(b"nope\n").contains("bad JSON"));
        assert!(err(b"{\"ok\":true}\n").contains("version"));
        assert!(err(b"{\"v\":2,\"ok\":true}\n").contains("version 2"));
        assert!(err(b"{\"v\":1}\n").contains("ok"));
        assert!(err(b"{\"v\":1,\"ok\":\"yes\"}\n").contains("bad reply"));
        // control characters (an ESC sequence) never reach the terminal; \n and \t stay
        let r = decode_reply(b"{\"v\":1,\"ok\":true,\"text\":\"a\\u001b[31m\\nb\\tc\\u0007\"}\n")
            .unwrap();
        assert_eq!(r.text.as_deref(), Some("a[31m\nb\tc"));
    }

    // ---- clean_text ----

    #[test]
    fn clean_text_trims_and_collapses() {
        assert_eq!(
            clean_text("  hello   world \t\n again ", 100),
            "hello world again"
        );
        assert_eq!(clean_text("", 10), "");
        assert_eq!(clean_text(" \t\r\n ", 10), "");
        assert_eq!(clean_text("one", 10), "one");
        // non-ASCII whitespace collapses too
        assert_eq!(clean_text("a\u{a0}\u{2003}b", 10), "a b");
    }

    #[test]
    fn clean_text_drops_control_chars() {
        assert_eq!(clean_text("a\u{7}b", 10), "ab");
        assert_eq!(clean_text("\u{1b}[31mred\u{1b}[0m", 20), "[31mred[0m");
        assert_eq!(clean_text("a \u{7} b", 10), "a b");
        assert_eq!(clean_text("\u{0}\u{1}\u{7f}", 10), "");
        // a line break is a space, not a join
        assert_eq!(clean_text("line1\nline2\r\nline3", 30), "line1 line2 line3");
    }

    #[test]
    fn clean_text_cuts_by_chars_not_bytes() {
        assert_eq!(clean_text("abcdef", 3), "abc");
        assert_eq!(clean_text("abcdef", 6), "abcdef");
        assert_eq!(clean_text("abcdef", 0), "");
        // emoji at the cut: one char each, never half of one
        assert_eq!(clean_text("ab\u{1F600}cd", 3), "ab\u{1F600}");
        assert_eq!(clean_text("ab\u{1F600}cd", 2), "ab");
        assert_eq!(
            clean_text("\u{1F600}\u{1F600}\u{1F600}", 2),
            "\u{1F600}\u{1F600}"
        );
        assert_eq!(clean_text("\u{e9}\u{e9}\u{e9}", 2), "\u{e9}\u{e9}");
        // a cut right after a space leaves no trailing space
        assert_eq!(clean_text("abc def", 4), "abc");
        assert_eq!(clean_text("abc def", 5), "abc d");
        assert_eq!(clean_text("a b c", 3), "a b");
        // the limit counts the collapsed text
        assert_eq!(clean_text("a      b", 3), "a b");
    }

    #[test]
    fn clean_text_is_idempotent() {
        for t in [
            "  a \n b  ",
            "x\u{7}y z",
            "\u{1F600} \u{1F600}",
            "abc def ghi",
        ] {
            for max in [0, 1, 4, 5, 50] {
                let once = clean_text(t, max);
                assert_eq!(clean_text(&once, max), once, "{t:?} {max}");
            }
        }
    }

    // ---- parse_args ----

    fn ok(args: &[&str]) -> Cli {
        parse_args(&s(args)).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    fn send(args: &[&str]) -> Request {
        match ok(args) {
            Cli::Send(r) => r,
            other => panic!("{args:?}: {other:?}"),
        }
    }

    fn err(args: &[&str]) -> String {
        parse_args(&s(args)).expect_err(&format!("{args:?}"))
    }

    fn progress(percent: Option<u8>, label: &str) -> Request {
        Request::Progress {
            percent,
            label: label.into(),
        }
    }

    #[test]
    fn parse_help_and_version() {
        assert_eq!(ok(&[]), Cli::Help);
        for a in ["-h", "--help", "help", "HELP"] {
            assert_eq!(ok(&[a]), Cli::Help, "{a}");
        }
        for a in ["-V", "-v", "--version", "version"] {
            assert_eq!(ok(&[a]), Cli::Version, "{a}");
        }
    }

    #[test]
    fn parse_progress_with_a_number() {
        assert_eq!(send(&["progress", "40"]), progress(Some(40), ""));
        assert_eq!(send(&["progress", "40%"]), progress(Some(40), ""));
        assert_eq!(
            send(&["progress", "40", "deploy"]),
            progress(Some(40), "deploy")
        );
        assert_eq!(
            send(&["progress", "40%", "deploy"]),
            progress(Some(40), "deploy")
        );
        assert_eq!(send(&["progress", "0"]), progress(Some(0), ""));
        assert_eq!(send(&["progress", "100"]), progress(Some(100), ""));
        assert_eq!(send(&["progress", "100%"]), progress(Some(100), ""));
        assert_eq!(send(&["progress", "007"]), progress(Some(7), ""));
        // several words make one label
        assert_eq!(
            send(&["progress", "40", "deploy", "to", "prod"]),
            progress(Some(40), "deploy to prod")
        );
        // the label is cleaned and cut
        let long = "w".repeat(100);
        match send(&["progress", "5", &long]) {
            Request::Progress { label, .. } => assert_eq!(label.chars().count(), MAX_LABEL),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            send(&["progress", "5", "  a\tb  ", "\u{7}c"]),
            progress(Some(5), "a b c")
        );
        // the command word is not case sensitive
        assert_eq!(send(&["Progress", "5"]), progress(Some(5), ""));
    }

    #[test]
    fn parse_progress_open_ended() {
        assert_eq!(send(&["progress", "deploy"]), progress(None, "deploy"));
        assert_eq!(
            send(&["progress", "building", "the", "thing"]),
            progress(None, "building the thing")
        );
        // not numeric at the start: a label even when digits follow
        assert_eq!(send(&["progress", "step", "2"]), progress(None, "step 2"));
        assert_eq!(
            send(&["progress", "2024-01-05", "backup"]),
            progress(None, "2024-01-05 backup")
        );
        assert_eq!(send(&["progress", "%"]), progress(None, "%"));
        assert_eq!(send(&["progress", "4x"]), progress(None, "4x"));
        // nothing at all is an open-ended progress with the default label
        assert_eq!(send(&["progress"]), progress(None, ""));
    }

    #[test]
    fn parse_progress_bounds() {
        for bad in [
            "101",
            "101%",
            "-1",
            "4.5",
            "+5",
            "1000",
            "99999999999999999999",
            "5.",
            ".5",
            "-0",
        ] {
            let e = err(&["progress", bad, "x"]);
            assert!(e.contains("0 to 100"), "{bad}: {e}");
        }
    }

    #[test]
    fn parse_done() {
        let done = |m: &str| Request::Done { message: m.into() };
        assert_eq!(send(&["done"]), done("done"));
        assert_eq!(send(&["done", "build ok"]), done("build ok"));
        assert_eq!(send(&["done", "build", "ok"]), done("build ok"));
        assert_eq!(send(&["done", "  "]), done("done"));
        let long = "m".repeat(300);
        match send(&["done", &long]) {
            Request::Done { message } => assert_eq!(message.chars().count(), MAX_MESSAGE),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_notify() {
        let notify = |m: &str| Request::Notify { message: m.into() };
        assert_eq!(send(&["notify", "tests failed"]), notify("tests failed"));
        assert_eq!(send(&["notify", "tests", "failed"]), notify("tests failed"));
        assert_eq!(send(&["notify", " a\tb "]), notify("a b"));
        assert!(err(&["notify"]).contains("message"));
        assert!(err(&["notify", ""]).contains("message"));
        assert!(err(&["notify", "  ", "\u{7}"]).contains("message"));
        let long = "n".repeat(500);
        match send(&["notify", &long]) {
            Request::Notify { message } => assert_eq!(message.chars().count(), MAX_MESSAGE),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_theme_snap_scene() {
        assert_eq!(send(&["theme"]), Request::Theme { id: None });
        assert_eq!(
            send(&["theme", "paper-zine"]),
            Request::Theme {
                id: Some("paper-zine".into())
            }
        );
        assert_eq!(send(&["theme", ""]), Request::Theme { id: None });
        assert!(err(&["theme", "a", "b"]).contains("one id"));
        assert_eq!(send(&["snap"]), Request::Snap);
        assert!(err(&["snap", "now"]).contains("no arguments"));
        assert_eq!(ok(&["scene", "focus"]), Cli::Scene("focus".into()));
        assert_eq!(
            ok(&["scene", "deep", "work"]),
            Cli::Scene("deep work".into())
        );
        assert!(err(&["scene"]).contains("name"));
    }

    #[test]
    fn parse_unknown_command() {
        let e = err(&["frobnicate", "now"]);
        assert!(e.contains("unknown command 'frobnicate'"), "{e}");
        // a hostile command word cannot smuggle control characters into the message
        let e = err(&["\u{1b}[2J"]);
        assert!(!e.contains('\u{1b}'));
        assert!(!err(&[""]).is_empty());
    }

    #[test]
    fn usage_lists_every_command_and_the_exit_codes() {
        for needle in [
            "dj progress",
            "dj done",
            "dj notify",
            "dj theme",
            "dj snap",
            "dj scene",
            "--help",
            "--version",
            "EXAMPLES",
            "EXIT CODES",
            "not running",
        ] {
            assert!(USAGE.contains(needle), "USAGE lacks {needle:?}");
        }
        assert!(!USAGE.ends_with('\n'));
        assert!(USAGE.lines().all(|l| l.chars().count() <= 100));
    }
}
