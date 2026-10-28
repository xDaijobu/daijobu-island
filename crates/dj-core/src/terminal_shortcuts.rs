//! Running island-terminal tabs beside the idle clock. No process discovery or timers.

use crate::layout::Rect;
use crate::term_agents::Agent;
use crate::term_tabs::{TabId, MAX_TABS};

pub const BUTTON_W: f32 = 24.0;
const GAP: f32 = 4.0;
const CLOCK_W: f32 = 64.0;
const CLOCK_GAP: f32 = 10.0;
const PAD: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub id: TabId,
    pub agent: Option<Agent>,
    pub active: bool,
    pub activity: bool,
}

fn content_width(count: usize) -> f32 {
    let count = count.min(MAX_TABS);
    CLOCK_W + CLOCK_GAP + count as f32 * BUTTON_W + count.saturating_sub(1) as f32 * GAP
}

/// Keep the configured width unless the running tabs need more room.
pub fn idle_width(configured: f32, count: usize) -> f32 {
    if count == 0 {
        configured
    } else {
        configured.max(content_width(count) + 2.0 * PAD)
    }
}

/// Clock/lamp column, sharing the same centering as the shortcut buttons.
pub fn clock_rect(body: Rect, count: usize) -> Rect {
    if count == 0 {
        return body;
    }
    Rect::new(
        body.x + (body.w - content_width(count)) / 2.0,
        body.y,
        CLOCK_W,
        body.h,
    )
}

/// Logical window coordinates, derived from the animated body for both paint and hits.
pub fn buttons(body: Rect, count: usize) -> Vec<Rect> {
    let clock = clock_rect(body, count);
    let height = BUTTON_W.min((body.h - 4.0).max(0.0));
    (0..count.min(MAX_TABS))
        .map(|i| {
            Rect::new(
                clock.x + CLOCK_W + CLOCK_GAP + i as f32 * (BUTTON_W + GAP),
                body.y + (body.h - height) / 2.0,
                BUTTON_W,
                height,
            )
        })
        .collect()
}

pub fn hit(body: Rect, count: usize, x: f32, y: f32) -> Option<usize> {
    if !body.contains(x, y) {
        return None;
    }
    buttons(body, count).iter().position(|r| r.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tabs_fit_the_existing_surface_at_every_notch_size() {
        for base in [96.0, 128.0, 240.0] {
            for height in [24.0, 32.0, 44.0] {
                assert_eq!(idle_width(base, 0), base);
                for count in 1..=MAX_TABS {
                    let width = idle_width(base, count);
                    assert!(width <= crate::layout::TERM_W);
                    let body = Rect::new(
                        (crate::layout::window_size().0 - width) / 2.0,
                        crate::layout::TOP_MARGIN,
                        width,
                        height,
                    );
                    let clock = clock_rect(body, count);
                    for (i, r) in buttons(body, count).into_iter().enumerate() {
                        assert!(r.x >= clock.x + clock.w + CLOCK_GAP);
                        assert!(body.contains(r.x, r.y));
                        assert!(body.contains(r.x + r.w - 0.01, r.y + r.h - 0.01));
                        assert_eq!(hit(body, count, r.x + r.w / 2.0, r.y + r.h / 2.0), Some(i));
                    }
                    assert_eq!(
                        hit(body, count, clock.x + 10.0, clock.y + height / 2.0),
                        None
                    );
                    assert_eq!(hit(body, count, body.x - 1.0, body.y), None);
                }
            }
        }
    }

    #[test]
    fn gaps_and_clipped_morph_frames_do_not_activate_a_shortcut() {
        let body = Rect::new(100.0, 6.0, 96.0, 24.0);
        let slots = buttons(body, MAX_TABS);
        for r in slots {
            if r.x < body.x {
                assert_eq!(hit(body, MAX_TABS, r.x + 0.1, r.y + 1.0), None);
            }
        }
        let body = Rect::new(10.0, 6.0, idle_width(128.0, 2), 32.0);
        let slots = buttons(body, 2);
        assert_eq!(hit(body, 2, slots[0].x + BUTTON_W + 1.0, 20.0), None);
        assert!(buttons(body, 0).is_empty());
    }
}
