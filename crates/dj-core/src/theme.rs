//! Theme engine: TOML schema, `base` inheritance, validation, registry and
//! token interpolation. Themes are data; the renderer only reads `ResolvedTokens`.

use crate::color::Rgb;
use crate::spring::SpringSpec;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const DEFAULT_THEME: &str = "tobacco-tape";
pub const MIN_TEXT_CONTRAST: f32 = 4.5;
/// Token interpolation time on theme switch.
pub const TRANSITION_SECS: f32 = 0.45;

const BUILTINS: [(&str, &str); 4] = [
    ("tobacco-tape", include_str!("../themes/tobacco-tape.toml")),
    (
        "midnight-cassette",
        include_str!("../themes/midnight-cassette.toml"),
    ),
    ("paper-zine", include_str!("../themes/paper-zine.toml")),
    ("clean", include_str!("../themes/clean.toml")),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct SpringToml {
    pub response: f32,
    pub damping: f32,
}

// Every field of every section is optional in a file so that `base` can fill the gaps.
macro_rules! section {
    ($name:ident { $($f:ident : $t:ty),* $(,)? }) => {
        #[derive(Clone, Debug, Default, Deserialize)]
        #[serde(default)]
        pub struct $name { $(pub $f: Option<$t>),* }
        impl $name {
            fn over(&self, base: &Self) -> Self {
                Self { $($f: self.$f.clone().or_else(|| base.$f.clone())),* }
            }
        }
    };
}

section!(Meta {
    name: String,
    author: String,
    base: String,
    mode: Mode
});
section!(ColorSec {
    pill: String,
    surface1: String,
    surface2: String,
    text: String,
    text_dim: String,
    accent: String,
    warn: String,
    highlight: String,
    accent_from_cover: f32,
});
section!(TextureSec {
    grain: f32,
    grain_fps: f32,
    dither_levels: f32,
    dither_cell: f32,
    scanlines: f32,
});
section!(TypeSec {
    title: String,
    label: String,
    digits: String
});
section!(ShapeSec {
    radius_scale: f32,
    hairline: f32,
    inner_glow: f32
});
section!(MotionSec {
    expand: SpringToml,
    collapse: SpringToml
});
section!(VisSec { style: String });

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ThemeFile {
    pub meta: Meta,
    pub color: ColorSec,
    pub texture: TextureSec,
    #[serde(rename = "type")]
    pub typography: TypeSec,
    pub shape: ShapeSec,
    pub motion: MotionSec,
    pub visualizer: VisSec,
}

impl ThemeFile {
    pub fn parse(src: &str) -> Result<Self, String> {
        toml::from_str(src).map_err(|e| e.to_string().replace('\n', " "))
    }

    /// `self` overrides `base`, field by field.
    fn over(&self, base: &Self) -> Self {
        Self {
            meta: self.meta.over(&base.meta),
            color: self.color.over(&base.color),
            texture: self.texture.over(&base.texture),
            typography: self.typography.over(&base.typography),
            shape: self.shape.over(&base.shape),
            motion: self.motion.over(&base.motion),
            visualizer: self.visualizer.over(&base.visualizer),
        }
    }
}

/// Everything the renderer and the island need, fully resolved and validated.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTokens {
    pub name: String,
    pub mode: Mode,
    pub pill: Rgb,
    pub surface1: Rgb,
    pub surface2: Rgb,
    pub text: Rgb,
    pub text_dim: Rgb,
    pub accent: Rgb,
    pub warn: Rgb,
    pub highlight: Rgb,
    pub accent_from_cover: f32,
    pub grain: f32,
    pub grain_fps: f32,
    pub dither_levels: f32,
    pub dither_cell: f32,
    pub scanlines: f32,
    pub font_title: String,
    pub font_label: String,
    pub font_digits: String,
    pub radius_scale: f32,
    pub hairline: f32,
    pub inner_glow: f32,
    pub expand: SpringSpec,
    pub collapse: SpringSpec,
    pub visualizer: String,
}

