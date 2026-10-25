//! SkSL effects. Compiled once at startup, uniforms updated per frame.

use skia_safe::runtime_effect::RuntimeShaderBuilder;
use skia_safe::{Matrix, RuntimeEffect, Shader};

/// Film grain + optional scanlines as a premultiplied overlay (white/black specks).
/// `seed` is the grain frame index (time quantised to the theme's grain fps).
const TEXTURE_SKSL: &str = r#"
uniform float seed;
uniform float grain;
uniform float scan;

// Hoskins hash12: no visible banding or streaks, unlike a bare fract(p * k) hash.
float hash(float2 p) {
    float3 p3 = fract(float3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

half4 main(float2 xy) {
    float n = hash(floor(xy) + float2(seed * 37.0, seed * 59.0)) - 0.5;
    float g = n * grain * 1.2;
    half a = half(abs(g));
    half4 c = g > 0.0 ? half4(a, a, a, a) : half4(0.0, 0.0, 0.0, a);
    half s = half(scan * 0.22 * mod(floor(xy.y), 2.0));
    return c * (1.0 - s) + half4(0.0, 0.0, 0.0, s);
}
"#;

/// Ordered (Bayer 4x4) dithered glow: a soft highlight fading down from the top of
/// the pill, quantised to `levels` steps in `cell`-pixel cells.
const GLOW_SKSL: &str = r#"
uniform float2 origin;
uniform float2 size;
uniform float levels;
uniform float cell;
uniform float strength;
uniform half3 tint;

float bayer2(float2 p) {
    float2 q = mod(floor(p), 2.0);
    return 2.0 * q.x + 3.0 * q.y - 4.0 * q.x * q.y;
}

float bayer4(float2 p) {
    return (4.0 * bayer2(p) + bayer2(p * 0.5)) / 16.0;
}

half4 main(float2 xy) {
    float2 c = floor(xy / cell);
    float2 d = float2((xy.x - (origin.x + size.x * 0.5)) / (size.x * 0.6),
                      (xy.y - origin.y) / (size.y * 0.95));
    float g = clamp(1.0 - length(d), 0.0, 1.0);
    g = g * g;
    float steps = levels - 1.0;
    float q = floor(g * steps + bayer4(c) + 0.0001) / steps;
    half a = half(q * strength);
    return half4(tint * a, a);
}
"#;

pub struct Effects {
    texture: RuntimeShaderBuilder,
    glow: RuntimeShaderBuilder,
}

fn build(sksl: &str, what: &str) -> Result<RuntimeShaderBuilder, String> {
    let effect =
        RuntimeEffect::make_for_shader(sksl, None).map_err(|e| format!("{what} shader: {e}"))?;
    Ok(RuntimeShaderBuilder::new(effect))
}

impl Effects {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            texture: build(TEXTURE_SKSL, "texture")?,
            glow: build(GLOW_SKSL, "glow")?,
        })
    }

    pub fn texture(&mut self, seed: f32, grain: f32, scan: f32) -> Option<Shader> {
        self.texture.set_uniform_float("seed", &[seed]).ok()?;
        self.texture.set_uniform_float("grain", &[grain]).ok()?;
        self.texture.set_uniform_float("scan", &[scan]).ok()?;
        self.texture.make_shader(&Matrix::new_identity())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn glow(
        &mut self,
        origin: [f32; 2],
        size: [f32; 2],
        levels: f32,
        cell: f32,
        strength: f32,
        tint: [f32; 3],
    ) -> Option<Shader> {
        let b = &mut self.glow;
        b.set_uniform_float("origin", &origin).ok()?;
        b.set_uniform_float("size", &size).ok()?;
        b.set_uniform_float("levels", &[levels]).ok()?;
        b.set_uniform_float("cell", &[cell]).ok()?;
        b.set_uniform_float("strength", &[strength]).ok()?;
        b.set_uniform_float("tint", &tint).ok()?;
        b.make_shader(&Matrix::new_identity())
    }
}
