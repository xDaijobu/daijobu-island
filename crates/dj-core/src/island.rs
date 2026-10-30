//! The island state machine: Idle pill, Home grid, the Music views, the Vitals panel with
//! its Sweep view, the Timer views, Settings, Tile, Terminal, Clipboard, the snap thumb
//! and the OSD overlay, driven by springs.
//! Time only enters through `step(dt)`, so everything here is deterministic. The timer
//! itself (`timer::Pomodoro`) lives in the app; the island only hears whether it is active
//! or done. Likewise the snap thumb's countdown and its shot live in the app; the island
//! only hears whether a thumb is up.

use crate::features::{Availability, FEATURES};
use crate::layout::{self, Rect};
use crate::osd::{Osd, OsdView, Volume};
use crate::settings::{self, SettingsAction, SettingsHit};
use crate::spring::{Spring, SpringSpec};
use crate::term_tabs::TabId;
use crate::terminal_shortcuts;
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
    /// The Terminal panel: a shell in the island, the largest state. Like the Tile panel,
    /// every click inside it is the app's to read (`ClickOutcome::Terminal`); the app
    /// closes it (the hotkey, the ✕), and the island's `collapse` returns to the rest state.
    Terminal,
    /// Clipboard history. The app owns its controls; background activity waits below it.
    Clipboard,
    Notifications,
    NotificationToast,
}

