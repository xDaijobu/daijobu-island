//! Tile: putting other apps' windows into layout zones. `dj_core::tile` has the layouts,
//! the zone math and the drag state machine; `dj_win::tile` the Win32 calls; this module
//! decides what happens on a window move, a hotkey and a click in the island's Tile
//! panel. The windows sit behind [`WindowOps`], so it runs in unit tests with a fake.
//!
//! The app syncs the island and the zone overlay from what this reports
//! (`panel_wanted`, `panel`, `overlay`) and arms the timers it asks for (`polling`,
//! `next_timer`). Nothing runs between moves: the move hook is the only always-on part.

use dj_core::config::TileConfig;
use dj_core::snap::IRect;
use dj_core::tile::{self, Dir, Drag, DragView, Geometry, Layout, Monitor, Placed, Target, Thumb};
use dj_win::tile::{MonitorInfo, PlaceError, Raw, WinInfo};
use std::time::{Duration, Instant};

/// About 60 Hz, only while a window is being moved.
pub const TIMER_TILE_POLL: usize = 10;
pub const POLL_MS: u32 = 16;
/// One-shot: re-check a window just placed (it may move itself back).
pub const TIMER_TILE_RETRY: usize = 11;
/// One-shot: the admin-window notice has been up long enough.
pub const TIMER_TILE_NOTICE: usize = 12;
/// Ctrl+Alt+1..9 are hotkey ids 6..=14, Ctrl+Alt+arrows 15..=18 (the app has 1..=5).
pub const HOTKEY_ZONE_1: i32 = 6;
pub const HOTKEY_ZONE_9: i32 = 14;
pub const HOTKEY_LEFT: i32 = 15;
pub const HOTKEY_RIGHT: i32 = 16;
pub const HOTKEY_UP: i32 = 17;
pub const HOTKEY_DOWN: i32 = 18;

/// Re-checks after a placement, from the placement.
const RETRY_AT: [Duration; 3] = [
    Duration::from_millis(150),
    Duration::from_millis(300),
    Duration::from_millis(500),
];
/// A frame this far off its zone (any edge) is placed again.
const DRIFT: i32 = 4;
/// A frame within this of a zone counts as being in it (for the arrows).
const IN_ZONE: i32 = 8;
const NOTICE: Duration = Duration::from_secs(2);
pub const ADMIN_NOTICE: &str = "can't move an admin window";
const NO_WINDOW: &str = "click a window first, then the tile";
const HINT: &str = "shift+drag · ctrl+alt+1‥9";

/// The windows: queries and placement. The real one is [`SystemWindows`].
pub trait WindowOps {
    fn info(&self, w: Raw) -> Option<WinInfo>;
    fn place(&mut self, w: Raw, r: IRect) -> Result<(), PlaceError>;
    /// Un-maximizes a maximized window (so it can be measured before placing).
    fn restore(&mut self, w: Raw);
    fn foreground(&self) -> Raw;
    fn monitors(&self) -> Vec<MonitorInfo>;
    fn cursor(&self) -> (i32, i32);
    fn shift_down(&self) -> bool;
    /// The primary mouse button is down (still held at the end of a move = Esc cancelled).
    fn button_down(&self) -> bool;
    /// A move/size that starts with the cursor at `at` grabs a sizing border.
    fn is_resize(&self, w: Raw, at: (i32, i32)) -> bool;
    /// The window's thread is still in its move/size loop.
    fn still_moving(&self, w: Raw) -> bool;
}

pub struct SystemWindows;

impl WindowOps for SystemWindows {
    fn info(&self, w: Raw) -> Option<WinInfo> {
        dj_win::tile::window_info(w)
    }

    fn place(&mut self, w: Raw, r: IRect) -> Result<(), PlaceError> {
        dj_win::tile::place(w, r.x, r.y, r.w, r.h)
    }

    fn restore(&mut self, w: Raw) {
        dj_win::tile::restore(w);
    }

    fn button_down(&self) -> bool {
        dj_win::tile::primary_button_down()
    }

    fn is_resize(&self, w: Raw, at: (i32, i32)) -> bool {
        dj_win::tile::is_resize_at(w, at.0, at.1)
    }

    fn still_moving(&self, w: Raw) -> bool {
        dj_win::tile::in_move_size(w)
    }

    fn foreground(&self) -> Raw {
        dj_win::tile::foreground()
    }

    fn monitors(&self) -> Vec<MonitorInfo> {
        dj_win::tile::monitors()
    }

    fn cursor(&self) -> (i32, i32) {
        dj_win::overlay::cursor_pos()
    }

    fn shift_down(&self) -> bool {
        dj_win::tile::shift_down()
    }
}

fn irect((x, y, w, h): (i32, i32, i32, i32)) -> IRect {
    IRect::new(x, y, w, h)
}

/// Where the island is during a move, desktop px: the summon area under its current lower
/// edge, the open panel's final rect, and the island's scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IslandGeo {
    pub summon: IRect,
    pub panel: IRect,
    pub scale: f32,
}

/// The zone overlay to show: on `monitor` (desktop px) at its `scale`, `zones` in desktop
/// px, one of them lit.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    pub monitor: IRect,
    pub scale: f32,
    pub zones: Vec<IRect>,
    pub highlight: Option<usize>,
}

