//! The island state machine: Idle pill <-> Home grid, driven by springs.
//! Time only enters through `step(dt)`, so everything here is deterministic.

use crate::features::{Availability, FEATURES};
use crate::layout::{self, Rect};
use crate::spring::{Spring, SpringSpec};
use crate::theme::ResolvedTokens;

/// Content cross-fade spring (response 0.25 s, zeta 1.0).
const FADE_SPEC: SpringSpec = SpringSpec::new(0.25, 1.0);
const SHAKE_SECS: f32 = 0.4;
const HINT_SECS: f32 = 1.8;
const HINT_FADE_SECS: f32 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandState {
    Idle,
    Home,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickOutcome {
    /// Click landed outside the pill.
    Ignored,
    Expanded,
    Collapsed,
    /// A locked tile was clicked (index into `FEATURES`): shake + "coming soon".
    Locked(usize),
    /// An available tile was clicked. Nothing is available in stage 1.
    Open(usize),
}

/// What the renderer needs to draw one frame of the island. Window-logical px.
#[derive(Clone, Copy, Debug)]
pub struct IslandView {
    pub rect: Rect,
    pub radius: f32,
    /// 0 = idle content, 1 = home content.
    pub fade: f32,
    pub hover: Option<usize>,
    /// Tile index and horizontal offset.
    pub shake: Option<(usize, f32)>,
    /// Tile index and opacity of the "coming soon" hint.
    pub hint: Option<(usize, f32)>,
}

pub struct Island {
    state: IslandState,
    w: Spring,
    h: Spring,
    r: Spring,
    fade: Spring,
    tiles: Vec<Rect>,
    hover: Option<usize>,
    shake: Option<(usize, f32)>,
    hint: Option<(usize, f32)>,
}

impl Island {
    pub fn new(tokens: &ResolvedTokens) -> Self {
        let spec = tokens.expand;
        Self {
            state: IslandState::Idle,
            w: Spring::new(layout::IDLE_W, spec, 0.02),
            h: Spring::new(layout::IDLE_H, spec, 0.02),
            r: Spring::new(layout::IDLE_R * tokens.radius_scale, spec, 0.02),
            fade: Spring::new(0.0, FADE_SPEC, 0.002),
            tiles: layout::home_tiles(),
            hover: None,
            shake: None,
            hint: None,
        }
    }

    pub fn state(&self) -> IslandState {
        self.state
    }

    fn base_size(state: IslandState) -> (f32, f32, f32) {
        match state {
            IslandState::Idle => (layout::IDLE_W, layout::IDLE_H, layout::IDLE_R),
            IslandState::Home => (layout::HOME_W, layout::HOME_H, layout::HOME_R),
        }
    }

    fn set_state(&mut self, state: IslandState, tokens: &ResolvedTokens) {
        self.state = state;
        let (w, h, r) = Self::base_size(state);
        let spec = if state == IslandState::Home { tokens.expand } else { tokens.collapse };
        self.w.retarget(w, spec);
        self.h.retarget(h, spec);
        self.r.retarget(r * tokens.radius_scale, spec);
        self.fade.retarget(if state == IslandState::Home { 1.0 } else { 0.0 }, FADE_SPEC);
        self.hover = None;
    }

    pub fn expand(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Home {
            self.set_state(IslandState::Home, tokens);
        }
    }

    pub fn collapse(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Idle {
            self.set_state(IslandState::Idle, tokens);
        }
    }

    /// Current pill rect in window-logical px (horizontally centred in the window).
    pub fn pill_rect(&self) -> Rect {
        let (win_w, _) = layout::window_size();
        let (w, h) = (self.w.value.max(1.0), self.h.value.max(1.0));
        Rect::new((win_w - w) / 2.0, layout::TOP_MARGIN, w, h)
    }

    fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        let p = self.pill_rect();
        if self.state != IslandState::Home || self.fade.value < 0.5 {
            return None;
        }
        layout::hit_tile(&self.tiles, x - p.x, y - p.y)
    }

    /// `(x, y)` in window-logical px.
    pub fn click(&mut self, x: f32, y: f32, tokens: &ResolvedTokens) -> ClickOutcome {
        if !self.pill_rect().contains(x, y) {
            return ClickOutcome::Ignored;
        }
        if self.state == IslandState::Idle {
            self.expand(tokens);
            return ClickOutcome::Expanded;
        }
        match self.tile_at(x, y) {
            Some(i) if FEATURES[i].availability == Availability::Locked => {
                self.shake = Some((i, 0.0));
                self.hint = Some((i, HINT_SECS));
                ClickOutcome::Locked(i)
            }
            Some(i) => ClickOutcome::Open(i),
            None => {
                self.collapse(tokens);
                ClickOutcome::Collapsed
            }
        }
    }

    /// Returns true when the hovered tile changed (needs a redraw).
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let h = self.tile_at(x, y);
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    pub fn clear_hover(&mut self) -> bool {
        self.hover.take().is_some()
    }

    pub fn step(&mut self, dt: f32, tokens: &ResolvedTokens) {
        // Theme switches change the radius scale and spring params mid-flight.
        let (_, _, base_r) = Self::base_size(self.state);
        self.r.target = base_r * tokens.radius_scale;
        let spec = if self.state == IslandState::Home { tokens.expand } else { tokens.collapse };
        for s in [&mut self.w, &mut self.h, &mut self.r] {
            s.spec = spec;
            s.step(dt);
        }
        self.fade.step(dt);
        if let Some((i, t)) = self.shake {
            self.shake = (t + dt < SHAKE_SECS).then_some((i, t + dt));
        }
        if let Some((i, left)) = self.hint {
            self.hint = (left - dt > 0.0).then_some((i, left - dt));
        }
    }

    pub fn is_animating(&self) -> bool {
        !(self.w.settled() && self.h.settled() && self.r.settled() && self.fade.settled())
            || self.shake.is_some()
            || self.hint.is_some()
    }

    pub fn view(&self) -> IslandView {
        let rect = self.pill_rect();
        IslandView {
            rect,
            radius: self.r.value.min(rect.h / 2.0).max(0.0),
            fade: self.fade.value,
            hover: self.hover,
            shake: self.shake.map(|(i, t)| {
                let decay = 1.0 - t / SHAKE_SECS;
                (i, (t * 55.0).sin() * 5.0 * decay * decay)
            }),
            hint: self.hint.map(|(i, left)| (i, (left / HINT_FADE_SECS).min(1.0))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeRegistry, DEFAULT_THEME};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins().get(DEFAULT_THEME).unwrap().clone()
    }

    fn settle(i: &mut Island, t: &ResolvedTokens) {
        for _ in 0..1000 {
            i.step(1.0 / 144.0, t);
        }
        assert!(!i.is_animating());
    }

    fn center(i: &Island) -> (f32, f32) {
        let p = i.pill_rect();
        (p.x + p.w / 2.0, p.y + p.h / 2.0)
    }

    #[test]
    fn starts_idle_and_still() {
        let t = tokens();
        let i = Island::new(&t);
        assert_eq!(i.state(), IslandState::Idle);
        assert!(!i.is_animating());
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (128.0, 32.0));
    }

    #[test]
    fn click_expands_then_click_outside_a_tile_collapses() {
        let t = tokens();
        let mut i = Island::new(&t);
        let (cx, cy) = center(&i);
        assert_eq!(i.click(cx, cy, &t), ClickOutcome::Expanded);
        assert!(i.is_animating());
        settle(&mut i, &t);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::HOME_W, layout::HOME_H));
        assert_eq!(i.view().fade, 1.0);
        // Header area is not a tile: collapses.
        assert_eq!(i.click(r.x + 40.0, r.y + 28.0, &t), ClickOutcome::Collapsed);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Idle);
        assert_eq!(i.pill_rect().w, 128.0);
    }

    #[test]
    fn locked_tile_shakes_and_hints_without_collapsing() {
        let t = tokens();
        let mut i = Island::new(&t);
        let (cx, cy) = center(&i);
        i.click(cx, cy, &t);
        settle(&mut i, &t);
        let p = i.pill_rect();
        let tile = layout::home_tiles()[3];
        let out = i.click(p.x + tile.x + 5.0, p.y + tile.y + 5.0, &t);
        assert_eq!(out, ClickOutcome::Locked(3));
        assert_eq!(i.state(), IslandState::Home);
        assert!(i.is_animating());
        i.step(0.05, &t);
        let v = i.view();
        assert_eq!(v.shake.map(|s| s.0), Some(3));
        assert_eq!(v.hint.map(|h| h.0), Some(3));
        settle(&mut i, &t);
        assert!(i.view().shake.is_none() && i.view().hint.is_none());
    }

    #[test]
    fn click_outside_pill_is_ignored() {
        let t = tokens();
        let mut i = Island::new(&t);
        assert_eq!(i.click(1.0, 1.0, &t), ClickOutcome::Ignored);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn retarget_midflight_is_smooth() {
        let t = tokens();
        let mut i = Island::new(&t);
        let (cx, cy) = center(&i);
        i.click(cx, cy, &t);
        for _ in 0..12 {
            i.step(1.0 / 144.0, &t);
        }
        let w_before = i.pill_rect().w;
        assert!(w_before > layout::IDLE_W && w_before < layout::HOME_W);
        i.collapse(&t);
        i.step(1.0 / 144.0, &t);
        // Velocity carried over: one frame later the width is still close and grew a bit.
        assert!((i.pill_rect().w - w_before).abs() < 40.0);
        settle(&mut i, &t);
        assert_eq!(i.pill_rect().w, layout::IDLE_W);
    }

    #[test]
    fn hover_only_in_home() {
        let t = tokens();
        let mut i = Island::new(&t);
        let (cx, cy) = center(&i);
        assert!(!i.hover(cx, cy));
        i.click(cx, cy, &t);
        settle(&mut i, &t);
        let p = i.pill_rect();
        let tile = layout::home_tiles()[0];
        assert!(i.hover(p.x + tile.x + 2.0, p.y + tile.y + 2.0));
        assert_eq!(i.view().hover, Some(0));
        assert!(i.clear_hover());
        assert_eq!(i.view().hover, None);
    }

    #[test]
    fn radius_follows_scale_and_never_exceeds_half_height() {
        let mut t = tokens();
        t.radius_scale = 2.0;
        let i = Island::new(&t);
        let v = i.view();
        assert!(v.radius <= v.rect.h / 2.0);
    }
}