/// Number of content layers (one per `IslandState`).
const LAYERS: usize = 16;

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
            IslandState::Terminal => 12,
            IslandState::Clipboard => 13,
            IslandState::Notifications => 14,
            IslandState::NotificationToast => 15,
        }
    }

    /// The states the island rests in. Facts that change the rest state (a track, the
    /// timer) move the island only from one of these; an open panel stays put.
    fn is_rest(self) -> bool {
        matches!(
            self,
            IslandState::Idle
                | IslandState::MusicCompact
                | IslandState::TimerCompact
                | IslandState::TimerAlert
                | IslandState::SnapThumb
                | IslandState::NotificationToast
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
    /// The gear on Home: the island collapsed, and the app opens the Settings window.
    OpenSettings,
    /// A running terminal tab beside the idle clock; select it without spawning a tab.
    TerminalShortcut(TabId),
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
    /// Any click inside the Terminal panel, in panel-local px (as for `Tile`). The app
    /// decides: the ✕ (`layout::TERM_CLOSE`) closes it, the text area starts a selection,
    /// and the island does nothing else.
    Terminal {
        x: f32,
        y: f32,
    },
    /// A click in Clipboard panel-local pixels, anchored while the pill morphs.
    Clipboard {
        x: f32,
        y: f32,
    },
    Notifications {
        x: f32,
        y: f32,
        toast: bool,
    },
}

/// What a click on the snap thumb asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapAction {
    /// Copy the shot to the clipboard again.
    Copy,
    /// Write it to the save folder.
    Save,
    /// Open an independent reference without copying or saving.
    PinScreen,
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
    pub terminal: f32,
    pub clipboard: f32,
    pub notifications: f32,
    pub notification_toast: f32,
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
    pub terminal_shortcut_hover: Option<TabId>,
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
    /// A snap thumb is up (until the app's countdown ends or it is dismissed).
    snap: bool,
    notification_toast: bool,
    /// The pointer is over the snap thumb, and the button under it.
    snap_pointer: bool,
    snap_hover: Option<SnapAction>,
    gear_hover: bool,
    osd: Osd,
    /// True while the user drags the OSD bar.
    dragging: bool,
    /// True when the pill is growing (picks the expand or collapse spring).
    expanding: bool,
    /// The resting (Idle) size `(w, h)` in logical px; radius is `h / 2`.
    idle: (f32, f32),
    terminal_shortcuts: Vec<TabId>,
    terminal_shortcut_hover: Option<TabId>,
    /// Latched while the island is out of a rest state (or showing the OSD) and until its
    /// pill springs have come back to rest; see `wants_front`.
    front: bool,
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
            snap: false,
            notification_toast: false,
            snap_pointer: false,
            snap_hover: None,
            gear_hover: false,
            osd: Osd::new(),
            dragging: false,
            expanding: true,
            idle: (layout::IDLE_W, layout::IDLE_H),
            terminal_shortcuts: Vec::new(),
            terminal_shortcut_hover: None,
            front: false,
        }
    }

    /// Whether the island should be above other windows (the "stay behind" setting sinks
    /// it to the desktop layer only while this is false): true while the state is not a
    /// rest state, while the OSD shows, and while the pill springs are still collapsing
    /// back to a rest size after leaving one of those, so it doesn't sink mid-collapse.
    pub fn wants_front(&self) -> bool {
        !self.state.is_rest()
            || self.state == IslandState::NotificationToast
            || self.osd.is_visible()
            || self.front
    }

    /// Sets the resting (Idle) size in logical px (the app passes
    /// `config.island.notch_size()`); the radius is half the height. When Idle is the
    /// current state the pill animates to it, otherwise it applies on the next collapse.
    pub fn set_idle_size(&mut self, w: f32, h: f32) {
        let old_r = self.idle.1 / 2.0;
        self.idle = (w, h);
        let w = terminal_shortcuts::idle_width(w, self.terminal_shortcuts.len());
        if self.state == IslandState::Idle && !self.osd.is_visible() {
            self.expanding = w * h > self.w.target * self.h.target;
            self.w.target = w;
            self.h.target = h;
            // The theme's radius scale is re-applied on the next step.
            self.r.target *= (h / 2.0) / old_r;
        }
    }

    pub fn state(&self) -> IslandState {
        self.state
    }

    /// Session order changed. Only Idle widens; other content retains its own geometry.
    pub fn set_terminal_shortcuts(&mut self, mut ids: Vec<TabId>, tokens: &ResolvedTokens) -> bool {
        ids.truncate(crate::term_tabs::MAX_TABS);
        if ids == self.terminal_shortcuts {
            return false;
        }
        self.terminal_shortcuts = ids;
        self.terminal_shortcut_hover = None;
        if self.state == IslandState::Idle {
            self.sync_pill(tokens);
        }
        true
    }

    fn terminal_shortcut_at(&self, x: f32, y: f32) -> Option<TabId> {
        if self.state != IslandState::Idle
            || self.layers[IslandState::Idle.layer()].value < 0.95
            || self.osd.is_visible()
        {
            return None;
        }
        let index = terminal_shortcuts::hit(self.pill_rect(), self.terminal_shortcuts.len(), x, y)?;
        self.terminal_shortcuts.get(index).copied()
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
            IslandState::Terminal => (layout::TERM_W, layout::TERM_H, layout::TERM_R),
            IslandState::Notifications => (
                crate::notifications::PANEL_W,
                crate::notifications::PANEL_H,
                32.0,
            ),
            IslandState::NotificationToast => (
                crate::notifications::TOAST_W,
                crate::notifications::TOAST_H,
                28.0,
            ),
            IslandState::Clipboard => (
                layout::CLIPBOARD_W,
                layout::CLIPBOARD_H,
                layout::CLIPBOARD_R,
            ),
        }
    }

    /// The state the island rests in when nothing is being opened, by activity
    /// priority (plan.html): snap thumb > timer done > media > timer running > idle.
    /// The OSD sits above all of these but only covers the state (see `target_size`).
    fn rest_state(&self) -> IslandState {
        if self.snap {
            IslandState::SnapThumb
        } else if self.timer.done {
            IslandState::TimerAlert
        } else if self.notification_toast {
            IslandState::NotificationToast
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

    /// Open panels and higher-priority alerts keep notifications queued.
    pub fn can_present_notification(&self) -> bool {
        self.state.is_rest() && !self.snap && !self.timer.done && !self.osd.is_visible()
    }

    pub fn set_notification_toast(&mut self, shown: bool, tokens: &ResolvedTokens) {
        self.notification_toast = shown;
        if shown {
            self.front = true;
        }
        self.settle_rest(tokens);
    }

    pub fn open_notifications(&mut self, tokens: &ResolvedTokens) {
        self.set_state(IslandState::Notifications, tokens);
    }

    pub fn timer_ui(&self) -> TimerUi {
        self.timer
    }

    /// Pushes new timer facts. A finished session shows the alert over the rest states;
    /// over an open panel (Home, Music, Vitals, Sweep, Timer, Tile, Terminal, Clipboard)
    /// it waits for the collapse.
    pub fn set_timer(&mut self, ui: TimerUi, tokens: &ResolvedTokens) {
        if ui == self.timer {
            return;
        }
        self.timer = ui;
        self.settle_rest(tokens);
    }

    /// A fresh shot is available. Open panels stay open; the thumb waits below them
    /// and appears on collapse, or expires while the panel remains open.
    pub fn show_snap(&mut self, tokens: &ResolvedTokens) {
        self.snap = true;
        self.settle_rest(tokens);
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
    /// returns to the rest state below it. A new snap thumb waits behind the panel.
    pub fn open_tile(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::TilePanel {
            self.set_state(IslandState::TilePanel, tokens);
        }
    }

    /// The Tile panel is the island's state (it may still be growing, or covered by the OSD).
    pub fn tile_open(&self) -> bool {
        self.state == IslandState::TilePanel
    }

    /// Opens the Terminal panel from any state: the user just asked for it (the hotkey or
    /// the Home tile), so it takes over an open panel too. Like the other panels it is not
    /// a rest state, so `collapse` returns to the rest state below it, and facts that
    /// arrive meanwhile wait for the collapse.
    pub fn open_terminal(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Terminal {
            self.set_state(IslandState::Terminal, tokens);
        }
    }

    /// The Terminal panel is the island's state (it may still be growing, or covered by
    /// the OSD).
    pub fn terminal_open(&self) -> bool {
        self.state == IslandState::Terminal
    }

    /// Opens history without disturbing the Snap, timer or music facts below it.
    pub fn show_clipboard(&mut self, tokens: &ResolvedTokens) {
        if self.state != IslandState::Clipboard {
            self.set_state(IslandState::Clipboard, tokens);
        }
    }

    /// The pill's resting size: the OSD size while it is up, else the state's size.
    /// The OSD only covers the state; it never changes it, so the state returns after.
    fn target_size(&self) -> (f32, f32, f32) {
        if self.osd.is_visible() {
            (layout::OSD_W, layout::OSD_H, layout::OSD_R)
        } else {
            self.state_size(self.state)
        }
    }

    /// `base_size`, with the configured idle size.
    fn state_size(&self, state: IslandState) -> (f32, f32, f32) {
        match state {
            IslandState::Idle => (
                terminal_shortcuts::idle_width(self.idle.0, self.terminal_shortcuts.len()),
                self.idle.1,
                self.idle.1 / 2.0,
            ),
            _ => Self::base_size(state),
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
        if !self.state.is_rest() {
            self.front = true;
        }
        self.state = state;
        for (i, layer) in self.layers.iter_mut().enumerate() {
            layer.retarget(if i == state.layer() { 1.0 } else { 0.0 }, FADE_SPEC);
        }
        self.sync_pill(tokens);
        self.hover = None;
        self.snap_pointer = false;
        self.gear_hover = false;
        self.snap_hover = None;
        self.terminal_shortcut_hover = None;
    }

    /// Shows the volume pill (or refreshes its value and hold timer).
    pub fn show_osd(&mut self, volume: Volume, tokens: &ResolvedTokens) {
        let was_visible = self.osd.is_visible();
        self.osd.show(volume);
        self.front = true;
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

    /// Collapses to the rest state (see `rest_state`): the timer alert when a session
    /// ended, Music compact when a track is loaded, Timer compact while a timer runs,
    /// otherwise Idle.
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
        if let Some(id) = self.terminal_shortcut_at(x, y) {
            return ClickOutcome::TerminalShortcut(id);
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
            // Likewise for the Terminal: the app reads the ✕ and the text area.
            IslandState::Terminal => {
                let (lx, ly) = Self::view_local(x, y, layout::TERM_W);
                ClickOutcome::Terminal { x: lx, y: ly }
            }
            IslandState::Notifications | IslandState::NotificationToast => {
                let toast = self.state == IslandState::NotificationToast;
                let w = if toast {
                    crate::notifications::TOAST_W
                } else {
                    crate::notifications::PANEL_W
                };
                let (x, y) = Self::view_local(x, y, w);
                let h = if toast {
                    crate::notifications::TOAST_H
                } else {
                    crate::notifications::PANEL_H
                };
                if Rect::new(0.0, 0.0, w, h).contains(x, y) {
                    ClickOutcome::Notifications { x, y, toast }
                } else {
                    ClickOutcome::Ignored
                }
            }
            IslandState::Clipboard => {
                let (lx, ly) = Self::view_local(x, y, layout::CLIPBOARD_W);
                // The outgoing panel may still be wider/taller during a morph.
                if Rect::new(0.0, 0.0, layout::CLIPBOARD_W, layout::CLIPBOARD_H).contains(lx, ly) {
                    ClickOutcome::Clipboard { x: lx, y: ly }
                } else {
                    ClickOutcome::Ignored
                }
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
                self.collapse(tokens);
                ClickOutcome::OpenSettings
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
        } else if layout::SN_PIN.contains(x, y) {
            Some(SnapAction::PinScreen)
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
        let shortcut = self.terminal_shortcut_at(x, y);
        let shortcut_changed = shortcut != self.terminal_shortcut_hover;
        self.terminal_shortcut_hover = shortcut;
        changed || button_changed || gear_changed || shortcut_changed
    }

    /// The pointer left the window. True when something hovered needs a redraw.
    pub fn clear_hover(&mut self) -> bool {
        self.snap_pointer = false;
        let button = self.snap_hover.take().is_some();
        let gear = std::mem::take(&mut self.gear_hover);
        let shortcut = self.terminal_shortcut_hover.take().is_some();
        self.hover.take().is_some() || button || gear || shortcut
    }

    pub fn step(&mut self, dt: f32, tokens: &ResolvedTokens) {
        if self.osd.is_visible() {
            self.front = true;
        }
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
        if !self.state.is_rest() || self.osd.is_visible() {
            self.front = true;
        } else if self.front && self.w.settled() && self.h.settled() && self.r.settled() {
            self.front = false;
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
                terminal: v(12),
                clipboard: v(13),
                notifications: v(14),
                notification_toast: v(15),
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
            terminal_shortcut_hover: self.terminal_shortcut_hover,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeRegistry, DEFAULT_THEME};

    #[test]
    fn notifications_wait_below_panels_and_alerts_then_settle_without_animation() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.open_notifications(&t);
        i.set_notification_toast(true, &t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Notifications);
        assert!(!i.can_present_notification());
        let p = i.pill_rect();
        assert!(matches!(
            i.click(p.x + 30.0, p.y + 110.0, &t),
            ClickOutcome::Notifications { toast: false, .. }
        ));
        i.collapse(&t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::NotificationToast);
        assert_eq!((i.pill_rect().w, i.pill_rect().h), (360.0, 72.0));
        assert!(i.wants_front());
        i.show_snap(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::NotificationToast);
        i.set_timer(
            TimerUi {
                active: true,
                done: true,
            },
            &t,
        );
        assert_eq!(i.state(), IslandState::TimerAlert);
        i.set_notification_toast(false, &t);
        i.set_timer(TimerUi::default(), &t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Idle);
        assert!(!i.is_animating());
    }

    #[test]
    fn notifications_tile_routes_open_without_locked_feedback() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        settle(&mut i, &t);
        let index = FEATURES
            .iter()
            .position(|f| f.id == "notifications")
            .unwrap();
        let tile = layout::home_tiles()[index];
        let p = i.pill_rect();
        assert_eq!(
            i.click(p.x + tile.x + 10.0, p.y + tile.y + 10.0, &t),
            ClickOutcome::Open(index)
        );
        assert!(i.view().hint.is_none());
    }

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
    fn running_terminal_shortcuts_resize_idle_and_restore_the_configured_size() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_idle_size(96.0, 24.0);
        assert!(i.set_terminal_shortcuts((1..=12).collect(), &t));
        settle(&mut i, &t);
        assert_eq!(i.pill_rect().w, terminal_shortcuts::idle_width(96.0, 12));
        assert_eq!(i.pill_rect().h, 24.0);
        assert!(!i.set_terminal_shortcuts((1..=12).collect(), &t));
        assert!(
            !i.is_animating(),
            "unchanged sessions do not create idle frames"
        );
        i.set_idle_size(240.0, 44.0);
        settle(&mut i, &t);
        assert_eq!(i.pill_rect().w, terminal_shortcuts::idle_width(240.0, 12));
        i.set_terminal_shortcuts(vec![], &t);
        settle(&mut i, &t);
        assert_eq!((i.pill_rect().w, i.pill_rect().h), (240.0, 44.0));
    }

    #[test]
    fn shortcut_hits_follow_ids_and_leave_the_clock_as_the_home_toggle() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_terminal_shortcuts(vec![17, 29], &t);
        settle(&mut i, &t);
        let slot = terminal_shortcuts::buttons(i.pill_rect(), 2)[1];
        let (x, y) = (slot.x + slot.w / 2.0, slot.y + slot.h / 2.0);
        assert!(i.hover(x, y));
        assert_eq!(i.view().terminal_shortcut_hover, Some(29));
        assert_eq!(i.click(x, y, &t), ClickOutcome::TerminalShortcut(29));
        i.set_terminal_shortcuts(vec![29, 17], &t);
        assert_eq!(i.view().terminal_shortcut_hover, None);
        assert_eq!(i.click(x, y, &t), ClickOutcome::TerminalShortcut(17));
        i.hover(x, y);
        assert!(i.clear_hover());
        let clock = terminal_shortcuts::clock_rect(i.pill_rect(), 2);
        assert_eq!(
            i.click(clock.x + clock.w / 2.0, y, &t),
            ClickOutcome::Expanded
        );
        assert_eq!(i.terminal_shortcut_at(x, y), None);
    }

    #[test]
    fn shortcuts_do_not_steal_osd_or_fading_content_clicks() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_terminal_shortcuts(vec![11], &t);
        settle(&mut i, &t);
        i.open_terminal(&t);
        settle(&mut i, &t);
        i.collapse(&t);
        let slot = terminal_shortcuts::buttons(i.pill_rect(), 1)[0];
        assert_eq!(i.terminal_shortcut_at(slot.x + 1.0, slot.y + 1.0), None);
        settle(&mut i, &t);
        let slot = terminal_shortcuts::buttons(i.pill_rect(), 1)[0];
        assert_eq!(i.terminal_shortcut_at(slot.x + 1.0, slot.y + 1.0), Some(11));
        i.show_osd(vol(0.5), &t);
        assert_eq!(i.terminal_shortcut_at(slot.x + 1.0, slot.y + 1.0), None);
        i.set_terminal_shortcuts(vec![], &t);
        settle_pinned(&mut i, &t);
        assert_eq!(i.pill_rect().w, layout::OSD_W);
    }

    #[test]
    fn clipboard_clicks_stay_panel_local_and_ignore_outgoing_content_during_morph() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.open_terminal(&t);
        settle(&mut i, &t);
        i.show_clipboard(&t);
        let (ww, _) = layout::window_size();
        let point = |x, y| ((ww - layout::CLIPBOARD_W) / 2.0 + x, layout::TOP_MARGIN + y);
        let (x, y) = point(12.0, 25.0);
        assert_eq!(
            i.click(x, y, &t),
            ClickOutcome::Clipboard { x: 12.0, y: 25.0 }
        );
        // Still in the old Terminal pill, but outside Clipboard's intended content.
        for (lx, ly) in [(-1.0, 20.0), (400.0, 20.0), (20.0, 360.0)] {
            let (x, y) = point(lx, ly);
            assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored);
        }
        settle(&mut i, &t);
        for (lx, ly) in [(0.0, 0.0), (399.0, 359.0), (200.0, 180.0)] {
            let (x, y) = point(lx, ly);
            assert_eq!(i.click(x, y, &t), ClickOutcome::Clipboard { x: lx, y: ly });
            assert_eq!(i.state(), IslandState::Clipboard);
        }
        assert_eq!(size(&i), (400.0, 360.0));
        assert!(i.wants_front());
        i.show_clipboard(&t);
        assert!(
            !i.is_animating(),
            "reopening a settled panel does not redraw"
        );
    }

    #[test]
    fn clipboard_keeps_rest_facts_and_snap_until_collapse_and_then_settles() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_clipboard(&t);
        i.set_music(with_track(), &t);
        i.set_timer(DONE, &t);
        i.show_snap(&t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Clipboard);
        assert!(!IslandState::Clipboard.is_rest());
        assert_eq!(i.view().layers.clipboard, 1.0);
        assert_eq!((i.view().layers.snap, i.view().layers.alert), (0.0, 0.0));
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        assert!(i.wants_front(), "stay above windows throughout collapse");
        settle(&mut i, &t);
        assert!(!i.wants_front());
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::TimerAlert);
        i.show_clipboard(&t);
        i.set_timer(TimerUi::default(), &t);
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::MusicCompact);
        settle(&mut i, &t);
        assert_eq!(i.view().layers.clipboard, 0.0);
    }

    #[test]
    fn osd_owns_clipboard_input_then_returns_to_the_panel() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_clipboard(&t);
        settle(&mut i, &t);
        i.show_osd(vol(0.5), &t);
        settle_pinned(&mut i, &t);
        assert_eq!(i.state(), IslandState::Clipboard);
        let r = layout::osd_rect();
        let bar = layout::OSD_BAR;
        assert_eq!(i.click(r.x + 2.0, r.y + 2.0, &t), ClickOutcome::Ignored);
        assert_eq!(
            i.click(r.x + bar.x + bar.w / 2.0, r.y + bar.y + bar.h / 2.0, &t),
            ClickOutcome::SetVolume(0.5)
        );
        i.release();
        settle(&mut i, &t);
        assert_eq!(size(&i), (400.0, 360.0));
        assert_eq!(i.view().osd.alpha, 0.0);
        assert_eq!(i.view().layers.clipboard, 1.0);
        let (x, y) = center(&i);
        assert!(matches!(i.click(x, y, &t), ClickOutcome::Clipboard { .. }));
    }

    #[test]
    fn clipboard_clears_shortcut_hover_and_yields_to_terminal() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_terminal_shortcuts(vec![7], &t);
        settle(&mut i, &t);
        let slot = terminal_shortcuts::buttons(i.pill_rect(), 1)[0];
        i.hover(slot.x + 1.0, slot.y + 1.0);
        assert_eq!(i.view().terminal_shortcut_hover, Some(7));
        i.show_clipboard(&t);
        assert_eq!(i.view().terminal_shortcut_hover, None);
        settle(&mut i, &t);
        let (x, y) = center(&i);
        assert!(!i.hover(x, y));
        assert!(!i.snap_hovered());
        assert!(!i.clear_hover());
        i.open_terminal(&t);
        settle(&mut i, &t);
        assert_eq!(
            (i.view().layers.terminal, i.view().layers.clipboard),
            (1.0, 0.0)
        );
        i.show_clipboard(&t);
        settle(&mut i, &t);
        assert_eq!(
            (i.view().layers.terminal, i.view().layers.clipboard),
            (0.0, 1.0)
        );
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
        let index = FEATURES
            .iter()
            .position(|f| f.availability == Availability::Locked)
            .unwrap();
        let tile = layout::home_tiles()[index];
        let out = i.click(p.x + tile.x + 5.0, p.y + tile.y + 5.0, &t);
        assert_eq!(out, ClickOutcome::Locked(index));
        assert_eq!(i.state(), IslandState::Home);
        assert!(i.is_animating());
        i.step(0.05, &t);
        let v = i.view();
        assert_eq!(v.shake.map(|s| s.0), Some(index));
        assert_eq!(v.hint.map(|h| h.0), Some(index));
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
        let panels: [(IslandState, Open); 7] = [
            (IslandState::Home, Island::expand),
            (IslandState::MusicExpanded, Island::open_music),
            (IslandState::Vitals, Island::open_vitals),
            (IslandState::Sweep, Island::open_sweep),
            (IslandState::TimerPanel, Island::open_timer),
            (IslandState::TilePanel, Island::open_tile),
            (IslandState::Terminal, Island::open_terminal),
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
    fn the_thumb_waits_behind_an_open_panel_and_can_expire_there() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_timer(RUNNING, &t);
        i.open_vitals(&t);
        i.show_snap(&t);
        assert_eq!(i.state(), IslandState::Vitals);
        i.hide_snap(&t);
        assert_eq!(i.state(), IslandState::Vitals);
        i.collapse(&t);
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
        let (x, y) = view_at(w, layout::SN_PIN);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::PinScreen));
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
        // The gear opens the Settings window: the island collapses.
        assert_eq!(i.click(x, y, &t), ClickOutcome::OpenSettings);
        assert_eq!(i.state(), IslandState::Idle);
        // The in-island panel is still reachable directly (kept for now).
        i.open_settings(&t);
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
        let states: [(IslandState, Setup); LAYERS] = [
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
            (IslandState::Terminal, Island::open_terminal),
            (IslandState::Clipboard, Island::show_clipboard),
            (IslandState::Notifications, Island::open_notifications),
            (IslandState::NotificationToast, |i, t| {
                i.set_notification_toast(true, t)
            }),
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
                l.terminal,
                l.clipboard,
                l.notifications,
                l.notification_toast,
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
    fn the_snap_thumb_waits_for_the_tile_panel_to_collapse() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_tile(&t);
        settle(&mut i, &t);
        i.show_snap(&t);
        assert!(i.tile_open());
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::TILE_PANEL_W, layout::TILE_PANEL_H));
        let l = i.view().layers;
        assert_eq!((l.tile, l.snap), (1.0, 0.0));
        let (x, y) = panel_pt(200.0, 100.0);
        assert!(matches!(i.click(x, y, &t), ClickOutcome::Tile { .. }));
        i.collapse(&t);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::SNAP_W, layout::SN_COPY);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Copy));
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

    // ---- Terminal ----

    fn terminal_index() -> usize {
        FEATURES.iter().position(|f| f.id == "terminal").unwrap()
    }

    /// Window-logical point for panel-local `(x, y)`: the panel hangs centre-top.
    fn term_pt(x: f32, y: f32) -> (f32, f32) {
        let (win_w, _) = layout::window_size();
        ((win_w - layout::TERM_W) / 2.0 + x, layout::TOP_MARGIN + y)
    }

    fn open_settled_terminal(t: &ResolvedTokens) -> Island {
        let mut i = Island::new(t);
        i.open_terminal(t);
        settle(&mut i, t);
        i
    }

    #[test]
    fn the_terminal_opens_from_idle_at_its_size_with_its_own_layer() {
        let t = tokens();
        let mut i = Island::new(&t);
        assert!(!i.terminal_open());
        i.open_terminal(&t);
        assert_eq!(i.state(), IslandState::Terminal);
        assert!(i.terminal_open());
        assert!(i.is_animating());
        // Mid-flight both contents are partly visible: the cross-fade.
        for _ in 0..6 {
            i.step(1.0 / 144.0, &t);
        }
        let l = i.view().layers;
        assert!(
            l.terminal > 0.0 && l.terminal < 1.0,
            "terminal {}",
            l.terminal
        );
        assert!(l.idle > 0.0 && l.idle < 1.0, "idle {}", l.idle);
        settle(&mut i, &t);
        assert_eq!(size(&i), (760.0, 420.0));
        let v = i.view();
        assert_eq!(v.radius, 22.0 * t.radius_scale);
        assert_eq!((v.layers.idle, v.layers.terminal), (0.0, 1.0));
        // Its own layer, after the Tile panel's, and the springs are sized for it.
        assert_eq!(IslandState::Terminal.layer(), 12);
        assert_eq!(LAYERS, 16);
        assert_eq!(
            Island::base_size(IslandState::Terminal),
            (layout::TERM_W, layout::TERM_H, layout::TERM_R)
        );
        // It is centred in the window, which it fills but for the notch shoulders.
        let (win_w, _) = layout::window_size();
        assert_eq!(i.pill_rect().x, (win_w - layout::TERM_W) / 2.0);
        assert_eq!(i.pill_rect().x, layout::NOTCH_SHOULDER);
        assert_eq!(i.pill_rect().y, layout::TOP_MARGIN);
    }

    #[test]
    fn the_terminal_is_not_a_rest_state() {
        assert!(!IslandState::Terminal.is_rest());
        let t = tokens();
        let mut i = open_settled_terminal(&t);
        // A rest-state change (a track, a timer) does not move an open terminal.
        i.set_music(with_track(), &t);
        i.set_timer(RUNNING, &t);
        assert_eq!(i.state(), IslandState::Terminal);
        assert!(i.terminal_open());
    }

    #[test]
    fn the_terminal_opens_from_every_rest_state_and_collapse_returns_to_it() {
        let t = tokens();
        type Setup = fn(&mut Island, &ResolvedTokens);
        let cases: [(Setup, IslandState); 6] = [
            (|_, _| {}, IslandState::Idle),
            (
                |i, t| i.set_music(with_track(), t),
                IslandState::MusicCompact,
            ),
            (|i, t| i.set_timer(RUNNING, t), IslandState::TimerCompact),
            (|i, t| i.set_timer(DONE, t), IslandState::TimerAlert),
            (|i, t| i.show_snap(t), IslandState::SnapThumb),
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
            i.open_terminal(&t);
            settle(&mut i, &t);
            assert_eq!(i.state(), IslandState::Terminal, "from {rest:?}");
            assert_eq!(size(&i), (layout::TERM_W, layout::TERM_H));
            assert_eq!(i.view().layers.terminal, 1.0);
            i.collapse(&t);
            assert_eq!(i.state(), rest, "back to {rest:?}");
            settle(&mut i, &t);
            let (w, h, _) = Island::base_size(rest);
            assert_eq!(size(&i), (w, h), "{rest:?}");
            assert_eq!(i.view().layers.terminal, 0.0);
            assert!(!i.terminal_open());
        }
    }

    #[test]
    fn the_terminal_takes_over_every_open_panel() {
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
            let mut i = Island::new(&t);
            open(&mut i, &t);
            settle(&mut i, &t);
            assert_eq!(i.state(), state);
            i.open_terminal(&t);
            assert_eq!(i.state(), IslandState::Terminal, "over {state:?}");
            settle(&mut i, &t);
            assert_eq!(size(&i), (layout::TERM_W, layout::TERM_H), "{state:?}");
            let l = i.view().layers;
            assert_eq!(
                (l.terminal, l.home, l.tile, l.settings),
                (1.0, 0.0, 0.0, 0.0)
            );
            // Not back to the panel it covered: the terminal is not a step down from it.
            i.collapse(&t);
            assert_eq!(i.state(), IslandState::Idle, "after {state:?}");
        }
    }

    #[test]
    fn the_terminal_goes_back_to_the_panels_after_it_without_leaving_a_layer_lit() {
        let t = tokens();
        let mut i = open_settled_terminal(&t);
        i.open_tile(&t);
        settle(&mut i, &t);
        let l = i.view().layers;
        assert_eq!((l.terminal, l.tile), (0.0, 1.0));
        assert!(!i.terminal_open() && i.tile_open());
        i.open_terminal(&t);
        settle(&mut i, &t);
        let l = i.view().layers;
        assert_eq!((l.terminal, l.tile), (1.0, 0.0));
        i.expand(&t);
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::HOME_W, layout::HOME_H));
        assert_eq!(i.view().layers.terminal, 0.0);
    }

    #[test]
    fn opening_the_terminal_again_changes_nothing() {
        let t = tokens();
        let mut i = open_settled_terminal(&t);
        i.open_terminal(&t);
        assert!(!i.is_animating());
        assert_eq!(i.state(), IslandState::Terminal);
        // Also while it is still growing: no restart of the springs.
        let mut i = Island::new(&t);
        i.open_terminal(&t);
        for _ in 0..4 {
            i.step(1.0 / 144.0, &t);
        }
        let before = i.pill_rect();
        i.open_terminal(&t);
        assert_eq!(i.pill_rect(), before);
    }

    #[test]
    fn facts_arriving_under_the_terminal_wait_for_the_collapse() {
        let t = tokens();
        let mut i = open_settled_terminal(&t);
        i.set_music(with_track(), &t);
        i.set_timer(RUNNING, &t);
        assert_eq!(i.state(), IslandState::Terminal);
        i.set_timer(DONE, &t);
        assert!(i.terminal_open());
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::TERM_W, layout::TERM_H));
        assert_eq!(i.view().layers.alert, 0.0);
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::TimerAlert);
        assert!(!i.terminal_open());
        // And a fact that goes away while it is open is gone when it closes.
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_terminal(&t);
        i.set_music(MusicUi::default(), &t);
        assert_eq!(i.state(), IslandState::Terminal);
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn terminal_clicks_report_panel_local_coordinates_and_leave_the_panel_open() {
        let t = tokens();
        let mut i = open_settled_terminal(&t);
        let (w, h) = (layout::TERM_W, layout::TERM_H);
        for (lx, ly) in [
            (0.0, 0.0),
            (w - 1.0, 0.0),
            (0.0, h - 1.0),
            (w - 1.0, h - 1.0),
            (w / 2.0, h / 2.0),
        ] {
            let (x, y) = term_pt(lx, ly);
            assert_eq!(
                i.click(x, y, &t),
                ClickOutcome::Terminal { x: lx, y: ly },
                "{lx},{ly}"
            );
            // Nothing else happens: closing and selecting are the app's call.
            assert_eq!(i.state(), IslandState::Terminal);
            assert!(!i.is_animating());
        }
        // The close button and the text area come out where the layout puts them.
        let c = layout::TERM_CLOSE;
        let (x, y) = view_at(layout::TERM_W, c);
        assert_eq!(
            i.click(x, y, &t),
            ClickOutcome::Terminal {
                x: c.x + c.w / 2.0,
                y: c.y + c.h / 2.0
            }
        );
        let tx = layout::TERM_TEXT;
        let (x, y) = view_at(layout::TERM_W, tx);
        let out = i.click(x, y, &t);
        assert_eq!(
            out,
            ClickOutcome::Terminal {
                x: tx.x + tx.w / 2.0,
                y: tx.y + tx.h / 2.0
            }
        );
        // Just outside the pill is not the panel's.
        for (lx, ly) in [(-1.0, 10.0), (w, 10.0), (10.0, -1.0), (10.0, h)] {
            let (x, y) = term_pt(lx, ly);
            assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored, "{lx},{ly}");
        }
        assert_eq!(i.state(), IslandState::Terminal);
    }

    #[test]
    fn terminal_clicks_are_panel_local_even_mid_morph() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        i.open_terminal(&t);
        // Not settled: the pill is still growing from Home, but the content is anchored
        // centre-top.
        i.step(1.0 / 144.0, &t);
        assert!(i.is_animating());
        assert!(i.pill_rect().w < layout::TERM_W);
        let (x, y) = term_pt(200.0, 100.0);
        assert!(i.pill_rect().contains(x, y));
        assert_eq!(
            i.click(x, y, &t),
            ClickOutcome::Terminal { x: 200.0, y: 100.0 }
        );
    }

    #[test]
    fn the_terminal_panel_has_no_home_hover_or_tile_hits() {
        let t = tokens();
        let mut i = open_settled_terminal(&t);
        // Where a Home tile would be: no hover, and a click is the panel's, not a tile's.
        let tile = layout::home_tiles()[terminal_index()];
        let p = i.pill_rect();
        let (x, y) = (p.x + tile.x + 5.0, p.y + tile.y + 5.0);
        assert!(!i.hover(x, y));
        assert_eq!(i.view().hover, None);
        assert!(matches!(i.click(x, y, &t), ClickOutcome::Terminal { .. }));
        // And the gear's spot in the Home header is not a Settings click.
        let (x, y) = view_at(layout::HOME_W, layout::HOME_GEAR);
        assert!(matches!(i.click(x, y, &t), ClickOutcome::Terminal { .. }));
        assert_eq!(i.state(), IslandState::Terminal);
    }

    #[test]
    fn the_osd_outranks_the_terminal_and_the_panel_returns() {
        let t = tokens();
        let mut i = open_settled_terminal(&t);
        i.show_osd(vol(0.5), &t);
        settle_pinned(&mut i, &t);
        // The OSD covers the pill; the state stays put.
        assert_eq!(i.state(), IslandState::Terminal);
        assert_eq!(size(&i), (layout::OSD_W, layout::OSD_H));
        // Where the panel's content would be: not a Terminal click.
        let (x, y) = term_pt(380.0, 200.0);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Ignored);
        let r = layout::osd_rect();
        let bar = layout::OSD_BAR;
        let out = i.click(r.x + bar.x + bar.w / 2.0, r.y + bar.y + bar.h / 2.0, &t);
        assert_eq!(out, ClickOutcome::SetVolume(0.5));
        i.release();
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::Terminal);
        assert_eq!(size(&i), (layout::TERM_W, layout::TERM_H));
        assert_eq!(i.view().layers.terminal, 1.0);
        assert_eq!(i.view().osd.alpha, 0.0);
        let (x, y) = term_pt(380.0, 200.0);
        assert!(matches!(i.click(x, y, &t), ClickOutcome::Terminal { .. }));
    }

    #[test]
    fn the_snap_thumb_waits_for_the_terminal_to_close() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(with_track(), &t);
        i.open_terminal(&t);
        settle(&mut i, &t);
        i.show_snap(&t);
        assert!(i.terminal_open());
        settle(&mut i, &t);
        assert_eq!(size(&i), (layout::TERM_W, layout::TERM_H));
        let l = i.view().layers;
        assert_eq!((l.terminal, l.snap), (1.0, 0.0));
        let (x, y) = term_pt(380.0, 200.0);
        assert!(matches!(i.click(x, y, &t), ClickOutcome::Terminal { .. }));
        i.collapse(&t);
        settle(&mut i, &t);
        let (x, y) = view_at(layout::SNAP_W, layout::SN_SHOT);
        assert_eq!(i.click(x, y, &t), ClickOutcome::Snap(SnapAction::Dismiss));
        assert_eq!(i.state(), IslandState::MusicCompact);
    }

    #[test]
    fn a_terminal_opened_over_the_thumb_gives_it_back_on_collapse() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_snap(&t);
        settle(&mut i, &t);
        i.open_terminal(&t);
        settle(&mut i, &t);
        assert!(i.terminal_open());
        // The thumb's countdown is the app's and keeps running under the panel.
        assert!(i.snap_shown());
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        // If the countdown ends while the panel is open there is nothing to give back.
        i.open_terminal(&t);
        i.hide_snap(&t);
        assert!(i.terminal_open());
        i.collapse(&t);
        assert_eq!(i.state(), IslandState::Idle);
    }

    #[test]
    fn the_home_terminal_tile_is_available_and_reports_open_without_shaking() {
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        let tile = layout::home_tiles()[terminal_index()];
        let p = i.pill_rect();
        let (x, y) = (p.x + tile.x + 5.0, p.y + tile.y + 5.0);
        assert!(i.hover(x, y));
        assert_eq!(i.view().hover, Some(terminal_index()));
        assert_eq!(i.click(x, y, &t), ClickOutcome::Open(terminal_index()));
        // The app decides what opening means; the island only reports the click.
        assert_eq!(i.state(), IslandState::Home);
        assert!(i.view().shake.is_none() && i.view().hint.is_none());
        i.open_terminal(&t);
        settle(&mut i, &t);
        assert_eq!(i.view().layers.terminal, 1.0);
    }

    #[test]
    fn home_keeps_its_place_in_the_wider_window() {
        // The window is as wide as the Terminal needs, so Home is centred with room to
        // spare; its content is anchored to the pill, and the click maths agrees.
        let t = tokens();
        let mut i = Island::new(&t);
        open_home(&mut i, &t);
        let (win_w, _) = layout::window_size();
        assert_eq!(i.pill_rect().x, (win_w - layout::HOME_W) / 2.0);
        let tile = layout::home_tiles()[0];
        let (x, y) = view_at(layout::HOME_W, tile);
        let p = i.pill_rect();
        assert_eq!(
            (x, y),
            (p.x + tile.x + tile.w / 2.0, p.y + tile.y + tile.h / 2.0)
        );
        assert_eq!(i.click(x, y, &t), ClickOutcome::Open(0));
        // The gear lines up the same way.
        let (x, y) = view_at(layout::HOME_W, layout::HOME_GEAR);
        assert!(i.hover(x, y));
        assert!(i.view().gear_hover);
    }
}

