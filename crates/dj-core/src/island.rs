//! The island state machine: Idle pill, Home grid, the Music views, the Vitals panel with
//! its Sweep view, the Timer views, Settings, the Tile panel, the snap thumb, the dj
//! progress and notice pills and the OSD overlay, driven by springs.
//! Time only enters through `step(dt)`, so everything here is deterministic. The timer
//! itself (`timer::Pomodoro`) lives in the app; the island only hears whether it is active
//! or done. Likewise the snap thumb's countdown and its shot live in the app; the island
//! only hears whether a thumb is up. The same goes for `dj` activity: its text, percent and
//! timeouts are the app's, and the island only hears whether a progress and a notice are up.

use crate::features::{Availability, FEATURES};
use crate::layout::{self, Rect};
use crate::osd::{Osd, OsdView, Volume};
use crate::settings::{self, SettingsAction, SettingsHit};
use crate::spring::{Spring, SpringSpec};
use crate::theme::ResolvedTokens;
use crate::timer::PRESETS;

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
    /// The Vitals panel, opened from the Home tile. An empty click collapses it.
    Vitals,
    /// Sweep, opened from the Vitals panel's button. Its controls are hit-tested by
    /// `sweep::SweepView`; empty clicks do nothing, so a clean is never left by accident.
    Sweep,
    /// The Timer panel, opened from the Home tile or Timer compact. An empty click
    /// collapses it. A finished session shows inline here.
    TimerPanel,
    /// The rest state while a timer runs (or is paused) and no music is loaded.
    TimerCompact,
    /// A session ended: waits for continue or stop. Shown over the rest states only.
    TimerAlert,
    /// The last snap: its shot, size, copy status, and copy / save. Outranks everything
    /// but the OSD and goes after 6 s (the app's countdown); a click elsewhere on it
    /// dismisses it.
    SnapThumb,
    /// The Settings panel, opened from the gear in the Home header. Back, or an empty
    /// click, returns to Home.
    Settings,
    /// The Tile panel (window zones): the layout picker, opened while a window is dragged
    /// towards the island or from the Home tile. Every click inside it is the app's to
    /// read (`ClickOutcome::Tile`); the app collapses it on an empty spot.
    TilePanel,
    /// A `dj progress` (or the ✓ after `dj done`) is up: a 300 x 36 pill. A rest state below
    /// the dj notice, the timer alert and the snap thumb, above Music and Timer compact. A
    /// click expands to Home, as from Idle.
    DjProgress,
    /// A `dj notify` message: a 360 x 36 pill. A rest state that outranks everything but the
    /// snap thumb (the OSD only covers it). A click is the app's to answer
    /// (`ClickOutcome::DjDismiss`).
    DjNotice,
}

/// Number of content layers (one per `IslandState`).
const LAYERS: usize = 14;

impl IslandState {
    /// Index into the per-content opacity springs.
    fn layer(self) -> usize {
        match self {
            IslandState::Idle => 0,
            IslandState::Home => 1,
            IslandState::MusicCompact => 2,
            IslandState::MusicExpanded => 3,
            IslandState::Vitals => 4,
            IslandState::Sweep => 5,
            IslandState::TimerPanel => 6,
            IslandState::TimerCompact => 7,
            IslandState::TimerAlert => 8,
            IslandState::SnapThumb => 9,
            IslandState::Settings => 10,
            IslandState::TilePanel => 11,
            IslandState::DjProgress => 12,
            IslandState::DjNotice => 13,
        }
    }

    /// The states the island rests in. Facts that change the rest state (a track, the
    /// timer, dj activity) move the island only from one of these; an open panel stays put.
    fn is_rest(self) -> bool {
        matches!(
            self,
            IslandState::Idle
                | IslandState::MusicCompact
                | IslandState::TimerCompact
                | IslandState::TimerAlert
                | IslandState::SnapThumb
                | IslandState::DjProgress
                | IslandState::DjNotice
        )
    }
}

/// What the app knows about the timer, pushed into the island on every change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimerUi {
    /// Running or paused.
    pub active: bool,
    /// A session ended and nobody has answered the alert yet.
    pub done: bool,
}

/// What the app knows about `dj` activity, pushed into the island on every change. The
/// text, the percent and the timeouts stay in the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DjUi {
    /// A progress pill is up (a progress, or the ✓ that follows `dj done`).
    pub progress: bool,
    /// A notice pill is up.
    pub notice: bool,
}

/// A control in the Timer panel or the alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerAction {
    /// Start, pause or resume.
    Toggle,
    Skip,
    Reset,
    /// Preset chip `i` of `timer::PRESETS`.
    Preset(usize),
    /// Start the next phase (from Done).
    Continue,
    /// Back to Stopped (from Done).
    Stop,
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
    /// An available tile was clicked (index into `FEATURES`). The app opens its view;
    /// the OSD tile calls `show_osd` with the current volume.
    Open(usize),
    /// The OSD level bar was clicked: set the master volume to this level (0..=1).
    SetVolume(f32),
    Music(MusicAction),
    /// The Vitals panel's "reclaimable" button: the island moved to Sweep. The app
    /// starts the preview scan.
    OpenSweep,
    /// A click inside the Sweep view, in view-local px. The app passes it to
    /// `SweepView::click`.
    Sweep {
        x: f32,
        y: f32,
    },
    /// A Timer panel or alert control. The app applies it to the Pomodoro and pushes
    /// the new `TimerUi`. Controls that don't apply right now (a preset while running)
    /// still land here, so they never collapse the panel.
    Timer(TimerAction),
    /// A Settings panel control. The app applies it (autostart, a hotkey chip) and
    /// saves the config. Back and empty clicks come out as `Home`.
    Settings(SettingsAction),
    /// A click on the snap thumb. `Dismiss` has already sent the island to its rest
    /// state; the app drops the shot and stops the countdown.
    Snap(SnapAction),
    /// Any click inside the Tile panel, in panel-local px (the panel's top-left is the
    /// origin; stable while the pill is still morphing). The app hit-tests it with
    /// `tile::panel_hit`: a cell places the window, an empty spot collapses the panel.
    Tile {
        x: f32,
        y: f32,
    },
    /// A click on the dj notice pill. The island has not moved: the app dismisses the
    /// notice and pushes the new `DjUi`, and the island leaves the pill then.
    DjDismiss,
}

/// What a click on the snap thumb asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapAction {
    /// Copy the shot to the clipboard again.
    Copy,
    /// Write it to the save folder.
    Save,
    /// Anywhere else on the thumb: it goes.
    Dismiss,
}

/// Opacity of each content layer, 0..1. At most one is 1 once settled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Layers {
    pub idle: f32,
    pub home: f32,
    pub compact: f32,
    pub expanded: f32,
    pub vitals: f32,
    pub sweep: f32,
    pub timer: f32,
    pub timer_compact: f32,
    pub alert: f32,
    pub snap: f32,
    pub settings: f32,
    pub tile: f32,
    pub dj_progress: f32,
    pub dj_notice: f32,
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
    /// The volume pill. Its `alpha` also hides the content layers while > 0.
    pub osd: OsdView,
    /// The snap thumb button under the pointer (Copy or Save).
    pub snap_hover: Option<SnapAction>,
    /// The pointer is over the gear in the Home header.
    pub gear_hover: bool,
}

