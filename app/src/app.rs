//! Application state: glues the island model, themes, the Music engine, renderer and
//! Win32 events.

use crate::storage;
use crate::sweep::{Job, Msg, ScanId, Sweeper};
use crate::vitals::{self, Cadence, Worker};
use dj_core::color::Rgb;
use dj_core::config::Config;
use dj_core::features::FEATURES;
use dj_core::island::{ClickOutcome, Island, IslandState, MusicAction};
use dj_core::layout;
use dj_core::music::{cover_accent, MusicMsg, Session};
use dj_core::osd::Volume;
use dj_core::sweep::{Estimate, Phase, SweepCache, SweepCmd, SweepView};
use dj_core::theme::{ThemeAnim, ThemeRegistry, DEFAULT_THEME};
use dj_core::vitals::Vitals;
use dj_music::art::{self, ArtResult};
use dj_music::{Engine, MUSIC_URL};
use dj_render::{Frame, MusicFrame, RenderError, Renderer};
use dj_win::tray::{self, MenuCommand, MenuSpec, MusicMenu};
use dj_win::{hook, system, volume, window, Event, Handler, HICON, HWND};
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

type IRect = (i32, i32, i32, i32);

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
    last_rect: Option<IRect>,
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
}

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
        let profile = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_default();
        let sweeper = Sweeper::start(
            hwnd,
            config.sweep.roots(&profile),
            storage::operations_log_path(),
        );
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
        if self.config.osd.replace_volume_flyout
            && self.mixer.is_some()
            && !hook::install(self.hwnd)
        {
            storage::log("cannot install the volume key hook");
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
                Msg::Progress { done, total } => self.sweep.clean_progress(done, total),
                Msg::Cleaned { freed, failed, gone } => {
                    self.cleaning = false;
                    self.sweep.finish_clean(freed, failed, &gone);
                    self.store_estimate();
                    // The disk ring and the score move at once.
                    self.worker.sample_now();
                    if self.island.state() == IslandState::Sweep {
                        window::set_timer(self.hwnd, TIMER_SWEEP_DONE, SWEEP_DONE_MS);
                    }
                }
                Msg::Recycled { path, ok } => {
                    self.cleaning = false;
                    if ok {
                        self.sweep.forget(&path);
                        self.sweep.status = Some("moved to the recycle bin".into());
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
        if (w, h) != self.win_px || self.renderer.is_none() {
            self.renderer = None; // drop the old swap chain first
            self.win_px = (w, h);
            self.last_rect = None;
            self.renderer = self.create_renderer(w, h);
        }
        self.scale = scale;
        self.dirty = true;
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
        window::set_visible(self.hwnd, visible);
        self.last_frame = None;
        self.dirty = visible;
        self.sync_vu_timer();
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
            Some(MenuCommand::Quit) => self.quit(),
            None => {}
        }
    }

    fn quit(&mut self) {
        self.engine = None;
        hook::uninstall();
        self.mixer = None; // unregisters the audio callbacks
        tray::remove(self.hwnd);
        window::unregister_hotkey(self.hwnd, HOTKEY_TOGGLE);
        window::unregister_hotkey(self.hwnd, HOTKEY_THEME);
        window::quit();
    }

    fn device_rect(&self) -> IRect {
        let r = self.island.view().rect;
        let s = self.scale;
        (
            (r.x * s).round() as i32,
            (r.y * s).round() as i32,
            ((r.x + r.w) * s).round() as i32,
            ((r.y + r.h) * s).round() as i32,
        )
    }

    fn set_region(&self, (l, t, r, b): IRect) {
        window::set_hit_region(
            self.hwnd,
            (l - REGION_PAD).max(0),
            (t - REGION_PAD).max(0),
            (r + REGION_PAD).min(self.win_px.0),
            (b + REGION_PAD).min(self.win_px.1),
        );
    }
}

impl Handler for App {
    fn event(&mut self, ev: Event) {
        match ev {
            Event::Click { x, y } => {
                let (lx, ly) = (x as f32 / self.scale, y as f32 / self.scale);
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
                if self
                    .island
                    .hover(x as f32 / self.scale, y as f32 / self.scale)
                {
                    self.dirty = true;
                }
            }
            Event::Drag { x, y } => {
                let (lx, ly) = (x as f32 / self.scale, y as f32 / self.scale);
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
                if self.island.clear_hover() {
                    self.dirty = true;
                }
            }
            Event::Hotkey(HOTKEY_TOGGLE) => self.set_visible(!self.visible),
            Event::Hotkey(HOTKEY_THEME) => self.cycle_theme(),
            Event::Hotkey(_) => {}
            Event::TrayLeft => self.set_visible(!self.visible),
            Event::TrayRight => self.show_tray_menu(),
            Event::DpiChanged | Event::DisplayChange => self.relayout(),
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
            Event::Timer(_) => {}
            Event::Music => self.drain_engine(),
            Event::Wake => self.drain_art(),
            Event::TaskbarCreated => {
                if let Some(icon) = self.icon {
                    tray::add(self.hwnd, icon, "Daijobu Island");
                }
            }
            Event::VitalsReady => self.drain_vitals(),
            Event::SweepReady => self.drain_sweep(),
        }
        self.sync_vu_timer();
        self.sync_vitals();
        self.sync_sweep();
    }

    fn wants_frame(&self) -> bool {
        self.visible && (self.dirty || self.island.is_animating() || self.anim.animating())
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(FIRST_DT, |t| (now - t).as_secs_f32().min(MAX_DT));
        self.last_frame = Some(now);
        self.anim.step(dt);
        self.island.step(dt, self.anim.current());
        self.dirty = false;

        // The region must cover both this frame's pill and the previous one, or the
        // compositor can clip a shrinking pill for a frame.
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
        if !self.wants_frame() {
            // Settled: one final tight region, and the next animation restarts its clock.
            self.set_region(rect);
            self.last_frame = None;
        }
    }
}