/// What the island's Tile panel shows.
pub struct Panel<'a> {
    pub title: &'static str,
    pub thumbs: &'a [Thumb],
    /// Index into `thumbs`' layouts: the island monitor's active layout.
    pub active: usize,
    pub hover: Option<(usize, usize)>,
    pub monitor: String,
    /// The footer line and whether it is a warning.
    pub footer: (&'static str, bool),
}

/// A window being moved (never resized: a resize is not followed at all).
struct Moving {
    drag: Drag,
    w: Raw,
    /// Its rect when the move started, for restore size.
    start: IRect,
}

/// A window just placed, checked again at `RETRY_AT` from `at`.
struct Retry {
    w: Raw,
    zone: IRect,
    at: Instant,
    step: usize,
}

pub struct Tiler {
    ops: Box<dyn WindowOps>,
    /// The picker's layouts (presets, then up to three custom ones) and their thumbnails.
    layouts: Vec<Layout>,
    thumbs: Vec<Thumb>,
    config: TileConfig,
    config_dirty: bool,
    monitors: Vec<Monitor>,
    placed: Placed,
    moving: Option<Moving>,
    /// What the last poll of the current move showed.
    view: DragView,
    retry: Option<Retry>,
    notice_until: Option<Instant>,
    /// The Home tile opened the panel for this window (the foreground app at the click).
    home: Option<Raw>,
    home_hover: Option<(usize, usize)>,
    errors: Vec<String>,
}

impl Tiler {
    /// `custom` is the user's `layouts.toml`, already parsed.
    pub fn new(ops: Box<dyn WindowOps>, config: TileConfig, custom: &[Layout]) -> Self {
        let layouts = tile::picker_layouts(custom);
        let thumbs = tile::panel_thumbs(&layouts);
        let mut t = Self {
            ops,
            layouts,
            thumbs,
            config,
            config_dirty: false,
            monitors: Vec::new(),
            placed: Placed::default(),
            moving: None,
            view: DragView::default(),
            retry: None,
            notice_until: None,
            home: None,
            home_hover: None,
            errors: Vec::new(),
        };
        t.refresh_monitors();
        t
    }

    /// The monitors may have changed (a display change, or a move or hotkey starting).
    pub fn refresh_monitors(&mut self) {
        self.monitors = self
            .ops
            .monitors()
            .into_iter()
            .map(|m| Monitor {
                id: m.device,
                rect: irect(m.rect),
                work: irect(m.work),
                scale: m.dpi.max(96) as f32 / 96.0,
            })
            .collect();
    }

    /// The island sits on the primary monitor, which `monitors()` lists first.
    fn island_monitor(&self) -> usize {
        0
    }

    fn active(&self) -> Vec<usize> {
        tile::active_layouts(&self.monitors, &self.config.monitors, &self.layouts)
    }

    fn gap_px(&self, m: &Monitor) -> i32 {
        (self.config.gap * m.scale).round() as i32
    }

    fn zones_on(&self, monitor: usize, layout: usize) -> Vec<IRect> {
        match (self.monitors.get(monitor), self.layouts.get(layout)) {
            (Some(m), Some(l)) => tile::zones(l, m.work, self.gap_px(m)),
            _ => Vec::new(),
        }
    }

