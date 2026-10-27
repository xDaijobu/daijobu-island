//! Application state: glues the island model, themes, the Music engine, renderer and
//! Win32 events.

use crate::chime::SystemChime;
use crate::notch::{self, Edges};
use crate::preview_store::Store;
use crate::previews::{self, Model, Previews, TIMER_SNAP_STACK};
use crate::snap::{
    self as snapper, End, RoughMeasure, Snapper, SystemClipboard, SystemScreen, HOTKEY_SNAP,
    HOTKEY_SNAP_ALT, HOTKEY_SNAP_ESC, START_DELAY_MS, TIMER_SNAP_START, TIMER_SNAP_THUMB,
    VK_ESCAPE,
};
use crate::storage;
use crate::sweep::{Job, Msg, ScanId, Sweeper};
use crate::tile::{
    IslandGeo, SystemWindows, Tiler, HOTKEY_DOWN, HOTKEY_LEFT, HOTKEY_RIGHT, HOTKEY_UP,
    HOTKEY_ZONE_1, HOTKEY_ZONE_9, POLL_MS, TIMER_TILE_NOTICE, TIMER_TILE_POLL, TIMER_TILE_RETRY,
};
use crate::timer::{Timer, TIMER_DEADLINE, TIMER_DIGITS};
use crate::vitals::{self, Cadence, Worker};
use dj_core::color::Rgb;
use dj_core::config::{AnnotateConfig, Config, IslandStyle, SnapHotkey};
use dj_core::features::FEATURES;
use dj_core::island::{ClickOutcome, Island, IslandState, MusicAction, SnapAction, TimerAction};
use dj_core::layout;
use dj_core::music::{cover_accent, MusicMsg, Session};
use dj_core::osd::Volume;
use dj_core::settings::{self, SettingsAction, SettingsView};
use dj_core::snap::{loupe_label, pixel_at, size_label, IRect, Loupe, Stamp};
use dj_core::snap_edit::{CursorKind, EditInput, Key, KeyCode, Outcome, Rgba, TextMeasure};
use dj_core::snap_stack::Stack;
use dj_core::sweep::{format_bytes, Category, Estimate, Phase, SweepCache, SweepCmd, SweepView};
use dj_core::theme::{ThemeAnim, ThemeRegistry, DEFAULT_THEME};
use dj_core::vitals::Vitals;
use dj_music::art::{self, ArtResult};
use dj_music::{Engine, MUSIC_URL};
use dj_render::{
    EditScene, Frame, MusicFrame, RenderError, Renderer, SnapFrame, SnapOverlay, SnapScene,
    TextMetrics, TilePanel, TileThumb, ZoneOverlay, ZoneScene,
};
use dj_win::overlay::{self, SnapInput};
use dj_win::tile::MoveEvent;
use dj_win::tray::{self, MenuCommand, MenuSpec, MusicMenu};
use dj_win::{appbar, hook, system, volume, window, Event, Handler, HICON, HWND};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HOTKEY_TOGGLE: i32 = 1;
const HOTKEY_THEME: i32 = 2;
const TIMER_CLOCK: usize = 1;
const TIMER_VU: usize = 2;
/// One-shot: the Sweep result has been on screen long enough, back to Vitals.
const TIMER_SWEEP_DONE: usize = 4;
const SWEEP_DONE_MS: u32 = 4_000;
/// A stale estimate is retried at most this often, so a scan that keeps failing or
/// being cancelled is never restarted in a loop.
const ESTIMATE_RETRY: Duration = Duration::from_secs(10 * 60);
/// About 30 fps for the VU.
const VU_MS: u32 = 33;
const VK_H: u32 = 0x48;
const VK_T: u32 = 0x54;
const VK_VOLUME_MUTE: u32 = 0xAD;
const VK_VOLUME_DOWN: u32 = 0xAE;
const VK_VOLUME_UP: u32 = 0xAF;
/// Frame step used for the first frame after idle, and the cap for long stalls.
const FIRST_DT: f32 = 1.0 / 120.0;
const MAX_DT: f32 = 0.05;
const REGION_PAD: i32 = 2;