pub struct Island {
    state: IslandState,
    w: Spring,
    h: Spring,
    r: Spring,
    /// One opacity spring per content layer, indexed by `IslandState::layer`.
    layers: [Spring; LAYERS],
    tiles: Vec<Rect>,
    hover: Option<usize>,
    shake: Option<(usize, f32)>,
    hint: Option<(usize, f32)>,
    music: MusicUi,
    timer: TimerUi,
    dj: DjUi,
    /// A snap thumb is up (until the app's countdown ends or it is dismissed).
    snap: bool,
    /// The pointer is over the snap thumb, and the button under it.
    snap_pointer: bool,
    snap_hover: Option<SnapAction>,
    gear_hover: bool,
    osd: Osd,
    /// True while the user drags the OSD bar.
    dragging: bool,
    /// True when the pill is growing (picks the expand or collapse spring).
    expanding: bool,
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
            layers: std::array::from_fn(|i| fade(i == IslandState::Idle.layer())),
            tiles: layout::home_tiles(),
            hover: None,
            shake: None,
            hint: None,
            music: MusicUi::default(),
            timer: TimerUi::default(),
            dj: DjUi::default(),
            snap: false,
            snap_pointer: false,
            snap_hover: None,
            gear_hover: false,
            osd: Osd::new(),
            dragging: false,
            expanding: true,
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
            IslandState::Vitals => (layout::VITALS_W, layout::VITALS_H, layout::VITALS_R),
            IslandState::Sweep => (layout::SWEEP_W, layout::SWEEP_H, layout::SWEEP_R),
            IslandState::TimerPanel => (layout::TIMER_W, layout::TIMER_H, layout::TIMER_R),
            IslandState::TimerCompact => (
                layout::TIMER_COMPACT_W,
                layout::TIMER_COMPACT_H,
                layout::TIMER_COMPACT_R,
            ),
            IslandState::TimerAlert => (layout::ALERT_W, layout::ALERT_H, layout::ALERT_R),
            IslandState::SnapThumb => (layout::SNAP_W, layout::SNAP_H, layout::SNAP_R),
            IslandState::Settings => (layout::SETTINGS_W, layout::SETTINGS_H, layout::SETTINGS_R),
            IslandState::TilePanel => (
                layout::TILE_PANEL_W,
                layout::TILE_PANEL_H,
                layout::TILE_PANEL_R,
            ),
            IslandState::DjProgress => (
                layout::DJ_PROGRESS_W,
                layout::DJ_PROGRESS_H,
                layout::DJ_PROGRESS_R,
            ),
            IslandState::DjNotice => (
                layout::DJ_NOTICE_W,
                layout::DJ_NOTICE_H,
                layout::DJ_NOTICE_R,
            ),
        }
    }

    /// The state the island rests in when nothing is being opened, by activity
    /// priority (plan.html): snap thumb > dj notice > timer done > dj progress > media >
    /// timer running > idle. The OSD sits above all of these but only covers the state
    /// (see `target_size`).
    fn rest_state(&self) -> IslandState {
        if self.snap {
            IslandState::SnapThumb
        } else if self.dj.notice {
            IslandState::DjNotice
        } else if self.timer.done {
            IslandState::TimerAlert
        } else if self.dj.progress {
            IslandState::DjProgress
        } else if self.music.has_track {
            IslandState::MusicCompact
        } else if self.timer.active {
            IslandState::TimerCompact
        } else {
            IslandState::Idle
        }
    }

    /// Moves to the rest state if the island is resting now. Open panels stay put and
    /// pick the new rest state up when they collapse.
    fn settle_rest(&mut self, tokens: &ResolvedTokens) {
        let rest = self.rest_state();
        if self.state.is_rest() && self.state != rest {
            self.set_state(rest, tokens);
        }
    }

    pub fn timer_ui(&self) -> TimerUi {
        self.timer
    }

    /// Pushes new timer facts. A finished session shows the alert over the rest states;
    /// over an open panel (Home, Music, Vitals, Sweep, Timer, Tile) it waits for the collapse.
    pub fn set_timer(&mut self, ui: TimerUi, tokens: &ResolvedTokens) {
        if ui == self.timer {
            return;
        }
        self.timer = ui;
        self.settle_rest(tokens);
    }

    pub fn dj_ui(&self) -> DjUi {
        self.dj
    }

    /// Pushes new `dj` facts. A progress or a notice shows over the rest states it
    /// outranks (see `rest_state`); over an open panel (Home, Music, Vitals, Sweep, Timer,
    /// Settings, Tile) it waits for the collapse, and so does its removal. Under the snap
    /// thumb it waits for the thumb to go.
    pub fn set_dj(&mut self, ui: DjUi, tokens: &ResolvedTokens) {
        if ui == self.dj {
            return;
        }
        self.dj = ui;
        self.settle_rest(tokens);
    }

    /// Shows the snap thumb for a fresh shot, from any state: the user just asked for
    /// it, so it takes over an open panel too (the app collapses the island before a
    /// snap, so normally this comes from a rest state).
    pub fn show_snap(&mut self, tokens: &ResolvedTokens) {
        self.snap = true;
        if self.state != IslandState::SnapThumb {
            self.set_state(IslandState::SnapThumb, tokens);
        }
    }

    /// The thumb goes (countdown over or dismissed): back to the rest state below it.
    pub fn hide_snap(&mut self, tokens: &ResolvedTokens) {
        self.snap = false;
        if self.state == IslandState::SnapThumb {
            self.collapse(tokens);
        }
    }

    /// A snap thumb is up.
    pub fn snap_shown(&self) -> bool {
        self.snap
    }

    /// The pointer is over the snap thumb (the app pauses the countdown).
    pub fn snap_hovered(&self) -> bool {
        self.state == IslandState::SnapThumb && self.snap_pointer
    }

    /// Opens the Timer panel (from the Home tile or Timer compact).
    pub fn open_timer(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::TimerPanel {
            self.set_state(IslandState::TimerPanel, tokens);
        }
    }

    /// Opens the Settings panel (from the gear on Home).
    pub fn open_settings(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Settings {
            self.set_state(IslandState::Settings, tokens);
        }
    }

    /// Opens the Tile panel: from any rest state (the picker while a window is dragged) or
    /// from the Home tile. Like the other panels it is not a rest state, so `collapse`
    /// returns to the rest state below it, and a snap thumb that arrives takes it over.
    pub fn open_tile(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::TilePanel {
            self.set_state(IslandState::TilePanel, tokens);
        }
    }

    /// The Tile panel is the island's state (it may still be growing, or covered by the OSD).
    pub fn tile_open(&self) -> bool {
        self.state == IslandState::TilePanel
    }

    /// The pill's resting size: the OSD size while it is up, else the state's size.
    /// The OSD only covers the state; it never changes it, so the state returns after.
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
        for (i, layer) in self.layers.iter_mut().enumerate() {
            layer.retarget(if i == state.layer() { 1.0 } else { 0.0 }, FADE_SPEC);
        }
        self.sync_pill(tokens);
        self.hover = None;
        self.snap_pointer = false;
        self.gear_hover = false;
        self.snap_hover = None;
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

    /// Opens the expanded Music view (from Home or the compact view).
    pub fn open_music(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::MusicExpanded {
            self.set_state(IslandState::MusicExpanded, tokens);
        }
    }

    /// Opens the Vitals panel (from the Home tile). The app samples at 1 Hz while
    /// `state()` is Vitals.
    pub fn open_vitals(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Vitals {
            self.set_state(IslandState::Vitals, tokens);
        }
    }

    /// Opens the Sweep view (from the Vitals panel's button).
    pub fn open_sweep(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Sweep {
            self.set_state(IslandState::Sweep, tokens);
        }
    }

    /// Window-logical `(x, y)` relative to a view of width `w` that is anchored at the
    /// pill's centre-top, as the painter draws it. Unlike `pill_rect`, this stays put
    /// while the pill is still morphing.
    fn view_local(x: f32, y: f32, w: f32) -> (f32, f32) {
        let (win_w, _) = layout::window_size();
        (x - (win_w - w) / 2.0, y - layout::TOP_MARGIN)
    }

    /// Collapses to the rest state (see `rest_state`): the snap thumb or a dj notice when
    /// one is up, the timer alert when a session ended, the dj progress, Music compact
    /// when a track is loaded, Timer compact while a timer runs, otherwise Idle.
    pub fn collapse(&mut self, tokens: &ResolvedTokens) {
        let rest = self.rest_state();
        if self.state != rest {
            self.set_state(rest, tokens);
        }
    }

    /// Pushes new Music facts. Only presence changes move the island: a track
    /// appearing or disappearing moves a resting island to its new rest state, and a
    /// track disappearing from the expanded view collapses it. Other state changes
    /// leave the view alone.
    pub fn set_music(&mut self, ui: MusicUi, tokens: &ResolvedTokens) {
        let had_track = self.music.has_track;
        self.music = ui;
        if ui.has_track == had_track {
            return;
        }
        if !ui.has_track && self.state == IslandState::MusicExpanded {
            self.collapse(tokens);
        } else {
            self.settle_rest(tokens);
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
        if self.state != IslandState::Home
            || self.layers[IslandState::Home.layer()].value < 0.5
            || self.osd.is_visible()
        {
            return None;
        }
        layout::hit_tile(&self.tiles, x - p.x, y - p.y)
    }

    /// True when `(x, y)` (window-logical) is on the Home header's gear.
    fn gear_at(&self, x: f32, y: f32) -> bool {
        if self.state != IslandState::Home
            || self.layers[IslandState::Home.layer()].value < 0.5
            || self.osd.is_visible()
        {
            return false;
        }
        let (lx, ly) = Self::view_local(x, y, layout::HOME_W);
        layout::HOME_GEAR.contains(lx, ly)
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
        match self.state {
            // The progress pill opens Home like the idle island; the notice only reports the
            // click, and the app dismisses it.
            IslandState::Idle | IslandState::DjProgress => {
                self.expand(tokens);
                ClickOutcome::Expanded
            }
            IslandState::DjNotice => ClickOutcome::DjDismiss,
            IslandState::MusicCompact => {
                self.open_music(tokens);
                ClickOutcome::Expanded
            }
            IslandState::MusicExpanded => self.music_click(x, y, tokens),
            // The Sweep button opens Sweep; anywhere else collapses, like the other views.
            IslandState::Vitals => {
                let (lx, ly) = Self::view_local(x, y, layout::VITALS_W);
                if layout::VITALS_SWEEP.contains(lx, ly) {
                    self.open_sweep(tokens);
                    ClickOutcome::OpenSweep
                } else {
                    self.collapse(tokens);
                    ClickOutcome::Collapsed
                }
            }
            IslandState::Sweep => {
                let (lx, ly) = Self::view_local(x, y, layout::SWEEP_W);
                ClickOutcome::Sweep { x: lx, y: ly }
            }
            IslandState::TimerCompact => {
                self.open_timer(tokens);
                ClickOutcome::Expanded
            }
            IslandState::TimerPanel => {
                let (lx, ly) = Self::view_local(x, y, layout::TIMER_W);
                match Self::timer_panel_action(lx, ly, self.timer.done) {
                    Some(a) => ClickOutcome::Timer(a),
                    None => {
                        self.collapse(tokens);
                        ClickOutcome::Collapsed
                    }
                }
            }
            // The alert waits for an answer: only its two buttons respond.
            IslandState::TimerAlert => {
                let (lx, ly) = Self::view_local(x, y, layout::ALERT_W);
                if layout::AL_CONTINUE.contains(lx, ly) {
                    ClickOutcome::Timer(TimerAction::Continue)
                } else if layout::AL_STOP.contains(lx, ly) {
                    ClickOutcome::Timer(TimerAction::Stop)
                } else {
                    ClickOutcome::Ignored
                }
            }
            IslandState::SnapThumb => {
                let (lx, ly) = Self::view_local(x, y, layout::SNAP_W);
                match Self::snap_button(lx, ly) {
                    Some(a) => ClickOutcome::Snap(a),
                    None => {
                        self.hide_snap(tokens);
                        ClickOutcome::Snap(SnapAction::Dismiss)
                    }
                }
            }
            // Every click inside the panel is the app's to read; it collapses on an empty spot.
            IslandState::TilePanel => {
                let (lx, ly) = Self::view_local(x, y, layout::TILE_PANEL_W);
                ClickOutcome::Tile { x: lx, y: ly }
            }
            IslandState::Settings => {
                let (lx, ly) = Self::view_local(x, y, layout::SETTINGS_W);
                match settings::hit(lx, ly) {
                    Some(SettingsHit::Action(a)) => ClickOutcome::Settings(a),
                    // Back, or an empty click.
                    _ => {
                        self.set_state(IslandState::Home, tokens);
                        ClickOutcome::Home
                    }
                }
            }
            IslandState::Home if self.gear_at(x, y) => {
                self.open_settings(tokens);
                ClickOutcome::Expanded
            }
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

    /// The Timer panel control at view-local `(x, y)`. While a session is done, the
    /// start button continues and reset stops, like the alert.
    fn timer_panel_action(x: f32, y: f32, done: bool) -> Option<TimerAction> {
        if layout::TP_TOGGLE.contains(x, y) {
            return Some(if done {
                TimerAction::Continue
            } else {
                TimerAction::Toggle
            });
        }
        if layout::TP_SKIP.contains(x, y) {
            return Some(TimerAction::Skip);
        }
        if layout::TP_RESET.contains(x, y) {
            return Some(if done {
                TimerAction::Stop
            } else {
                TimerAction::Reset
            });
        }
        (0..PRESETS.len())
            .find(|&i| layout::tp_chip(i).contains(x, y))
            .map(TimerAction::Preset)
    }

    /// The snap thumb button at view-local `(x, y)`.
    fn snap_button(x: f32, y: f32) -> Option<SnapAction> {
        if layout::SN_COPY.contains(x, y) {
            Some(SnapAction::Copy)
        } else if layout::SN_SAVE.contains(x, y) {
            Some(SnapAction::Save)
        } else {
            None
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

    /// Returns true when the hovered tile or snap button changed (needs a redraw).
    /// Also tracks whether the pointer is over the snap thumb (`snap_hovered`).
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let h = self.tile_at(x, y);
        let changed = h != self.hover;
        self.hover = h;
        let on_thumb = self.state == IslandState::SnapThumb
            && !self.osd.is_visible()
            && self.pill_rect().contains(x, y);
        self.snap_pointer = on_thumb;
        let button = if on_thumb {
            let (lx, ly) = Self::view_local(x, y, layout::SNAP_W);
            Self::snap_button(lx, ly)
        } else {
            None
        };
        let button_changed = button != self.snap_hover;
        self.snap_hover = button;
        let gear = self.gear_at(x, y);
        let gear_changed = gear != self.gear_hover;
        self.gear_hover = gear;
        changed || button_changed || gear_changed
    }

    /// The pointer left the window. True when something hovered needs a redraw.
    pub fn clear_hover(&mut self) -> bool {
        self.snap_pointer = false;
        let button = self.snap_hover.take().is_some();
        let gear = std::mem::take(&mut self.gear_hover);
        self.hover.take().is_some() || button || gear
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
            || self.osd.is_animating()
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
                vitals: v(4),
                sweep: v(5),
                timer: v(6),
                timer_compact: v(7),
                alert: v(8),
                snap: v(9),
                settings: v(10),
                tile: v(11),
                dj_progress: v(12),
                dj_notice: v(13),
            },
            hover: self.hover,
            shake: self.shake.map(|(i, t)| {
                let decay = 1.0 - t / SHAKE_SECS;
                (i, (t * 55.0).sin() * 5.0 * decay * decay)
            }),
            hint: self
                .hint
                .map(|(i, left)| (i, (left / HINT_FADE_SECS).min(1.0))),
            osd: self.osd.view(),
            snap_hover: self.snap_hover,
            gear_hover: self.gear_hover,
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

    /// Pins the OSD so it stays up, then steps until the springs rest at the OSD size.
    fn settle_pinned(i: &mut Island, t: &ResolvedTokens) {
        i.osd.pin(true);
        for _ in 0..1000 {
            i.step(1.0 / 144.0, t);
        }
        assert!(i.osd.is_visible());
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

    fn vol(level: f32) -> Volume {
        Volume {
            level,
            muted: false,
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
        let tile = layout::home_tiles()[5];
        let out = i.click(p.x + tile.x + 5.0, p.y + tile.y + 5.0, &t);
        assert_eq!(out, ClickOutcome::Locked(5));
        assert_eq!(i.state(), IslandState::Home);
        assert!(i.is_animating());
        i.step(0.05, &t);
        let v = i.view();
        assert_eq!(v.shake.map(|s| s.0), Some(5));
        assert_eq!(v.hint.map(|h| h.0), Some(5));
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
            (l.idle, l.home, l.compact, l.expanded, l.vitals, l.sweep),
            (0.0, 0.0, 0.0, 1.0, 0.0, 0.0)
        );
    }

    fn vitals_index() -> usize {
        FEATURES.iter().position(|f| f.id == "vitals").unwrap()
    }

    #[test]
    fn vitals_tile_opens_the_panel_and_an_empty_click_collapses() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        settle(&mut i, &t);
        let tile = layout::home_tiles()[vitals_index()];
        let out = i.click(
            i.pill_rect().x + tile.x + 5.0,
            i.pill_rect().y + tile.y + 5.0,
            &t,
        );
        assert_eq!(out, ClickOutcome::Open(vitals_index()));
        i.open_vitals(&t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Vitals);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::VITALS_W, layout::VITALS_H));
        let l = i.view().layers;
        assert_eq!((l.idle, l.home, l.vitals), (0.0, 0.0, 1.0));
        // No tiles or hover in the panel.
        let (x, y) = center(&i);
        assert!(!i.hover(x, y));
        assert_eq!(i.click(x, y, &t), ClickOutcome::Collapsed);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Idle);
        assert_eq!(i.view().layers.vitals, 0.0);
    }

    #[test]
    fn vitals_empty_click_with_a_track_returns_to_compact() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.expand(&t);
        i.open_vitals(&t);
        settle(&mut i, &t);
        // A track change does not pull the island out of the panel.
        i.set_music(MusicUi::default(), &t);
        i.set_music(with_track(), &t);
        assert_eq!(i.state(), IslandState::Vitals);
        let (x, y) = center(&i);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Collapsed);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::MusicCompact);
    }

    #[test]
    fn osd_over_vitals_returns_to_vitals() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        i.open_vitals(&t);
        settle(&mut i, &t);
        i.show_osd(vol(0.3), &t);
        settle_pinned(&mut i, &t);
        assert_eq!(i.state(), IslandState::Vitals);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::OSD_W, layout::OSD_H));
        // While the OSD is up, a click off its bar does not close the panel.
        let (x, _) = center(&i);
        let r = layout::osd_rect();
        assert_eq!(i.click(x, r.y + r.h + 20.0, &t), ClickOutcome::Ignored);
        i.osd.pin(false);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Vitals);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::VITALS_W, layout::VITALS_H));
        assert_eq!(i.view().layers.vitals, 1.0);
        assert_eq!(i.view().osd.alpha, 0.0);
    }

    fn open_settled_vitals(t: &ResolvedTokens) -> Island {
        let mut i = Island::new(t);
        i.expand(t);
        i.open_vitals(t);
        settle(&mut i, t);
        i
    }

    #[test]
    fn vitals_button_opens_sweep_and_back_returns() {
        let t = tokens();
        let mut i = open_settled_vitals(&t);
        let (x, y) = at(&i, layout::VITALS_SWEEP);
        assert_eq!(i.click(x, y, &t), ClickOutcome::OpenSweep);
        assert_eq!(i.state(), IslandState::Sweep);
        settle(&mut i, &t);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::SWEEP_W, layout::SWEEP_H));
        let l = i.view().layers;
        assert_eq!((l.vitals, l.sweep), (0.0, 1.0));
        // Back is the app's call (after `SweepView::click` says so).
        i.open_vitals(&t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Vitals);
        assert_eq!(i.view().layers.sweep, 0.0);
    }

    #[test]
    fn sweep_clicks_report_view_local_coordinates_even_mid_morph() {
        let t = tokens();
        let mut i = open_settled_vitals(&t);
        i.open_sweep(&t);
        // Not settled: the pill is still growing, but the content is anchored centre-top.
        i.step(1.0 / 144.0, &t);
        let (win_w, _) = layout::window_size();
        let x0 = (win_w - layout::SWEEP_W) / 2.0;
        let c = layout::SW_CLEAN;
        let out = i.click(x0 + c.x + 3.0, layout::TOP_MARGIN + c.y + 3.0, &t);
        let ClickOutcome::Sweep { x, y } = out else {
            panic!("sweep click expected, got {out:?}");
        };
        assert!(c.contains(x, y));
        settle(&mut i, &t);
        // An empty spot does not collapse Sweep.
        let (cx, cy) = center(&i);
        assert!(matches!(i.click(cx, cy, &t), ClickOutcome::Sweep { .. }));
        assert_eq!(i.state(), IslandState::Sweep);
    }

    #[test]
    fn osd_over_sweep_returns_to_sweep() {
        let t = tokens();
        let mut i = open_settled_vitals(&t);
        i.open_sweep(&t);
        settle(&mut i, &t);
        i.show_osd(vol(0.6), &t);
        settle_pinned(&mut i, &t);
        assert_eq!(i.state(), IslandState::Sweep);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::OSD_W, layout::OSD_H));
        i.osd.pin(false);
        settle(&mut i, &t);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::SWEEP_W, layout::SWEEP_H));
        assert_eq!(i.view().layers.sweep, 1.0);
    }

    #[test]
    fn radius_follows_scale_and_never_exceeds_half_height() {
        let mut t = tokens();
        t.radius_scale = 2.0;
        let i = Island::new(&t);
        let v = i.view();
        assert!(v.radius <= v.rect.h / 2.0);
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
        assert_eq!(i.view().layers.home, 1.0);
    }

    #[test]
    fn osd_over_music_returns_to_music() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_music(&t);
        settle(&mut i, &t);
        i.show_osd(vol(0.4), &t);
        settle_pinned(&mut i, &t);
        // The OSD takes the pill and hides the Music content, but the state stays put.
        assert_eq!(i.state(), IslandState::MusicExpanded);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::OSD_W, layout::OSD_H));
        assert_eq!(i.view().osd.alpha, 1.0);
        i.osd.pin(false);
        settle(&mut i, &t);
        // Once the OSD hides, the expanded Music view is back, and it still answers.
        assert_eq!(i.state(), IslandState::MusicExpanded);
        let r = i.pill_rect();
        assert_eq!((r.w, r.h), (layout::MUSIC_EXP_W, layout::MUSIC_EXP_H));
        assert_eq!(i.view().layers.expanded, 1.0);
        assert_eq!(i.view().osd.alpha, 0.0);
        let (x, y) = at(&i, layout::ME_PLAY);
        assert_eq!(
            i.click(x, y, &t),
            ClickOutcome::Music(MusicAction::PlayPause)
        );
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

    // ---- Timer ----

    const RUNNING: TimerUi = TimerUi {
        active: true,
        done: false,
    };
    const DONE: TimerUi = TimerUi {
        active: false,
        done: true,
    };

    fn timer_index() -> usize {
        FEATURES.iter().position(|f| f.id == "timer").unwrap()
    }

    /// Window-logical centre of `r`, for a view of width `w` anchored centre-top.
    fn view_at(w: f32, r: Rect) -> (f32, f32) {
        let (win_w, _) = layout::window_size();
        (
            (win_w - w) / 2.0 + r.x + r.w / 2.0,
            layout::TOP_MARGIN + r.y + r.h / 2.0,
        )
    }

    fn size(i: &Island) -> (f32, f32) {
        let r = i.pill_rect();
        (r.w, r.h)
    }

    #[test]
    fn timer_tile_reports_open_and_the_panel_collapses_on_an_empty_click() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        settle(&mut i, &t);
        let tile = layout::home_tiles()[timer_index()];
        let p = i.pill_rect();
        let out = i.click(p.x + tile.x + 5.0, p.y + tile.y + 5.0, &t);
        assert_eq!(out, ClickOutcome::Open(timer_index()));
        i.open_timer(&t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::TimerPanel);
        assert_eq!(size(&i), (layout::TIMER_W, layout::TIMER_H));
        let l = i.view().layers;
        assert_eq!(
            (l.home, l.timer, l.timer_compact, l.alert),
            (0.0, 1.0, 0.0, 0.0)
        );
        // The ring is not a control: collapses. Stopped, so back to Idle.
        let (x, y) = view_at(layout::TIMER_W, layout::TP_RING);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Collapsed);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn timer_panel_controls_report_their_actions() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.open_timer(&t);
        settle(&mut i, &t);
        let w = layout::TIMER_W;
        let cases = [
            (layout::TP_TOGGLE, TimerAction::Toggle),
            (layout::TP_SKIP, TimerAction::Skip),
            (layout::TP_RESET, TimerAction::Reset),
            (layout::tp_chip(0), TimerAction::Preset(0)),
            (layout::tp_chip(1), TimerAction::Preset(1)),
            (layout::tp_chip(2), TimerAction::Preset(2)),
        ];
        for (r, want) in cases {
            let (x, y) = view_at(w, r);
            assert_eq!(i.click(x, y, &t), ClickOutcome::Timer(want), "{r:?}");
        }
        assert_eq!(i.state(), IslandState::TimerPanel);
        // Done inline: start continues and reset stops; the panel stays open.
        i.set_timer(DONE, &t);
        assert_eq!(i.state(), IslandState::TimerPanel);
        let (x, y) = view_at(w, layout::TP_TOGGLE);
        assert_eq!(
            i.click(x, y, &t),
            ClickOutcome::Timer(TimerAction::Continue)
        );
        let (x, y) = view_at(w, layout::TP_RESET);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Timer(TimerAction::Stop));
    }

    #[test]
    fn running_timer_rests_in_timer_compact_without_music() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_timer(RUNNING, &t);
        assert_eq!(i.state(), IslandState::TimerCompact);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::TIMER_COMPACT_W, layout::TIMER_COMPACT_H));
        assert_eq!(i.view().layers.timer_compact, 1.0);
        // A click on the compact opens the panel; an empty click comes back.
        let (cx, cy) = center(&i);
        assert_eq!(i.click(cx, cy, &t), ClickOutcome::Expanded);
        assert_eq!(i.state(), IslandState::TimerPanel);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::TIMER_W, layout::TP_RING);
        i.click(x, y, &t);
        assert_eq!(i.state(), IslandState::TimerCompact);
        // Stopping the timer returns to Idle.
        i.set_timer(TimerUi::default(), &t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn music_outranks_a_running_timer_and_hands_back_when_it_leaves() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.set_timer(RUNNING, &t);
        assert_eq!(i.state(), IslandState::MusicCompact);
        i.set_music(MusicUi::default(), &t);
        assert_eq!(i.state(), IslandState::TimerCompact);
        i.set_music(with_track(), &t);
        assert_eq!(i.state(), IslandState::MusicCompact);
        // The expanded player losing its track collapses to the timer.
        i.open_music(&t);
        i.set_music(MusicUi::default(), &t);
        assert_eq!(i.state(), IslandState::TimerCompact);
    }

    #[test]
    fn done_shows_the_alert_over_the_rest_states() {
        let t = tokens();
        for music in [false, true] {
            let mut i = Island::new(&t);
            if music {
                i.set_music(with_track(), &t);
            }
            i.set_timer(RUNNING, &t);
            i.set_timer(DONE, &t);
            assert_eq!(i.state(), IslandState::TimerAlert, "music {music}");
            settle(&mut i, &t);
            assert_eq!(size(&i), (layout::ALERT_W, layout::ALERT_H));
            assert_eq!(i.view().layers.alert, 1.0);
            // A track change does not knock the alert off.
            i.set_music(MusicUi::default(), &t);
            i.set_music(with_track(), &t);
            assert_eq!(i.state(), IslandState::TimerAlert);
        }
    }

    #[test]
    fn alert_waits_for_continue_or_stop() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_timer(DONE, &t);
        settle(&mut i, &t);
        let w = layout::ALERT_W;
        let (x, y) = view_at(w, layout::AL_TEXT);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored);
        assert_eq!(i.state(), IslandState::TimerAlert);
        assert!(!i.hover(x, y));
        let (x, y) = view_at(w, layout::AL_STOP);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Timer(TimerAction::Stop));
        let (x, y) = view_at(w, layout::AL_CONTINUE);
        assert_eq!(
            i.click(x, y, &t),
            ClickOutcome::Timer(TimerAction::Continue)
        );
        // The app continues the timer and pushes the new facts.
        i.set_timer(RUNNING, &t);
        assert_eq!(i.state(), IslandState::TimerCompact);
        i.set_timer(DONE, &t);
        i.set_timer(TimerUi::default(), &t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn done_over_an_open_panel_waits_for_the_collapse() {
        let t = tokens();
        type Open = fn(&mut Island, &ResolvedTokens);
        let panels: [(IslandState, Open); 6] = [
            (IslandState::Home, Island::expand),
            (IslandState::MusicExpanded, Island::open_music),
            (IslandState::Vitals, Island::open_vitals),
            (IslandState::Sweep, Island::open_sweep),
            (IslandState::TimerPanel, Island::open_timer),
            (IslandState::TilePanel, Island::open_tile),
        ];
        for (state, open) in panels {
            let mut i = Island::new(&t);
            i.set_timer(RUNNING, &t);
            open(&mut i, &t);
            i.set_timer(DONE, &t);
            assert_eq!(i.state(), state);
            i.collapse(&t);
            assert_eq!(i.state(), IslandState::TimerAlert, "after {state:?}");
        }
    }

    #[test]
    fn osd_over_the_alert_returns_to_the_alert() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_timer(DONE, &t);
        settle(&mut i, &t);
        i.show_osd(vol(0.5), &t);
        settle_pinned(&mut i, &t);
        assert_eq!(size(&i), (layout::OSD_W, layout::OSD_H));
        assert_eq!(i.state(), IslandState::TimerAlert);
        i.osd.pin(false);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::ALERT_W, layout::ALERT_H));
        assert_eq!(i.view().layers.alert, 1.0);
    }

    // ---- Snap ----

    fn snap_index() -> usize {
        FEATURES.iter().position(|f| f.id == "snap").unwrap()
    }

    #[test]
    fn snap_tile_reports_open() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        settle(&mut i, &t);
        let tile = layout::home_tiles()[snap_index()];
        let p = i.pill_rect();
        let out = i.click(p.x + tile.x + 5.0, p.y + tile.y + 5.0, &t);
        assert_eq!(out, ClickOutcome::Open(snap_index()));
        // The app collapses the island before it freezes the screen.
        assert_eq!(i.state(), IslandState::Home);
    }

    #[test]
    fn the_thumb_shows_at_its_size_and_returns_to_idle() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_snap(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        assert!(i.snap_shown());
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::SNAP_W, layout::SNAP_H));
        let v = i.view();
        assert_eq!((v.layers.idle, v.layers.snap), (0.0, 1.0));
        assert_eq!(v.radius, layout::SNAP_R * t.radius_scale);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::Idle);
        assert!(!i.snap_shown());
        settle(&mut i, &t);
        assert_eq!(i.view().layers.snap, 0.0);
    }

    #[test]
    fn the_thumb_outranks_media_and_the_timer_and_hands_back() {
        let t = tokens();
        type Setup = fn(&mut Island, &ResolvedTokens);
        let cases: [(Setup, IslandState); 4] = [
            (|_, _| {}, IslandState::Idle),
            (
                |i, t| i.set_music(with_track(), t),
                IslandState::MusicCompact,
            ),
            (|i, t| i.set_timer(RUNNING, t), IslandState::TimerCompact),
            (|i, t| i.set_timer(DONE, t), IslandState::TimerAlert),
        ];
        for (setup, rest) in cases {
            let mut i = Island::new(&t);
            setup(&mut i, &t);
            assert_eq!(i.state(), rest);
            i.show_snap(&t);
            assert_eq!(i.state(), IslandState::SnapThumb, "over {rest:?}");
            i.hide_snap(&t);
            assert_eq!(i.state(), rest, "back to {rest:?}");
        }
    }

    #[test]
    fn facts_arriving_under_the_thumb_wait_for_it_to_go() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_snap(&t);
        i.set_music(with_track(), &t);
        i.set_timer(RUNNING, &t);
        i.set_timer(DONE, &t);
        // The timer finishing ranks below the thumb: it stays.
        assert_eq!(i.state(), IslandState::SnapThumb);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::TimerAlert);
        // Music leaving under the thumb hands back to Idle once it goes.
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.show_snap(&t);
        i.set_music(MusicUi::default(), &t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::Idle);
        // Hiding a thumb that isn't up is harmless.
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn the_thumb_takes_over_an_open_panel_and_returns_to_rest() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_timer(RUNNING, &t);
        i.open_vitals(&t);
        i.show_snap(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::TimerCompact);
    }

    #[test]
    fn thumb_buttons_report_and_elsewhere_dismisses() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_snap(&t);
        settle(&mut i, &t);
        let w = layout::SNAP_W;
        let (x, y) = view_at(w, layout::SN_COPY);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Copy));
        let (x, y) = view_at(w, layout::SN_SAVE);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Save));
        assert_eq!(i.state(), IslandState::SnapThumb);
        // The shot itself is not a button.
        let (x, y) = view_at(w, layout::SN_SHOT);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Dismiss));
        assert_eq!(i.state(), IslandState::Idle);
        assert!(!i.snap_shown());
    }

    #[test]
    fn a_dismissed_thumb_over_music_returns_to_music_compact() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.show_snap(&t);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::SNAP_W, layout::SN_CAPTION);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Dismiss));
        assert_eq!(i.state(), IslandState::MusicCompact);
    }

    #[test]
    fn hovering_the_thumb_is_tracked_with_its_buttons() {
        let t = tokens();
        let mut i = Island::new(&t);
        // Nothing to hover before a thumb is up.
        let (cx, cy) = center(&i);
        i.hover(cx, cy);
        assert!(!i.snap_hovered());
        i.show_snap(&t);
        settle(&mut i, &t);
        let w = layout::SNAP_W;
        let (x, y) = view_at(w, layout::SN_SHOT);
        assert!(!i.hover(x, y), "no button under the pointer");
        assert!(i.snap_hovered());
        let (x, y) = view_at(w, layout::SN_SAVE);
        assert!(i.hover(x, y));
        assert_eq!(i.view().snap_hover, Some(SnapAction::Save));
        // Off the pill, inside the window.
        assert!(i.hover(2.0, 2.0));
        assert!(!i.snap_hovered());
        assert_eq!(i.view().snap_hover, None);
        i.hover(x, y);
        assert!(i.clear_hover());
        assert!(!i.snap_hovered());
    }

    #[test]
    fn osd_over_the_thumb_returns_to_the_thumb() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_snap(&t);
        settle(&mut i, &t);
        i.show_osd(vol(0.5), &t);
        settle_pinned(&mut i, &t);
        assert_eq!(size(&i), (layout::OSD_W, layout::OSD_H));
        assert_eq!(i.state(), IslandState::SnapThumb);
        // While the OSD is up, the thumb is neither hovered nor clickable.
        let (x, y) = center(&i);
        i.hover(x, y);
        assert!(!i.snap_hovered());
        i.osd.pin(false);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::SNAP_W, layout::SNAP_H));
        assert_eq!(i.view().layers.snap, 1.0);
    }

    // ---- Settings ----

    fn open_home(i: &mut Island, t: &ResolvedTokens) {
        i.expand(t);
        settle(i, t);
    }

    #[test]
    fn the_gear_opens_settings_and_back_returns_to_home() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        let (x, y) = view_at(layout::HOME_W, layout::HOME_GEAR);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Expanded);
        assert_eq!(i.state(), IslandState::Settings);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::SETTINGS_W, layout::SETTINGS_H));
        let l = i.view().layers;
        assert_eq!((l.settings, l.home), (1.0, 0.0));
        let (x, y) = view_at(layout::SETTINGS_W, layout::ST_BACK);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Home);
        assert_eq!(i.state(), IslandState::Home);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::HOME_W, layout::HOME_H));
    }

    #[test]
    fn an_empty_click_in_settings_returns_to_home() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.open_settings(&t);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::SETTINGS_W, layout::ST_ROW_ALT);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Home);
        assert_eq!(i.state(), IslandState::Home);
    }

    #[test]
    fn settings_controls_report_their_actions_and_stay_open() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.open_settings(&t);
        settle(&mut i, &t);
        for (r, want) in [
            (layout::ST_TOGGLE, SettingsAction::ToggleAutostart),
            (layout::ST_HOTKEY, SettingsAction::CycleHotkey),
            (layout::ST_ALT, SettingsAction::CycleAltHotkey),
            (layout::ST_STYLE, SettingsAction::ToggleStyle),
            (layout::ST_RESERVE, SettingsAction::ToggleReserve),
        ] {
            let (x, y) = view_at(layout::SETTINGS_W, r);
            assert_eq!(i.click(x, y, &t), ClickOutcome::Settings(want), "{r:?}");
            assert_eq!(i.state(), IslandState::Settings);
        }
    }

    #[test]
    fn the_gear_only_responds_on_home_and_a_header_click_elsewhere_still_collapses() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        // The header outside the gear is an empty click.
        let (x, y) = view_at(layout::HOME_W, layout::HEADER);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Collapsed);
        // The same spot on Idle or in another panel is no gear.
        i.open_timer(&t);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::HOME_W, layout::HOME_GEAR);
        assert!(!i.hover(x, y));
        assert_ne!(i.click(x, y, &t), ClickOutcome::Expanded);
        assert_ne!(i.state(), IslandState::Settings);
    }

    #[test]
    fn the_gear_hover_follows_the_pointer_and_clears() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        let (x, y) = view_at(layout::HOME_W, layout::HOME_GEAR);
        assert!(i.hover(x, y));
        assert!(i.view().gear_hover);
        assert!(!i.hover(x, y));
        assert!(i.hover(x - 100.0, y));
        assert!(!i.view().gear_hover);
        assert!(i.hover(x, y));
        assert!(i.clear_hover());
        assert!(!i.view().gear_hover);
    }

    #[test]
    fn settings_is_not_a_rest_state_and_a_finished_timer_waits_for_the_collapse() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.open_settings(&t);
        settle(&mut i, &t);
        i.set_timer(DONE, &t);
        assert_eq!(i.state(), IslandState::Settings);
        let (x, y) = view_at(layout::SETTINGS_W, layout::ST_BACK);
        i.click(x, y, &t);
        settle(&mut i, &t);
        let p = i.pill_rect();
        assert_eq!(i.click(p.x + 2.0, p.y + 2.0, &t), ClickOutcome::Collapsed);
        assert_eq!(i.state(), IslandState::TimerAlert);
    }

    // ---- Tile ----

    fn tile_index() -> usize {
        FEATURES.iter().position(|f| f.id == "tile").unwrap()
    }

    /// Window-logical point for panel-local `(x, y)`: the panel hangs centre-top.
    fn panel_pt(x: f32, y: f32) -> (f32, f32) {
        let (win_w, _) = layout::window_size();
        (
            (win_w - layout::TILE_PANEL_W) / 2.0 + x,
            layout::TOP_MARGIN + y,
        )
    }

    fn open_settled_tile(t: &ResolvedTokens) -> Island {
        let mut i = Island::new(t);
        i.open_tile(t);
        settle(&mut i, t);
        i
    }

    #[test]
    fn the_tile_panel_opens_from_idle_at_its_size_and_cross_fades() {
        let t = tokens();
        let mut i = Island::new(&t);
        assert!(!i.tile_open());
        i.open_tile(&t);
        assert_eq!(i.state(), IslandState::TilePanel);
        assert!(i.tile_open());
        assert!(i.is_animating());
        // Mid-flight both contents are partly visible: the cross-fade.
        for _ in 0..6 {
            i.step(1.0 / 144.0, &t);
        }
        let l = i.view().layers;
        assert!(l.tile > 0.0 && l.tile < 1.0, "tile {}", l.tile);
        assert!(l.idle > 0.0 && l.idle < 1.0, "idle {}", l.idle);
        settle(&mut i, &t);
        assert_eq!((layout::TILE_PANEL_W, layout::TILE_PANEL_H), (400.0, 168.0));
        assert_eq!(size(&i), (400.0, 168.0));
        let v = i.view();
        assert_eq!(v.radius, 32.0 * t.radius_scale);
        assert_eq!((v.layers.idle, v.layers.tile), (0.0, 1.0));
    }

    #[test]
    fn the_tile_panel_opens_from_home_and_collapse_goes_to_the_rest_state() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        i.open_tile(&t);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::TILE_PANEL_W, layout::TILE_PANEL_H));
        let l = i.view().layers;
        assert_eq!((l.home, l.tile), (0.0, 1.0));
        // Not back to Home: the panel is not a step down from it.
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::Idle);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::IDLE_W, layout::IDLE_H));
        let l = i.view().layers;
        assert_eq!((l.idle, l.tile), (1.0, 0.0));
    }

    #[test]
    fn the_tile_panel_opens_from_every_rest_state_and_collapse_returns_to_it() {
        let t = tokens();
        type Setup = fn(&mut Island, &ResolvedTokens);
        let cases: [(Setup, IslandState); 5] = [
            (|_, _| {}, IslandState::Idle),
            (
                |i, t| i.set_music(with_track(), t),
                IslandState::MusicCompact,
            ),
            (|i, t| i.set_timer(RUNNING, t), IslandState::TimerCompact),
            (|i, t| i.set_timer(DONE, t), IslandState::TimerAlert),
            (
                |i, t| {
                    i.set_timer(RUNNING, t);
                    i.set_music(with_track(), t);
                },
                IslandState::MusicCompact,
            ),
        ];
        for (setup, rest) in cases {
            let mut i = Island::new(&t);
            setup(&mut i, &t);
            assert_eq!(i.state(), rest);
            i.open_tile(&t);
            settle(&mut i, &t);
            assert_eq!(i.state(), IslandState::TilePanel, "from {rest:?}");
            assert_eq!(size(&i), (layout::TILE_PANEL_W, layout::TILE_PANEL_H));
            assert_eq!(i.view().layers.tile, 1.0);
            i.collapse(&t);
            assert_eq!(i.state(), rest, "back to {rest:?}");
            settle(&mut i, &t);
            let (w, h, _) = Island::base_size(rest);
            assert_eq!(size(&i), (w, h), "{rest:?}");
            assert_eq!(i.view().layers.tile, 0.0);
            assert!(!i.tile_open());
        }
    }

    #[test]
    fn opening_the_tile_panel_again_changes_nothing() {
        let t = tokens();
        let mut i = open_settled_tile(&t);
        i.open_tile(&t);
        assert!(!i.is_animating());
        assert_eq!(i.state(), IslandState::TilePanel);
    }

    #[test]
    fn facts_arriving_under_the_tile_panel_wait_for_the_collapse() {
        let t = tokens();
        let mut i = open_settled_tile(&t);
        i.set_music(with_track(), &t);
        i.set_timer(RUNNING, &t);
        assert_eq!(i.state(), IslandState::TilePanel);
        i.set_timer(DONE, &t);
        assert!(i.tile_open());
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::TimerAlert);
        assert!(!i.tile_open());
    }

    #[test]
    fn each_state_lights_exactly_its_own_layer() {
        let t = tokens();
        type Setup = fn(&mut Island, &ResolvedTokens);
        let states: [(IslandState, Setup); 14] = [
            (IslandState::Idle, |_, _| {}),
            (IslandState::Home, Island::expand),
            (IslandState::MusicCompact, |i, t| {
                i.set_music(with_track(), t)
            }),
            (IslandState::MusicExpanded, Island::open_music),
            (IslandState::Vitals, Island::open_vitals),
            (IslandState::Sweep, Island::open_sweep),
            (IslandState::TimerPanel, Island::open_timer),
            (IslandState::TimerCompact, |i, t| i.set_timer(RUNNING, t)),
            (IslandState::TimerAlert, |i, t| i.set_timer(DONE, t)),
            (IslandState::SnapThumb, Island::show_snap),
            (IslandState::Settings, Island::open_settings),
            (IslandState::TilePanel, Island::open_tile),
            (IslandState::DjProgress, |i, t| i.set_dj(DJ_PROGRESS, t)),
            (IslandState::DjNotice, |i, t| i.set_dj(DJ_NOTICE, t)),
        ];
        assert_eq!(states.len(), LAYERS);
        for (state, setup) in states {
            let mut i = Island::new(&t);
            setup(&mut i, &t);
            assert_eq!(i.state(), state);
            settle(&mut i, &t);
            let l = i.view().layers;
            let got = [
                l.idle,
                l.home,
                l.compact,
                l.expanded,
                l.vitals,
                l.sweep,
                l.timer,
                l.timer_compact,
                l.alert,
                l.snap,
                l.settings,
                l.tile,
                l.dj_progress,
                l.dj_notice,
            ];
            let want: [f32; LAYERS] =
                std::array::from_fn(|n| if n == state.layer() { 1.0 } else { 0.0 });
            assert_eq!(got, want, "{state:?}");
        }
    }

    #[test]
    fn tile_clicks_report_panel_local_coordinates_and_leave_the_panel_open() {
        let t = tokens();
        let mut i = open_settled_tile(&t);
        let (w, h) = (layout::TILE_PANEL_W, layout::TILE_PANEL_H);
        for (lx, ly) in [
            (0.0, 0.0),
            (w - 1.0, 0.0),
            (0.0, h - 1.0),
            (w - 1.0, h - 1.0),
            (w / 2.0, h / 2.0),
        ] {
            let (x, y) = panel_pt(lx, ly);
            assert_eq!(
                i.click(x, y, &t),
                ClickOutcome::Tile { x: lx, y: ly },
                "{lx},{ly}"
            );
            // An empty spot does not collapse by itself: that is the app's call.
            assert_eq!(i.state(), IslandState::TilePanel);
        }
        // Just outside the pill is not the panel's.
        for (lx, ly) in [(-1.0, 10.0), (w, 10.0), (10.0, -1.0), (10.0, h)] {
            let (x, y) = panel_pt(lx, ly);
            assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored, "{lx},{ly}");
        }
        assert_eq!(i.state(), IslandState::TilePanel);
    }

    #[test]
    fn tile_clicks_are_panel_local_even_mid_morph() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        i.open_tile(&t);
        // Not settled: the pill is still shrinking from Home, but the content is anchored
        // centre-top.
        i.step(1.0 / 144.0, &t);
        assert!(i.is_animating());
        assert!(i.pill_rect().w > layout::TILE_PANEL_W);
        let (x, y) = panel_pt(120.0, 60.0);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Tile { x: 120.0, y: 60.0 });
    }

    #[test]
    fn the_osd_outranks_the_tile_panel_and_the_panel_returns() {
        let t = tokens();
        let mut i = open_settled_tile(&t);
        i.show_osd(vol(0.5), &t);
        settle_pinned(&mut i, &t);
        // The OSD covers the pill; the state stays put.
        assert_eq!(i.state(), IslandState::TilePanel);
        assert_eq!(size(&i), (layout::OSD_W, layout::OSD_H));
        // Where the panel's content would be, and on the OSD off its bar: not a Tile click.
        let (x, y) = panel_pt(200.0, 100.0);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored);
        let r = layout::osd_rect();
        assert_eq!(i.click(r.x + 2.0, r.y + 2.0, &t), ClickOutcome::Ignored);
        let bar = layout::OSD_BAR;
        let out = i.click(r.x + bar.x + bar.w / 2.0, r.y + bar.y + bar.h / 2.0, &t);
        assert_eq!(out, ClickOutcome::SetVolume(0.5));
        i.release();
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::TilePanel);
        assert_eq!(size(&i), (layout::TILE_PANEL_W, layout::TILE_PANEL_H));
        assert_eq!(i.view().layers.tile, 1.0);
        assert_eq!(i.view().osd.alpha, 0.0);
        let (x, y) = panel_pt(200.0, 100.0);
        assert!(matches!(i.click(x, y, &t), ClickOutcome::Tile { .. }));
    }

    #[test]
    fn the_snap_thumb_takes_over_the_tile_panel_and_hands_back_to_rest() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_tile(&t);
        settle(&mut i, &t);
        i.show_snap(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        assert!(!i.tile_open());
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::SNAP_W, layout::SNAP_H));
        let l = i.view().layers;
        assert_eq!((l.tile, l.snap), (0.0, 1.0));
        // A click is the thumb's, never the panel's; the panel's old area is outside it.
        let (x, y) = view_at(layout::SNAP_W, layout::SN_COPY);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Copy));
        let (x, y) = panel_pt(200.0, 100.0);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored);
        let (x, y) = view_at(layout::SNAP_W, layout::SN_SHOT);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Dismiss));
        assert_eq!(i.state(), IslandState::MusicCompact);
    }

    #[test]
    fn a_panel_opened_over_the_thumb_gives_it_back_on_collapse() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_snap(&t);
        settle(&mut i, &t);
        i.open_tile(&t);
        settle(&mut i, &t);
        assert!(i.tile_open());
        assert_eq!(size(&i), (layout::TILE_PANEL_W, layout::TILE_PANEL_H));
        // The thumb's countdown is the app's and keeps running under the panel.
        assert!(i.snap_shown());
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        // If the countdown ends while the panel is open there is nothing to give back.
        i.open_tile(&t);
        i.hide_snap(&t);
        assert!(i.tile_open());
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn the_home_tile_is_available_and_reports_open_without_shaking() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        let tile = layout::home_tiles()[tile_index()];
        let p = i.pill_rect();
        let (x, y) = (p.x + tile.x + 5.0, p.y + tile.y + 5.0);
        assert!(i.hover(x, y));
        assert_eq!(i.view().hover, Some(tile_index()));
        assert_eq!(i.click(x, y, &t), ClickOutcome::Open(tile_index()));
        // The app decides what opening means; the island only reports the click.
        assert_eq!(i.state(), IslandState::Home);
        assert!(i.view().shake.is_none() && i.view().hint.is_none());
        i.open_tile(&t);
        settle(&mut i, &t);
        assert_eq!(i.view().layers.tile, 1.0);
    }

    // ---- dj ----

    const DJ_PROGRESS: DjUi = DjUi {
        progress: true,
        notice: false,
    };
    const DJ_NOTICE: DjUi = DjUi {
        progress: false,
        notice: true,
    };
    const DJ_BOTH: DjUi = DjUi {
        progress: true,
        notice: true,
    };

    /// One fact that picks the rest state, switched on or off on its own.
    type Fact = fn(&mut Island, bool, &ResolvedTokens);

    fn snap_fact(i: &mut Island, on: bool, t: &ResolvedTokens) {
        if on {
            i.show_snap(t);
        } else {
            i.hide_snap(t);
        }
    }

    fn notice_fact(i: &mut Island, on: bool, t: &ResolvedTokens) {
        let ui = DjUi {
            notice: on,
            ..i.dj_ui()
        };
        i.set_dj(ui, t);
    }

    fn progress_fact(i: &mut Island, on: bool, t: &ResolvedTokens) {
        let ui = DjUi {
            progress: on,
            ..i.dj_ui()
        };
        i.set_dj(ui, t);
    }

    fn alert_fact(i: &mut Island, on: bool, t: &ResolvedTokens) {
        let ui = TimerUi {
            done: on,
            ..i.timer_ui()
        };
        i.set_timer(ui, t);
    }

    fn timer_fact(i: &mut Island, on: bool, t: &ResolvedTokens) {
        let ui = TimerUi {
            active: on,
            ..i.timer_ui()
        };
        i.set_timer(ui, t);
    }

    fn music_fact(i: &mut Island, on: bool, t: &ResolvedTokens) {
        let ui = MusicUi {
            has_track: on,
            ..i.music_ui()
        };
        i.set_music(ui, t);
    }

    /// Every fact that picks the rest state, highest priority first, and the state it gives.
    const BY_PRIORITY: [(&str, Fact, IslandState); 6] = [
        ("snap thumb", snap_fact, IslandState::SnapThumb),
        ("dj notice", notice_fact, IslandState::DjNotice),
        ("timer alert", alert_fact, IslandState::TimerAlert),
        ("dj progress", progress_fact, IslandState::DjProgress),
        ("music", music_fact, IslandState::MusicCompact),
        ("timer", timer_fact, IslandState::TimerCompact),
    ];

    #[test]
    fn the_dj_pills_show_at_their_sizes_and_cross_fade() {
        let t = tokens();
        let mut i = Island::new(&t);
        assert_eq!(i.dj_ui(), DjUi::default());
        i.set_dj(DJ_PROGRESS, &t);
        assert_eq!(i.state(), IslandState::DjProgress);
        assert_eq!(i.dj_ui(), DJ_PROGRESS);
        assert!(i.is_animating());
        // Mid-flight both contents are partly visible: the cross-fade.
        for _ in 0..6 {
            i.step(1.0 / 144.0, &t);
        }
        let l = i.view().layers;
        assert!(
            l.dj_progress > 0.0 && l.dj_progress < 1.0,
            "{}",
            l.dj_progress
        );
        assert!(l.idle > 0.0 && l.idle < 1.0, "{}", l.idle);
        settle(&mut i, &t);
        assert_eq!(size(&i), (300.0, 36.0));
        let v = i.view();
        let radius = |r: f32, h: f32| (r * t.radius_scale).min(h / 2.0);
        assert_eq!(
            v.radius,
            radius(layout::DJ_PROGRESS_R, layout::DJ_PROGRESS_H)
        );
        assert_eq!(
            (v.layers.idle, v.layers.dj_progress, v.layers.dj_notice),
            (0.0, 1.0, 0.0)
        );
        assert!(!i.is_animating());

        // A notice takes the pill over, at its own size.
        i.set_dj(DJ_BOTH, &t);
        assert_eq!(i.state(), IslandState::DjNotice);
        settle(&mut i, &t);
        assert_eq!(size(&i), (360.0, 36.0));
        let v = i.view();
        assert_eq!(v.radius, radius(layout::DJ_NOTICE_R, layout::DJ_NOTICE_H));
        assert_eq!(
            (v.layers.dj_progress, v.layers.dj_notice, v.layers.idle),
            (0.0, 1.0, 0.0)
        );

        // The notice going hands the pill back to the progress, and that going to Idle.
        i.set_dj(DJ_PROGRESS, &t);
        assert_eq!(i.state(), IslandState::DjProgress);
        settle(&mut i, &t);
        assert_eq!(size(&i), (300.0, 36.0));
        assert_eq!(i.view().layers.dj_notice, 0.0);
        i.set_dj(DjUi::default(), &t);
        assert_eq!(i.state(), IslandState::Idle);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::IDLE_W, layout::IDLE_H));
        assert_eq!(i.view().layers.dj_progress, 0.0);
    }

    #[test]
    fn the_dj_pills_sit_inside_the_reserved_strip() {
        let t = tokens();
        for ui in [DJ_PROGRESS, DJ_NOTICE] {
            let mut i = Island::new(&t);
            i.set_dj(ui, &t);
            settle(&mut i, &t);
            let p = i.pill_rect();
            assert_eq!(p.h, 36.0);
            assert!(p.y + p.h <= layout::RESERVE_H, "{ui:?}");
            // Centred like every other view.
            let (win_w, _) = layout::window_size();
            assert_eq!(p.x, (win_w - p.w) / 2.0);
        }
    }

    #[test]
    fn setting_the_same_dj_facts_again_changes_nothing() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_dj(DjUi::default(), &t);
        assert!(!i.is_animating());
        i.set_dj(DJ_NOTICE, &t);
        settle(&mut i, &t);
        i.set_dj(DJ_NOTICE, &t);
        assert!(!i.is_animating());
        assert_eq!(i.state(), IslandState::DjNotice);
    }

    #[test]
    fn every_pair_of_rest_facts_resolves_by_priority() {
        let t = tokens();
        for (a, (hi_name, hi, hi_state)) in BY_PRIORITY.iter().enumerate() {
            for (lo_name, lo, lo_state) in &BY_PRIORITY[a + 1..] {
                // Either arrival order gives the same state.
                for hi_first in [false, true] {
                    let why = format!("{hi_name} over {lo_name} (higher first: {hi_first})");
                    let mut i = Island::new(&t);
                    if hi_first {
                        hi(&mut i, true, &t);
                        lo(&mut i, true, &t);
                    } else {
                        lo(&mut i, true, &t);
                        hi(&mut i, true, &t);
                    }
                    assert_eq!(i.state(), *hi_state, "{why}");
                    settle(&mut i, &t);
                    let (w, h, _) = Island::base_size(*hi_state);
                    assert_eq!(size(&i), (w, h), "{why}");
                    // The higher one leaving hands the pill to the lower one, then Idle.
                    hi(&mut i, false, &t);
                    assert_eq!(i.state(), *lo_state, "{why}: higher gone");
                    settle(&mut i, &t);
                    let (w, h, _) = Island::base_size(*lo_state);
                    assert_eq!(size(&i), (w, h), "{why}: higher gone");
                    lo(&mut i, false, &t);
                    assert_eq!(i.state(), IslandState::Idle, "{why}: both gone");
                }
            }
        }
    }

    #[test]
    fn the_rest_states_climb_and_fall_in_priority_order() {
        let t = tokens();
        let mut i = Island::new(&t);
        for (name, fact, state) in BY_PRIORITY.iter().rev() {
            fact(&mut i, true, &t);
            assert_eq!(i.state(), *state, "{name} on");
        }
        for (k, (name, fact, _)) in BY_PRIORITY.iter().enumerate() {
            fact(&mut i, false, &t);
            let next = BY_PRIORITY
                .get(k + 1)
                .map_or(IslandState::Idle, |(_, _, state)| *state);
            assert_eq!(i.state(), next, "{name} off");
        }
    }

    #[test]
    fn dj_facts_move_a_resting_island_from_every_rest_state() {
        let t = tokens();
        type Setup = fn(&mut Island, &ResolvedTokens);
        // (setup, the rest state, the state with a progress up, the state with a notice up)
        let cases: [(Setup, IslandState, IslandState, IslandState); 5] = [
            (
                |_, _| {},
                IslandState::Idle,
                IslandState::DjProgress,
                IslandState::DjNotice,
            ),
            (
                |i, t| i.set_music(with_track(), t),
                IslandState::MusicCompact,
                IslandState::DjProgress,
                IslandState::DjNotice,
            ),
            (
                |i, t| i.set_timer(RUNNING, t),
                IslandState::TimerCompact,
                IslandState::DjProgress,
                IslandState::DjNotice,
            ),
            // The alert outranks a progress but not a notice.
            (
                |i, t| i.set_timer(DONE, t),
                IslandState::TimerAlert,
                IslandState::TimerAlert,
                IslandState::DjNotice,
            ),
            // The thumb outranks both.
            (
                Island::show_snap,
                IslandState::SnapThumb,
                IslandState::SnapThumb,
                IslandState::SnapThumb,
            ),
        ];
        for (setup, rest, with_progress, with_notice) in cases {
            for (ui, want) in [
                (DJ_PROGRESS, with_progress),
                (DJ_NOTICE, with_notice),
                (DJ_BOTH, with_notice),
            ] {
                let why = format!("{ui:?} over {rest:?}");
                let mut i = Island::new(&t);
                setup(&mut i, &t);
                assert_eq!(i.state(), rest, "{why}");
                i.set_dj(ui, &t);
                assert_eq!(i.state(), want, "{why}");
                settle(&mut i, &t);
                let (w, h, _) = Island::base_size(want);
                assert_eq!(size(&i), (w, h), "{why}");
                // The same facts again change nothing.
                i.set_dj(ui, &t);
                assert!(!i.is_animating(), "{why}");
                // And clearing them returns to where the island was.
                i.set_dj(DjUi::default(), &t);
                assert_eq!(i.state(), rest, "{why}: cleared");
            }
        }
    }

    #[test]
    fn dj_facts_over_an_open_panel_wait_for_the_collapse() {
        let t = tokens();
        type Open = fn(&mut Island, &ResolvedTokens);
        let panels: [(IslandState, Open); 7] = [
            (IslandState::Home, Island::expand),
            (IslandState::MusicExpanded, Island::open_music),
            (IslandState::Vitals, Island::open_vitals),
            (IslandState::Sweep, Island::open_sweep),
            (IslandState::TimerPanel, Island::open_timer),
            (IslandState::Settings, Island::open_settings),
            (IslandState::TilePanel, Island::open_tile),
        ];
        for (state, open) in panels {
            for (ui, rest) in [
                (DJ_PROGRESS, IslandState::DjProgress),
                (DJ_NOTICE, IslandState::DjNotice),
                (DJ_BOTH, IslandState::DjNotice),
            ] {
                let why = format!("{ui:?} over {state:?}");
                let mut i = Island::new(&t);
                open(&mut i, &t);
                settle(&mut i, &t);
                i.set_dj(ui, &t);
                // The panel stays put: same state, same size, still lit.
                assert_eq!(i.state(), state, "{why}");
                assert!(!i.is_animating(), "{why}");
                let (w, h, _) = Island::base_size(state);
                assert_eq!(size(&i), (w, h), "{why}");
                assert_eq!(i.view().layers.dj_progress, 0.0, "{why}");
                assert_eq!(i.view().layers.dj_notice, 0.0, "{why}");
                // It shows once the panel collapses.
                i.collapse(&t);
                assert_eq!(i.state(), rest, "{why}: collapsed");
                settle(&mut i, &t);
                let (w, h, _) = Island::base_size(rest);
                assert_eq!(size(&i), (w, h), "{why}: collapsed");
            }
            // A pill that came and went under the panel leaves nothing to show.
            let mut i = Island::new(&t);
            open(&mut i, &t);
            i.set_dj(DJ_NOTICE, &t);
            i.set_dj(DjUi::default(), &t);
            assert_eq!(i.state(), state);
            i.collapse(&t);
            assert_eq!(i.state(), IslandState::Idle, "after {state:?}");
        }
    }

    #[test]
    fn a_panel_opened_over_a_dj_pill_gives_it_back_on_collapse() {
        let t = tokens();
        type Open = fn(&mut Island, &ResolvedTokens);
        let opens: [Open; 3] = [Island::expand, Island::open_timer, Island::open_tile];
        for open in opens {
            for (ui, rest) in [
                (DJ_PROGRESS, IslandState::DjProgress),
                (DJ_NOTICE, IslandState::DjNotice),
            ] {
                let mut i = Island::new(&t);
                i.set_dj(ui, &t);
                settle(&mut i, &t);
                open(&mut i, &t);
                settle(&mut i, &t);
                assert_ne!(i.state(), rest);
                assert_eq!(i.view().layers.dj_progress, 0.0);
                assert_eq!(i.view().layers.dj_notice, 0.0);
                i.collapse(&t);
                assert_eq!(i.state(), rest);
                settle(&mut i, &t);
                let (w, h, _) = Island::base_size(rest);
                assert_eq!(size(&i), (w, h));
            }
        }
    }

    #[test]
    fn a_click_on_the_progress_pill_opens_home_like_idle() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_dj(DJ_PROGRESS, &t);
        settle(&mut i, &t);
        // Outside the pill is not its click.
        assert_eq!(i.click(1.0, 1.0, &t), ClickOutcome::Ignored);
        let p = i.pill_rect();
        assert_eq!(i.click(p.x - 1.0, p.y + 10.0, &t), ClickOutcome::Ignored);
        assert_eq!(i.state(), IslandState::DjProgress);
        let (cx, cy) = center(&i);
        assert_eq!(i.click(cx, cy, &t), ClickOutcome::Expanded);
        assert_eq!(i.state(), IslandState::Home);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::HOME_W, layout::HOME_H));
        let l = i.view().layers;
        assert_eq!((l.home, l.dj_progress, l.idle), (1.0, 0.0, 0.0));
        // A notice arriving while Home is open waits for the collapse.
        i.set_dj(DJ_BOTH, &t);
        assert_eq!(i.state(), IslandState::Home);
        // An empty click on Home (the header) collapses to the pill that is up by then.
        let r = i.pill_rect();
        assert_eq!(i.click(r.x + 40.0, r.y + 28.0, &t), ClickOutcome::Collapsed);
        assert_eq!(i.state(), IslandState::DjNotice);
        // Had the notice gone meanwhile, the progress pill is the one left.
        i.open_tile(&t);
        i.set_dj(DJ_PROGRESS, &t);
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::DjProgress);
    }

    #[test]
    fn a_click_on_the_notice_asks_the_app_to_dismiss_it() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_dj(DJ_BOTH, &t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::DjNotice);
        let p = i.pill_rect();
        for (x, y) in [
            center(&i),
            (p.x + 2.0, p.y + 2.0),
            (p.x + p.w - 2.0, p.y + p.h - 2.0),
        ] {
            assert_eq!(i.click(x, y, &t), ClickOutcome::DjDismiss, "{x},{y}");
            // The island has not moved: it leaves when the app pushes the new facts.
            assert_eq!(i.state(), IslandState::DjNotice);
        }
        // Just outside the pill: not the notice's.
        for (x, y) in [
            (p.x - 1.0, p.y + 10.0),
            (p.x + p.w, p.y + 10.0),
            (p.x + 10.0, p.y - 1.0),
            (p.x + 10.0, p.y + p.h),
        ] {
            assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored, "{x},{y}");
        }
        // The app dismissed the notice: the progress is next, and after it Idle.
        i.set_dj(DJ_PROGRESS, &t);
        assert_eq!(i.state(), IslandState::DjProgress);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::DJ_PROGRESS_W, layout::DJ_PROGRESS_H));
        i.set_dj(DjUi::default(), &t);
        assert_eq!(i.state(), IslandState::Idle);
        // A notice that was up alone goes straight to what is under it.
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.set_dj(DJ_NOTICE, &t);
        settle(&mut i, &t);
        let (cx, cy) = center(&i);
        assert_eq!(i.click(cx, cy, &t), ClickOutcome::DjDismiss);
        i.set_dj(DjUi::default(), &t);
        assert_eq!(i.state(), IslandState::MusicCompact);
    }

    #[test]
    fn the_notice_is_clickable_mid_morph_where_the_pill_is() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_dj(DJ_NOTICE, &t);
        i.step(1.0 / 144.0, &t);
        assert!(i.is_animating());
        // The pill is still growing from Idle: its centre is inside it already.
        let (cx, cy) = center(&i);
        assert_eq!(i.click(cx, cy, &t), ClickOutcome::DjDismiss);
    }

    #[test]
    fn the_dj_pills_have_no_hover() {
        let t = tokens();
        for ui in [DJ_PROGRESS, DJ_NOTICE] {
            let mut i = Island::new(&t);
            i.set_dj(ui, &t);
            settle(&mut i, &t);
            let (x, y) = center(&i);
            assert!(!i.hover(x, y), "{ui:?}");
            assert!(!i.snap_hovered());
            let v = i.view();
            assert_eq!((v.hover, v.snap_hover, v.gear_hover), (None, None, false));
            assert!(!i.clear_hover());
        }
    }

    #[test]
    fn the_osd_covers_the_dj_pills_and_they_return() {
        let t = tokens();
        for (ui, state, w) in [
            (DJ_PROGRESS, IslandState::DjProgress, layout::DJ_PROGRESS_W),
            (DJ_NOTICE, IslandState::DjNotice, layout::DJ_NOTICE_W),
        ] {
            let mut i = Island::new(&t);
            i.set_dj(ui, &t);
            settle(&mut i, &t);
            i.show_osd(vol(0.5), &t);
            settle_pinned(&mut i, &t);
            // The OSD takes the pill; the state stays put.
            assert_eq!(i.state(), state);
            assert_eq!(size(&i), (layout::OSD_W, layout::OSD_H));
            // Its clicks are the OSD's: off the bar nothing happens, not even a dismiss.
            let r = layout::osd_rect();
            assert_eq!(i.click(r.x + 2.0, r.y + 2.0, &t), ClickOutcome::Ignored);
            let (cx, cy) = center(&i);
            let out = i.click(cx, cy, &t);
            assert!(
                matches!(out, ClickOutcome::Ignored | ClickOutcome::SetVolume(_)),
                "{out:?}"
            );
            i.release();
            i.osd.pin(false);
            settle(&mut i, &t);
            assert_eq!(i.state(), state);
            assert_eq!(size(&i), (w, 36.0));
            assert_eq!(i.view().osd.alpha, 0.0);
            let (cx, cy) = center(&i);
            let want = match state {
                IslandState::DjNotice => ClickOutcome::DjDismiss,
                _ => ClickOutcome::Expanded,
            };
            assert_eq!(i.click(cx, cy, &t), want);
        }
        // A fact arriving under the OSD moves the state, not the pill: it stays the OSD.
        let mut i = Island::new(&t);
        i.show_osd(vol(0.5), &t);
        settle_pinned(&mut i, &t);
        i.set_dj(DJ_NOTICE, &t);
        assert_eq!(i.state(), IslandState::DjNotice);
        settle_pinned(&mut i, &t);
        assert_eq!(size(&i), (layout::OSD_W, layout::OSD_H));
        i.osd.pin(false);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::DJ_NOTICE_W, layout::DJ_NOTICE_H));
        assert_eq!(i.view().layers.dj_notice, 1.0);
    }

    #[test]
    fn dj_facts_under_the_thumb_wait_for_it_and_hand_back_when_it_goes() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_snap(&t);
        i.set_dj(DJ_BOTH, &t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::SNAP_W, layout::SNAP_H));
        assert_eq!(i.view().layers.snap, 1.0);
        // Dismissing the thumb shows the notice, which still outranks the progress.
        let (x, y) = view_at(layout::SNAP_W, layout::SN_SHOT);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Dismiss));
        assert_eq!(i.state(), IslandState::DjNotice);
        i.set_dj(DJ_PROGRESS, &t);
        assert_eq!(i.state(), IslandState::DjProgress);
        // A new thumb takes over a visible pill and hands it back.
        i.show_snap(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::DjProgress);
        // If the facts went while the thumb was up there is nothing to give back.
        i.show_snap(&t);
        i.set_dj(DjUi::default(), &t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn the_alert_and_the_dj_notice_hand_the_pill_to_each_other() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_timer(DONE, &t);
        i.set_dj(DJ_NOTICE, &t);
        // The notice outranks the alert; the alert's buttons are not reachable under it.
        assert_eq!(i.state(), IslandState::DjNotice);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::ALERT_W, layout::AL_STOP);
        let out = i.click(x, y, &t);
        assert!(!matches!(out, ClickOutcome::Timer(_)), "{out:?}");
        let (cx, cy) = center(&i);
        assert_eq!(i.click(cx, cy, &t), ClickOutcome::DjDismiss);
        // The app dismisses it and the alert is back with its buttons.
        i.set_dj(DjUi::default(), &t);
        assert_eq!(i.state(), IslandState::TimerAlert);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::ALERT_W, layout::AL_STOP);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Timer(TimerAction::Stop));
        // A progress under the alert waits: the alert's answer shows it.
        i.set_dj(DJ_PROGRESS, &t);
        assert_eq!(i.state(), IslandState::TimerAlert);
        i.set_timer(TimerUi::default(), &t);
        assert_eq!(i.state(), IslandState::DjProgress);
    }
}