    /// The monitor holding most of `frame` (its centre), else the nearest one.
    fn monitor_of(&self, frame: IRect) -> Option<usize> {
        let (cx, cy) = (frame.x + frame.w / 2, frame.y + frame.h / 2);
        self.monitors
            .iter()
            .position(|m| m.rect.contains(cx, cy))
            .or_else(|| {
                self.monitors
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, m)| {
                        let dx = (m.rect.x - cx).max(cx - m.rect.right()).max(0) as i64;
                        let dy = (m.rect.y - cy).max(cy - m.rect.bottom()).max(0) as i64;
                        dx * dx + dy * dy
                    })
                    .map(|(i, _)| i)
            })
    }

    // ---- moves ----

    /// `EVENT_SYSTEM_MOVESIZESTART`: a window started moving or resizing. Only a move is
    /// followed; the start is decided by what the cursor grabbed, because a maximized or
    /// snapped window changes size as soon as it is dragged.
    pub fn move_start(&mut self, w: Raw) {
        self.moving = None;
        self.view = DragView::default();
        // A drag supersedes the Home panel, and a window grabbed again isn't pulled back.
        self.home = None;
        self.home_hover = None;
        if self.retry.as_ref().is_some_and(|r| r.w == w) {
            self.retry = None;
        }
        let Some(info) = self.ops.info(w).filter(|i| i.eligible) else {
            return;
        };
        if self.ops.is_resize(w, self.ops.cursor()) {
            // The user picks this size: no restore later.
            self.placed.forget(w as u64);
            return;
        }
        self.refresh_monitors();
        self.moving = Some(Moving {
            drag: Drag::new(w as u64),
            w,
            start: irect(info.rect),
        });
    }

    /// A move is in progress: the island should poll.
    pub fn polling(&self) -> bool {
        self.moving.is_some()
    }

    /// One cursor sample during a move (`TIMER_TILE_POLL`). True if what the island or
    /// the overlay shows changed. If the window's thread has left its move loop without
    /// an end event, the move ends here.
    pub fn poll(&mut self, geo: IslandGeo, now: Instant) -> bool {
        let before = self.view;
        match self.moving.as_ref().map(|m| m.w) {
            Some(w) if !self.ops.still_moving(w) => self.move_end(w, geo, now),
            _ => self.sample(geo),
        }
        self.view != before
    }

    fn sample(&mut self, geo: IslandGeo) {
        let Some(mv) = self.moving.as_ref() else {
            return;
        };
        // The window went away mid-move: drop it quietly.
        if self.ops.info(mv.w).is_none() {
            self.placed.forget(mv.w as u64);
            self.moving = None;
            self.view = DragView::default();
            return;
        }
        let cursor = self.ops.cursor();
        let shift = self.ops.shift_down();
        let active = self.active();
        let geometry = Geometry {
            monitors: &self.monitors,
            layouts: &self.layouts,
            active: &active,
            island_monitor: self.island_monitor(),
            summon: geo.summon,
            panel: geo.panel,
            panel_scale: geo.scale,
            gap: self.config.gap,
        };
        let view = match self.moving.as_mut() {
            Some(mv) => mv.drag.poll(cursor, shift, false, &geometry),
            None => return,
        };
        self.view = view;
    }

    /// `EVENT_SYSTEM_MOVESIZEEND`: the window lands in the zone under the cursor, or gets
    /// its old size back if it was in a zone and was dragged out.
    pub fn move_end(&mut self, w: Raw, geo: IslandGeo, now: Instant) {
        if self.moving.as_ref().is_none_or(|mv| mv.w != w) {
            return;
        }
        // The release itself is the last sample.
        self.sample(geo);
        self.view = DragView::default();
        let Some(mv) = self.moving.take() else {
            return;
        };
        // The button is still down: Esc cancelled the move and Windows put it back.
        if self.ops.button_down() {
            return;
        }
        let start = mv.start;
        match mv.drag.end() {
            Some(target) => self.place_target(w, target, now),
            None => {
                let Some(info) = self.ops.info(w) else {
                    self.placed.forget(w as u64);
                    return;
                };
                if !self.config.restore_size {
                    return;
                }
                if let Some(r) = self.placed.after_move(w as u64, start, irect(info.rect)) {
                    self.apply(w, r, None, now);
                }
            }
        }
    }

    // ---- placement ----

    fn place_target(&mut self, w: Raw, t: Target, now: Instant) {
        let Some(zone) = self.zones_on(t.monitor, t.layout).get(t.zone).copied() else {
            return;
        };
        let active = self.active().get(t.monitor).copied();
        if self.place_in(w, zone, now) && t.from_panel && active != Some(t.layout) {
            // The layout just used becomes the monitor's active one.
            if let (Some(m), Some(l)) = (self.monitors.get(t.monitor), self.layouts.get(t.layout)) {
                self.config.monitors.insert(m.id.clone(), l.name.clone());
                self.config_dirty = true;
            }
        }
    }

    /// Puts `w` so that its visible frame fills `zone`. True if the window was moved.
    fn place_in(&mut self, w: Raw, zone: IRect, now: Instant) -> bool {
        let Some(mut info) = self.ops.info(w).filter(|i| i.eligible) else {
            return false;
        };
        if info.maximized {
            // Measured as a normal window: a maximized one has another border, and its
            // size would be the monitor's.
            self.ops.restore(w);
            match self.ops.info(w) {
                Some(i) => info = i,
                None => return false,
            }
        }
        let rect = tile::window_rect_for(zone, irect(info.rect), irect(info.frame));
        let before = (info.rect.2, info.rect.3);
        self.apply(w, rect, Some(zone), now) && {
            self.placed.placed(w as u64, before);
            true
        }
    }

    /// `SetWindowPos`, with the outcome handled: re-checks for a zone, the notice for an
    /// admin window, the window forgotten if it went away.
    fn apply(&mut self, w: Raw, rect: IRect, zone: Option<IRect>, now: Instant) -> bool {
        match self.ops.place(w, rect) {
            Ok(()) => {
                if let Some(zone) = zone {
                    self.retry = Some(Retry {
                        w,
                        zone,
                        at: now,
                        step: 0,
                    });
                }
                true
            }
            Err(PlaceError::AccessDenied) => {
                self.notice_until = Some(now + NOTICE);
                false
            }
            Err(PlaceError::Gone) => {
                self.placed.forget(w as u64);
                false
            }
            Err(PlaceError::Other(e)) => {
                self.errors.push(format!("tile: cannot move a window: {e}"));
                false
            }
        }
    }

    /// `TIMER_TILE_RETRY`: an app that moved itself back (or was still restoring from
    /// maximized) is placed again, up to three times.
    pub fn retry(&mut self, now: Instant) {
        let Some(r) = self.retry.as_mut() else {
            return;
        };
        if now < r.at + RETRY_AT[r.step] {
            return;
        }
        r.step += 1;
        let (w, zone, done) = (r.w, r.zone, r.step >= RETRY_AT.len());
        if done {
            self.retry = None;
        }
        let Some(info) = self.ops.info(w) else {
            self.retry = None;
            self.placed.forget(w as u64);
            return;
        };
        let frame = irect(info.frame);
        let drift = (frame.x - zone.x)
            .abs()
            .max((frame.y - zone.y).abs())
            .max((frame.right() - zone.right()).abs())
            .max((frame.bottom() - zone.bottom()).abs());
        if drift > DRIFT {
            let rect = tile::window_rect_for(zone, irect(info.rect), frame);
            if self.ops.place(w, rect).is_err() {
                self.retry = None;
            }
        }
    }

    // ---- hotkeys ----

    /// Ctrl+Alt+1..9 and Ctrl+Alt+arrows on the foreground window.
    pub fn hotkey(&mut self, id: i32, now: Instant) {
        let w = self.ops.foreground();
        let Some(info) = self.ops.info(w).filter(|i| i.eligible) else {
            return;
        };
        self.refresh_monitors();
        let frame = irect(info.frame);
        let Some(m) = self.monitor_of(frame) else {
            return;
        };
        let layout = self.active().get(m).copied().unwrap_or(0);
        let zones = self.zones_on(m, layout);
        let target = match id {
            HOTKEY_ZONE_1..=HOTKEY_ZONE_9 => {
                Some((id - HOTKEY_ZONE_1) as usize).filter(|&n| n < zones.len())
            }
            HOTKEY_LEFT..=HOTKEY_DOWN => {
                let dir = match id {
                    HOTKEY_LEFT => Dir::Left,
                    HOTKEY_RIGHT => Dir::Right,
                    HOTKEY_UP => Dir::Up,
                    _ => Dir::Down,
                };
                match tile::zone_of(&zones, frame, IN_ZONE) {
                    Some(from) => tile::neighbour(&zones, from, dir),
                    None => tile::nearest(&zones, frame),
                }
            }
            _ => None,
        };
        if let Some(zone) = target.and_then(|n| zones.get(n).copied()) {
            self.place_in(w, zone, now);
        }
    }

    /// The hotkey ids `[tile]` turns on.
    pub fn hotkeys(&self) -> Vec<i32> {
        let mut ids = Vec::new();
        if self.config.number_keys {
            ids.extend(HOTKEY_ZONE_1..=HOTKEY_ZONE_9);
        }
        if self.config.arrow_keys {
            ids.extend(HOTKEY_LEFT..=HOTKEY_DOWN);
        }
        ids
    }

    // ---- the Home tile's panel ----

    /// The Home tile was clicked: the panel is for the app that was in front.
    pub fn open_home(&mut self) {
        self.home = Some(self.ops.foreground());
        self.home_hover = None;
        self.refresh_monitors();
    }

    /// The pointer over the panel (panel-local logical px). True if what is lit changed.
    pub fn home_hover(&mut self, x: f32, y: f32) -> bool {
        let hover = self.home.and_then(|_| tile::panel_hit(&self.thumbs, x, y));
        let changed = hover != self.home_hover;
        self.home_hover = hover;
        changed
    }

    /// The pointer left the island. True if a cell was lit.
    pub fn clear_hover(&mut self) -> bool {
        self.home_hover.take().is_some()
    }

    /// A click in the panel (panel-local logical px): a cell places the Home window there.
    /// The app collapses the panel either way.
    pub fn panel_click(&mut self, x: f32, y: f32, now: Instant) {
        self.notice_until = None;
        self.home_hover = None;
        let Some(w) = self.home.take() else {
            return;
        };
        if let Some((layout, zone)) = tile::panel_hit(&self.thumbs, x, y) {
            let t = Target {
                monitor: self.island_monitor(),
                layout,
                zone,
                from_panel: true,
            };
            self.place_target(w, t, now);
        }
    }

    /// The island closed the panel by itself (another view took over): the Home window
    /// and the notice go with it, so the panel doesn't come back on its own.
    pub fn panel_closed(&mut self) {
        self.home = None;
        self.home_hover = None;
        self.notice_until = None;
    }

    // ---- what the app shows ----

    fn notice_up(&self, now: Instant) -> bool {
        self.notice_until.is_some_and(|t| now < t)
    }

    /// The island should show the Tile panel: a move is over the summon area, the Home
    /// tile opened it, or the admin notice is up.
    pub fn panel_wanted(&self, now: Instant) -> bool {
        self.view.panel_open || self.home.is_some() || self.notice_up(now)
    }

    pub fn panel(&self, now: Instant) -> Panel<'_> {
        let m = self.island_monitor();
        Panel {
            title: if self.home.is_some() {
                "tile · pick a zone"
            } else {
                "tile · drop on a zone"
            },
            thumbs: &self.thumbs,
            active: self.active().get(m).copied().unwrap_or(0),
            hover: self.view.panel_hover.or(self.home_hover),
            monitor: self
                .monitors
                .get(m)
                .map(|mon| tile::monitor_label(m, mon.rect))
                .unwrap_or_default(),
            footer: if self.notice_up(now) {
                (ADMIN_NOTICE, true)
            } else if self
                .home
                .is_some_and(|w| !self.ops.info(w).is_some_and(|i| i.eligible))
            {
                // The app in front is the desktop, the taskbar or one of ours.
                (NO_WINDOW, true)
            } else {
                (HINT, false)
            },
        }
    }

    /// The zone overlay for the current move, if any.
    pub fn overlay(&self) -> Option<Overlay> {
        let o = self.view.overlay?;
        let m = self.monitors.get(o.monitor)?;
        Some(Overlay {
            monitor: m.rect,
            scale: m.scale,
            zones: self.zones_on(o.monitor, o.layout),
            highlight: o.highlight,
        })
    }

    /// When the app should call back (`retry` or the notice's end), from `now`.
    pub fn next_timer(&self, now: Instant) -> (Option<Duration>, Option<Duration>) {
        let retry = self
            .retry
            .as_ref()
            .map(|r| (r.at + RETRY_AT[r.step]).saturating_duration_since(now));
        let notice = self.notice_until.map(|t| t.saturating_duration_since(now));
        (retry, notice)
    }

    /// `TIMER_TILE_NOTICE`.
    pub fn notice_done(&mut self, now: Instant) {
        if !self.notice_up(now) {
            self.notice_until = None;
        }
    }

    /// The `[tile]` config, once, after the active layout of a monitor changed.
    pub fn take_config(&mut self) -> Option<TileConfig> {
        std::mem::take(&mut self.config_dirty).then(|| self.config.clone())
    }

    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    const DISPLAY1: &str = r"\\.\DISPLAY1";
    const DISPLAY2: &str = r"\\.\DISPLAY2";

    /// Windows 11's invisible resize border: 7 px left, right and bottom, none on top.
    fn win(frame: IRect) -> WinInfo {
        WinInfo {
            rect: (frame.x - 7, frame.y, frame.w + 14, frame.h + 7),
            frame: (frame.x, frame.y, frame.w, frame.h),
            maximized: false,
            eligible: true,
        }
    }

    /// A maximized window on `work`: the border hangs 8 px outside on every side.
    fn maxed(work: IRect) -> WinInfo {
        WinInfo {
            rect: (work.x - 8, work.y - 8, work.w + 16, work.h + 16),
            frame: (work.x, work.y, work.w, work.h),
            maximized: true,
            eligible: true,
        }
    }

    #[derive(Default)]
    struct State {
        windows: HashMap<Raw, WinInfo>,
        /// The visible frame a maximized window restores to.
        normal: HashMap<Raw, IRect>,
        places: Vec<(Raw, IRect)>,
        deny: bool,
        foreground: Raw,
        cursor: (i32, i32),
        shift: bool,
        /// The primary button is still held (at the end: Esc cancelled the move).
        button: bool,
        /// The move starts on a sizing border.
        resize: bool,
        /// The window's thread left its move loop (the watchdog's view).
        ended: bool,
    }

    struct Fake(Rc<RefCell<State>>);

    impl WindowOps for Fake {
        fn info(&self, w: Raw) -> Option<WinInfo> {
            self.0.borrow().windows.get(&w).copied()
        }

        fn place(&mut self, w: Raw, r: IRect) -> Result<(), PlaceError> {
            let mut s = self.0.borrow_mut();
            if s.deny {
                return Err(PlaceError::AccessDenied);
            }
            if !s.windows.contains_key(&w) {
                return Err(PlaceError::Gone);
            }
            s.places.push((w, r));
            // The window goes where it was put; its visible frame is inside the border.
            s.windows.insert(
                w,
                WinInfo {
                    rect: (r.x, r.y, r.w, r.h),
                    frame: (r.x + 7, r.y, r.w - 14, r.h - 7),
                    maximized: false,
                    eligible: true,
                },
            );
            Ok(())
        }

        fn restore(&mut self, w: Raw) {
            let mut s = self.0.borrow_mut();
            let normal = s.normal.get(&w).copied();
            if let (Some(frame), Some(info)) = (normal, s.windows.get_mut(&w)) {
                if info.maximized {
                    *info = win(frame);
                }
            }
        }

        fn button_down(&self) -> bool {
            self.0.borrow().button
        }

        fn is_resize(&self, _w: Raw, _at: (i32, i32)) -> bool {
            self.0.borrow().resize
        }

        fn still_moving(&self, _w: Raw) -> bool {
            !self.0.borrow().ended
        }

        fn foreground(&self) -> Raw {
            self.0.borrow().foreground
        }

        fn monitors(&self) -> Vec<MonitorInfo> {
            vec![
                MonitorInfo {
                    device: DISPLAY1.into(),
                    rect: (0, 0, 1920, 1080),
                    work: (0, 0, 1920, 1032),
                    dpi: 96,
                },
                MonitorInfo {
                    device: DISPLAY2.into(),
                    rect: (-1920, 0, 1920, 1080),
                    work: (-1920, 0, 1920, 1080),
                    dpi: 144,
                },
            ]
        }

        fn cursor(&self) -> (i32, i32) {
            self.0.borrow().cursor
        }

        fn shift_down(&self) -> bool {
            self.0.borrow().shift
        }
    }

    const GEO: IslandGeo = IslandGeo {
        summon: IRect::new(860, 40, 200, 80),
        panel: IRect::new(760, 6, 400, 168),
        scale: 1.0,
    };

    fn rig() -> (Tiler, Rc<RefCell<State>>) {
        rig_with(TileConfig::default())
    }

    fn rig_with(config: TileConfig) -> (Tiler, Rc<RefCell<State>>) {
        let s = Rc::new(RefCell::new(State::default()));
        let t = Tiler::new(Box::new(Fake(s.clone())), config, &[]);
        (t, s)
    }

    fn frame_of(s: &Rc<RefCell<State>>, w: Raw) -> IRect {
        irect(s.borrow().windows[&w].frame)
    }

    /// The desktop point over `layout`'s `zone` cell in the panel.
    fn cell(t: &Tiler, layout: usize, zone: usize) -> (i32, i32) {
        let c = t.thumbs[layout].cells[zone];
        (
            GEO.panel.x + (c.x + c.w / 2.0) as i32,
            GEO.panel.y + (c.y + c.h / 2.0) as i32,
        )
    }

    fn work1() -> IRect {
        IRect::new(0, 0, 1920, 1032)
    }

    fn zones1(t: &Tiler, name: &str) -> Vec<IRect> {
        let l = tile::find(&t.layouts, name).unwrap();
        tile::zones(&t.layouts[l], work1(), 8)
    }

    #[test]
    fn dropping_on_a_panel_cell_fills_that_zone_and_makes_the_layout_active() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        t.move_start(1);
        assert!(t.polling());
        // Up into the summon area: the panel opens.
        s.borrow_mut().cursor = (960, 60);
        assert!(t.poll(GEO, now));
        assert!(t.panel_wanted(now));
        // Onto the right cell of "1+2"'s thumbnail.
        let l = tile::find(&t.layouts, "1+2").unwrap();
        s.borrow_mut().cursor = cell(&t, l, 1);
        t.poll(GEO, now);
        assert_eq!(t.panel(now).hover, Some((l, 1)));
        let overlay = t.overlay().unwrap();
        assert_eq!(
            (overlay.monitor, overlay.highlight),
            (IRect::new(0, 0, 1920, 1080), Some(1))
        );
        t.move_end(1, GEO, now);
        assert!(!t.polling());
        assert!(!t.panel_wanted(now));
        assert!(t.overlay().is_none());
        // The visible frame fills the zone exactly (the border corrected).
        assert_eq!(frame_of(&s, 1), zones1(&t, "1+2")[1]);
        let config = t.take_config().unwrap();
        assert_eq!(
            config.monitors.get(DISPLAY1).map(String::as_str),
            Some("1+2")
        );
        assert!(t.take_config().is_none(), "written once");
    }

    #[test]
    fn a_placed_window_dragged_out_gets_its_old_size_back() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        s.borrow_mut().foreground = 1;
        t.hotkey(HOTKEY_ZONE_1, now);
        let placed = s.borrow().windows[&1];
        // Dragged into the middle of the screen, same size, no zone under the cursor.
        t.move_start(1);
        s.borrow_mut().cursor = (900, 600);
        t.poll(GEO, now);
        let (x, y) = (400, 300);
        s.borrow_mut().windows.get_mut(&1).unwrap().rect = (x, y, placed.rect.2, placed.rect.3);
        t.move_end(1, GEO, now);
        let last = *s.borrow().places.last().unwrap();
        assert_eq!(last, (1, IRect::new(x, y, 814, 607)), "the old window size");
    }

    #[test]
    fn a_resize_never_snaps_and_never_restores() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        s.borrow_mut().foreground = 1;
        t.hotkey(HOTKEY_ZONE_1, now);
        let places = s.borrow().places.len();
        // The edge is grabbed: a resize is not followed at all, even across the panel.
        s.borrow_mut().resize = true;
        t.move_start(1);
        assert!(!t.polling());
        s.borrow_mut().cursor = cell(&t, 0, 0);
        t.poll(GEO, now);
        assert!(!t.panel_wanted(now));
        s.borrow_mut().windows.get_mut(&1).unwrap().rect.2 += 50;
        t.move_end(1, GEO, now);
        assert_eq!(s.borrow().places.len(), places, "nothing moved");
        // And the size memory is gone: a later drag out restores nothing.
        s.borrow_mut().resize = false;
        t.move_start(1);
        s.borrow_mut().windows.get_mut(&1).unwrap().rect.0 += 300;
        t.move_end(1, GEO, now);
        assert_eq!(s.borrow().places.len(), places);
    }

    #[test]
    fn dragging_a_maximized_window_by_its_title_still_opens_the_picker() {
        let (mut t, s) = rig();
        let now = Instant::now();
        {
            let mut st = s.borrow_mut();
            st.windows.insert(1, maxed(work1()));
            st.normal.insert(1, IRect::new(300, 200, 900, 700));
        }
        t.move_start(1);
        // Windows restores it as the drag begins: a new size, but still a move.
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(600, 0, 900, 700)));
        s.borrow_mut().cursor = (960, 60);
        t.poll(GEO, now);
        assert!(t.panel_wanted(now));
        s.borrow_mut().cursor = cell(&t, 0, 0);
        t.poll(GEO, now);
        t.move_end(1, GEO, now);
        assert_eq!(frame_of(&s, 1), zones1(&t, "50/50")[0]);
    }

    #[test]
    fn esc_during_a_drag_over_a_cell_places_nothing() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        t.move_start(1);
        s.borrow_mut().cursor = (960, 60);
        t.poll(GEO, now);
        s.borrow_mut().cursor = cell(&t, 0, 1);
        t.poll(GEO, now);
        // Esc ends the move while the button is still held.
        s.borrow_mut().button = true;
        t.move_end(1, GEO, now);
        assert!(s.borrow().places.is_empty());
        assert!(!t.panel_wanted(now));
        assert!(t.take_config().is_none());
    }

    #[test]
    fn a_lost_end_event_ends_the_move_at_the_next_poll() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        t.move_start(1);
        s.borrow_mut().cursor = (960, 60);
        t.poll(GEO, now);
        s.borrow_mut().cursor = cell(&t, 0, 1);
        t.poll(GEO, now);
        // The thread left its move loop; no end event arrives.
        s.borrow_mut().ended = true;
        assert!(t.poll(GEO, now));
        assert!(!t.polling());
        assert!(!t.panel_wanted(now));
        assert_eq!(frame_of(&s, 1), zones1(&t, "50/50")[1]);
        // The late end event changes nothing.
        let count = s.borrow().places.len();
        t.move_end(1, GEO, now);
        assert_eq!(s.borrow().places.len(), count);
    }

    #[test]
    fn grabbing_a_placed_window_again_cancels_its_retry() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        s.borrow_mut().foreground = 1;
        t.hotkey(HOTKEY_ZONE_1, now);
        assert!(t.next_timer(now).0.is_some());
        t.move_start(1);
        assert_eq!(t.next_timer(now).0, None);
        // The user drags it somewhere: the old zone doesn't pull it back.
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(500, 500, 700, 500)));
        let count = s.borrow().places.len();
        t.retry(now + Duration::from_millis(150));
        assert_eq!(s.borrow().places.len(), count);
    }

    #[test]
    fn picking_the_layout_that_is_already_active_writes_no_config() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        t.move_start(1);
        s.borrow_mut().cursor = (960, 60);
        t.poll(GEO, now);
        let l = tile::find(&t.layouts, "50/50").unwrap();
        s.borrow_mut().cursor = cell(&t, l, 0);
        t.poll(GEO, now);
        t.move_end(1, GEO, now);
        assert_eq!(frame_of(&s, 1), zones1(&t, "50/50")[0]);
        assert!(
            t.take_config().is_none(),
            "50/50 was already the active one"
        );
    }

    #[test]
    fn shift_drag_on_the_second_monitor_uses_its_zones_and_scale() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(-1500, 100, 600, 400)));
        t.move_start(1);
        {
            let mut st = s.borrow_mut();
            st.cursor = (-1500, 500);
            st.shift = true;
        }
        t.poll(GEO, now);
        let overlay = t.overlay().unwrap();
        assert_eq!(overlay.monitor, IRect::new(-1920, 0, 1920, 1080));
        assert_eq!(overlay.scale, 1.5);
        assert_eq!(overlay.highlight, Some(0));
        t.move_end(1, GEO, now);
        let l = tile::find(&t.layouts, "50/50").unwrap();
        let zone = tile::zones(&t.layouts[l], IRect::new(-1920, 0, 1920, 1080), 12)[0];
        assert_eq!(frame_of(&s, 1), zone);
        assert!(
            t.take_config().is_none(),
            "Shift doesn't change the active layout"
        );
    }

    #[test]
    fn a_maximized_window_is_restored_first_and_lands_exactly_at_once() {
        let (mut t, s) = rig();
        let now = Instant::now();
        {
            let mut st = s.borrow_mut();
            st.windows.insert(1, maxed(work1()));
            st.normal.insert(1, IRect::new(300, 200, 900, 700));
            st.foreground = 1;
        }
        t.hotkey(HOTKEY_ZONE_1 + 1, now);
        // One placement, measured on the restored window: no fix-up needed.
        assert_eq!(s.borrow().places.len(), 1);
        assert_eq!(frame_of(&s, 1), zones1(&t, "50/50")[1]);
        // Its normal size is what a drag out gives back.
        t.move_start(1);
        let placed = s.borrow().windows[&1];
        s.borrow_mut().windows.get_mut(&1).unwrap().rect.0 += 100;
        t.move_end(1, GEO, now);
        let last = s.borrow().places.last().unwrap().1;
        assert_eq!((last.w, last.h), (914, 707));
        assert_eq!(last.x, placed.rect.0 + 100);
    }

    #[test]
    fn the_home_panel_says_when_there_is_no_window_to_place() {
        let (mut t, s) = rig();
        let now = Instant::now();
        let mut info = win(IRect::new(0, 0, 1920, 1080));
        info.eligible = false;
        s.borrow_mut().windows.insert(5, info);
        s.borrow_mut().foreground = 5;
        t.open_home();
        assert_eq!(t.panel(now).footer, (NO_WINDOW, true));
        // A lit cell goes when the pointer leaves the island.
        let c = t.thumbs[0].cells[0];
        t.home_hover(c.x + 1.0, c.y + 1.0);
        assert!(t.clear_hover());
        assert_eq!(t.panel(now).hover, None);
        assert!(!t.clear_hover());
    }

    #[test]
    fn a_window_that_moves_itself_back_is_placed_again_three_times_at_most() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        s.borrow_mut().foreground = 1;
        t.hotkey(HOTKEY_ZONE_1, now);
        assert_eq!(t.next_timer(now).0, Some(Duration::from_millis(150)));
        let zone = zones1(&t, "50/50")[0];
        let fight = |s: &Rc<RefCell<State>>| {
            s.borrow_mut()
                .windows
                .insert(1, win(IRect::new(100, 100, 800, 600)));
        };
        let mut count = s.borrow().places.len();
        for (i, ms) in [150, 300, 500].into_iter().enumerate() {
            fight(&s);
            t.retry(now + Duration::from_millis(ms));
            assert_eq!(s.borrow().places.len(), count + 1, "retry {i}");
            assert_eq!(frame_of(&s, 1), zone);
            count += 1;
        }
        assert_eq!(t.next_timer(now).0, None, "done after three");
        fight(&s);
        t.retry(now + Duration::from_secs(1));
        assert_eq!(s.borrow().places.len(), count);
    }

    #[test]
    fn a_window_that_stays_put_is_not_placed_again() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        s.borrow_mut().foreground = 1;
        t.hotkey(HOTKEY_ZONE_1, now);
        let count = s.borrow().places.len();
        t.retry(now + Duration::from_millis(100));
        t.retry(now + Duration::from_millis(150));
        assert_eq!(s.borrow().places.len(), count);
        assert_eq!(t.next_timer(now).0, Some(Duration::from_millis(300)));
    }

    #[test]
    fn an_admin_window_is_left_alone_with_a_notice_for_two_seconds() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        {
            let mut st = s.borrow_mut();
            st.foreground = 1;
            st.deny = true;
        }
        t.hotkey(HOTKEY_ZONE_1, now);
        assert!(s.borrow().places.is_empty());
        assert!(t.panel_wanted(now));
        assert_eq!(t.panel(now).footer, (ADMIN_NOTICE, true));
        assert_eq!(t.next_timer(now).1, Some(NOTICE));
        let later = now + NOTICE + Duration::from_millis(1);
        assert!(!t.panel_wanted(later));
        assert!(!t.panel(later).footer.1);
        t.notice_done(later);
        assert_eq!(t.next_timer(later).1, None);
    }

    #[test]
    fn a_window_closed_mid_drag_is_dropped_quietly() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        t.move_start(1);
        s.borrow_mut().cursor = (960, 60);
        t.poll(GEO, now);
        s.borrow_mut().windows.remove(&1);
        t.poll(GEO, now);
        assert!(!t.polling());
        assert!(!t.panel_wanted(now));
        t.move_end(1, GEO, now);
        assert!(s.borrow().places.is_empty());
        assert!(t.take_errors().is_empty());
    }

    #[test]
    fn a_number_beyond_the_layout_does_nothing() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(100, 100, 800, 600)));
        s.borrow_mut().foreground = 1;
        t.hotkey(HOTKEY_ZONE_1 + 2, now);
        assert!(s.borrow().places.is_empty(), "50/50 has two zones");
        t.hotkey(HOTKEY_ZONE_1 + 1, now);
        assert_eq!(frame_of(&s, 1), zones1(&t, "50/50")[1]);
    }

    #[test]
    fn windows_we_must_not_touch_are_ignored() {
        let (mut t, s) = rig();
        let now = Instant::now();
        let mut info = win(IRect::new(100, 100, 800, 600));
        info.eligible = false;
        s.borrow_mut().windows.insert(1, info);
        s.borrow_mut().foreground = 1;
        t.hotkey(HOTKEY_ZONE_1, now);
        t.move_start(1);
        assert!(!t.polling());
        // No foreground window at all (the desktop with nothing focused).
        s.borrow_mut().foreground = 0;
        t.hotkey(HOTKEY_RIGHT, now);
        assert!(s.borrow().places.is_empty());
    }

    #[test]
    fn arrows_walk_the_zones_of_the_active_layout() {
        let mut config = TileConfig::default();
        config.monitors.insert(DISPLAY1.into(), "2x2".into());
        let (mut t, s) = rig_with(config);
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(1, win(IRect::new(300, 300, 500, 400)));
        s.borrow_mut().foreground = 1;
        let z = zones1(&t, "2x2");
        // Not in a zone yet: the nearest one.
        t.hotkey(HOTKEY_RIGHT, now);
        assert_eq!(frame_of(&s, 1), z[0]);
        t.hotkey(HOTKEY_RIGHT, now);
        assert_eq!(frame_of(&s, 1), z[1]);
        t.hotkey(HOTKEY_DOWN, now);
        assert_eq!(frame_of(&s, 1), z[3]);
        t.hotkey(HOTKEY_LEFT, now);
        assert_eq!(frame_of(&s, 1), z[2]);
        t.hotkey(HOTKEY_UP, now);
        assert_eq!(frame_of(&s, 1), z[0]);
        // Nothing further left: it stays.
        let count = s.borrow().places.len();
        t.hotkey(HOTKEY_LEFT, now);
        assert_eq!(s.borrow().places.len(), count);
    }

    #[test]
    fn the_home_panel_places_the_app_that_was_in_front() {
        let (mut t, s) = rig();
        let now = Instant::now();
        s.borrow_mut()
            .windows
            .insert(7, win(IRect::new(100, 100, 800, 600)));
        s.borrow_mut().foreground = 7;
        t.open_home();
        assert!(t.panel_wanted(now));
        assert_eq!(t.panel(now).title, "tile · pick a zone");
        let l = tile::find(&t.layouts, "3 columns").unwrap();
        let c = t.thumbs[l].cells[2];
        let (x, y) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        assert!(t.home_hover(x, y));
        assert_eq!(t.panel(now).hover, Some((l, 2)));
        t.panel_click(x, y, now);
        assert!(!t.panel_wanted(now));
        assert_eq!(frame_of(&s, 7), zones1(&t, "3 columns")[2]);
        // A click on nothing just closes it.
        t.open_home();
        let count = s.borrow().places.len();
        t.panel_click(1.0, 1.0, now);
        assert!(!t.panel_wanted(now));
        assert_eq!(s.borrow().places.len(), count);
    }

    #[test]
    fn the_hotkeys_follow_the_config() {
        let (t, _) = rig();
        assert_eq!(
            t.hotkeys(),
            (HOTKEY_ZONE_1..=HOTKEY_DOWN).collect::<Vec<_>>()
        );
        let config = TileConfig {
            number_keys: false,
            ..TileConfig::default()
        };
        let (t, _) = rig_with(config);
        assert_eq!(
            t.hotkeys(),
            vec![HOTKEY_LEFT, HOTKEY_RIGHT, HOTKEY_UP, HOTKEY_DOWN]
        );
        let config = TileConfig {
            number_keys: false,
            arrow_keys: false,
            ..TileConfig::default()
        };
        let (t, _) = rig_with(config);
        assert!(t.hotkeys().is_empty());
    }
}
