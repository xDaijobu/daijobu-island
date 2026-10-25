//! `~/.daijobu/`: config, user themes and a tiny error log.

use dj_core::config::Config;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".daijobu")
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

pub fn log(msg: &str) {
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join("daijobu.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}
