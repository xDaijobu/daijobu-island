//! `~/.daijobu/`: config, user themes, the sweep cache, the operations log path and a
//! tiny error log.

use dj_core::config::Config;
use dj_core::sweep::SweepCache;
use std::fs;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    dj_win::system::data_dir()
}

pub fn themes_dir() -> PathBuf {
    data_dir().join("themes")
}

fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

pub fn ensure_dirs() {
    if let Err(e) = fs::create_dir_all(themes_dir()) {
        log(&format!("cannot create {}: {e}", themes_dir().display()));
    }
}

pub fn load_config() -> Config {
    fs::read_to_string(config_path())
        .map(|s| Config::parse(&s))
        .unwrap_or_default()
}

pub fn save_config(cfg: &Config) {
    if let Err(e) = fs::write(config_path(), cfg.to_toml()) {
        log(&format!("cannot save config: {e}"));
    }
}

fn sweep_cache_path() -> PathBuf {
    data_dir().join("sweep-cache.toml")
}

/// Every sweep action is appended here (by `dj_sweep::OpLog`).
pub fn operations_log_path() -> PathBuf {
    data_dir().join("operations.log")
}

/// The last reclaimable estimate. Missing or unreadable means "never scanned".
pub fn load_sweep_cache() -> SweepCache {
    fs::read_to_string(sweep_cache_path())
        .map(|s| SweepCache::parse(&s))
        .unwrap_or_default()
}

pub fn save_sweep_cache(cache: &SweepCache) {
    if let Err(e) = fs::write(sweep_cache_path(), cache.to_toml()) {
        log(&format!("cannot save the sweep cache: {e}"));
    }
}

/// `(id, source)` for every `*.toml` in the themes directory; the id is the lowercased file stem.
pub fn read_user_themes() -> Vec<(String, String)> {
    let Ok(rd) = fs::read_dir(themes_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()?.eq_ignore_ascii_case("toml") {
                let id = p.file_stem()?.to_str()?.to_lowercase();
                Some((id, fs::read_to_string(&p).ok()?))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

/// `layouts.toml`: the user's custom Tile layouts. Empty if there is none.
pub fn read_layouts() -> String {
    fs::read_to_string(data_dir().join("layouts.toml")).unwrap_or_default()
}

fn terminal_tabs_path() -> PathBuf {
    data_dir().join("terminal-tabs.toml")
}

/// `terminal-tabs.toml`: the terminal's pinned tabs. Empty if there is none.
pub fn read_terminal_tabs() -> String {
    fs::read_to_string(terminal_tabs_path()).unwrap_or_default()
}

/// Writes the pinned tabs (the file goes when none is left).
pub fn save_terminal_tabs(toml: &str) {
    let path = terminal_tabs_path();
    let result = if toml.trim().is_empty() {
        match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        fs::write(&path, toml)
    };
    if let Err(e) = result {
        log(&format!("terminal tabs: {e}"));
    }
}

pub fn read_terminal_folders() -> dj_core::terminal_folders::History {
    let text =
        fs::File::open(data_dir().join("terminal-folders.toml")).and_then(read_folder_history);
    match text {
        Ok(text) => dj_core::terminal_folders::History::parse(&text),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log(&format!(
                    "Cannot read folder history; using empty history: {e}"
                ));
            }
            dj_core::terminal_folders::History::default()
        }
    }
}

fn read_folder_history(reader: impl std::io::Read) -> std::io::Result<String> {
    use std::io::Read;
    let limit = dj_core::terminal_folders::MAX_HISTORY_BYTES;
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Folder history exceeds 2 MiB.",
        ));
    }
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn save_terminal_folders(text: &str) -> Result<(), String> {
    fs::write(data_dir().join("terminal-folders.toml"), text)
        .map_err(|e| format!("Could not save folder history: {e}"))
}

pub fn log(msg: &str) {
    dj_win::system::log(msg);
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    #[test]
    fn history_reader_caps_bytes_and_rejects_partial_or_invalid_text() {
        let limit = dj_core::terminal_folders::MAX_HISTORY_BYTES;
        let mut input = std::io::Cursor::new(vec![b'x'; limit * 2]);
        assert_eq!(
            read_folder_history(&mut input).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(input.position(), limit as u64 + 1);
        assert_eq!(
            read_folder_history(&b"recent = []"[..]).unwrap(),
            "recent = []"
        );
        assert!(read_folder_history(&[0xff][..]).is_err());
        let text = "日".repeat(limit / 3 + 1);
        assert!(read_folder_history(text.as_bytes()).is_err());
    }
}