#[cfg(test)]
mod front_and_size_tests {
    use super::*;
    use crate::theme::{ThemeRegistry, DEFAULT_THEME};

    fn tokens() -> ResolvedTokens {
        ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone()
    }

    fn settle(i: &mut Island, t: &ResolvedTokens) {
        for _ in 0..2000 {
            i.step(1.0 / 144.0, t);
        }
        assert!(!i.is_animating());
    }

    fn size(i: &Island) -> (f32, f32) {
        let r = i.pill_rect();
        (r.w, r.h)
    }

    #[test]
    fn a_fresh_island_rests_behind() {
        let t = tokens();
        let i = Island::new(&t);
        assert!(!i.wants_front());
    }

    #[test]
    fn wants_front_follows_the_state() {
        let t = tokens();
        type Open = fn(&mut Island, &ResolvedTokens);
        let open: [(&str, Open); 8] = [
            ("home", |i, t| i.expand(t)),
            ("music", |i, t| i.open_music(t)),
            ("vitals", |i, t| i.open_vitals(t)),
            ("sweep", |i, t| i.open_sweep(t)),
            ("timer", |i, t| i.open_timer(t)),
            ("settings", |i, t| i.open_settings(t)),
            ("tile", |i, t| i.open_tile(t)),
            ("terminal", |i, t| i.open_terminal(t)),
        ];
        for (name, f) in open {
            let mut i = Island::new(&t);
            f(&mut i, &t);
            assert!(i.wants_front(), "{name} just opened");
            settle(&mut i, &t);
            assert!(i.wants_front(), "{name} open");
            i.collapse(&t);
            assert!(i.wants_front(), "{name} collapsing");
            settle(&mut i, &t);
            assert!(!i.wants_front(), "{name} back at rest");
        }
    }