fn lf(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl ResolvedTokens {
    /// Colours interpolate in OKLab, numbers linearly, names switch at the midpoint.
    pub fn lerp(&self, to: &Self, t: f32) -> Self {
        let pick = |a: &str, b: &str| (if t < 0.5 { a } else { b }).to_string();
        Self {
            name: pick(&self.name, &to.name),
            mode: if t < 0.5 { self.mode } else { to.mode },
            pill: self.pill.lerp(to.pill, t),
            surface1: self.surface1.lerp(to.surface1, t),
            surface2: self.surface2.lerp(to.surface2, t),
            text: self.text.lerp(to.text, t),
            text_dim: self.text_dim.lerp(to.text_dim, t),
            accent: self.accent.lerp(to.accent, t),
            warn: self.warn.lerp(to.warn, t),
            highlight: self.highlight.lerp(to.highlight, t),
            accent_from_cover: lf(self.accent_from_cover, to.accent_from_cover, t),
            grain: lf(self.grain, to.grain, t),
            grain_fps: lf(self.grain_fps, to.grain_fps, t),
            dither_levels: lf(self.dither_levels, to.dither_levels, t),
            dither_cell: lf(self.dither_cell, to.dither_cell, t),
            scanlines: lf(self.scanlines, to.scanlines, t),
            font_title: pick(&self.font_title, &to.font_title),
            font_label: pick(&self.font_label, &to.font_label),
            font_digits: pick(&self.font_digits, &to.font_digits),
            radius_scale: lf(self.radius_scale, to.radius_scale, t),
            hairline: lf(self.hairline, to.hairline, t),
            inner_glow: lf(self.inner_glow, to.inner_glow, t),
            expand: self.expand.lerp(to.expand, t),
            collapse: self.collapse.lerp(to.collapse, t),
            visualizer: pick(&self.visualizer, &to.visualizer),
        }
    }
}

fn req<T>(v: Option<T>, key: &str) -> Result<T, String> {
    v.ok_or_else(|| format!("missing `{key}` (no `base` supplies it)"))
}

fn range(v: f32, lo: f32, hi: f32, key: &str) -> Result<f32, String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(format!("`{key}` = {v} is out of range {lo}..={hi}"))
    }
}

fn spring(s: SpringToml, key: &str) -> Result<SpringSpec, String> {
    Ok(SpringSpec::new(
        range(s.response, 0.1, 2.0, &format!("{key}.response"))?,
        range(s.damping, 0.2, 2.0, &format!("{key}.damping"))?,
    ))
}

/// Turns a fully merged file into validated tokens. Rejects low-contrast themes.
pub fn resolve_tokens(f: &ThemeFile) -> Result<ResolvedTokens, String> {
    let c = &f.color;
    let col = |v: &Option<String>, k: &str| Rgb::from_hex(&req(v.clone(), k)?);
    let t = ResolvedTokens {
        name: req(f.meta.name.clone(), "meta.name")?,
        mode: req(f.meta.mode, "meta.mode")?,
        pill: col(&c.pill, "color.pill")?,
        surface1: col(&c.surface1, "color.surface1")?,
        surface2: col(&c.surface2, "color.surface2")?,
        text: col(&c.text, "color.text")?,
        text_dim: col(&c.text_dim, "color.text_dim")?,
        accent: col(&c.accent, "color.accent")?,
        warn: col(&c.warn, "color.warn")?,
        highlight: col(&c.highlight, "color.highlight")?,
        accent_from_cover: range(
            req(c.accent_from_cover, "color.accent_from_cover")?,
            0.0,
            1.0,
            "color.accent_from_cover",
        )?,
        grain: range(
            req(f.texture.grain, "texture.grain")?,
            0.0,
            1.0,
            "texture.grain",
        )?,
        grain_fps: range(
            req(f.texture.grain_fps, "texture.grain_fps")?,
            1.0,
            60.0,
            "texture.grain_fps",
        )?,
        dither_levels: range(
            req(f.texture.dither_levels, "texture.dither_levels")?,
            2.0,
            64.0,
            "texture.dither_levels",
        )?,
        dither_cell: range(
            req(f.texture.dither_cell, "texture.dither_cell")?,
            1.0,
            8.0,
            "texture.dither_cell",
        )?,
        scanlines: range(
            req(f.texture.scanlines, "texture.scanlines")?,
            0.0,
            1.0,
            "texture.scanlines",
        )?,
        font_title: req(f.typography.title.clone(), "type.title")?,
        font_label: req(f.typography.label.clone(), "type.label")?,
        font_digits: req(f.typography.digits.clone(), "type.digits")?,
        radius_scale: range(
            req(f.shape.radius_scale, "shape.radius_scale")?,
            0.25,
            2.0,
            "shape.radius_scale",
        )?,
        hairline: range(
            req(f.shape.hairline, "shape.hairline")?,
            0.0,
            1.0,
            "shape.hairline",
        )?,
        inner_glow: range(
            req(f.shape.inner_glow, "shape.inner_glow")?,
            0.0,
            1.0,
            "shape.inner_glow",
        )?,
        expand: spring(req(f.motion.expand, "motion.expand")?, "motion.expand")?,
        collapse: spring(
            req(f.motion.collapse, "motion.collapse")?,
            "motion.collapse",
        )?,
        visualizer: req(f.visualizer.style.clone(), "visualizer.style")?,
    };
    let ratio = t.text.contrast(t.pill);
    if ratio < MIN_TEXT_CONTRAST {
        return Err(format!(
            "text/pill contrast {ratio:.1}:1 is below the required {MIN_TEXT_CONTRAST}:1"
        ));
    }
    Ok(t)
}

