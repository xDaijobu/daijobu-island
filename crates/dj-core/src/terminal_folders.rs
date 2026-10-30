use crate::layout::Rect;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const LIMIT: usize = 6;
pub const EXPLORER_LIMIT: usize = 6;
pub const SOURCE_ROW: usize = 8;
pub const FIRST_FOLDER_ROW: usize = 9;
pub const MAX_PATH: usize = 32767;
pub const MAX_HISTORY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct ExplorerSnapshot {
    pub folders: Vec<String>,
    pub active: Option<String>,
    pub incomplete: bool,
}

impl ExplorerSnapshot {
    /// Called on the worker, since filesystem validation may block on a share.
    pub fn bounded(folders: Vec<String>, active: Option<String>, incomplete: bool) -> Self {
        let mut unique = Vec::new();
        for path in active.iter().chain(folders.iter()) {
            if validate(path).is_ok()
                && !unique.iter().any(|p: &String| p.eq_ignore_ascii_case(path))
            {
                unique.push(path.clone());
            }
        }
        let active = active.filter(|p| unique.contains(p));
        let incomplete = incomplete || unique.len() > EXPLORER_LIMIT;
        unique.truncate(EXPLORER_LIMIT);
        Self {
            folders: unique,
            active,
            incomplete,
        }
    }
}

/// Explicit paths remain authoritative even when empty/invalid: the chooser
/// reports validation errors instead of silently launching somewhere else.
pub fn starting_folder<'a>(
    explicit: Option<&'a str>,
    explorer: Option<&'a str>,
    last: &'a str,
    configured: &'a str,
    home: &'a str,
) -> &'a str {
    explicit.or(explorer).unwrap_or({
        if !last.is_empty() {
            last
        } else if !configured.is_empty() {
            configured
        } else {
            home
        }
    })
}

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

    #[test]
    fn explicit_explorer_and_fallback_precedence() {
        assert_eq!(
            starting_folder(Some("explicit"), Some("explorer"), "last", "config", "home"),
            "explicit"
        );
        assert_eq!(
            starting_folder(Some(""), Some("explorer"), "last", "config", "home"),
            ""
        );
        assert_eq!(
            starting_folder(None, Some("explorer"), "last", "config", "home"),
            "explorer"
        );
        assert_eq!(
            starting_folder(None, None, "last", "config", "home"),
            "last"
        );
        assert_eq!(starting_folder(None, None, "", "config", "home"), "config");
        assert_eq!(starting_folder(None, None, "", "", "home"), "home");
    }

    #[test]
    fn explorer_bounds_deduplicates_validates_and_prioritizes_active() {
        let root = std::env::temp_dir().join(format!("dj-explorer-bounds-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut folders = Vec::new();
        for i in 0..12 {
            let p = root.join(i.to_string());
            std::fs::create_dir_all(&p).unwrap();
            folders.push(p.to_str().unwrap().to_string());
        }
        let active = folders[10].clone();
        folders.extend([
            active.clone(),
            "shell:Downloads".into(),
            "::{virtual}".into(),
            "relative".into(),
            std::env::current_exe().unwrap().to_str().unwrap().into(),
        ]);
        let snapshot = ExplorerSnapshot::bounded(folders, Some(active.clone()), false);
        assert_eq!(snapshot.folders.len(), EXPLORER_LIMIT);
        assert_eq!(snapshot.folders[0], active);
        assert_eq!(snapshot.active, Some(active));
        assert!(snapshot.incomplete);
        assert!(snapshot.folders.iter().all(|p| validate(p).is_ok()));
        assert!(row(FIRST_FOLDER_ROW + EXPLORER_LIMIT - 1).y + 21.0 < crate::layout::TERM_H - 16.0);
        let invalid = ExplorerSnapshot::bounded(vec![], Some("shell:Home".into()), false);
        assert!(invalid.active.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
