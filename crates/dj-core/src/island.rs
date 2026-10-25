//! The island state machine: Idle pill, Home grid and the Music views, driven by
//! springs. Time only enters through `step(dt)`, so everything here is deterministic.

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
    MusicCompact,
    MusicExpanded,
}

impl IslandState {
    /// Index into the per-content opacity springs.
    fn layer(self) -> usize {
        match self {
            IslandState::Idle => 0,
            IslandState::Home => 1,
            IslandState::MusicCompact => 2,
            IslandState::MusicExpanded => 3,
        }
    }

    /// Expanded states use the theme's expand spring; the rest use collapse.
    fn grows(self) -> bool {
        matches!(self, IslandState::Home | IslandState::MusicExpanded)
    }
}

/// What the app knows about Music, pushed into the island on every change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MusicUi {
    pub signed_in: bool,
    pub has_track: bool,
    /// Google refused the embedded sign-in; "Open in browser" is offered.
    pub blocked: bool,
}

/// A button or control inside the expanded Music view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MusicAction {
    PlayPause,
    Prev,
    Next,
    /// Fraction of the track, 0..1, from a click on the scrubber.
    Seek(f32),
    /// Show the engine window (browse and search).
    OpenWindow,
    /// Open music.youtube.com in the default browser.
    OpenBrowser,
    SignIn,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClickOutcome {
    /// Click landed outside the pill.
    Ignored,
    Expanded,
    Collapsed,
    /// Back to the Home grid from a Music view.
    Home,
    /// A locked tile was clicked (index into `FEATURES`): shake + "coming soon".
    Locked(usize),
    /// An available tile was clicked (index into `FEATURES`). The app opens it.
    Open(usize),
    Music(MusicAction),
}

/// Opacity of each content layer, 0..1. At most one is 1 once settled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Layers {
    pub idle: f32,
    pub home: f32,
    pub compact: f32,
    pub expanded: f32,
}

/// What the renderer needs to draw one frame of the island. Window-logical px.
#[derive(Clone, Copy, Debug)]
pub struct IslandView {
    pub rect: Rect,
    pub radius: f32,
    pub layers: Layers,
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
    /// One opacity spring per content layer, indexed by `IslandState::layer`.
    layers: [Spring; 4],
    tiles: Vec<Rect>,
    hover: Option<usize>,
    shake: Option<(usize, f32)>,
    hint: Option<(usize, f32)>,
    music: MusicUi,
}

impl Island {
    pub fn new(tokens: &ResolvedTokens) -> Self {
        let spec = tokens.expand;
        let fade = |on: bool| Spring::new(if on { 1.0 } else { 0.0 }, FADE_SPEC, 0.002);
        Self {
            state: IslandState::Idle,
            w: Spring::new(layout::IDLE_W, spec, 0.02),
            h: Spring::new(layout::IDLE_H, spec, 0.02),
            r: Spring::new(layout::IDLE_R * tokens.radius_scale, spec, 0.02),
            layers: [fade(true), fade(false), fade(false), fade(false)],
            tiles: layout::home_tiles(),
            hover: None,
            shake: None,
            hint: None,
            music: MusicUi::default(),
        }
    }

    pub fn state(&self) -> IslandState {
        self.state
    }

    pub fn music_ui(&self) -> MusicUi {
        self.music
    }

    fn base_size(state: IslandState) -> (f32, f32, f32) {
        match state {
            IslandState::Idle => (layout::IDLE_W, layout::IDLE_H, layout::IDLE_R),
            IslandState::Home => (layout::HOME_W, layout::HOME_H, layout::HOME_R),
            IslandState::MusicCompact => (
                layout::MUSIC_COMPACT_W,
                layout::MUSIC_COMPACT_H,
                layout::MUSIC_COMPACT_R,
            ),
            IslandState::MusicExpanded => (
                layout::MUSIC_EXP_W,
                layout::MUSIC_EXP_H,
                layout::MUSIC_EXP_R,
            ),
        }
    }

    /// The state the island rests in when nothing is being opened.
    fn rest_state(&self) -> IslandState {
        if self.music.has_track {
            IslandState::MusicCompact
        } else {
            IslandState::Idle
        }
    }

    fn set_state(&mut self, state: IslandState, tokens: &ResolvedTokens) {
        self.state = state;
        let (w, h, r) = Self::base_size(state);
        let spec = if state.grows() {
            tokens.expand
        } else {
            tokens.collapse
        };
        self.w.retarget(w, spec);
        self.h.retarget(h, spec);
        self.r.retarget(r * tokens.radius_scale, spec);
        for (i, layer) in self.layers.iter_mut().enumerate() {
            layer.retarget(if i == state.layer() { 1.0 } else { 0.0 }, FADE_SPEC);
        }
        self.hover = None;
    }

    pub fn expand(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Home {
            self.set_state(IslandState::Home, tokens);
        }
    }