    #[test]
    fn rest_states_stay_behind() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.set_music(
            MusicUi {
                signed_in: true,
                has_track: true,
                blocked: false,
            },
            &t,
        );
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::MusicCompact);
        assert!(!i.wants_front());

        let mut i = Island::new(&t);
        i.set_timer(
            TimerUi {
                active: true,
                done: false,
            },
            &t,
        );
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::TimerCompact);
        assert!(!i.wants_front());
        i.set_timer(
            TimerUi {
                active: false,
                done: true,
            },
            &t,
        );
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::TimerAlert);
        assert!(!i.wants_front());

        let mut i = Island::new(&t);
        i.show_snap(&t);
        settle(&mut i, &t);
        assert_eq!(i.state(), IslandState::SnapThumb);
        assert!(!i.wants_front());
    }

    #[test]
    fn the_osd_brings_it_forward_until_it_has_collapsed() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.show_osd(
            Volume {
                level: 0.5,
                muted: false,
            },
            &t,
        );
        assert!(i.wants_front());
        for _ in 0..30 {
            i.step(1.0 / 144.0, &t);
            assert!(i.wants_front());
        }
        // Run until the hold ends; it must stay in front until the pill is back at rest.
        let mut hidden_at = None;
        for n in 0..4000 {
            i.step(1.0 / 144.0, &t);
            if !i.wants_front() {
                hidden_at = Some(n);
                break;
            }
        }
        assert!(hidden_at.is_some());
        assert!(!i.is_animating() || size(&i).0 - layout::IDLE_W < 1.0);
        assert!((size(&i).0 - layout::IDLE_W).abs() < 1.0);
    }

    #[test]
    fn the_collapse_tail_keeps_it_in_front() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        settle(&mut i, &t);
        i.collapse(&t);
        let mut moved = 0;
        while i.is_animating() {
            if i.pill_rect().w > layout::IDLE_W + 1.0 {
                assert!(i.wants_front());
                moved += 1;
            }
            i.step(1.0 / 144.0, &t);
        }
        assert!(moved > 3);
        assert!(!i.wants_front());
    }

    #[test]
    fn idle_size_defaults_and_applies_while_idle() {
        let t = tokens();
        let mut i = Island::new(&t);
        assert_eq!(size(&i), (layout::IDLE_W, layout::IDLE_H));
        i.set_idle_size(160.0, 40.0);
        assert!(i.is_animating());
        settle(&mut i, &t);
        assert_eq!(size(&i), (160.0, 40.0));
        assert_eq!(i.view().radius, 20.0 * t.radius_scale);
        i.set_idle_size(96.0, 24.0);
        settle(&mut i, &t);
        assert_eq!(size(&i), (96.0, 24.0));
        assert_eq!(i.view().radius, 12.0 * t.radius_scale);
        i.set_idle_size(240.0, 44.0);
        settle(&mut i, &t);
        assert_eq!(size(&i), (240.0, 44.0));
        assert_eq!(i.view().radius, 22.0 * t.radius_scale);
        // The default keeps radius 16.
        i.set_idle_size(layout::IDLE_W, layout::IDLE_H);
        settle(&mut i, &t);
        assert_eq!(i.view().radius, layout::IDLE_R * t.radius_scale);
    }

    #[test]
    fn idle_size_set_while_home_applies_on_collapse() {
        let t = tokens();
        let mut i = Island::new(&t);
        i.expand(&t);
        settle(&mut i, &t);
        i.set_idle_size(200.0, 40.0);
        assert_eq!(i.state(), IslandState::Home);
        assert!(!i.is_animating());
        assert_eq!(size(&i), (layout::HOME_W, layout::HOME_H));
        i.collapse(&t);
        settle(&mut i, &t);
        assert_eq!(size(&i), (200.0, 40.0));
        assert_eq!(i.view().radius, 20.0 * t.radius_scale);
    }

    #[test]
    fn reserve_height_follows_the_idle_height() {
        assert_eq!(layout::reserve_h(layout::IDLE_H), layout::RESERVE_H);
        // Small notches still reserve room for the compact music/timer views.
        assert_eq!(layout::reserve_h(24.0), layout::RESERVE_H);
        assert_eq!(layout::reserve_h(44.0), layout::TOP_MARGIN + 44.0);
    }
}
