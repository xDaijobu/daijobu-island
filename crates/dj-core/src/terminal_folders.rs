use crate::layout::Rect;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const LIMIT: usize = 6;
pub const MAX_PATH: usize = 32767;
pub const MAX_HISTORY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct History {
    pub last: [String; 3],
    pub recent: Vec<String>,
}

impl History {
    pub fn parse(text: &str) -> Self {
        if text.len() > MAX_HISTORY_BYTES {
            return Self::default();
        }
        let mut value: Self = toml::from_str(text).unwrap_or_default();
        let mut recent = Vec::with_capacity(LIMIT);
        for path in std::mem::take(&mut value.recent) {
            if valid_text(&path) && !recent.contains(&path) {
                recent.push(path);
                if recent.len() == LIMIT {
                    break;
                }
            }
        }
        value.recent = recent;
        for p in &mut value.last {
            if !valid_text(p) {
                p.clear();
            }
        }
        value
    }

    pub fn remember(&mut self, profile: usize, path: String) {
        self.last[profile] = path.clone();
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(LIMIT);
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("folder history serializes")
    }
}

pub fn valid_text(text: &str) -> bool {
    !text.is_empty()
        && text.encode_utf16().count() < MAX_PATH
        && !text.chars().any(char::is_control)
}

pub fn validate(text: &str) -> Result<PathBuf, String> {
    if !valid_text(text) {
        return Err(
            "Enter a folder path (no control characters, under 32767 UTF-16 units).".into(),
        );
    }
    let path = Path::new(text);
    if !path.is_absolute() {
        return Err("Enter an absolute folder path.".into());
    }
    match path.metadata() {
        Ok(m) if m.is_dir() => Ok(path.to_path_buf()),
        Ok(_) => Err("Path is not a folder.".into()),
        Err(e) => Err(format!("Cannot open folder: {e}")),
    }
}

pub fn row(index: usize) -> Rect {
    Rect {
        x: 24.0,
        y: 72.0 + index as f32 * 22.0,
        w: 720.0,
        h: 21.0,
    }
}

#[derive(Clone, Debug)]
pub struct ChooserView {
    pub rows: Vec<String>,
    pub focus: usize,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_history_profiles_roundtrip_clear() {
        let mut h = History::default();
        for n in 0..20 {
            h.remember(n % 3, format!("C:\\folder {n} 日本語"));
        }
        assert_eq!(h.recent.len(), LIMIT);
        let path = h.recent[2].clone();
        h.remember(0, path.clone());
        assert_eq!(h.recent[0], path);
        assert_eq!(History::parse(&h.to_toml()), h);
        assert_eq!(History::parse("broken"), History::default());
        h = History::default();
        assert!(h.recent.is_empty());
        assert!(h.last.iter().all(String::is_empty));
    }

    #[test]
    fn huge_history_is_bounded_and_first_six_unique_valid_paths_win() {
        assert_eq!(
            History::parse(&"x".repeat(MAX_HISTORY_BYTES + 1)),
            History::default()
        );
        let mut h = History {
            last: [
                "日".repeat(MAX_PATH),
                "😀".repeat(MAX_PATH / 2),
                "ok 日本語".into(),
            ],
            recent: vec![String::new(); 10000],
        };
        for n in 0..20 {
            h.recent
                .extend([format!("folder {n} 日本語"), format!("folder {n} 日本語")]);
        }
        let parsed = History::parse(&h.to_toml());
        assert_eq!(
            parsed.recent,
            (0..LIMIT)
                .map(|n| format!("folder {n} 日本語"))
                .collect::<Vec<_>>()
        );
        assert!(parsed.last[0].is_empty());
        assert_eq!(parsed.last[1], h.last[1]);
        assert_eq!(parsed.last[2], h.last[2]);
        assert_eq!(
            History::parse("last = ['a', 'b', 'c', 'd']").last,
            ["a", "b", "c"]
        );
    }

    #[test]
    fn reject_relative_missing_file_and_controls() {
        for p in ["", ".", "C:relative", "C:\\bad\0", "C:\\bad\n"] {
            assert!(validate(p).is_err());
        }
        let root = std::env::temp_dir();
        assert_eq!(validate(root.to_str().unwrap()).unwrap(), root);
        assert!(validate(
            root.join("daijobu-nonexistent-folder-5b0ab5e8")
                .to_str()
                .unwrap()
        )
        .is_err());
        assert!(validate(std::env::current_exe().unwrap().to_str().unwrap()).is_err());
        assert!(valid_text(r"\\server\share\space 日本語 & $(no command)"));
    }
}
