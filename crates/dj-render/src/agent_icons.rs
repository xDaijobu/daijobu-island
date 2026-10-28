//! Local CC0 Simple Icons vectors; Codex is identified by the OpenAI mark.

use crate::icons::draw_icon;
use crate::paint::{fill_paint, stroke_paint};
use dj_core::features::Icon;
use dj_core::term_agents::Agent;
use skia_safe::{Canvas, Color, Path};

fn path(svg: &str) -> Path {
    let data = svg
        .split_once(" d=\"")
        .and_then(|(_, tail)| tail.split_once('"'))
        .map(|(data, _)| data)
        .expect("bundled logo SVG has a path");
    Path::from_svg(data).expect("bundled logo path is valid")
}

thread_local! {
    static CLAUDE: Path = path(include_str!("../../../assets/icons/claude.svg"));
    static OPENAI: Path = path(include_str!("../../../assets/icons/openai.svg"));
}

/// Full-contrast monochrome mark, shared by idle shortcuts and the new-tab menu.
pub(crate) fn draw(canvas: &Canvas, agent: Option<Agent>, cx: f32, cy: f32, size: f32, ink: Color) {
    let Some(agent) = agent else {
        draw_icon(canvas, Icon::Cli, cx, cy, size, &stroke_paint(ink, 1.6));
        return;
    };
    canvas.save();
    canvas.translate((cx - size / 2.0, cy - size / 2.0));
    canvas.scale((size / 24.0, size / 24.0));
    let draw = |p: &Path| canvas.draw_path(p, &fill_paint(ink));
    match agent {
        Agent::Claude => CLAUDE.with(draw),
        Agent::Codex => OPENAI.with(draw),
    };
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};

    #[test]
    fn both_logo_assets_render_sharp_nonempty_marks_without_spilling() {
        for agent in [None, Some(Agent::Claude), Some(Agent::Codex)] {
            for size in [14.0, 16.0, 24.0, 32.0] {
                let mut surface = surfaces::raster_n32_premul((48, 48)).unwrap();
                surface.canvas().clear(Color::TRANSPARENT);
                draw(surface.canvas(), agent, 24.0, 24.0, size, Color::WHITE);
                let info = ImageInfo::new((48, 48), ColorType::BGRA8888, AlphaType::Premul, None);
                let mut pixels = vec![0u8; 48 * 48 * 4];
                assert!(surface.image_snapshot().read_pixels(
                    &info,
                    &mut pixels,
                    48 * 4,
                    (0, 0),
                    skia_safe::image::CachingHint::Allow,
                ));
                let covered = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] > 0)
                    .count();
                assert!(
                    covered > 25,
                    "{agent:?} size {size}: only {covered} covered pixels"
                );
                for y in 0..48 {
                    for x in 0..48 {
                        if (x as f32 - 24.0).abs() > size / 2.0 + 2.0
                            || (y as f32 - 24.0).abs() > size / 2.0 + 2.0
                        {
                            assert_eq!(pixels[(y * 48 + x) * 4 + 3], 0);
                        }
                    }
                }
            }
        }
    }
}
