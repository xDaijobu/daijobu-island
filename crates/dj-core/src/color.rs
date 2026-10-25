//! sRGB colour with OKLab interpolation and WCAG contrast.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

impl Rgb {
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    /// Parses `#RRGGBB` (the leading `#` is optional).
    pub fn from_hex(s: &str) -> Result<Self, String> {
        let bad = || format!("invalid colour `{s}` (expected #RRGGBB)");
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 || !h.is_ascii() {
            return Err(bad());
        }
        let ch = |i: usize| {
            u8::from_str_radix(&h[i..i + 2], 16)
                .map(|v| v as f32 / 255.0)
                .map_err(|_| bad())
        };
        Ok(Self::new(ch(0)?, ch(2)?, ch(4)?))
    }

    pub fn to_hex(self) -> String {
        let [r, g, b] = self.to_u8();
        format!("#{r:02X}{g:02X}{b:02X}")
    }

    pub fn to_u8(self) -> [u8; 3] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [q(self.r), q(self.g), q(self.b)]
    }

    pub fn relative_luminance(self) -> f32 {
        0.2126 * to_linear(self.r) + 0.7152 * to_linear(self.g) + 0.0722 * to_linear(self.b)
    }

    /// WCAG contrast ratio, always >= 1.
    pub fn contrast(self, other: Self) -> f32 {
        let (a, b) = (self.relative_luminance(), other.relative_luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn to_oklab(self) -> [f32; 3] {
        let (r, g, b) = (to_linear(self.r), to_linear(self.g), to_linear(self.b));
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }

    fn from_oklab([l, a, b]: [f32; 3]) -> Self {
        let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
        let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
        let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        Self::new(
            from_linear(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s),
            from_linear(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s),
            from_linear(-0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s),
        )
    }

    /// Interpolates in OKLab.
    pub fn lerp(self, other: Self, t: f32) -> Self {
        if t <= 0.0 {
            return self;
        }
        if t >= 1.0 {
            return other;
        }
        let (a, b) = (self.to_oklab(), other.to_oklab());
        Self::from_oklab([
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Rgb::from_hex("#E0A458").unwrap();
        assert_eq!(c.to_hex(), "#E0A458");
        assert!(Rgb::from_hex("#12345").is_err());
        assert!(Rgb::from_hex("#GGGGGG").is_err());
    }

    #[test]
    fn contrast_black_white() {
        let w = Rgb::new(1.0, 1.0, 1.0);
        let k = Rgb::new(0.0, 0.0, 0.0);
        assert!((w.contrast(k) - 21.0).abs() < 0.01);
        assert!((w.contrast(w) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn oklab_lerp_endpoints_and_mid() {
        let a = Rgb::from_hex("#14110F").unwrap();
        let b = Rgb::from_hex("#EFE8DA").unwrap();
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        let m = a.lerp(b, 0.5);
        assert!(m.relative_luminance() > a.relative_luminance());
        assert!(m.relative_luminance() < b.relative_luminance());
        let rt = Rgb::from_oklab(a.to_oklab());
        assert!((rt.r - a.r).abs() < 0.01 && (rt.g - a.g).abs() < 0.01 && (rt.b - a.b).abs() < 0.01);
    }
}
