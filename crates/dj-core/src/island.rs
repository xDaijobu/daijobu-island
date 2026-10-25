//! The island state machine: Idle pill <-> Home grid, driven by springs.
//! Time only enters through `step(dt)`, so everything here is deterministic.

use crate::features::{Availability, FEATURES};
use crate::layout::{self, Rect};
use crate::osd::{Osd, OsdView, Volume};
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
    /// The Vitals panel, opened from the Home tile. Any click returns to Home.
    Vitals,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClickOutcome {
    /// Click landed outside the pill.
    Ignored,
    Expanded,
    Collapsed,
    /// A locked tile was clicked (index into `FEATURES`): shake + "coming soon".
    Locked(usize),
    /// An available tile was clicked (index into `FEATURES`). The app opens its view;
    /// the OSD tile calls `show_osd` with the current volume.
    Open(usize),
    /// The OSD level bar was clicked: set the master volume to this level (0..=1).
    SetVolume(f32),
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
    /// The volume pill. Its `alpha` also hides the idle/home content while > 0.
    pub osd: OsdView,
    /// Opacity of the Vitals panel, 0..=1. Hides the idle/home content while > 0.
    pub vitals: f32,
}

pub struct Island {
    state: IslandState,
    w: Spring,
    h: Spring,
    r: Spring,
    fade: Spring,
    /// Vitals panel opacity: 1 while the state is Vitals.
    vfade: Spring,
    tiles: Vec<Rect>,
    hover: Option<usize>,
    shake: Option<(usize, f32)>,
    hint: Option<(usize, f32)>,
    osd: Osd,
    /// True while the user drags the OSD bar.
    dragging: bool,
    /// True when the pill is growing (picks the expand or collapse spring).
    expanding: bool,
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
            vfade: Spring::new(0.0, FADE_SPEC, 0.002),
            tiles: layout::home_tiles(),
            hover: None,
            shake: None,
            hint: None,
            osd: Osd::new(),
            dragging: false,
            expanding: true,
        }
    }

    pub fn state(&self) -> IslandState {
        self.state
    }

    fn base_size(state: IslandState) -> (f32, f32, f32) {
        match state {
            IslandState::Idle => (layout::IDLE_W, layout::IDLE_H, layout::IDLE_R),
            IslandState::Home => (layout::HOME_W, layout::HOME_H, layout::HOME_R),
            IslandState::Vitals => (layout::VITALS_W, layout::VITALS_H, layout::VITALS_R),
        }
    }

    /// The pill's resting size: the OSD size while it is up, else the state's size.
    fn target_size(&self) -> (f32, f32, f32) {
        if self.osd.is_visible() {
            (layout::OSD_W, layout::OSD_H, layout::OSD_R)
        } else {
            Self::base_size(self.state)
        }
    }

    /// Retargets the pill springs to `target_size`. Grow uses the expand spring,
    /// shrink the collapse spring.
    fn sync_pill(&mut self, tokens: &ResolvedTokens) {
        let (w, h, r) = self.target_size();
        self.expanding = w * h > self.w.target * self.h.target;
        let spec = if self.expanding {
            tokens.expand
        } else {
            tokens.collapse
        };
        self.w.retarget(w, spec);
        self.h.retarget(h, spec);
        self.r.retarget(r * tokens.radius_scale, spec);
    }

    fn set_state(&mut self, state: IslandState, tokens: &ResolvedTokens) {
        self.state = state;
        self.sync_pill(tokens);
        self.fade.retarget(
            if state == IslandState::Home { 1.0 } else { 0.0 },
            FADE_SPEC,
        );
        self.vfade.retarget(
            if state == IslandState::Vitals {
                1.0
            } else {
                0.0
            },
            FADE_SPEC,
        );
        self.hover = None;
    }

    /// Opens the Vitals panel. The app samples only while `state()` is Vitals.
    pub fn open_vitals(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Vitals {
            self.set_state(IslandState::Vitals, tokens);
        }
    }

    /// Shows the volume pill (or refreshes its value and hold timer).
    pub fn show_osd(&mut self, volume: Volume, tokens: &ResolvedTokens) {
        let was_visible = self.osd.is_visible();
        self.osd.show(volume);
        if !was_visible {
            self.sync_pill(tokens);
        }
        self.hover = None;
    }

    /// Ends a bar drag. Call on mouse-up.
    pub fn release(&mut self) {
        if self.dragging {
            self.dragging = false;
            self.osd.pin(false);
        }
    }

    /// Level for a drag at window-logical `x` while the bar is held. None if not dragging.
    /// Unlike the click, the drag doesn't need to stay over the bar.
    pub fn drag(&mut self, x: f32) -> Option<f32> {
        if !self.dragging {
            return None;
        }
        Some(Self::bar_level(x))
    }

    /// Maps a window-logical `x` onto the OSD bar, clamped to 0..=1.
    fn bar_level(x: f32) -> f32 {
        let bar = layout::osd_rect().x + layout::OSD_BAR.x;
        ((x - bar) / layout::OSD_BAR.w).clamp(0.0, 1.0)
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
        if self.state != IslandState::Home || self.fade.value < 0.5 || self.osd.is_visible() {
            return None;
        }
        layout::hit_tile(&self.tiles, x - p.x, y - p.y)
    }

    /// `(x, y)` in window-logical px.
    pub fn click(&mut self, x: f32, y: f32, tokens: &ResolvedTokens) -> ClickOutcome {
        if self.osd.is_visible() {
            // The OSD owns the pill while it is up: only the bar responds.
            let r = layout::osd_rect();
            return match layout::osd_level_at(x - r.x, y - r.y) {
                Some(level) => {
                    self.dragging = true;
                    self.osd.pin(true);
                    ClickOutcome::SetVolume(level)
                }
                None => ClickOutcome::Ignored,
            };
        }
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
            None if self.state == IslandState::Vitals => {
                self.set_state(IslandState::Home, tokens);
                ClickOutcome::Collapsed
            }
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
        if self.osd.step(dt) {
            self.sync_pill(tokens);
        }
        // Theme switches change the radius scale and spring params mid-flight.
        let (_, _, base_r) = self.target_size();
        self.r.target = base_r * tokens.radius_scale;
        let spec = if self.expanding {
            tokens.expand
        } else {
            tokens.collapse
        };
        for s in [&mut self.w, &mut self.h, &mut self.r] {
            s.spec = spec;
            s.step(dt);
        }
        self.fade.step(dt);
        self.vfade.step(dt);
        if let Some((i, t)) = self.shake {
            self.shake = (t + dt < SHAKE_SECS).then_some((i, t + dt));
        }
        if let Some((i, left)) = self.hint {
            self.hint = (left - dt > 0.0).then_some((i, left - dt));
        }
    }

    pub fn is_animating(&self) -> bool {
        !(self.w.settled()
            && self.h.settled()
            && self.r.settled()
            && self.fade.settled()
            && self.vfade.settled())
            || self.shake.is_some()
            || self.hint.is_some()
            || self.osd.is_animating()
    }

    pub fn view(&self) -> IslandView {
        let rect = self.pill_rect();
        IslandView {
            rect,
            radius: self.r.value.min(rect.h / 2.0).max(0.0),
            fade: self.fade.value,
            vitals: self.vfade.value.clamp(0.0, 1.0),
            hover: self.hover,
            shake: self.shake.map(|(i, t)| {
                let decay = 1.0 - t / SHAKE_SECS;
                (i, (t * 55.0).sin() * 5.0 * decay * decay)
            }),
            hint: self
                .hint
                .map(|(i, left)| (i, (left / HINT_FADE_SECS).min(1.0))),
            osd: self.osd.view(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeRegistry, DEFAULT_THEME};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
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
    fn vitals_opens_from_home_and_any_click_returns_to_home() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        settle(&mut i, &t);
        i.open_vitals(&t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Vitals);
        let v = i.view();
        assert!((v.vitals - 1.0).abs() < 0.01);
        assert!(v.fade < 0.01);
        assert!((v.rect.w - layout::VITALS_W).abs() < 0.5);
        assert!((v.rect.h - layout::VITALS_H).abs() < 0.5);
        let (x, y) = center(&i);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Collapsed);
        assert_eq!(i.state(), IslandState::Home);
        settle(&mut i, &t);
        assert!(i.view().vitals < 0.01);
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

    fn vol(level: f32) -> Volume {
        Volume {
            level,
            muted: false,
        }
    }

    #[test]
    fn osd_morphs_the_pill_and_returns_to_idle_after_the_hold() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_osd(vol(0.5), &t);
        settle_pinned(&mut i, &t);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::OSD_W, layout::OSD_H));
        assert_eq!(i.view().osd.alpha, 1.0);
        i.osd.pin(false);
        settle(&mut i, &t);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::IDLE_W, layout::IDLE_H));
        assert_eq!(i.state(), IslandState::Idle);
        assert_eq!(i.view().osd.alpha, 0.0);
    }

    #[test]
    fn osd_from_home_returns_to_home() {
        let t = tokens();
        let mut i = Island::new(&t);
        let (cx, cy) = center(&i);
        i.click(cx, cy, &t);
        settle(&mut i, &t);
        i.show_osd(vol(0.2), &t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Home);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::HOME_W, layout::HOME_H));
        assert_eq!(i.view().fade, 1.0);
    }

    #[test]
    fn osd_tile_opens_through_home_and_tiles_are_not_hit_while_osd_is_up() {
        let t = tokens();
        let mut i = Island::new(&t);
        let (cx, cy) = center(&i);
        i.click(cx, cy, &t);
        settle(&mut i, &t);
        i.show_osd(vol(0.5), &t);
        let p = i.pill_rect();
        let tile = layout::home_tiles()[1];
        assert!(!i.hover(p.x + tile.x + 2.0, p.y + tile.y + 2.0));
        // The OSD tile opens the OSD view: clicking it on Home reports Open.
        settle(&mut i, &t);
        let p = i.pill_rect();
        assert_eq!(
            i.click(p.x + tile.x + 5.0, p.y + tile.y + 5.0, &t),
            ClickOutcome::Open(1)
        );
    }

    #[test]
    fn osd_bar_click_and_drag_set_the_level_and_clamp() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_osd(vol(0.0), &t);
        settle_pinned(&mut i, &t);
        let r = layout::osd_rect();
        let bar = layout::OSD_BAR;
        let mid_x = r.x + bar.x + bar.w / 2.0;
        let mid_y = r.y + bar.y + bar.h / 2.0;
        assert_eq!(i.click(mid_x, mid_y, &t), ClickOutcome::SetVolume(0.5));
        // Drag past the right end: clamped to 1.0 even though the pointer left the hit area.
        assert_eq!(i.drag(r.x + bar.x + bar.w + 80.0), Some(1.0));
        assert_eq!(i.drag(r.x + bar.x - 80.0), Some(0.0));
        i.release();
        assert_eq!(i.drag(mid_x), None);
        // A click outside the bar, while the OSD is up, does nothing.
        assert_eq!(i.click(r.x + 2.0, r.y + 2.0, &t), ClickOutcome::Ignored);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn osd_stays_up_while_dragging() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_osd(vol(0.3), &t);
        let r = layout::osd_rect();
        let bar = layout::OSD_BAR;
        i.click(r.x + bar.x + 1.0, r.y + bar.y + 1.0, &t);
        for _ in 0..600 {
            i.step(1.0 / 120.0, &t);
        }
        assert_eq!(i.view().osd.alpha, 1.0);
        i.release();
        settle(&mut i, &t);
        assert_eq!(i.view().osd.alpha, 0.0);
    }

    /// Pins the OSD so it stays up, then steps until the springs rest at the OSD size.
    fn settle_pinned(i: &mut Island, t: &ResolvedTokens) {
        i.osd.pin(true);
        for _ in 0..1000 {
            i.step(1.0 / 144.0, t);
        }
        assert!(i.osd.is_visible());
    }
}