/// The local time now, for a saved snap's file name.
fn local_stamp() -> Stamp {
    let (year, month, day, hour, minute, second) = system::local_datetime();
    Stamp {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

/// The Snap overlay while a snap is in progress: its window and its renderer.
struct Overlay {
    hwnd: HWND,
    /// Dropped before the window is destroyed.
    renderer: Option<SnapOverlay>,
    /// The window's desktop origin (the virtual screen's top-left; can be negative).
    origin: (i32, i32),
    /// Input changed what it shows: draw one frame.
    dirty: bool,
}

pub struct App {
    hwnd: HWND,
    renderer: Option<Renderer>,
    scale: f32,
    win_px: (i32, i32),
    registry: ThemeRegistry,
    theme_id: String,
    anim: ThemeAnim,
    island: Island,
    config: Config,
    visible: bool,
    /// One more frame is needed even though nothing is animating.
    dirty: bool,
    started: Instant,
    last_frame: Option<Instant>,
    last_rect: Option<Edges>,
    /// The top strip asked of the shell while `[island] reserve_top` holds (see
    /// `sync_appbar`); `None` while nothing is reserved.
    reserved: Option<Edges>,
    icon: Option<HICON>,
    exe: PathBuf,
    /// The default render endpoint. None if the audio stack is unavailable.
    mixer: Option<volume::Mixer>,
    session: Session,
    /// The hidden YouTube Music engine, when running.
    engine: Option<Engine>,
    /// When the last state message arrived, to interpolate the scrubber.
    state_at: Instant,
    /// Cover art: the URL being shown (or fetched), its average colour once decoded.
    art_url: Option<String>,
    cover: Option<Rgb>,
    art_tx: Sender<ArtResult>,
    art_rx: Receiver<ArtResult>,
    vu_armed: bool,
    /// Health samples and their history for the Vitals panel and the idle lamp.
    vitals: Vitals,
    worker: Worker,
    /// Sweep: the view model, the reclaimable cache and the worker running the jobs.
    sweep: SweepView,
    sweep_cache: SweepCache,
    sweeper: Sweeper,
    /// The running estimate and preview scans, by id. Results of other ids are stale.
    estimate_scan: Option<ScanId>,
    preview_scan: Option<ScanId>,
    estimate_started: Option<Instant>,
    /// A clean (or a single recycle) is running.
    cleaning: bool,
    /// The Pomodoro and its chime.
    timer: Timer,
    /// The deadline the one-shot `TIMER_DEADLINE` is armed for, if any.
    deadline_armed: Option<Instant>,
    /// The one-shot `TIMER_DIGITS` redraw is armed.
    digits_armed: bool,
    /// Snap: the frozen frames while the overlay is up, then the last shot while its
    /// thumb is.
    snapper: Snapper,
    overlay: Option<Overlay>,
    /// A snap waits for the island to finish collapsing, and `TIMER_SNAP_START` is armed.
    snap_pending: bool,
    snap_start_armed: bool,
    /// The one-shot `TIMER_SNAP_THUMB` is armed.
    thumb_armed: bool,
    /// What the thumb shows. Kept after the shot goes, until the thumb has faded out.
    snap_frame: Option<SnapFrame>,
    /// Another app holds the Snap hotkey / the alt hotkey (the Settings chips go warn).
    snap_taken: (bool, bool),
    /// The Snap preview stack below the island.
    previews: Previews,
    /// The island window's desktop position (device px).
    win_pos: (i32, i32),
    /// Tile: window zones, the zone overlay while a move shows one, and the timers.
    tiler: Tiler,
    tile_overlay: Option<TileOverlay>,
    /// The island's Tile panel was opened for Tile (so Tile closes it again).
    tile_panel: bool,
    tile_poll_armed: bool,
    /// The one-shot retry and notice timers, by when they fire.
    tile_retry_at: Option<Instant>,
    tile_notice_at: Option<Instant>,
}

/// The zone overlay on one monitor while a move shows it: its window and its surface.
struct TileOverlay {
    hwnd: HWND,
    /// Dropped before the window is destroyed.
    surface: Option<ZoneOverlay>,
    monitor: IRect,
    shown: bool,
    /// What it last drew, so a poll redraws only on change.
    drawn: Option<(Vec<IRect>, Option<usize>)>,
}

/// The zone keys: Ctrl+Alt+1..9, then the arrows.
const TILE_KEYS: [(i32, u32, &str); 13] = [
    (HOTKEY_ZONE_1, 0x31, "Ctrl+Alt+1"),
    (HOTKEY_ZONE_1 + 1, 0x32, "Ctrl+Alt+2"),
    (HOTKEY_ZONE_1 + 2, 0x33, "Ctrl+Alt+3"),
    (HOTKEY_ZONE_1 + 3, 0x34, "Ctrl+Alt+4"),
    (HOTKEY_ZONE_1 + 4, 0x35, "Ctrl+Alt+5"),
    (HOTKEY_ZONE_1 + 5, 0x36, "Ctrl+Alt+6"),
    (HOTKEY_ZONE_1 + 6, 0x37, "Ctrl+Alt+7"),
    (HOTKEY_ZONE_1 + 7, 0x38, "Ctrl+Alt+8"),
    (HOTKEY_ZONE_9, 0x39, "Ctrl+Alt+9"),
    (HOTKEY_LEFT, 0x25, "Ctrl+Alt+Left"),
    (HOTKEY_RIGHT, 0x27, "Ctrl+Alt+Right"),
    (HOTKEY_UP, 0x26, "Ctrl+Alt+Up"),
    (HOTKEY_DOWN, 0x28, "Ctrl+Alt+Down"),
];

impl App {
    pub fn new(hwnd: HWND, config: Config, exe: PathBuf) -> Self {
        let mut registry = ThemeRegistry::with_builtins();
        registry.set_user_themes(storage::read_user_themes());
        let theme_id = if registry.get(&config.theme).is_some() {
            config.theme.clone()
        } else {
            DEFAULT_THEME.to_string()
        };
        let tokens = registry
            .get(&theme_id)
            .expect("default theme exists")
            .clone();
        let island = Island::new(&tokens);
        let (art_tx, art_rx) = mpsc::channel();
        let session = Session {
            signed_in: config.music.signed_in,
            ..Session::default()
        };
        let worker = Worker::start(hwnd);
        let timer = Timer::new(
            config.timer.settings(),
            config.timer.chime,
            Box::new(SystemChime),
        );
        let profile = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_default();
        let sweeper = Sweeper::start(
            hwnd,
            config.sweep.roots(&profile),
            storage::operations_log_path(),
        );
        let raw = hwnd.0 as isize;
        let previews = Previews::new(
            Model::new(
                Stack::new(Duration::from_secs(config.snap.preview_secs())),
                Store::new(Box::new(move || window::wake(raw))),
                Box::new(SystemClipboard { owner: raw }),
                Store::reveal,
                config.snap.save_dir(&profile),
                local_stamp,
            ),
            hwnd,
            &tokens,
        );
        let (custom, errors) = dj_core::tile::parse_custom(&storage::read_layouts());
        for e in errors {
            storage::log(&format!("tile: {e}"));
        }
        let tiler = Tiler::new(Box::new(SystemWindows), config.tile.clone(), &custom);
        let mut app = Self {
            hwnd,
            renderer: None,
            scale: 1.0,
            win_px: (0, 0),
            registry,
            theme_id,
            anim: ThemeAnim::new(tokens),
            island,
            config,
            visible: true,
            dirty: true,
            started: Instant::now(),
            last_frame: None,
            last_rect: None,
            icon: None,
            exe,
            mixer: None,
            session,
            engine: None,
            state_at: Instant::now(),
            art_url: None,
            cover: None,
            art_tx,
            art_rx,
            vu_armed: false,
            vitals: Vitals::new(),
            worker,
            sweep: SweepView::new(),
            sweep_cache: storage::load_sweep_cache(),
            sweeper,
            estimate_scan: None,
            preview_scan: None,
            estimate_started: None,
            cleaning: false,
            timer,
            deadline_armed: None,
            digits_armed: false,
            snapper: Snapper::new(
                Box::new(SystemScreen),
                Box::new(SystemClipboard {
                    owner: hwnd.0 as isize,
                }),
                Self::text_measure(),
            ),
            overlay: None,
            snap_pending: false,
            snap_start_armed: false,
            thumb_armed: false,
            snap_frame: None,
            snap_taken: (false, false),
            reserved: None,
            previews,
            win_pos: (0, 0),
            tiler,
            tile_overlay: None,
            tile_panel: false,
            tile_poll_armed: false,
            tile_retry_at: None,
            tile_notice_at: None,
        };
        app.relayout();
        app.install_shell_integration();
        if app.config.music.signed_in {
            // Signed in before: the engine runs hidden from the start.
            app.ensure_engine(false);
        }
        app.sync_vitals();
        app
    }

    fn install_shell_integration(&mut self) {
        match tray::make_icon() {
            Ok(icon) => {
                self.icon = Some(icon);
                tray::add(self.hwnd, icon, "Daijobu Island");
            }
            Err(e) => storage::log(&format!("tray icon: {e}")),
        }
        if !window::register_hotkey(self.hwnd, HOTKEY_TOGGLE, window::MOD_CTRL_ALT, VK_H) {
            storage::log("Ctrl+Alt+H is already taken by another app");
        }
        if !window::register_hotkey(self.hwnd, HOTKEY_THEME, window::MOD_CTRL_ALT, VK_T) {
            storage::log("Ctrl+Alt+T is already taken by another app");
        }
        self.register_snap_hotkey();
        // Tile: window moves of other apps, and the zone keys `[tile]` turns on.
        if !dj_win::tile::install_move_hook() {
            storage::log("tile: cannot watch window moves");
        }
        let wanted = self.tiler.hotkeys();
        for (id, vk, name) in TILE_KEYS.iter().filter(|k| wanted.contains(&k.0)) {
            if !window::register_hotkey(self.hwnd, *id, window::MOD_CTRL_ALT, *vk) {
                storage::log(&format!("tile: {name} is already taken by another app"));
            }
        }
        system::install_topmost_hook(self.hwnd);
        system::watch_dir(self.hwnd, &storage::themes_dir());
        self.install_volume();
        self.arm_clock_timer();
        // Make the Run entry match the config (also refreshes a stale exe path).
        if self.config.start_with_windows || system::autostart_enabled() {
            system::set_autostart(self.config.start_with_windows, &self.exe);
        }
    }

    /// Volume endpoint notifications always run (the pill shows any change). The volume
    /// keys are only taken over when `[osd] replace_volume_flyout` is on.
    fn install_volume(&mut self) {
        volume::init_com();
        match volume::Mixer::new(self.hwnd) {
            Ok(m) => self.mixer = Some(m),
            Err(e) => storage::log(&format!("volume endpoint: {e}")),
        }
        // Without an endpoint the keys would be swallowed and do nothing, so the flyout stays.
        if self.config.osd.replace_volume_flyout && self.mixer.is_some() {
            hook::set_volume_keys(true);
            if !hook::install(self.hwnd) {
                storage::log("cannot install the volume key hook");
            }
        }
    }

    /// Shows the OSD with the endpoint's current level and mute state.
    fn show_volume(&mut self) {
        let Some((level, muted)) = self.mixer.as_ref().and_then(|m| m.read()) else {
            return;
        };
        let tokens = self.anim.current().clone();
        self.island.show_osd(Volume { level, muted }, &tokens);
        self.dirty = true;
    }

    fn set_level(&mut self, level: f32) {
        if let Some(m) = self.mixer.as_ref() {
            m.set_level(level);
        }
        self.show_volume();
    }

    /// Sets the sampler's cadence from what is on screen (see `Cadence::for_view`).
    fn sync_vitals(&mut self) {
        let cadence = Cadence::for_view(
            self.visible,
            self.island.state() == IslandState::Vitals,
            self.config.vitals.health_lamp,
        );
        self.worker.set_cadence(cadence);
    }

    /// True when the idle lamp should show the warn colour.
    fn lamp_warn(&self) -> bool {
        self.config.vitals.health_lamp && self.vitals.health().is_some_and(|h| h.band.is_warn())
    }

    /// Queues the worker's readings. Redraws when the panel is open or the lamp colour changed.
    fn drain_vitals(&mut self) {
        let lamp_before = self.lamp_warn();
        for sample in self.worker.drain() {
            self.vitals.push(sample.reading);
            if let Some(top) = sample.top {
                self.vitals.set_top(top);
            }
        }
        if self.island.state() == IslandState::Vitals || self.lamp_warn() != lamp_before {
            self.dirty = true;
        }
    }

    // ---- Sweep ----

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    /// What the Vitals panel's Sweep button shows.
    fn estimate(&self) -> Estimate {
        Estimate::from_cache(&self.sweep_cache, self.estimate_scan.is_some())
    }

    /// Starts, keeps or cancels sweep scans for what is on screen:
    /// - the estimate runs only while the Vitals panel is open and visible, when the
    ///   cache is stale, and at most once per `ESTIMATE_RETRY`;
    /// - the preview runs only while the Sweep view is open and visible.
    fn sync_sweep(&mut self) {
        let state = self.island.state();
        let in_panel = matches!(state, IslandState::Vitals | IslandState::Sweep);
        if self.estimate_scan.is_some() && (!self.visible || !in_panel) {
            self.sweeper.cancel_scan();
            self.estimate_scan = None;
            // Cancelled, not failed: the next panel open may try again.
            self.estimate_started = None;
        }
        if self.preview_scan.is_some() && (!self.visible || state != IslandState::Sweep) {
            self.sweeper.cancel_scan();
            self.preview_scan = None;
        }
        if !self.visible || self.cleaning {
            return;
        }
        if state == IslandState::Sweep
            && self.sweep.phase == Phase::Scanning
            && self.preview_scan.is_none()
        {
            // The preview was cancelled (hidden mid-scan): run it again.
            self.start_preview();
        } else if state == IslandState::Vitals
            && self.estimate_scan.is_none()
            && self.preview_scan.is_none()
            && self.sweep_cache.is_stale(Self::now_unix())
            && self
                .estimate_started
                .is_none_or(|t| t.elapsed() >= ESTIMATE_RETRY)
        {
            self.estimate_started = Some(Instant::now());
            self.estimate_scan = Some(self.sweeper.scan(false));
            self.dirty = true;
        }
    }

    fn start_preview(&mut self) {
        if self.estimate_scan.take().is_some() {
            // The preview gives fresher totals anyway.
            self.sweeper.cancel_scan();
            self.estimate_started = None;
        }
        self.sweep.start_scan();
        self.preview_scan = Some(self.sweeper.scan(true));
        self.dirty = true;
    }

    /// Saves the totals of a finished preview or clean as the new estimate.
    fn store_estimate(&mut self) {
        self.sweep_cache = SweepCache::from_totals(Self::now_unix(), &self.sweep.totals());
        storage::save_sweep_cache(&self.sweep_cache);
    }

    fn back_to_vitals(&mut self) {
        window::kill_timer(self.hwnd, TIMER_SWEEP_DONE);
        let tokens = self.anim.current().clone();
        self.island.open_vitals(&tokens);
        self.dirty = true;
    }

    /// Runs what a click in the Sweep view asked for.
    fn sweep_cmd(&mut self, cmd: SweepCmd) {
        match cmd {
            SweepCmd::Back => self.back_to_vitals(),
            SweepCmd::Clean => {
                if self.cleaning || !self.sweep.can_clean() {
                    return;
                }
                let items: Vec<_> = self
                    .sweep
                    .selection()
                    .into_iter()
                    .map(|(cat, item)| (cat, item.path))
                    .collect();
                self.sweep.begin_clean(items.len());
                self.cleaning = true;
                self.sweeper.send(Job::Clean(items));
            }
            SweepCmd::FlushDns => {
                self.sweep.status = Some("flushing dns…".into());
                self.sweeper.send(Job::FlushDns);
            }
            SweepCmd::EmptyBin => {
                self.sweep.status = Some("emptying the bin…".into());
                self.sweeper.send(Job::EmptyBin);
            }
            SweepCmd::Reveal(path) => self.sweeper.send(Job::Reveal(path)),
            SweepCmd::Recycle(path) => {
                if self.cleaning {
                    return;
                }
                self.cleaning = true;
                self.sweep.status = Some("recycling…".into());
                self.sweeper.send(Job::Recycle(path));
            }
        }
    }

    /// Applies the sweep worker's results.
    fn drain_sweep(&mut self) {
        let msgs: Vec<Msg> = self.sweeper.drain().collect();
        for msg in msgs {
            match msg {
                Msg::Category {
                    id,
                    cat,
                    items,
                    note,
                } => {
                    if self.preview_scan == Some(id) {
                        self.sweep.set_category(cat, items, note);
                    }
                }
                Msg::ScanDone {
                    id,
                    estimate,
                    totals,
                } => {
                    if estimate && self.estimate_scan == Some(id) {
                        self.estimate_scan = None;
                        if let Some(totals) = totals {
                            self.sweep_cache = SweepCache::from_totals(Self::now_unix(), &totals);
                            storage::save_sweep_cache(&self.sweep_cache);
                        }
                    } else if !estimate && self.preview_scan == Some(id) {
                        self.preview_scan = None;
                        if totals.is_some() {
                            self.sweep.finish_scan();
                            self.store_estimate();
                        }
                    }
                }
                Msg::ScanFailed { id, estimate, why } => {
                    if estimate && self.estimate_scan == Some(id) {
                        // `estimate_started` stays set, so it is retried after a while.
                        self.estimate_scan = None;
                    } else if !estimate && self.preview_scan == Some(id) {
                        // An empty, finished preview: nothing to clean, and no rescan loop.
                        self.preview_scan = None;
                        for cat in Category::ALL {
                            self.sweep.set_category(cat, Vec::new(), None);
                        }
                        self.sweep.finish_scan();
                        self.sweep.status = Some(why);
                    }
                }
                Msg::Progress { done, total } => self.sweep.clean_progress(done, total),
                Msg::Cleaned {
                    freed,
                    recycled,
                    failed,
                    gone,
                    note,
                } => {
                    self.cleaning = false;
                    // Recycled bytes still sit on the disk (in the bin), so "Freed" only
                    // counts permanent removals and the bin share is named separately.
                    self.sweep.finish_clean(freed, failed, &gone);
                    if let Some(note) = note {
                        self.sweep.status = Some(note);
                    } else if recycled > 0 {
                        let mut line = format!(
                            "Freed {} · {} to bin",
                            format_bytes(freed),
                            format_bytes(recycled)
                        );
                        if failed > 0 {
                            line.push_str(&format!(" · {failed} failed"));
                        }
                        self.sweep.status = Some(line);
                    }
                    self.store_estimate();
                    // The disk ring and the score move at once.
                    self.worker.sample_now();
                    if self.island.state() == IslandState::Sweep {
                        window::set_timer(self.hwnd, TIMER_SWEEP_DONE, SWEEP_DONE_MS);
                    }
                }
                Msg::Recycled { path, ok, in_bin } => {
                    self.cleaning = false;
                    if ok {
                        self.sweep.forget(&path);
                        self.sweep.status = Some(
                            if in_bin {
                                "moved to the recycle bin"
                            } else {
                                "not recycled: deleted for good"
                            }
                            .into(),
                        );
                        self.worker.sample_now();
                    } else {
                        self.sweep.status = Some("could not recycle that file".into());
                    }
                }
                Msg::Status(text) => self.sweep.status = Some(text),
            }
        }
        if matches!(
            self.island.state(),
            IslandState::Vitals | IslandState::Sweep
        ) {
            self.dirty = true;
        }
    }

    // ---- Snap ----

    /// The annotation editor measures text with the font the markup is drawn in.
    fn text_measure() -> Box<dyn TextMeasure> {
        match TextMetrics::new() {
            Ok(m) => Box::new(m),
            Err(e) => {
                storage::log(&format!("snap: {e}; text boxes are approximate"));
                Box::new(RoughMeasure)
            }
        }
    }

    /// Sets up `[snap] hotkey` and `[snap] alt_hotkey`, dropping whatever was set up
    /// before, so it also serves a change from the Settings panel. PrtSc goes through
    /// the keyboard hook, which keeps it from Windows' own screen capture (that one
    /// doesn't use `RegisterHotKey`, so registering PrtSc alone would not win). If the
    /// hook can't be installed, PrtSc is registered as a plain hotkey instead. Every
    /// failure goes to the log, and `snap_taken` remembers which key failed.
    fn register_snap_hotkey(&mut self) {
        window::unregister_hotkey(self.hwnd, HOTKEY_SNAP);
        window::unregister_hotkey(self.hwnd, HOTKEY_SNAP_ALT);
        hook::set_print_screen(false);
        self.snap_taken = (false, false);
        let snap = self.config.snap.clone();
        if SnapHotkey::parse(&snap.hotkey).is_none() {
            storage::log(&format!(
                "snap: unknown hotkey {:?}, using PrtSc",
                snap.hotkey
            ));
        }
        let main = snap.hotkey();
        let hooked = main == SnapHotkey::PrtSc && {
            hook::set_print_screen(true);
            let ok = hook::install(self.hwnd);
            if !ok {
                hook::set_print_screen(false);
                storage::log("snap: cannot install the keyboard hook for PrtSc");
            }
            ok
        };
        if !hooked {
            self.snap_taken.0 = !self.register_snap_key(HOTKEY_SNAP, main);
        }
        match snap.alt_hotkey() {
            Some(alt) => self.snap_taken.1 = !self.register_snap_key(HOTKEY_SNAP_ALT, alt),
            None => {
                let off = snap.alt_hotkey.trim();
                if !off.is_empty() && !off.eq_ignore_ascii_case("none") {
                    storage::log(&format!("snap: alt_hotkey {off:?} not used"));
                }
            }
        }
    }

    /// Registers one Snap shortcut under `id`, logging why if another app holds it.
    fn register_snap_key(&mut self, id: i32, key: SnapHotkey) -> bool {
        let (mods, vk) = snapper::hotkey_keys(key);
        match window::try_register_hotkey(self.hwnd, id, mods, vk) {
            Ok(()) => true,
            Err(e) => {
                storage::log(&format!(
                    "snap: {} is not available: {}",
                    key.label(),
                    e.trim()
                ));
                false
            }
        }
    }

    // ---- Settings ----

    /// A control in the Settings panel. The panel redraws from the config, so applying
    /// the change and saving it is all there is to do.
    fn settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::ToggleAutostart => {
                let want = !system::autostart_enabled();
                if system::set_autostart(want, &self.exe) {
                    self.config.start_with_windows = want;
                    storage::save_config(&self.config);
                } else {
                    storage::log("settings: cannot change the Run entry");
                }
            }
            SettingsAction::CycleHotkey | SettingsAction::CycleAltHotkey => {
                if action == SettingsAction::CycleHotkey {
                    settings::cycle_hotkey(&mut self.config.snap);
                } else {
                    settings::cycle_alt_hotkey(&mut self.config.snap);
                }
                self.register_snap_hotkey();
                storage::save_config(&self.config);
            }
            SettingsAction::ToggleStyle => {
                let next = self.config.island.style().toggled();
                self.config.island.style = next.label().to_string();
                storage::save_config(&self.config);
                // Switch at once: the next frame draws the new shape and `device_rect`
                // (which reads the style) moves the window region with it.
                self.dirty = true;
                self.sync_appbar(false);
            }
            SettingsAction::ToggleReserve => {
                self.config.island.reserve_top = !self.config.island.reserve_top;
                storage::save_config(&self.config);
                self.dirty = true;
                self.sync_appbar(false);
            }
        }
    }

    /// Reserves the top strip (an AppBar) while the island is a visible notch with
    /// `[island] reserve_top`, and gives it back otherwise. `force` sets it again even if
    /// nothing changed here (the shell moved the bars, or the monitor changed).
    fn sync_appbar(&mut self, force: bool) {
        let want = (self.visible && self.config.island.reserves_top()).then(|| {
            let mon = window::primary_monitor();
            notch::reserve_strip(mon.left, mon.top, mon.width, mon.dpi as f32 / 96.0)
        });
        if want == self.reserved && !force {
            return;
        }
        match want {
            Some(strip) => {
                if appbar::reserve(self.hwnd, strip).is_none() {
                    storage::log("appbar: the shell refused to reserve the top strip");
                }
            }
            None => appbar::release(self.hwnd),
        }
        self.reserved = want;
    }

    fn settings_view(&self) -> SettingsView {
        SettingsView {
            autostart: self.config.start_with_windows,
            hotkey: self.config.snap.hotkey(),
            alt: self.config.snap.alt_hotkey(),
            hotkey_taken: self.snap_taken.0,
            alt_taken: self.snap_taken.1,
            style: self.config.island.style(),
            reserve: self.config.island.reserve_top,
        }
    }

    /// PrtSc or the Home tile. Any open panel collapses first (so it isn't in the
    /// shot) and the freeze waits until the island is still; a resting island is
    /// frozen at once. A thumb that is still up goes first.
    fn request_snap(&mut self) {
        if self.overlay.is_some() || self.snap_pending {
            return;
        }
        let tokens = self.anim.current().clone();
        let before = self.island.state();
        if self.island.snap_shown() {
            self.snapper.dismiss();
            self.island.hide_snap(&tokens);
        }
        self.island.collapse(&tokens);
        // Only a state change (a panel or an old thumb going) is worth waiting for; a
        // resting island is part of the screen like the taskbar.
        if self.visible && self.island.state() != before {
            self.snap_pending = true;
            self.dirty = true;
        } else {
            self.begin_snap();
        }
    }

    /// Freezes the screen and puts the overlay up over the whole virtual desktop.
    fn begin_snap(&mut self) {
        self.snap_pending = false;
        if self.overlay.is_some() {
            return;
        }
        // The preview stack must not be in the shot: hidden, and gone from the composed
        // desktop, before the freeze.
        self.previews.hide_for_capture(Instant::now());
        let (vx, vy, vw, vh) = overlay::virtual_screen();
        if vw <= 0 || vh <= 0 {
            storage::log("snap: no virtual screen");
            self.previews.set_allowed(self.visible, Instant::now());
            return;
        }
        let desktop = IRect::new(vx, vy, vw, vh);
        let prefs = self.config.snap.annotate.prefs();
        let accent = Rgba::from_rgb(self.anim.current().accent);
        let frames = match self
            .snapper
            .begin(desktop, overlay::cursor_pos(), prefs, accent)
        {
            Ok(session) => &session.frames,
            Err(e) => {
                storage::log(&format!("snap: cannot capture the screen: {e}"));
                self.previews.set_allowed(self.visible, Instant::now());
                return;
            }
        };
        let hwnd = match overlay::create(vx, vy, vw, vh) {
            Ok(h) => h,
            Err(e) => {
                storage::log(&format!("snap: cannot create the overlay: {e}"));
                self.snapper.end(End::Cancel, Instant::now());
                self.previews.set_allowed(self.visible, Instant::now());
                return;
            }
        };
        let renderer =
            match SnapOverlay::new(hwnd.0 as isize, (vx, vy), vw as u32, vh as u32, frames) {
                Ok(r) => r,
                Err(e) => {
                    storage::log(&format!("snap: overlay renderer: {e}"));
                    overlay::destroy(hwnd);
                    self.snapper.end(End::Cancel, Instant::now());
                    self.previews.set_allowed(self.visible, Instant::now());
                    return;
                }
            };
        self.overlay = Some(Overlay {
            hwnd,
            renderer: Some(renderer),
            origin: (vx, vy),
            dirty: true,
        });
        // The island is topmost too and re-asserts it on every foreground change, so it
        // hides while the overlay is up (`visible` is the user's choice and stays).
        window::set_visible(self.hwnd, false);
        // Esc works even if the overlay didn't get the keyboard.
        if let Err(e) =
            window::try_register_hotkey(self.hwnd, HOTKEY_SNAP_ESC, window::MOD_NONE, VK_ESCAPE)
        {
            storage::log(&format!("snap: Esc hotkey: {}", e.trim()));
        }
        // Draw the first frame before showing, so the live desktop never flashes through.
        self.render_overlay();
        if let Some(ov) = &self.overlay {
            overlay::show(ov.hwnd);
        }
    }

    /// Overlay input, in the overlay's client px. Before the release it drives the 7a
    /// selection; after it, the annotation editor.
    fn snap_input(&mut self, input: SnapInput) {
        let Some(origin) = self.overlay.as_ref().map(|o| o.origin) else {
            return;
        };
        // Alt+F4 on the overlay: cancel, whatever the phase.
        if input == SnapInput::Close {
            self.end_snap(End::Cancel);
            return;
        }
        let editing = self.snapper.session().is_some_and(|s| s.editor.is_some());
        if editing {
            self.edit_input(input, origin);
        } else {
            self.select_input(input, origin);
        }
    }

    /// The selecting phase: hover, drag, window click, arrow nudge. Esc or a right click
    /// cancels; the release starts editing (a too-small selection cancels).
    fn select_input(&mut self, input: SnapInput, origin: (i32, i32)) {
        let Some(session) = self.snapper.session_mut() else {
            return;
        };
        let sel = &mut session.selector;
        let desk = |x: i32, y: i32| (x + origin.0, y + origin.1);
        let arrow = |vk: u16| match vk {
            0x25 => Some((-1, 0)),
            0x27 => Some((1, 0)),
            0x26 => Some((0, -1)),
            0x28 => Some((0, 1)),
            _ => None,
        };
        let changed = match input {
            SnapInput::Press { x, y } | SnapInput::DoubleClick { x, y } => {
                let (x, y) = desk(x, y);
                sel.press(x, y);
                true
            }
            SnapInput::Motion { x, y } => {
                let (x, y) = desk(x, y);
                sel.move_to(x, y)
            }
            SnapInput::Key { vk, shift, .. } if arrow(vk).is_some() => {
                let (dx, dy) = arrow(vk).unwrap_or_default();
                let (x, y) = sel.nudge(dx, dy, shift);
                // The pointer follows, so the next mouse move agrees with the nudge.
                overlay::set_cursor_pos(x, y);
                true
            }
            SnapInput::Key { vk, .. } if u32::from(vk) == VK_ESCAPE => {
                self.end_snap(End::Cancel);
                return;
            }
            SnapInput::RightPress => {
                self.end_snap(End::Cancel);
                return;
            }
            // A release without a press on the overlay (the button was already down
            // when it opened) is ignored.
            SnapInput::Release { .. } if !sel.is_pressed() => false,
            SnapInput::Release { x, y } => {
                let (x, y) = desk(x, y);
                sel.move_to(x, y);
                if self.snapper.start_editing() {
                    // The selection stays, the toolbar shows; the clipboard is untouched.
                    self.sync_overlay_cursor();
                    true
                } else {
                    self.end_snap(End::Cancel);
                    return;
                }
            }
            SnapInput::Key { .. } | SnapInput::Char(_) | SnapInput::Wheel(_) => false,
            SnapInput::Close => false,
        };
        if changed {
            if let Some(ov) = self.overlay.as_mut() {
                ov.dirty = true;
            }
        }
    }

    /// The editing phase: everything goes to the editor, except a right click (which does
    /// nothing here). Copy, save and cancel end the snap.
    fn edit_input(&mut self, input: SnapInput, origin: (i32, i32)) {
        let desk = |x: i32, y: i32| (x + origin.0, y + origin.1);
        let ev = match input {
            SnapInput::Press { x, y } => {
                let (x, y) = desk(x, y);
                EditInput::Press { x, y }
            }
            SnapInput::Motion { x, y } => {
                let (x, y) = desk(x, y);
                EditInput::Motion { x, y }
            }
            SnapInput::Release { x, y } => {
                let (x, y) = desk(x, y);
                EditInput::Release { x, y }
            }
            SnapInput::DoubleClick { x, y } => {
                let (x, y) = desk(x, y);
                EditInput::DoubleClick { x, y }
            }
            SnapInput::Key { vk, ctrl, shift } => EditInput::Key(Key {
                code: KeyCode::from_vk(vk),
                ctrl,
                shift,
            }),
            SnapInput::Char(c) => EditInput::Char(c),
            SnapInput::Wheel(n) => EditInput::Wheel(n),
            SnapInput::RightPress | SnapInput::Close => return,
        };
        match self.snapper.edit(ev) {
            Outcome::Nothing => {}
            Outcome::Redraw => {
                if let Some(ov) = self.overlay.as_mut() {
                    ov.dirty = true;
                }
            }
            Outcome::Copy => {
                self.end_snap(End::Copy);
                return;
            }
            Outcome::Save => {
                let (dir, stamp) = self.save_target();
                self.end_snap(End::Save { dir, stamp });
                return;
            }
            Outcome::Cancel => {
                self.end_snap(End::Cancel);
                return;
            }
        }
        self.sync_overlay_cursor();
    }

    /// Esc: the `HOTKEY_SNAP_ESC` hotkey and the overlay's own key both come here, so it
    /// goes through the editor's steps (commit the text, drop the shape selection, then
    /// cancel) rather than straight to a cancel.
    fn snap_escape(&mut self) {
        self.snap_input(SnapInput::Key {
            vk: VK_ESCAPE as u16,
            ctrl: false,
            shift: false,
        });
    }

    /// The overlay shows the cursor the editor asks for (the crosshair while selecting).
    fn sync_overlay_cursor(&self) {
        let kind = self
            .snapper
            .session()
            .and_then(|s| s.editor.as_ref())
            .map_or(CursorKind::Cross, |e| e.cursor_kind());
        overlay::set_cursor(match kind {
            CursorKind::Cross => overlay::Cursor::Cross,
            CursorKind::IBeam => overlay::Cursor::IBeam,
            CursorKind::SizeNwse => overlay::Cursor::SizeNwse,
            CursorKind::SizeNesw => overlay::Cursor::SizeNesw,
            CursorKind::SizeWe => overlay::Cursor::SizeWe,
            CursorKind::SizeNs => overlay::Cursor::SizeNs,
            CursorKind::Move => overlay::Cursor::Move,
            CursorKind::Arrow => overlay::Cursor::Arrow,
        });
    }

    /// Where a save goes: `[snap] save_dir`, named from the local time now.
    fn save_target(&self) -> (PathBuf, Stamp) {
        let profile = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_default();
        (self.config.snap.save_dir(&profile), local_stamp())
    }

    /// Closes the overlay (its GPU side goes first), then ends the session: a copy or a
    /// save renders the selection with its markup, copies it (and saves it) and shows
    /// the thumb; a cancel keeps nothing and leaves the clipboard alone. Changed prefs
    /// are written to `[snap.annotate]`, keeping the rest of the config.
    fn end_snap(&mut self, how: End) {
        let Some(mut ov) = self.overlay.take() else {
            return;
        };
        ov.renderer = None;
        overlay::destroy(ov.hwnd);
        window::unregister_hotkey(self.hwnd, HOTKEY_SNAP_ESC);
        if self.visible {
            window::set_visible(self.hwnd, true);
        }
        self.last_frame = None;
        self.dirty = true;
        let ended = self.snapper.end(how, Instant::now());
        for e in &ended.errors {
            storage::log(&format!("snap: {e}"));
        }
        if let Some(prefs) = ended.prefs {
            self.config.snap.annotate = AnnotateConfig::from_prefs(&prefs);
            storage::save_config(&self.config);
        }
        let now = Instant::now();
        if ended.taken {
            self.show_thumb();
            // The same shot (shared, not copied) becomes the stack's newest card.
            let card = self.snapper.payload().zip(
                self.snapper
                    .shot()
                    .map(|s| (previews::first_status(s.status), s.saved.clone())),
            );
            if let Some((payload, (status, saved))) = card {
                self.previews.set_anchor(self.stack_anchor(), now);
                self.previews.add(payload, status, saved, now);
            }
        }
        self.previews.set_allowed(self.visible, now);
    }

    /// The fresh shot goes into the island's thumb with a new countdown.
    fn show_thumb(&mut self) {
        if self.thumb_armed {
            window::kill_timer(self.hwnd, TIMER_SNAP_THUMB);
            self.thumb_armed = false;
        }
        self.restore_snap_shot();
        self.update_snap_frame();
        let tokens = self.anim.current().clone();
        self.island.show_snap(&tokens);
    }

    /// Gives the renderer the thumb's copy of the shot (after a snap, or after the
    /// renderer was rebuilt).
    fn restore_snap_shot(&mut self) {
        if let (Some(r), Some(shot)) = (self.renderer.as_mut(), self.snapper.shot()) {
            if !r.set_snap_shot(&shot.bitmap) {
                storage::log("snap: cannot make the thumbnail");
            }
        }
    }

    fn update_snap_frame(&mut self) {
        if let Some(shot) = self.snapper.shot() {
            self.snap_frame = Some(SnapFrame {
                width: shot.bitmap.width,
                height: shot.bitmap.height,
                status: shot.status,
                saved: shot.saved.is_some(),
            });
        }
    }

    /// Copy, save or dismiss from the thumb.
    fn snap_action(&mut self, action: SnapAction) {
        match action {
            SnapAction::Copy => {
                if let Err(e) = self.snapper.copy() {
                    storage::log(&format!("snap: copy failed: {e}"));
                }
            }
            // Once saved, the button reads "open": show the file in Explorer.
            SnapAction::Save if self.snapper.shot().is_some_and(|s| s.saved.is_some()) => {
                let path = self.snapper.shot().and_then(|s| s.saved.clone());
                if let Some(path) = path {
                    if let Err(e) = dj_sweep::Ops::reveal(&dj_sweep::WinOps, &path) {
                        storage::log(&format!("snap: cannot open explorer: {e}"));
                    }
                }
            }
            SnapAction::Save => {
                let (dir, stamp) = self.save_target();
                match self.snapper.save(&dir, stamp) {
                    // The stack's card for this shot reads "open" for the same file.
                    Ok(path) => {
                        if let Some(bitmap) = self.snapper.shot().map(|s| s.bitmap.clone()) {
                            self.previews.mark_saved(&bitmap, &path);
                        }
                    }
                    Err(e) => storage::log(&format!("snap: save failed: {e}")),
                }
            }
            // The island has gone to its rest state already.
            SnapAction::Dismiss => self.snapper.dismiss(),
        }
        self.update_snap_frame();
        self.dirty = true;
    }

    /// Pauses the thumb's countdown while hovered and keeps its one-shot timer in step.
    fn sync_snap(&mut self) {
        let now = Instant::now();
        self.snapper.set_hover(self.island.snap_hovered(), now);
        match self.snapper.thumb_ms(now) {
            Some(ms) if !self.thumb_armed => {
                window::set_timer(self.hwnd, TIMER_SNAP_THUMB, ms);
                self.thumb_armed = true;
            }
            None if self.thumb_armed => {
                window::kill_timer(self.hwnd, TIMER_SNAP_THUMB);
                self.thumb_armed = false;
            }
            _ => {}
        }
    }

    /// Draws one overlay frame: the selection, or once released, the editor.
    fn render_overlay(&mut self) {
        let Some(ov) = self.overlay.as_mut() else {
            return;
        };
        ov.dirty = false;
        let Some(session) = self.snapper.session() else {
            return;
        };
        let sel = &session.selector;
        let cursor = sel.cursor();
        let selection = sel.selection();
        let hover = sel.hover();
        let editor = session.editor.as_ref();
        let size = match editor {
            Some(e) => Some(size_label(e.selection().w, e.selection().h)),
            None => selection.or(hover).map(|r| size_label(r.w, r.h)),
        };
        let text = if editor.is_some() {
            String::new()
        } else {
            loupe_label(pixel_at(&session.frames, cursor.0, cursor.1), cursor)
        };
        let (monitor, dpi) = session
            .frames
            .iter()
            .find(|f| f.rect.contains(cursor.0, cursor.1))
            .map_or((sel.desktop(), 96), |f| (f.rect, f.dpi));
        let scene = SnapScene {
            tokens: self.anim.current(),
            cursor,
            selection,
            hover,
            loupe: Loupe::at(cursor, sel.desktop()),
            loupe_text: &text,
            size_text: size.as_deref(),
            monitor,
            scale: dpi as f32 / 96.0,
            edit: editor.map(|e| EditScene {
                editor: e,
                frames: &session.frames,
                size_text: size.as_deref().unwrap_or_default(),
            }),
        };
        let failed = match ov.renderer.as_mut().map(|r| r.render(&scene)) {
            Some(Err(e)) => {
                storage::log(&format!("snap: overlay render: {e}"));
                true
            }
            _ => false,
        };
        if failed {
            self.end_snap(End::Cancel);
        }
    }

    // ---- Timer ----

    /// The timer's digits are on screen: Timer compact or panel, or Music compact with
    /// a timer on it. The alert shows no ticking digits.
    fn timer_digits_on_screen(&self) -> bool {
        self.visible
            && match self.island.state() {
                IslandState::TimerCompact | IslandState::TimerPanel => true,
                IslandState::MusicCompact => self.timer.ui().active,
                _ => false,
            }
    }

    /// Pushes the timer facts into the island, which may change its state.
    fn apply_timer_ui(&mut self) {
        let tokens = self.anim.current().clone();
        self.island.set_timer(self.timer.ui(), &tokens);
        self.dirty = true;
    }

    /// Ends the phase if its deadline has passed: the chime plays and the alert comes
    /// up (or waits for an open panel to collapse). Harmless when called early.
    fn check_timer(&mut self) {
        if self.timer.fire(Instant::now()).is_some() {
            self.apply_timer_ui();
        }
    }

    fn timer_action(&mut self, action: TimerAction) {
        if self.timer.apply(action, Instant::now()) {
            // The digits jumped, so realign their redraw.
            if self.digits_armed {
                window::kill_timer(self.hwnd, TIMER_DIGITS);
                self.digits_armed = false;
            }
            self.apply_timer_ui();
        }
    }

    /// Arms or kills the two one-shot timer messages to match `Timer::plan`. The
    /// deadline stays armed while the island is hidden; the digit redraw does not.
    fn sync_timer(&mut self) {
        let plan = self
            .timer
            .plan(Instant::now(), self.timer_digits_on_screen());
        let deadline = self.timer.deadline();
        if deadline != self.deadline_armed {
            match plan.deadline_ms {
                Some(ms) => window::set_timer(self.hwnd, TIMER_DEADLINE, ms),
                None => window::kill_timer(self.hwnd, TIMER_DEADLINE),
            }
            self.deadline_armed = deadline;
        }
        match plan.digits_ms {
            Some(ms) if !self.digits_armed => {
                window::set_timer(self.hwnd, TIMER_DIGITS, ms);
                self.digits_armed = true;
            }
            None if self.digits_armed => {
                window::kill_timer(self.hwnd, TIMER_DIGITS);
                self.digits_armed = false;
            }
            _ => {}
        }
    }

    /// Fires at the next minute boundary so the clock never needs polling.
    fn arm_clock_timer(&self) {
        let (_, _, sec) = system::local_time();
        window::set_timer(self.hwnd, TIMER_CLOCK, (60 - sec) * 1000 + 30);
    }

    /// Recomputes DPI scale, window size and position; rebuilds the renderer if the size changed.
    fn relayout(&mut self) {
        let mon = window::primary_monitor();
        let scale = mon.dpi as f32 / 96.0;
        let (lw, lh) = layout::window_size();
        let (w, h) = ((lw * scale).ceil() as i32, (lh * scale).ceil() as i32);
        let (x, y) = window::top_center(&mon, w);
        window::place(self.hwnd, x, y, w, h);
        self.win_pos = (x, y);
        if (w, h) != self.win_px || self.renderer.is_none() {
            self.renderer = None; // drop the old swap chain first
            self.win_px = (w, h);
            self.last_rect = None;
            self.renderer = self.create_renderer(w, h);
            self.restore_snap_shot();
        }
        self.scale = scale;
        self.dirty = true;
        // The monitor or its DPI may have changed: the strip's size goes with it, and the
        // preview stack is made again at the new scale under the island.
        self.sync_appbar(true);
        let now = Instant::now();
        self.previews.relayout(self.stack_anchor(), now);
    }

    /// The island itself needs a frame: it moves, crossfades a theme, or changed.
    fn island_due(&self) -> bool {
        self.visible && (self.dirty || self.island.is_animating() || self.anim.animating())
    }

    // ---- Tile ----

    /// Where the island is for a window move, desktop px: the summon area (from the top of
    /// the screen to 80 logical px under the island's current lower edge, its width + 40 on
    /// each side, so an overshoot to the very top keeps the picker) and the Tile panel's
    /// final rect. The picker only opens over a resting island (or its own panel): never
    /// while the island is hidden, a snap is up, or another panel or the thumb is open.
    fn tile_geo(&self) -> IslandGeo {
        let s = self.scale;
        let px = |v: f32| (v * s).round() as i32;
        let pill = self.island.pill_rect();
        let cx = self.win_pos.0 + self.win_px.0 / 2;
        let resting = matches!(
            self.island.state(),
            IslandState::Idle
                | IslandState::MusicCompact
                | IslandState::TimerCompact
                | IslandState::TilePanel
        );
        let summon = if self.visible && self.overlay.is_none() && resting {
            let half = px(pill.w / 2.0 + 40.0);
            IRect::new(
                cx - half,
                self.win_pos.1,
                2 * half,
                px(pill.y + pill.h + 80.0),
            )
        } else {
            IRect::new(0, 0, 0, 0)
        };
        IslandGeo {
            summon,
            panel: IRect::new(
                cx - px(layout::TILE_PANEL_W / 2.0),
                self.win_pos.1 + px(layout::TOP_MARGIN),
                px(layout::TILE_PANEL_W),
                px(layout::TILE_PANEL_H),
            ),
            scale: s,
        }
    }

    /// A window-logical point in the Tile panel's own px (the island's `view_local`).
    fn tile_local(lx: f32, ly: f32) -> (f32, f32) {
        let (win_w, _) = layout::window_size();
        (
            lx - (win_w - layout::TILE_PANEL_W) / 2.0,
            ly - layout::TOP_MARGIN,
        )
    }

    /// Arms a one-shot timer for `want`, leaving one already armed for about that time
    /// alone (re-arming on every event would keep it from firing).
    fn arm_once(
        hwnd: HWND,
        id: usize,
        armed: &mut Option<Instant>,
        want: Option<Instant>,
        now: Instant,
    ) {
        match want {
            Some(t) if armed.is_none_or(|a| a.max(t) - a.min(t) > Duration::from_millis(5)) => {
                let ms = t.saturating_duration_since(now).as_millis() as u32;
                window::set_timer(hwnd, id, ms.saturating_add(1));
                *armed = Some(t);
            }
            None if armed.is_some() => {
                window::kill_timer(hwnd, id);
                *armed = None;
            }
            _ => {}
        }
    }

    /// Keeps the island's Tile panel, the zone overlay and the Tile timers in step with
    /// `tiler`, and writes `[tile]` when a monitor's active layout changed.
    fn sync_tile(&mut self) {
        let now = Instant::now();
        for e in self.tiler.take_errors() {
            storage::log(&e);
        }
        if let Some(tile) = self.tiler.take_config() {
            self.config.tile = tile;
            storage::save_config(&self.config);
        }
        // The cursor is polled only while a window moves.
        let polling = self.tiler.polling();
        if polling != self.tile_poll_armed {
            if polling {
                window::set_timer(self.hwnd, TIMER_TILE_POLL, POLL_MS);
            } else {
                window::kill_timer(self.hwnd, TIMER_TILE_POLL);
            }
            self.tile_poll_armed = polling;
        }
        let (retry, notice) = self.tiler.next_timer(now);
        Self::arm_once(
            self.hwnd,
            TIMER_TILE_RETRY,
            &mut self.tile_retry_at,
            retry.map(|d| now + d),
            now,
        );
        Self::arm_once(
            self.hwnd,
            TIMER_TILE_NOTICE,
            &mut self.tile_notice_at,
            notice.map(|d| now + d),
            now,
        );

        // Something else took the island over (the snap thumb, a hidden island).
        if self.tile_panel && !self.island.tile_open() {
            self.tiler.panel_closed();
            self.tile_panel = false;
        }
        let want = self.tiler.panel_wanted(now) && self.visible && self.overlay.is_none();
        if want && !self.island.tile_open() {
            let tokens = self.anim.current().clone();
            self.island.open_tile(&tokens);
            self.tile_panel = true;
            self.dirty = true;
        } else if !want && self.tile_panel {
            let tokens = self.anim.current().clone();
            self.island.collapse(&tokens);
            self.tile_panel = false;
            // Whatever wanted it is over (or the island was hidden): it must not reopen.
            self.tiler.panel_closed();
            self.dirty = true;
        }
        self.sync_tile_overlay();
    }

    /// The zone overlay: made once per move on the target monitor (a new GPU device each
    /// time, so never per hover), hidden while nothing is lit, redrawn only when the zones
    /// change, gone when the move ends.
    fn sync_tile_overlay(&mut self) {
        if !self.tiler.polling() {
            self.destroy_tile_overlay();
            return;
        }
        let Some(want) = self.tiler.overlay() else {
            if let Some(o) = self.tile_overlay.as_mut().filter(|o| o.shown) {
                dj_win::tile::hide_overlay(o.hwnd);
                o.shown = false;
            }
            return;
        };
        if self
            .tile_overlay
            .as_ref()
            .is_some_and(|o| o.monitor != want.monitor)
        {
            self.destroy_tile_overlay();
        }
        if self.tile_overlay.is_none() {
            let m = want.monitor;
            let hwnd = match dj_win::tile::create_overlay(m.x, m.y, m.w, m.h) {
                Ok(h) => h,
                Err(e) => {
                    storage::log(&format!("tile: cannot create the zone overlay: {e}"));
                    return;
                }
            };
            let surface = match ZoneOverlay::new(hwnd.0 as isize, m.w as u32, m.h as u32) {
                Ok(s) => s,
                Err(e) => {
                    storage::log(&format!("tile: zone overlay renderer: {e}"));
                    dj_win::tile::destroy_overlay(hwnd);
                    return;
                }
            };
            self.tile_overlay = Some(TileOverlay {
                hwnd,
                surface: Some(surface),
                monitor: m,
                shown: false,
                drawn: None,
            });
        }
        let Some(o) = self.tile_overlay.as_mut() else {
            return;
        };
        let key = (want.zones.clone(), want.highlight);
        if o.drawn.as_ref() != Some(&key) {
            let zones: Vec<layout::Rect> = want
                .zones
                .iter()
                .map(|z| {
                    layout::Rect::new(
                        (z.x - o.monitor.x) as f32,
                        (z.y - o.monitor.y) as f32,
                        z.w as f32,
                        z.h as f32,
                    )
                })
                .collect();
            let scene = ZoneScene {
                tokens: self.anim.current(),
                scale: want.scale,
                zones: &zones,
                highlight: want.highlight,
            };
            if let Some(Err(e)) = o.surface.as_mut().map(|s| s.render(&scene)) {
                storage::log(&format!("tile: zone overlay render: {e}"));
            }
            o.drawn = Some(key);
        }
        if !o.shown {
            // Under the island (both topmost), so the zones never cover the Tile panel.
            dj_win::tile::show_overlay_below(o.hwnd, self.hwnd);
            o.shown = true;
        }
    }

    fn destroy_tile_overlay(&mut self) {
        if let Some(mut o) = self.tile_overlay.take() {
            o.surface = None;
            dj_win::tile::destroy_overlay(o.hwnd);
        }
    }

    /// The island's centre and lower edge, desktop px: the Snap preview stack hangs
    /// below it and follows it as panels open and close.
    fn stack_anchor(&self) -> (i32, i32) {
        let pill = self.island.pill_rect();
        (
            self.win_pos.0 + self.win_px.0 / 2,
            self.win_pos.1 + ((pill.y + pill.h) * self.scale).ceil() as i32,
        )
    }

    fn create_renderer(&self, w: i32, h: i32) -> Option<Renderer> {
        match Renderer::new(self.hwnd.0 as isize, w as u32, h as u32) {
            Ok(r) => Some(r),
            Err(e) => {
                storage::log(&format!("renderer: {e}"));
                None
            }
        }
    }

    fn set_theme(&mut self, id: &str) {
        let Some(tokens) = self.registry.get(id).cloned() else {
            return;
        };
        self.theme_id = id.to_string();
        self.anim.retarget(tokens);
        self.config.theme = id.to_string();
        storage::save_config(&self.config);
        self.dirty = true;
    }

    fn cycle_theme(&mut self) {
        let next = self.registry.next_id(&self.theme_id).to_string();
        self.set_theme(&next);
    }

    fn reload_themes(&mut self) {
        self.registry.set_user_themes(storage::read_user_themes());
        match self.registry.get(&self.theme_id).cloned() {
            Some(tokens) => self.anim.retarget(tokens),
            None => {
                self.theme_id = DEFAULT_THEME.to_string();
                if let Some(tokens) = self.registry.get(DEFAULT_THEME).cloned() {
                    self.anim.retarget(tokens);
                }
            }
        }
        self.dirty = true;
    }

    fn notice(&self) -> Option<String> {
        self.registry
            .rejected()
            .first()
            .map(|(id, why)| format!("theme {id} rejected: {why}"))
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        // The island stays hidden while the Snap overlay is up; it comes back after.
        window::set_visible(self.hwnd, visible && self.overlay.is_none());
        if !visible && self.snap_pending && !self.snap_start_armed {
            // Hidden mid-collapse: no frame will see it settle, so start now-ish.
            window::set_timer(self.hwnd, TIMER_SNAP_START, START_DELAY_MS);
            self.snap_start_armed = true;
        }
        self.last_frame = None;
        self.dirty = visible;
        self.sync_vu_timer();
        // A hidden island gives the screen back.
        self.sync_appbar(false);
        // The preview stack hides with the island (its countdowns pause).
        self.previews
            .set_allowed(visible && self.overlay.is_none(), Instant::now());
    }

    // ---- YouTube Music ----

    fn profile_dir() -> PathBuf {
        storage::data_dir().join("ytmusic")
    }

    /// Starts the hidden engine if it is not running. `show` also shows its window.
    fn ensure_engine(&mut self, show: bool) {
        if self.engine.is_none() {
            match Engine::start(&Self::profile_dir(), false) {
                Ok(e) => self.engine = Some(e),
                Err(e) => {
                    storage::log(&format!("music: {e}"));
                    return;
                }
            }
        }
        if let Some(engine) = &self.engine {
            engine.show(show);
        }
    }

    /// Stops the engine. The page goes away with it, so the track goes too.
    fn stop_engine(&mut self) {
        self.engine = None;
        self.session.now = None;
        self.session.bands = None;
        self.session.blocked = false;
        self.sync_art();
        self.apply_music_ui();
    }

    /// Stops the engine, deletes the profile and forgets the sign-in.
    fn sign_out(&mut self) {
        self.stop_engine();
        let dir = Self::profile_dir();
        if dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                storage::log(&format!("music: cannot delete {}: {e}", dir.display()));
            }
        }
        self.config.music.signed_in = false;
        storage::save_config(&self.config);
        self.session.signed_in = false;
        self.apply_music_ui();
        self.dirty = true;
    }

    /// Pushes the Music facts into the island, which may change its state.
    fn apply_music_ui(&mut self) {
        let ui = self.session.ui();
        let tokens = self.anim.current();
        self.island.set_music(ui, tokens);
        self.dirty = true;
    }

    /// Handles everything the engine queued since the last call.
    fn drain_engine(&mut self) {
        let msgs = match self.engine.as_ref() {
            Some(e) => e.poll(),
            None => return,
        };
        for msg in msgs {
            match &msg {
                MusicMsg::Auth { signed_in } => {
                    storage::log(&format!("music: auth signedIn={signed_in}"));
                    if *signed_in {
                        // Signed in: the window has done its job, so hide it and remember.
                        if let Some(engine) = &self.engine {
                            engine.show(false);
                        }
                        if !self.config.music.signed_in {
                            self.config.music.signed_in = true;
                            storage::save_config(&self.config);
                        }
                    }
                }
                MusicMsg::State { .. } => self.state_at = Instant::now(),
                _ => {}
            }
            self.session.apply(msg);
        }
        self.sync_art();
        self.apply_music_ui();
    }

    /// Starts the download when the track's artwork URL changes.
    fn sync_art(&mut self) {
        let url = self.session.now.as_ref().and_then(|n| n.art_url.clone());
        if url == self.art_url {
            return;
        }
        self.art_url = url.clone();
        self.cover = None;
        if let Some(r) = self.renderer.as_mut() {
            r.clear_art();
        }
        if let Some(url) = url {
            art::fetch(url, self.hwnd.0 as isize, self.art_tx.clone());
        }
    }

    /// Decodes finished artwork downloads on the UI thread.
    fn drain_art(&mut self) {
        while let Ok(res) = self.art_rx.try_recv() {
            if self.art_url.as_deref() != Some(res.url.as_str()) {
                continue; // the track changed while this was downloading
            }
            let cover = match (res.bytes, self.renderer.as_mut()) {
                (Some(bytes), Some(r)) => r.set_art(bytes.as_slice()),
                _ => None,
            };
            self.cover = cover;
            self.dirty = true;
        }
    }

    /// Runs one of the expanded view's controls.
    fn music_action(&mut self, action: MusicAction) {
        match action {
            MusicAction::PlayPause => self.exec_music("toggle()"),
            MusicAction::Prev => self.exec_music("prev()"),
            MusicAction::Next => self.exec_music("next()"),
            MusicAction::Seek(fraction) => {
                let dur = self.session.now.as_ref().map_or(0.0, |n| n.dur);
                self.exec_music(&format!("seek({:.1})", fraction * dur));
            }
            MusicAction::OpenWindow | MusicAction::SignIn => self.ensure_engine(true),
            MusicAction::OpenBrowser => {
                if !system::open_url(MUSIC_URL) {
                    storage::log("music: cannot open the browser");
                }
            }
        }
    }

    fn exec_music(&self, call: &str) {
        if let Some(engine) = &self.engine {
            engine.exec(&format!("window.__dj && window.__dj.{call}"));
        }
    }

    /// Opens the Music view from the Home grid. Signed-in users get the engine started.
    fn open_music_tile(&mut self) {
        if self.engine.is_none() && self.session.signed_in {
            self.ensure_engine(false);
        }
        let tokens = self.anim.current().clone();
        self.island.open_music(&tokens);
    }

    /// Playback position, moved along between state messages while playing.
    fn playback_pos(&self) -> f32 {
        match &self.session.now {
            Some(n) => {
                let live = if n.playing {
                    self.state_at.elapsed().as_secs_f32()
                } else {
                    0.0
                };
                let pos = n.pos + live;
                if n.dur > 0.0 {
                    pos.min(n.dur)
                } else {
                    pos
                }
            }
            None => 0.0,
        }
    }

    /// The VU redraws on a timer at about 30 fps, but only while it has something to show.
    fn sync_vu_timer(&mut self) {
        let in_music = matches!(
            self.island.state(),
            IslandState::MusicCompact | IslandState::MusicExpanded
        );
        let want = self.visible && in_music && self.session.vu_live();
        if want && !self.vu_armed {
            window::set_timer(self.hwnd, TIMER_VU, VU_MS);
            self.vu_armed = true;
        } else if !want && self.vu_armed {
            window::kill_timer(self.hwnd, TIMER_VU);
            self.vu_armed = false;
        }
    }

    fn accent(&self) -> Rgb {
        let t = self.anim.current();
        match self.cover {
            Some(c) => cover_accent(c, t.accent, t.accent_from_cover),
            None => t.accent,
        }
    }

    fn show_tray_menu(&mut self) {
        let themes: Vec<(String, bool)> = self
            .registry
            .ids()
            .iter()
            .map(|id| {
                (
                    self.registry.name_of(id).unwrap_or(id).to_string(),
                    *id == self.theme_id,
                )
            })
            .collect();
        let spec = MenuSpec {
            themes: &themes,
            autostart: system::autostart_enabled(),
            music: MusicMenu {
                running: self.engine.is_some(),
                signed_in: self.session.signed_in,
            },
            previews: self.previews.can_focus(),
        };
        match tray::show_menu(self.hwnd, &spec) {
            Some(MenuCommand::ToggleVisible) => self.set_visible(!self.visible),
            Some(MenuCommand::Theme(i)) => {
                if let Some(id) = self.registry.ids().get(i).cloned() {
                    self.set_theme(&id);
                }
            }
            Some(MenuCommand::ToggleAutostart) => {
                let want = !system::autostart_enabled();
                if system::set_autostart(want, &self.exe) {
                    self.config.start_with_windows = want;
                    storage::save_config(&self.config);
                }
            }
            Some(MenuCommand::MusicShow) => self.ensure_engine(true),
            Some(MenuCommand::MusicSignOut) => self.sign_out(),
            Some(MenuCommand::MusicStop) => self.stop_engine(),
            Some(MenuCommand::SnapPreviews) => self.previews.focus_keyboard(Instant::now()),
            Some(MenuCommand::Quit) => self.quit(),
            None => {}
        }
    }

    fn quit(&mut self) {
        // Give the strip back first, or the work area stays short after we're gone.
        appbar::release(self.hwnd);
        self.reserved = None;
        self.end_snap(End::Cancel);
        self.previews.shutdown();
        dj_win::tile::uninstall_move_hook();
        self.destroy_tile_overlay();
        for (id, _, _) in TILE_KEYS {
            window::unregister_hotkey(self.hwnd, id);
        }
        window::unregister_hotkey(self.hwnd, HOTKEY_SNAP);
        window::unregister_hotkey(self.hwnd, HOTKEY_SNAP_ALT);
        self.engine = None;
        hook::uninstall();
        self.mixer = None; // unregisters the audio callbacks
        tray::remove(self.hwnd);
        window::unregister_hotkey(self.hwnd, HOTKEY_TOGGLE);
        window::unregister_hotkey(self.hwnd, HOTKEY_THEME);
        window::quit();
    }

    /// Device-px edges of the island's column: the pill, or in notch style the pill
    /// stretched up to the window's top edge (`set_region` adds the shoulders).
    fn device_rect(&self) -> Edges {
        let body = notch::body_rect(self.config.island.style(), self.island.pill_rect());
        notch::device_edges(body, self.scale)
    }

    /// A window device-px point in window-logical px, as the island sees it: in notch
    /// style the neck and the shoulders act as the nearest point of the pill.
    fn island_point(&self, x: i32, y: i32) -> (f32, f32) {
        let (lx, ly) = (x as f32 / self.scale, y as f32 / self.scale);
        notch::to_pill(self.config.island.style(), self.island.pill_rect(), lx, ly)
    }

    /// Clips the window to the island's column (`device_rect`), plus in notch style the
    /// shoulders' strip along the top edge.
    fn set_region(&self, column: Edges) {
        let pad = |(l, t, r, b): Edges| {
            (
                (l - REGION_PAD).max(0),
                (t - REGION_PAD).max(0),
                (r + REGION_PAD).min(self.win_px.0),
                (b + REGION_PAD).min(self.win_px.1),
            )
        };
        match self.config.island.style() {
            IslandStyle::Pill => window::set_hit_rects(self.hwnd, &[pad(column)]),
            IslandStyle::Notch => window::set_hit_rects(
                self.hwnd,
                &[pad(column), pad(notch::shoulder_strip(column, self.scale))],
            ),
        }
    }
}