#[derive(Debug)]
struct Entry {
    file: ThemeFile,
    user: bool,
}

/// Built-in themes plus the user's `~/.daijobu/themes/*.toml`.
pub struct ThemeRegistry {
    raw: BTreeMap<String, Entry>,
    resolved: BTreeMap<String, ResolvedTokens>,
    /// Built-ins first (fixed order), then user themes sorted by id.
    order: Vec<String>,
    /// `(id, reason)` for user themes that failed to parse, resolve or validate.
    rejected: Vec<(String, String)>,
}

impl ThemeRegistry {
    pub fn with_builtins() -> Self {
        let mut r = Self {
            raw: BTreeMap::new(),
            resolved: BTreeMap::new(),
            order: Vec::new(),
            rejected: Vec::new(),
        };
        r.set_user_themes(std::iter::empty::<(String, String)>());
        r
    }

    /// Replaces all user themes. `files` is `(id, toml source)`. A user file may
    /// override a built-in by id, but only if it validates.
    pub fn set_user_themes(&mut self, files: impl IntoIterator<Item = (String, String)>) {
        self.raw.clear();
        self.rejected.clear();
        let mut builtin_ids = Vec::new();
        for (id, src) in BUILTINS {
            let file = ThemeFile::parse(src).expect("built-in theme must parse");
            self.raw.insert(id.to_string(), Entry { file, user: false });
            builtin_ids.push(id.to_string());
        }
        let mut shadowed: BTreeMap<String, Entry> = BTreeMap::new();
        let mut user_ids = Vec::new();
        for (id, src) in files {
            match ThemeFile::parse(&src) {
                Ok(file) => {
                    if let Some(old) = self.raw.insert(id.clone(), Entry { file, user: true }) {
                        if !old.user {
                            shadowed.insert(id.clone(), old);
                        }
                    }
                    user_ids.push(id);
                }
                Err(e) => self.rejected.push((id, e)),
            }
        }
        self.resolved.clear();
        for id in builtin_ids.iter().chain(user_ids.iter()) {
            match self.resolve_raw(id, &mut Vec::new()) {
                Ok(t) => {
                    self.resolved.insert(id.clone(), t);
                }
                Err(e) => {
                    self.rejected.push((id.clone(), e));
                    if let Some(old) = shadowed.remove(id) {
                        self.raw.insert(id.clone(), old);
                    } else {
                        self.raw.remove(id);
                    }
                }
            }
        }
        // A rejected override restored a built-in whose resolution we still need.
        for id in &builtin_ids {
            if !self.resolved.contains_key(id) {
                if let Ok(t) = self.resolve_raw(id, &mut Vec::new()) {
                    self.resolved.insert(id.clone(), t);
                }
            }
        }
        user_ids.sort();
        user_ids.dedup();
        self.order = builtin_ids;
        self.order.extend(
            user_ids
                .into_iter()
                .filter(|i| self.resolved.contains_key(i) && !BUILTINS.iter().any(|b| b.0 == i)),
        );
    }

