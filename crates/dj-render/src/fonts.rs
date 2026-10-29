//! Embedded OFL fonts (Geist, Geist Mono, Doto) with a system-font fallback.

use skia_safe::font::Edging;
use skia_safe::font_style::{Slant, Weight as SkWeight, Width};
use skia_safe::{Data, Font, FontHinting, FontMgr, FontStyle, Typeface};
use std::collections::HashMap;

static GEIST_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Geist-Regular.ttf");
pub(crate) static GEIST_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Geist-Medium.ttf");
static GEIST_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Geist-SemiBold.ttf");
static MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Regular.ttf");
static MONO_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Medium.ttf");
static DOTO: &[u8] = include_bytes!("../../../assets/fonts/Doto.ttf");

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Weight {
    Regular,
    Medium,
    SemiBold,
}

/// The most per-character fallback answers kept (the terminal asks for every glyph it
/// draws); the table is emptied when it fills up.
const FALLBACK_CACHE_MAX: usize = 2048;

/// The faces that carry the Nerd Font icons on their own, asked by name for a Private Use
/// Area character the terminal's family lacks (most prompt themes need one installed).
const NERD_SYMBOLS: [&str; 2] = ["Symbols Nerd Font Mono", "Symbols Nerd Font"];

/// The BMP Private Use Area and the two supplementary private planes.
fn is_private_use(ch: char) -> bool {
    matches!(ch as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
}

/// A face for terminal cells in one style, and what of that style the face doesn't have and
/// the painter must fake (an embolden for bold, a skew for italic).
#[derive(Clone)]
pub(crate) struct CellFace {
    pub typeface: Typeface,
    pub fake_bold: bool,
    pub fake_italic: bool,
}

impl CellFace {
    /// The face at `size` device px: unhinted-metrics like `Fonts::font`, with the fake
    /// bold and italic applied.
    pub fn font(&self, size: f32) -> Font {
        let mut f = Font::from_typeface(self.typeface.clone(), size);
        f.set_subpixel(false);
        f.set_hinting(FontHinting::None);
        f.set_edging(Edging::AntiAlias);
        f.set_embolden(self.fake_bold);
        if self.fake_italic {
            f.set_skew_x(-0.25);
        }
        f
    }
}

pub struct Fonts {
    mgr: FontMgr,
    embedded: HashMap<(&'static str, Weight), Typeface>,
    resolved: HashMap<(String, Weight), Typeface>,
    cell_faces: HashMap<(String, bool, bool), CellFace>,
    fallbacks: HashMap<(char, bool, bool), Option<CellFace>>,
}

impl Fonts {
    pub(crate) fn memory_counters(&self) -> dj_core::dev_memory::Row {
        let source: usize = [
            GEIST_REGULAR,
            GEIST_MEDIUM,
            GEIST_SEMIBOLD,
            MONO_REGULAR,
            MONO_MEDIUM,
            DOTO,
        ]
        .iter()
        .map(|b| b.len())
        .sum();
        dj_core::dev_memory::Row::resource("Fonts", format!("{} source", dj_core::dev_memory::bytes(Some(source as u64))), format!("Draw UI/terminal text. {} embedded, {} resolved, {} cell, {} fallback entries; source length, not decoded RAM.", self.embedded.len(), self.resolved.len(), self.cell_faces.len(), self.fallbacks.len()))
    }

    pub fn new() -> Result<Self, String> {
        let mgr = FontMgr::default();
        let mut embedded = HashMap::new();
        let load = |bytes: &'static [u8], what: &str| {
            mgr.new_from_data(Data::new_copy(bytes), None)
                .ok_or_else(|| format!("embedded font {what} failed to load"))
        };
        embedded.insert(
            ("Geist", Weight::Regular),
            load(GEIST_REGULAR, "Geist Regular")?,
        );
        embedded.insert(
            ("Geist", Weight::Medium),
            load(GEIST_MEDIUM, "Geist Medium")?,
        );
        embedded.insert(
            ("Geist", Weight::SemiBold),
            load(GEIST_SEMIBOLD, "Geist SemiBold")?,
        );
        embedded.insert(
            ("Geist Mono", Weight::Regular),
            load(MONO_REGULAR, "Geist Mono Regular")?,
        );
        embedded.insert(
            ("Geist Mono", Weight::Medium),
            load(MONO_MEDIUM, "Geist Mono Medium")?,
        );
        embedded.insert(("Doto", Weight::Regular), load(DOTO, "Doto")?);
        Ok(Self {
            mgr,
            embedded,
            resolved: HashMap::new(),
            cell_faces: HashMap::new(),
            fallbacks: HashMap::new(),
        })
    }

    fn embedded_face(&self, family: &str, weight: Weight) -> Option<Typeface> {
        let fam = match family {
            "Geist" => "Geist",
            "Geist Mono" => "Geist Mono",
            "Doto" => "Doto",
            _ => return None,
        };
        let w = if fam == "Doto" {
            Weight::Regular
        } else if fam == "Geist Mono" && weight == Weight::SemiBold {
            Weight::Medium
        } else {
            weight
        };
        self.embedded.get(&(fam, w)).cloned()
    }

    /// Embedded family first, then a system font by that name, then Geist Mono.
    pub fn typeface(&mut self, family: &str, weight: Weight) -> Typeface {
        if let Some(t) = self.embedded_face(family, weight) {
            return t;
        }
        let key = (family.to_string(), weight);
        if let Some(t) = self.resolved.get(&key) {
            return t.clone();
        }
        let style = match weight {
            Weight::Regular => FontStyle::normal(),
            Weight::Medium => FontStyle::normal(),
            Weight::SemiBold => FontStyle::bold(),
        };
        let face = self
            .mgr
            .match_family_style(family, style)
            .or_else(|| self.embedded_face("Geist Mono", weight))
            .expect("embedded fallback font");
        self.resolved.insert(key, face.clone());
        face
    }

    /// `family` in the requested style for terminal cells: the system family if there is one
    /// by that name (its bold or italic face when it has one), else embedded Geist Mono.
    /// A style the face lacks is faked: Geist Mono has no italic, and its bold is the
    /// Medium face emboldened.
    pub(crate) fn cell_face(&mut self, family: &str, bold: bool, italic: bool) -> CellFace {
        let key = (family.to_string(), bold, italic);
        if let Some(f) = self.cell_faces.get(&key) {
            return f.clone();
        }
        let face = if self.embedded_face(family, Weight::Regular).is_some() {
            self.embedded_cell_face(family, bold, italic)
        } else {
            match self
                .mgr
                .match_family_style(family, Self::style_of(bold, italic))
            {
                Some(tf) => Self::real_style(tf, bold, italic),
                None => self.embedded_cell_face("Geist Mono", bold, italic),
            }
        };
        self.cell_faces.insert(key, face.clone());
        face
    }

    /// A face with a glyph for `ch` from some installed font, in the requested style, for a
    /// character the terminal's own family lacks (CJK, emoji, symbols, Nerd-font icons).
    /// `family` steers the choice toward that family's fallback chain. `None` when no
    /// installed font has it.
    pub(crate) fn fallback_face(
        &mut self,
        family: &str,
        ch: char,
        bold: bool,
        italic: bool,
    ) -> Option<CellFace> {
        let key = (ch, bold, italic);
        if let Some(found) = self.fallbacks.get(&key) {
            return found.clone();
        }
        if self.fallbacks.len() >= FALLBACK_CACHE_MAX {
            self.fallbacks.clear();
        }
        let family = if self.embedded_face(family, Weight::Regular).is_some() {
            ""
        } else {
            family
        };
        let has = |tf: &Typeface| tf.unichar_to_glyph(ch as i32) != 0;
        // Prompt icons (oh-my-posh, starship, Powerline) live in the Private Use Area, where
        // the system's fallback never looks and Windows' own icon fonts would draw other
        // icons: the Nerd Font symbol faces are asked by name first.
        let symbols = if is_private_use(ch) {
            NERD_SYMBOLS.iter().find_map(|name| {
                self.mgr
                    .match_family_style(name, FontStyle::normal())
                    .filter(has)
            })
        } else {
            None
        };
        let found = symbols
            .or_else(|| {
                self.mgr
                    .match_family_style_character(
                        family,
                        Self::style_of(bold, italic),
                        &[],
                        ch as i32,
                    )
                    .filter(has)
            })
            .map(|tf| Self::real_style(tf, bold, italic));
        self.fallbacks.insert(key, found.clone());
        found
    }

    fn style_of(bold: bool, italic: bool) -> FontStyle {
        FontStyle::new(
            if bold {
                SkWeight::BOLD
            } else {
                SkWeight::NORMAL
            },
            Width::NORMAL,
            if italic {
                Slant::Italic
            } else {
                Slant::Upright
            },
        )
    }

    /// `tf` as the font manager gave it: what it lacks of the asked style is faked.
    fn real_style(tf: Typeface, bold: bool, italic: bool) -> CellFace {
        let (fake_bold, fake_italic) = (bold && !tf.is_bold(), italic && !tf.is_italic());
        CellFace {
            typeface: tf,
            fake_bold,
            fake_italic,
        }
    }

    /// An embedded family has no italic, and its bold is the Medium face emboldened.
    fn embedded_cell_face(&self, family: &str, bold: bool, italic: bool) -> CellFace {
        let weight = if bold {
            Weight::Medium
        } else {
            Weight::Regular
        };
        CellFace {
            typeface: self
                .embedded_face(family, weight)
                .expect("embedded terminal font"),
            fake_bold: bold,
            fake_italic: italic,
        }
    }

    /// A crisp, unhinted-metrics font at `size` device pixels.
    pub fn font(&mut self, family: &str, weight: Weight, size: f32) -> Font {
        let mut f = Font::from_typeface(self.typeface(family, weight), size);
        f.set_subpixel(false);
        f.set_hinting(FontHinting::None);
        f.set_edging(Edging::AntiAlias);
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Vitals panel draws these glyphs with the embedded faces. A missing glyph
    /// would show as a box, so check the cmap directly.
    #[test]
    fn vitals_glyphs_are_in_the_embedded_fonts() {
        let mut fonts = Fonts::new().expect("embedded fonts load");
        let label = fonts.typeface("Geist Mono", Weight::Medium);
        for c in "…↓↑·+%".chars() {
            assert_ne!(label.unichar_to_glyph(c as i32), 0, "Geist Mono lacks {c}");
        }
        let digits = fonts.typeface("Doto", Weight::Regular);
        for c in "0123456789".chars() {
            assert_ne!(digits.unichar_to_glyph(c as i32), 0, "Doto lacks {c}");
        }
    }

    #[test]
    fn private_use_ranges() {
        for c in [
            '\u{E000}',
            '\u{E0B0}',
            '\u{F8FF}',
            '\u{F0001}',
            '\u{10FFFD}',
        ] {
            assert!(is_private_use(c), "{:X}", c as u32);
        }
        for c in ['a', '\u{D7FF}', '\u{F900}', '漢', '🙂'] {
            assert!(!is_private_use(c), "{:X}", c as u32);
        }
    }

    /// A Powerline separator comes from the Nerd Font symbols when one is installed (skipped
    /// otherwise), never from a system icon font that happens to use the same code point.
    #[test]
    fn prompt_icons_come_from_the_nerd_symbols() {
        let mut fonts = Fonts::new().expect("embedded fonts load");
        let installed = NERD_SYMBOLS.iter().any(|n| {
            fonts
                .mgr
                .match_family_style(n, FontStyle::normal())
                .is_some()
        });
        if !installed {
            return;
        }
        let face = fonts
            .fallback_face("Cascadia Mono", '\u{E0B0}', false, false)
            .expect("a face for U+E0B0");
        let family = face.typeface.family_name();
        assert!(family.contains("Nerd"), "{family}");
    }
}