impl Handler for App {
    fn event(&mut self, ev: Event) {
        match ev {
            Event::Click { x, y } => {
                let (lx, ly) = self.island_point(x, y);
                let tokens = self.anim.current().clone();
                let outcome = self.island.click(lx, ly, &tokens);
                if outcome != ClickOutcome::Ignored {
                    self.dirty = true;
                }
                match outcome {
                    ClickOutcome::Open(i) if FEATURES[i].id == "music" => self.open_music_tile(),
                    ClickOutcome::Music(action) => self.music_action(action),
                    ClickOutcome::SetVolume(level) => self.set_level(level),
                    // The OSD tile is a manual peek at the current volume.
                    ClickOutcome::Open(i) if FEATURES[i].id == "osd" => self.show_volume(),
                    ClickOutcome::Open(i) if FEATURES[i].id == "vitals" => {
                        self.island.open_vitals(&tokens);
                    }
                    ClickOutcome::Open(i) if FEATURES[i].id == "timer" => {
                        self.island.open_timer(&tokens);
                    }
                    ClickOutcome::Open(i) if FEATURES[i].id == "snap" => self.request_snap(),
                    // The panel is for the app in front (the island never takes focus).
                    ClickOutcome::Open(i) if FEATURES[i].id == "tile" => {
                        self.tiler.open_home();
                        self.island.open_tile(&tokens);
                        self.tile_panel = true;
                    }
                    // A cell places that window; anywhere else just closes the panel.
                    ClickOutcome::Tile { x, y } => {
                        self.tiler.panel_click(x, y, Instant::now());
                        self.island.collapse(&tokens);
                        self.tile_panel = false;
                    }
                    ClickOutcome::Snap(action) => self.snap_action(action),
                    ClickOutcome::Timer(action) => self.timer_action(action),
                    ClickOutcome::Settings(action) => self.settings_action(action),
                    // A clean that is still running keeps its view; otherwise preview afresh.
                    ClickOutcome::OpenSweep if !self.cleaning => self.start_preview(),
                    ClickOutcome::Sweep { x, y } => {
                        if let Some(cmd) = self.sweep.click(x, y) {
                            self.sweep_cmd(cmd);
                        }
                    }
                    _ => {}
                }
            }
            Event::Move { x, y } => {
                let (lx, ly) = self.island_point(x, y);
                if self.island.hover(lx, ly) {
                    self.dirty = true;
                }
                if self.island.tile_open() {
                    let (px, py) = Self::tile_local(lx, ly);
                    if self.tiler.home_hover(px, py) {
                        self.dirty = true;
                    }
                }
            }
            Event::Drag { x, y } => {
                let (lx, ly) = self.island_point(x, y);
                match self.island.drag(lx) {
                    Some(level) => {
                        self.set_level(level);
                        self.dirty = true;
                    }
                    None => {
                        if self.island.hover(lx, ly) {
                            self.dirty = true;
                        }
                    }
                }
            }
            Event::Release => self.island.release(),
            Event::VolumeChanged => self.show_volume(),
            Event::DefaultDeviceChanged => {
                if let Some(m) = self.mixer.as_mut() {
                    if let Err(e) = m.reacquire() {
                        storage::log(&format!("volume endpoint: {e}"));
                    }
                }
            }
            Event::VolumeKey(vk) => {
                if let Some(m) = self.mixer.as_ref() {
                    match vk {
                        VK_VOLUME_UP => m.step(true),
                        VK_VOLUME_DOWN => m.step(false),
                        VK_VOLUME_MUTE => m.toggle_mute(),
                        _ => {}
                    }
                }
                self.show_volume();
            }
            Event::Leave => {
                if self.island.clear_hover() | self.tiler.clear_hover() {
                    self.dirty = true;
                }
            }
            Event::Hotkey(HOTKEY_TOGGLE) => self.set_visible(!self.visible),
            Event::Hotkey(HOTKEY_THEME) => self.cycle_theme(),
            Event::Hotkey(HOTKEY_SNAP | HOTKEY_SNAP_ALT) | Event::PrintScreen => {
                self.request_snap()
            }
            Event::Hotkey(HOTKEY_SNAP_ESC) => self.snap_escape(),
            Event::Hotkey(id @ HOTKEY_ZONE_1..=HOTKEY_DOWN) => {
                self.tiler.hotkey(id, Instant::now());
                // An open panel may now carry the admin notice.
                self.dirty = true;
            }
            Event::Hotkey(_) => {}
            Event::Snap(input) => self.snap_input(input),
            Event::Previews(input) => self.previews.input(input, Instant::now()),
            Event::Assist(_) => {}
            Event::TrayLeft => self.set_visible(!self.visible),
            Event::TrayRight => self.show_tray_menu(),
            Event::DpiChanged => self.relayout(),
            Event::DisplayChange => {
                // The monitors changed under the frozen frames: drop the snap.
                self.end_snap(End::Cancel);
                self.relayout();
                self.tiler.refresh_monitors();
            }
            Event::ThemesChanged => self.reload_themes(),
            Event::Timer(TIMER_CLOCK) => {
                self.arm_clock_timer();
                self.dirty = true;
            }
            Event::Timer(TIMER_VU) => self.dirty = true,
            Event::Timer(vitals::TIMER) => self.worker.tick(),
            Event::Timer(TIMER_SWEEP_DONE) => {
                window::kill_timer(self.hwnd, TIMER_SWEEP_DONE);
                let done = matches!(self.sweep.phase, Phase::Done { .. });
                if done && self.island.state() == IslandState::Sweep {
                    self.back_to_vitals();
                }
            }
            // Both are one-shot: SetTimer repeats, so each is killed when it fires and
            // `sync_timer` arms it again if it is still wanted.
            Event::Timer(TIMER_DEADLINE) => {
                window::kill_timer(self.hwnd, TIMER_DEADLINE);
                self.deadline_armed = None;
                self.check_timer();
            }
            Event::Timer(TIMER_DIGITS) => {
                window::kill_timer(self.hwnd, TIMER_DIGITS);
                self.digits_armed = false;
                self.dirty = true;
                self.check_timer();
            }
            Event::Timer(TIMER_SNAP_START) => {
                window::kill_timer(self.hwnd, TIMER_SNAP_START);
                self.snap_start_armed = false;
                if self.snap_pending {
                    self.begin_snap();
                }
            }
            Event::Timer(TIMER_SNAP_THUMB) => {
                window::kill_timer(self.hwnd, TIMER_SNAP_THUMB);
                self.thumb_armed = false;
                // `sync_snap` re-arms it if it fired early or the thumb is hovered.
                if self.snapper.expired(Instant::now()) {
                    self.snapper.dismiss();
                    let tokens = self.anim.current().clone();
                    self.island.hide_snap(&tokens);
                    self.dirty = true;
                }
            }
            Event::Timer(TIMER_SNAP_STACK) => self.previews.timer(Instant::now()),
            Event::Timer(TIMER_TILE_POLL) => {
                let geo = self.tile_geo();
                if self.tiler.poll(geo, Instant::now()) {
                    self.dirty = true;
                }
            }
            Event::Timer(TIMER_TILE_RETRY) => {
                window::kill_timer(self.hwnd, TIMER_TILE_RETRY);
                self.tile_retry_at = None;
                self.tiler.retry(Instant::now());
            }
            Event::Timer(TIMER_TILE_NOTICE) => {
                window::kill_timer(self.hwnd, TIMER_TILE_NOTICE);
                self.tile_notice_at = None;
                self.tiler.notice_done(Instant::now());
                self.dirty = true;
            }
            Event::Timer(_) => {}
            Event::Music => self.drain_engine(),
            Event::Wake => {
                self.drain_art();
                // A card saved the thumb's shot: the thumb reads "open" too.
                let saved = self.previews.drain(Instant::now());
                if !saved.is_empty() {
                    for (bitmap, path) in &saved {
                        self.snapper.mark_saved(bitmap, path);
                    }
                    self.update_snap_frame();
                    self.dirty = true;
                }
            }
            Event::TaskbarCreated => {
                if let Some(icon) = self.icon {
                    tray::add(self.hwnd, icon, "Daijobu Island");
                }
                // Explorer restarted and forgot every AppBar: register again.
                appbar::forget();
                self.sync_appbar(true);
            }
            Event::AppBarMoved => self.sync_appbar(true),
            Event::TileMove(MoveEvent::Start(w)) => self.tiler.move_start(w),
            Event::TileMove(MoveEvent::End(w)) => {
                let geo = self.tile_geo();
                self.tiler.move_end(w, geo, Instant::now());
                self.dirty = true;
            }
            Event::VitalsReady => self.drain_vitals(),
            Event::SweepReady => self.drain_sweep(),
        }
        self.sync_vu_timer();
        self.sync_vitals();
        self.sync_sweep();
        self.sync_timer();
        self.sync_snap();
        self.sync_tile();
    }

