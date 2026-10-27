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

pub fn log(msg: &str) {
    dj_win::system::log(msg);
}
