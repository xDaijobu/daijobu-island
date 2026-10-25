//! Embedded OFL fonts (Geist, Geist Mono, Doto) with a system-font fallback.

use skia_safe::font::Edging;
use skia_safe::{Data, Font, FontHinting, FontMgr, FontStyle, Typeface};
use std::collections::HashMap;

static GEIST_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Geist-Regular.ttf");
static GEIST_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Geist-Medium.ttf");
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

pub struct Fonts {
    mgr: FontMgr,
    embedded: HashMap<(&'static str, Weight), Typeface>,
    resolved: HashMap<(String, Weight), Typeface>,
}

impl Fonts {
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

    /// A crisp, unhinted-metrics font at `size` device pixels.
    pub fn font(&mut self, family: &str, weight: Weight, size: f32) -> Font {
        let mut f = Font::from_typeface(self.typeface(family, weight), size);
        f.set_subpixel(false);
        f.set_hinting(FontHinting::None);
        f.set_edging(Edging::AntiAlias);
        f
    }
}