    fn merged(&self, id: &str, visiting: &mut Vec<String>) -> Result<ThemeFile, String> {
        if visiting.iter().any(|v| v == id) {
            return Err(format!("`base` cycle: {} -> {id}", visiting.join(" -> ")));
        }
        let entry = self
            .raw
            .get(id)
            .ok_or_else(|| format!("unknown base theme `{id}`"))?;
        visiting.push(id.to_string());
        let merged = match &entry.file.meta.base {
            Some(base) => entry.file.over(&self.merged(base, visiting)?),
            None => entry.file.clone(),
        };
        visiting.pop();
        Ok(merged)
    }

    fn resolve_raw(&self, id: &str, visiting: &mut Vec<String>) -> Result<ResolvedTokens, String> {
        resolve_tokens(&self.merged(id, visiting)?)
    }

    pub fn get(&self, id: &str) -> Option<&ResolvedTokens> {
        self.resolved.get(id)
    }

    /// Ids of valid themes in cycling order.
    pub fn ids(&self) -> &[String] {
        &self.order
    }

    pub fn name_of(&self, id: &str) -> Option<&str> {
        self.resolved.get(id).map(|t| t.name.as_str())
    }

    pub fn rejected(&self) -> &[(String, String)] {
        &self.rejected
    }

    /// The id after `current` in cycling order (wraps; falls back to the first).
    pub fn next_id(&self, current: &str) -> &str {
        let i = self
            .order
            .iter()
            .position(|i| i == current)
            .map_or(0, |i| i + 1);
        &self.order[i % self.order.len()]
    }
}

/// Cross-fades from the previous tokens to a new target over `TRANSITION_SECS`.
pub struct ThemeAnim {
    from: ResolvedTokens,
    to: ResolvedTokens,
    current: ResolvedTokens,
    t: f32,
}

impl ThemeAnim {
    pub fn new(tokens: ResolvedTokens) -> Self {
        Self {
            from: tokens.clone(),
            to: tokens.clone(),
            current: tokens,
            t: 1.0,
        }
    }

    /// Starts a transition from wherever the tokens currently are.
    pub fn retarget(&mut self, to: ResolvedTokens) {
        if to == self.to {
            return;
        }
        self.from = self.current.clone();
        self.to = to;
        self.t = 0.0;
    }

    pub fn step(&mut self, dt: f32) {
        if !self.animating() {
            return;
        }
        self.t = (self.t + dt / TRANSITION_SECS).min(1.0);
        let e = if self.t < 0.5 {
            4.0 * self.t * self.t * self.t
        } else {
            1.0 - (-2.0 * self.t + 2.0).powi(3) / 2.0
        };
        self.current = if self.t >= 1.0 {
            self.to.clone()
        } else {
            self.from.lerp(&self.to, e)
        };
    }

    pub fn animating(&self) -> bool {
        self.t < 1.0
    }

    pub fn current(&self) -> &ResolvedTokens {
        &self.current
    }

    pub fn target(&self) -> &ResolvedTokens {
        &self.to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, src: &str) -> (String, String) {
        (id.to_string(), src.to_string())
    }

    #[test]
    fn builtins_parse_validate_and_are_ordered() {
        let r = ThemeRegistry::with_builtins();
        assert_eq!(
            r.ids(),
            ["tobacco-tape", "midnight-cassette", "paper-zine", "clean"]
        );
        assert!(r.rejected().is_empty());
        let paper = r.get("paper-zine").unwrap();
        assert_eq!(paper.mode, Mode::Light);
        assert_eq!(r.get(DEFAULT_THEME).unwrap().name, "Tobacco Tape");
        for id in r.ids() {
            let t = r.get(id).unwrap();
            assert!(t.text.contrast(t.pill) >= MIN_TEXT_CONTRAST, "{id}");
        }
    }

    #[test]
    fn base_inheritance_overrides_only_given_fields() {
        let mut r = ThemeRegistry::with_builtins();
        r.set_user_themes([user(
            "matcha-cafe",
            r##"[meta]
name = "Matcha Cafe"
base = "tobacco-tape"
[color]
accent = "#A8C686"
"##,
        )]);
        let base = r.get("tobacco-tape").unwrap().clone();
        let m = r.get("matcha-cafe").unwrap();
        assert_eq!(m.name, "Matcha Cafe");
        assert_eq!(m.accent.to_hex(), "#A8C686");
        assert_eq!(m.pill, base.pill);
        assert_eq!(m.expand, base.expand);
        assert_eq!(m.font_digits, "Doto");
        assert_eq!(r.ids().last().unwrap(), "matcha-cafe");
    }