    /// Opens the expanded Music view (from Home or the compact view).
    pub fn open_music(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::MusicExpanded {
            self.set_state(IslandState::MusicExpanded, tokens);
        }
    }

    /// Collapses to the rest state: compact when a track is loaded, otherwise Idle.
    pub fn collapse(&mut self, tokens: &ResolvedTokens) {
        let rest = self.rest_state();
        if self.state != rest {
            self.set_state(rest, tokens);
        }
    }

    /// Pushes new Music facts. Only presence changes move the island: a track
    /// appearing from Idle shows the compact view, and a track disappearing from a
    /// Music view returns to Idle. Other state changes leave the view alone.
    pub fn set_music(&mut self, ui: MusicUi, tokens: &ResolvedTokens) {
        let had_track = self.music.has_track;
        self.music = ui;
        if ui.has_track == had_track {
            return;
        }
        if ui.has_track {
            if self.state == IslandState::Idle {
                self.set_state(IslandState::MusicCompact, tokens);
            }
        } else if matches!(
            self.state,
            IslandState::MusicCompact | IslandState::MusicExpanded
        ) {
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
        if self.state != IslandState::Home || self.layers[IslandState::Home.layer()].value < 0.5 {
            return None;
        }
        layout::hit_tile(&self.tiles, x - p.x, y - p.y)
    }

    /// `(x, y)` in window-logical px.
    pub fn click(&mut self, x: f32, y: f32, tokens: &ResolvedTokens) -> ClickOutcome {
        if !self.pill_rect().contains(x, y) {
            return ClickOutcome::Ignored;
        }
        match self.state {
            IslandState::Idle => {
                self.expand(tokens);
                ClickOutcome::Expanded
            }
            IslandState::MusicCompact => {
                self.open_music(tokens);
                ClickOutcome::Expanded
            }
            IslandState::MusicExpanded => self.music_click(x, y, tokens),
            IslandState::Home => match self.tile_at(x, y) {
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
            },
        }
    }

    fn music_click(&mut self, x: f32, y: f32, tokens: &ResolvedTokens) -> ClickOutcome {
        let p = self.pill_rect();
        let (lx, ly) = (x - p.x, y - p.y);
        let hit = |r: Rect| r.contains(lx, ly);
        let ui = self.music;
        if hit(layout::ME_BTN_HOME) {
            self.set_state(IslandState::Home, tokens);
            return ClickOutcome::Home;
        }
        let action = if !ui.signed_in {
            if hit(layout::ME_SIGN_IN) {
                Some(MusicAction::SignIn)
            } else if ui.blocked && hit(layout::ME_BTN_OPEN) {
                Some(MusicAction::OpenBrowser)
            } else {
                None
            }
        } else if ui.has_track {
            if hit(layout::ME_PREV) {
                Some(MusicAction::Prev)
            } else if hit(layout::ME_PLAY) {
                Some(MusicAction::PlayPause)
            } else if hit(layout::ME_NEXT) {
                Some(MusicAction::Next)
            } else if hit(layout::ME_SCRUB) {
                let s = &layout::ME_SCRUB;
                Some(MusicAction::Seek(((lx - s.x) / s.w).clamp(0.0, 1.0)))
            } else if hit(layout::ME_BTN_OPEN) {
                Some(MusicAction::OpenWindow)
            } else {
                None
            }
        } else if hit(layout::ME_BTN_OPEN) {
            Some(MusicAction::OpenWindow)
        } else {
            None
        };
        match action {
            Some(a) => ClickOutcome::Music(a),
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
        let spec = if self.state.grows() {
            tokens.expand
        } else {
            tokens.collapse
        };
        for s in [&mut self.w, &mut self.h, &mut self.r] {
            s.spec = spec;
            s.step(dt);
        }
        for layer in &mut self.layers {
            layer.step(dt);
        }
        if let Some((i, t)) = self.shake {
            self.shake = (t + dt < SHAKE_SECS).then_some((i, t + dt));
        }
        if let Some((i, left)) = self.hint {
            self.hint = (left - dt > 0.0).then_some((i, left - dt));
        }
    }

    pub fn is_animating(&self) -> bool {
        !(self.w.settled() && self.h.settled() && self.r.settled())
            || self.layers.iter().any(|l| !l.settled())
            || self.shake.is_some()
            || self.hint.is_some()
    }

    pub fn view(&self) -> IslandView {
        let rect = self.pill_rect();
        let v = |i: usize| self.layers[i].value;
        IslandView {
            rect,
            radius: self.r.value.min(rect.h / 2.0).max(0.0),
            layers: Layers {
                idle: v(0),
                home: v(1),
                compact: v(2),
                expanded: v(3),
            },
            hover: self.hover,
            shake: self.shake.map(|(i, t)| {
                let decay = 1.0 - t / SHAKE_SECS;
                (i, (t * 55.0).sin() * 5.0 * decay * decay)
            }),
            hint: self
                .hint
                .map(|(i, left)| (i, (left / HINT_FADE_SECS).min(1.0))),
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

    fn at(i: &Island, r: Rect) -> (f32, f32) {
        let p = i.pill_rect();
        (p.x + r.x + r.w / 2.0, p.y + r.y + r.h / 2.0)
    }

    fn with_track() -> MusicUi {
        MusicUi {
            signed_in: true,
            has_track: true,
            blocked: false,
        }
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
        assert_eq!(i.view().layers.home, 1.0);
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
    fn music_tile_is_available_and_reports_open() {
        let t = tokens();
        let mut i = Island::new(&t);
        let (cx, cy) = center(&i);
        i.click(cx, cy, &t);
        settle(&mut i, &t);
        let p = i.pill_rect();
        let tile = layout::home_tiles()[0];
        let out = i.click(p.x + tile.x + 5.0, p.y + tile.y + 5.0, &t);
        assert_eq!(out, ClickOutcome::Open(0));
        // The app decides what opening means; the island only reports the click.
        assert_eq!(i.state(), IslandState::Home);
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
    fn track_appearing_from_idle_shows_compact_and_disappearing_returns_idle() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        assert_eq!(i.state(), IslandState::MusicCompact);
        settle(&mut i, &t);
        let p = i.pill_rect();
        assert_eq!(
            (p.w, p.h),
            (layout::MUSIC_COMPACT_W, layout::MUSIC_COMPACT_H)
        );
        i.set_music(
            MusicUi {
                has_track: false,
                ..with_track()
            },
            &t,
        );
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn compact_click_opens_expanded_and_empty_click_returns_to_compact() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        settle(&mut i, &t);
        let (cx, cy) = center(&i);
        assert_eq!(i.click(cx, cy, &t), ClickOutcome::Expanded);
        assert_eq!(i.state(), IslandState::MusicExpanded);
        settle(&mut i, &t);
        let p = i.pill_rect();
        assert_eq!((p.w, p.h), (layout::MUSIC_EXP_W, layout::MUSIC_EXP_H));
        // A point in the title area is empty space: back to compact.
        let (ex, ey) = at(&i, layout::ME_TITLE);
        assert_eq!(i.click(ex, ey, &t), ClickOutcome::Collapsed);
        assert_eq!(i.state(), IslandState::MusicCompact);
    }

    #[test]
    fn expanded_controls_report_their_actions() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_music(&t);
        settle(&mut i, &t);
        let cases = [
            (layout::ME_PREV, MusicAction::Prev),
            (layout::ME_PLAY, MusicAction::PlayPause),
            (layout::ME_NEXT, MusicAction::Next),
            (layout::ME_BTN_OPEN, MusicAction::OpenWindow),
        ];
        for (r, want) in cases {
            let (x, y) = at(&i, r);
            assert_eq!(i.click(x, y, &t), ClickOutcome::Music(want));
        }
        assert_eq!(i.state(), IslandState::MusicExpanded);
    }

    #[test]
    fn scrubber_click_seeks_by_fraction() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_music(&t);
        settle(&mut i, &t);
        let s = layout::ME_SCRUB;
        let p = i.pill_rect();
        let out = i.click(p.x + s.x + s.w * 0.25, p.y + s.y + 2.0, &t);
        let ClickOutcome::Music(MusicAction::Seek(f)) = out else {
            panic!("seek expected, got {out:?}");
        };
        assert!((f - 0.25).abs() < 0.01);
    }

    #[test]
    fn signed_out_sign_in_button_and_browser_fallback() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(MusicUi::default(), &t);
        i.open_music(&t);
        settle(&mut i, &t);
        let (x, y) = at(&i, layout::ME_SIGN_IN);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Music(MusicAction::SignIn));

        i.set_music(
            MusicUi {
                blocked: true,
                ..MusicUi::default()
            },
            &t,
        );
        let (x, y) = at(&i, layout::ME_BTN_OPEN);
        assert_eq!(
            i.click(x, y, &t),
            ClickOutcome::Music(MusicAction::OpenBrowser)
        );
    }

    #[test]
    fn home_button_leaves_music_for_the_grid() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_music(&t);
        settle(&mut i, &t);
        let (x, y) = at(&i, layout::ME_BTN_HOME);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Home);
        assert_eq!(i.state(), IslandState::Home);
        settle(&mut i, &t);
        assert_eq!(i.view().layers.expanded, 0.0);
    }

    #[test]
    fn expanded_empty_click_without_track_returns_to_idle() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(
            MusicUi {
                signed_in: true,
                ..MusicUi::default()
            },
            &t,
        );
        i.open_music(&t);
        settle(&mut i, &t);
        let (x, y) = at(&i, layout::ME_TITLE);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Collapsed);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn layers_are_exclusive_once_settled() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_music(&t);
        settle(&mut i, &t);
        let l = i.view().layers;
        assert_eq!(
            (l.idle, l.home, l.compact, l.expanded),
            (0.0, 0.0, 0.0, 1.0)
        );
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