    fn wants_frame(&self) -> bool {
        // While a snap is up, only the overlay draws, and only after input.
        if let Some(ov) = &self.overlay {
            return ov.dirty;
        }
        self.island_due() || self.previews.wants_frame()
    }

    fn frame(&mut self) {
        if self.overlay.is_some() {
            self.render_overlay();
            return;
        }
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(FIRST_DT, |t| (now - t).as_secs_f32().min(MAX_DT));
        self.last_frame = Some(now);
        let island_due = self.island_due();
        let theming = self.anim.animating();
        self.anim.step(dt);
        self.island.step(dt, self.anim.current());
        // The preview stack follows the island's edge and its theme, and draws only when
        // it has something new (a ring tick or a spring can come without the island).
        if theming {
            self.previews.set_tokens(self.anim.current());
        }
        self.previews.set_anchor(self.stack_anchor(), now);
        self.previews.frame(dt, now);
        if !island_due {
            if !self.wants_frame() {
                self.last_frame = None;
            }
            return;
        }
        self.dirty = false;

        // The region must cover both this frame's body and the previous one, or the
        // compositor can clip a shrinking pill (or a style switch) for a frame.
        let rect = self.device_rect();
        let region = self.last_rect.map_or(rect, |p| {
            (
                p.0.min(rect.0),
                p.1.min(rect.1),
                p.2.max(rect.2),
                p.3.max(rect.3),
            )
        });
        self.set_region(region);
        self.last_rect = Some(rect);

        if self.renderer.is_none() {
            self.renderer = self.create_renderer(self.win_px.0, self.win_px.1);
            self.restore_snap_shot();
        }
        let view = self.island.view();
        let (h, m, _) = system::local_time();
        let notice = self.notice();
        let accent = self.accent();
        let pos = self.playback_pos();
        let music = MusicFrame {
            ui: self.island.music_ui(),
            now: self.session.now.as_ref(),
            bands: self.session.bands.as_deref(),
            accent,
            pos,
        };
        let estimate = self.estimate();
        let timer = self.timer.view(now);
        let settings = self.settings_view();
        // The Tile panel's content, only while its layer shows.
        let tile = (view.layers.tile > 0.0).then(|| self.tiler.panel(now));
        let tile_thumbs: Vec<TileThumb> = tile.as_ref().map_or_else(Vec::new, |p| {
            p.thumbs
                .iter()
                .map(|t| TileThumb {
                    rect: t.rect,
                    cells: &t.cells,
                    active: t.layout == p.active,
                    hover: p.hover.filter(|&(l, _)| l == t.layout).map(|(_, z)| z),
                })
                .collect()
        });
        let tile_panel = tile.as_ref().map(|p| TilePanel {
            title: p.title,
            monitor: &p.monitor,
            thumbs: &tile_thumbs,
            footer: Some(p.footer),
            header: dj_core::tile::PANEL_HEADER,
            footer_rect: dj_core::tile::PANEL_FOOTER,
        });
        let Some(renderer) = self.renderer.as_mut() else {
            self.last_frame = None;
            return;
        };
        let frame = Frame {
            tokens: self.anim.current(),
            view: &view,
            scale: self.scale,
            clock: (h, m),
            time: self.started.elapsed().as_secs_f32(),
            notice: notice.as_deref(),
            music,
            vitals: &self.vitals,
            health_lamp: self.config.vitals.health_lamp,
            sweep: &self.sweep,
            estimate,
            timer: &timer,
            snap: self.snap_frame,
            settings: &settings,
            tile: tile_panel,
            island_style: self.config.island.style(),
        };
        match renderer.render(&frame) {
            Ok(()) => {}
            Err(RenderError::DeviceLost) => {
                storage::log("GPU device lost; rebuilding renderer");
                self.renderer = None;
                self.dirty = true;
            }
            Err(e) => {
                storage::log(&format!("render: {e}"));
                self.last_frame = None;
            }
        }
        if !self.island_due() {
            // Settled: one final tight region, and the next animation restarts its clock.
            self.set_region(rect);
            if !self.wants_frame() {
                self.last_frame = None;
            }
        }
        // The thumb has faded out: drop its small copy of the shot.
        if self.snap_frame.is_some() && !self.island.snap_shown() && view.layers.snap <= 0.0 {
            self.snap_frame = None;
            if let Some(r) = self.renderer.as_mut() {
                r.clear_snap_shot();
            }
        }
        // A snap waits for the collapse: freeze shortly after the island is still.
        if self.snap_pending && !self.snap_start_armed && !self.island.is_animating() {
            window::set_timer(self.hwnd, TIMER_SNAP_START, START_DELAY_MS);
            self.snap_start_armed = true;
        }
    }
}