    #[test]
    fn chained_and_cyclic_bases() {
        let mut r = ThemeRegistry::with_builtins();
        r.set_user_themes([
            user("a", "[meta]\nname=\"A\"\nbase=\"b\"\n[texture]\ngrain=0.5"),
            user(
                "b",
                "[meta]\nname=\"B\"\nbase=\"clean\"\n[texture]\ngrain=0.2\nscanlines=0.3",
            ),
            user("x", "[meta]\nname=\"X\"\nbase=\"y\""),
            user("y", "[meta]\nname=\"Y\"\nbase=\"x\""),
        ]);
        let a = r.get("a").unwrap();
        assert_eq!(a.grain, 0.5);
        assert_eq!(a.scanlines, 0.3);
        assert!(r.get("x").is_none() && r.get("y").is_none());
        assert!(r
            .rejected()
            .iter()
            .any(|(id, why)| id == "x" && why.contains("cycle")));
    }

    #[test]
    fn low_contrast_theme_is_rejected_and_flagged() {
        let mut r = ThemeRegistry::with_builtins();
        r.set_user_themes([user(
            "murky",
            "[meta]\nname=\"Murky\"\nbase=\"tobacco-tape\"\n[color]\ntext=\"#2B2520\"",
        )]);
        assert!(r.get("murky").is_none());
        assert!(!r.ids().contains(&"murky".to_string()));
        let (_, why) = r.rejected().iter().find(|(id, _)| id == "murky").unwrap();
        assert!(why.contains("contrast"), "{why}");
    }

    #[test]
    fn invalid_input_is_rejected() {
        let mut r = ThemeRegistry::with_builtins();
        r.set_user_themes([
            user("junk", "this is [ not toml"),
            user("nobase", "[meta]\nname=\"No base\""),
            user(
                "badhex",
                "[meta]\nname=\"Bad\"\nbase=\"clean\"\n[color]\npill=\"red\"",
            ),
            user(
                "badrange",
                "[meta]\nname=\"R\"\nbase=\"clean\"\n[texture]\ngrain=3.0",
            ),
            user("nobasetheme", "[meta]\nname=\"N\"\nbase=\"nope\""),
        ]);
        for id in ["junk", "nobase", "badhex", "badrange", "nobasetheme"] {
            assert!(r.get(id).is_none(), "{id}");
            assert!(r.rejected().iter().any(|(i, _)| i == id), "{id}");
        }
        assert_eq!(r.ids().len(), 4);
    }

    #[test]
    fn invalid_override_of_builtin_keeps_builtin() {
        let mut r = ThemeRegistry::with_builtins();
        r.set_user_themes([user("clean", "[texture]\ngrain=9")]);
        assert_eq!(r.get("clean").unwrap().grain, 0.0);
        assert_eq!(r.rejected().len(), 1);
    }

    #[test]
    fn next_id_cycles_and_recovers_from_unknown() {
        let r = ThemeRegistry::with_builtins();
        assert_eq!(r.next_id("tobacco-tape"), "midnight-cassette");
        assert_eq!(r.next_id("clean"), "tobacco-tape");
        assert_eq!(r.next_id("gone"), "tobacco-tape");
    }

    #[test]
    fn interpolation_and_anim() {
        let r = ThemeRegistry::with_builtins();
        let a = r.get("tobacco-tape").unwrap().clone();
        let b = r.get("paper-zine").unwrap().clone();
        let mid = a.lerp(&b, 0.5);
        assert!((mid.grain - (a.grain + b.grain) / 2.0).abs() < 1e-6);
        assert_eq!(a.lerp(&b, 0.0), a);
        assert_eq!(a.lerp(&b, 1.0), b);

        let mut anim = ThemeAnim::new(a.clone());
        assert!(!anim.animating());
        anim.retarget(b.clone());
        assert!(anim.animating());
        anim.step(TRANSITION_SECS / 2.0);
        assert!(anim.animating());
        assert_ne!(anim.current(), &a);
        assert_ne!(anim.current(), &b);
        anim.step(TRANSITION_SECS);
        assert!(!anim.animating());
        assert_eq!(anim.current(), &b);
    }
}
