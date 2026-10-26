//! The app's side of Snap: freeze, select, crop, copy, save, and the thumb's countdown.
//! The screen and the clipboard sit behind traits, so this runs in unit tests with
//! synthetic frames and a fake clipboard. The overlay window and its renderer are glue
//! in `app.rs`.
//!
//! Memory: the frozen frames live only while the overlay is up (`Session`). After a
//! release only the crop and its PNG are kept (`Shot`), until the thumb goes.

use crate::storage;
use dj_core::config::SnapHotkey;
use dj_core::snap::{
    crop, file_name, Bitmap, Countdown, IRect, MonitorFrame, Selector, Stamp, ThumbStatus,
    THUMB_SECS,
};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Hotkey ids: the app has 1 (toggle) and 2 (theme).
pub const HOTKEY_SNAP: i32 = 3;
/// Esc, registered only while the overlay is up, in case it didn't get the keyboard.
pub const HOTKEY_SNAP_ESC: i32 = 4;
/// One-shot: the thumb's countdown ends. App timer ids 1-6 are taken.
pub const TIMER_SNAP_THUMB: usize = 7;
/// One-shot: the island has finished collapsing, so the screen can be frozen.
pub const TIMER_SNAP_START: usize = 8;
/// How long after the collapse settles before the freeze (one composed frame or so).
pub const START_DELAY_MS: u32 = 60;

const VK_SNAPSHOT: u32 = 0x2C;
const VK_S: u32 = 0x53;
pub const VK_ESCAPE: u32 = 0x1B;
/// A save never overwrites: `-2` … `-999`, then it gives up.
const MAX_NAME_TRIES: u32 = 999;

/// `(modifiers, virtual key)` for `RegisterHotKey`.
pub fn hotkey_keys(k: SnapHotkey) -> (u32, u32) {
    match k {
        SnapHotkey::PrtSc => (dj_win::window::MOD_NONE, VK_SNAPSHOT),
        SnapHotkey::CtrlShiftS => (dj_win::window::MOD_CTRL_SHIFT, VK_S),
        SnapHotkey::CtrlAltS => (dj_win::window::MOD_CTRL_ALT, VK_S),
    }
}

/// Freezes the screen and lists the windows. The real one is `SystemScreen`.
pub trait Screen {
    fn freeze(&mut self) -> Result<Vec<MonitorFrame>, String>;
    /// Window rects for click-to-window, top of the z-order first.
    fn windows(&mut self) -> Vec<IRect>;
}

/// Replaces the clipboard with a PNG and a `CF_DIB`. The real one is `SystemClipboard`.
pub trait Clipboard {
    fn put(&mut self, png: &[u8], dib: &[u8]) -> Result<(), String>;
}

pub struct SystemScreen;

impl Screen for SystemScreen {
    fn freeze(&mut self) -> Result<Vec<MonitorFrame>, String> {
        let (frames, report) = dj_capture::freeze()?;
        if report.gdi > 0 {
            let why = report.note.as_deref().unwrap_or("duplication unavailable");
            storage::log(&format!(
                "snap: {} monitor(s) by DXGI, {} by GDI ({why})",
                report.dxgi, report.gdi
            ));
        }
        Ok(frames)
    }

    fn windows(&mut self) -> Vec<IRect> {
        dj_capture::window_rects()
    }
}

/// The clipboard, owned by the island window.
pub struct SystemClipboard {
    pub owner: isize,
}

impl Clipboard for SystemClipboard {
    fn put(&mut self, png: &[u8], dib: &[u8]) -> Result<(), String> {
        dj_capture::put_image(self.owner, png, dib)
    }
}

/// While the overlay is up: the frozen monitors and the selection.
pub struct Session {
    pub frames: Vec<MonitorFrame>,
    pub selector: Selector,
}

/// The last snap, kept for the thumb's copy and save until the thumb goes.
pub struct Shot {
    pub bitmap: Bitmap,
    /// None if encoding failed; copy and save try again.
    png: Option<Vec<u8>>,
    pub status: ThumbStatus,
    /// Where the last save went. The thumb's save button then reads "open" and reveals
    /// this file instead of saving a second copy.
    pub saved: Option<PathBuf>,
    countdown: Countdown,
}

pub struct Snapper {
    screen: Box<dyn Screen>,
    clipboard: Box<dyn Clipboard>,
    session: Option<Session>,
    shot: Option<Shot>,
}

impl Snapper {
    pub fn new(screen: Box<dyn Screen>, clipboard: Box<dyn Clipboard>) -> Self {
        Self {
            screen,
            clipboard,
            session: None,
            shot: None,
        }
    }

    /// Freezes every monitor and starts a selection over `desktop` at `cursor`. Click
    /// targets are the windows, then the monitors (a click on the bare desktop takes
    /// the monitor under it).
    pub fn begin(&mut self, desktop: IRect, cursor: (i32, i32)) -> Result<&Session, String> {
        let frames = self.screen.freeze()?;
        let mut targets = self.screen.windows();
        targets.extend(frames.iter().map(|f| f.rect));
        let selector = Selector::new(desktop, targets, cursor);
        Ok(self.session.insert(Session { frames, selector }))
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// Esc or a right click: the frames go, and the clipboard is not touched.
    pub fn cancel(&mut self) {
        self.session = None;
    }

    /// The button went up. Cancelled (None) if the selection is too small; otherwise the
    /// crop replaces the last shot and goes to the clipboard, and its countdown starts.
    /// `Some(Err)` carries the reason the copy failed (the shot is kept for a retry).
    pub fn finish(&mut self, now: Instant) -> Option<Result<(), String>> {
        let mut session = self.session.take()?;
        let rect = session.selector.release()?;
        let bitmap = crop(&session.frames, rect);
        drop(session);
        let mut shot = Shot {
            bitmap,
            png: None,
            status: ThumbStatus::Copied,
            saved: None,
            countdown: Countdown::start(Duration::from_secs(THUMB_SECS), now),
        };
        let result = copy_shot(&mut shot, self.clipboard.as_mut());
        self.shot = Some(shot);
        Some(result)
    }

    pub fn shot(&self) -> Option<&Shot> {
        self.shot.as_ref()
    }

    /// The thumb's **copy**: puts the shot on the clipboard again.
    pub fn copy(&mut self) -> Result<(), String> {
        let shot = self.shot.as_mut().ok_or("no shot")?;
        copy_shot(shot, self.clipboard.as_mut())
    }

    /// The thumb's **save**: writes the PNG to `dir` (created if missing) under a new
    /// name. Never overwrites.
    pub fn save(&mut self, dir: &Path, stamp: Stamp) -> Result<PathBuf, String> {
        let shot = self.shot.as_mut().ok_or("no shot")?;
        let result = ensure_png(shot).and_then(|png| write_new(dir, stamp, png));
        match &result {
            Ok(path) => {
                shot.status = ThumbStatus::Saved;
                shot.saved = Some(path.clone());
            }
            Err(_) => shot.status = ThumbStatus::SaveFailed,
        }
        result
    }

    /// The thumb goes: the crop and its PNG are freed.
    pub fn dismiss(&mut self) {
        self.shot = None;
    }

    /// Pauses the countdown while the pointer is over the thumb.
    pub fn set_hover(&mut self, hovered: bool, now: Instant) {
        if let Some(shot) = self.shot.as_mut() {
            if hovered {
                shot.countdown.pause(now);
            } else {
                shot.countdown.resume(now);
            }
        }
    }

    /// Milliseconds to the end of the countdown, for a one-shot timer. None while
    /// paused or without a shot.
    pub fn thumb_ms(&self, now: Instant) -> Option<u32> {
        let c = &self.shot.as_ref()?.countdown;
        if c.is_paused() {
            return None;
        }
        let left = c.left(now);
        Some((left.as_millis() as u32).saturating_add(5).max(1))
    }

    /// The countdown has ended (never while paused).
    pub fn expired(&self, now: Instant) -> bool {
        self.shot.as_ref().is_some_and(|s| s.countdown.expired(now))
    }
}

/// The shot's PNG, encoded on first use.
fn ensure_png(shot: &mut Shot) -> Result<&[u8], String> {
    if shot.png.is_none() {
        shot.png = Some(dj_capture::png::encode(&shot.bitmap)?);
    }
    Ok(shot.png.as_deref().unwrap_or_default())
}

fn copy_shot(shot: &mut Shot, clipboard: &mut dyn Clipboard) -> Result<(), String> {
    let dib = dj_capture::dib::from_bitmap(&shot.bitmap);
    let result = ensure_png(shot).and_then(|png| clipboard.put(png, &dib));
    shot.status = if result.is_ok() {
        ThumbStatus::Copied
    } else {
        ThumbStatus::CopyFailed
    };
    result
}

/// Writes `bytes` to the first free `snap-…png` name in `dir`. `create_new` makes the
/// check and the create one step, so nothing is ever overwritten.
fn write_new(dir: &Path, stamp: Stamp, bytes: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    for n in 1..=MAX_NAME_TRIES {
        let path = dir.join(file_name(stamp, n));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                return match f.write_all(bytes).and_then(|()| f.flush()) {
                    Ok(()) => Ok(path),
                    Err(e) => {
                        drop(f);
                        let _ = fs::remove_file(&path);
                        Err(format!("cannot write {}: {e}", path.display()))
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
        }
    }
    Err(format!("no free file name in {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Two synthetic monitors, the left one at negative x; pixel (x, y) of a monitor is
    /// [seed, x, y, 255].
    fn frames() -> Vec<MonitorFrame> {
        let mon = |rect: IRect, seed: u8| {
            let mut bitmap = Bitmap::black(rect.w as u32, rect.h as u32);
            for y in 0..rect.h {
                for x in 0..rect.w {
                    let i = (y * rect.w + x) as usize * 4;
                    bitmap.pixels[i] = seed;
                    bitmap.pixels[i + 1] = x as u8;
                    bitmap.pixels[i + 2] = y as u8;
                }
            }
            MonitorFrame {
                rect,
                dpi: 96,
                bitmap,
            }
        };
        vec![
            mon(IRect::new(0, 0, 64, 48), 1),
            mon(IRect::new(-32, 0, 32, 48), 2),
        ]
    }

    const DESK: IRect = IRect::new(-32, 0, 96, 48);

    struct FakeScreen {
        windows: Vec<IRect>,
        freezes: Rc<RefCell<u32>>,
    }

    impl Screen for FakeScreen {
        fn freeze(&mut self) -> Result<Vec<MonitorFrame>, String> {
            *self.freezes.borrow_mut() += 1;
            Ok(frames())
        }

        fn windows(&mut self) -> Vec<IRect> {
            self.windows.clone()
        }
    }

    type Puts = Rc<RefCell<Vec<(Vec<u8>, Vec<u8>)>>>;

    /// Every `put`, as (png, dib); fails while `fail` is set.
    #[derive(Default)]
    struct FakeClipboard {
        puts: Puts,
        fail: Rc<RefCell<bool>>,
    }

    impl Clipboard for FakeClipboard {
        fn put(&mut self, png: &[u8], dib: &[u8]) -> Result<(), String> {
            if *self.fail.borrow() {
                return Err("busy".into());
            }
            self.puts.borrow_mut().push((png.to_vec(), dib.to_vec()));
            Ok(())
        }
    }

    fn snapper(windows: Vec<IRect>) -> (Snapper, Puts, Rc<RefCell<bool>>) {
        let clip = FakeClipboard::default();
        let (puts, fail) = (clip.puts.clone(), clip.fail.clone());
        let screen = FakeScreen {
            windows,
            freezes: Rc::default(),
        };
        (Snapper::new(Box::new(screen), Box::new(clip)), puts, fail)
    }

    fn drag(s: &mut Snapper, from: (i32, i32), to: (i32, i32)) {
        let sel = &mut s.session_mut().unwrap().selector;
        sel.press(from.0, from.1);
        sel.move_to(to.0, to.1);
    }

    const STAMP: Stamp = Stamp {
        year: 2026,
        month: 10,
        day: 26,
        hour: 14,
        minute: 3,
        second: 9,
    };

    #[test]
    fn hotkeys_map_to_virtual_keys() {
        assert_eq!(hotkey_keys(SnapHotkey::PrtSc), (0, 0x2C));
        assert_eq!(hotkey_keys(SnapHotkey::CtrlShiftS), (0x0002 | 0x0004, 0x53));
        assert_eq!(hotkey_keys(SnapHotkey::CtrlAltS), (0x0002 | 0x0001, 0x53));
    }

    #[test]
    fn ids_are_free() {
        // Hotkeys 1 and 2 belong to app.rs; timers 1, 2, 4 to app.rs, 3 to vitals.rs and
        // 5, 6 to timer.rs.
        for id in [HOTKEY_SNAP, HOTKEY_SNAP_ESC] {
            assert!(![1, 2].contains(&id));
        }
        assert_ne!(HOTKEY_SNAP, HOTKEY_SNAP_ESC);
        let taken = [
            1,
            2,
            4,
            crate::vitals::TIMER,
            crate::timer::TIMER_DEADLINE,
            crate::timer::TIMER_DIGITS,
        ];
        for id in [TIMER_SNAP_THUMB, TIMER_SNAP_START] {
            assert!(!taken.contains(&id), "{id}");
        }
        assert_ne!(TIMER_SNAP_THUMB, TIMER_SNAP_START);
    }

    #[test]
    fn a_drag_copies_the_exact_pixels_as_png_and_dib() {
        let (mut s, puts, _) = snapper(vec![]);
        s.begin(DESK, (0, 0)).unwrap();
        // Across the seam between the two monitors.
        drag(&mut s, (-2, 3), (2, 5));
        assert_eq!(s.finish(Instant::now()), Some(Ok(())));
        assert!(s.session().is_none(), "the frozen frames are gone");
        let shot = s.shot().unwrap();
        assert_eq!((shot.bitmap.width, shot.bitmap.height), (4, 2));
        assert_eq!(shot.status, ThumbStatus::Copied);
        let puts = puts.borrow();
        assert_eq!(puts.len(), 1, "one clipboard session");
        let (png, dib) = &puts[0];
        let back = dj_capture::png::decode(png).unwrap();
        assert_eq!(back.pixels, shot.bitmap.pixels);
        // Top-left pixel: the left monitor's (30, 3).
        assert_eq!(&back.pixels[0..4], &[2, 30, 3, 255]);
        // Its right half is the primary's (0, 3).
        assert_eq!(&back.pixels[8..12], &[1, 0, 3, 255]);
        assert_eq!(dib.len(), 40 + 4 * 2 * 4);
        // The DIB is bottom-up: its first row is the crop's last.
        assert_eq!(&dib[40..44], &[2, 30, 4, 255]);
    }

    #[test]
    fn a_click_takes_the_window_under_the_cursor_or_the_monitor() {
        let win = IRect::new(4, 4, 10, 8);
        let (mut s, _, _) = snapper(vec![win]);
        s.begin(DESK, (0, 0)).unwrap();
        s.session_mut().unwrap().selector.press(6, 6);
        s.finish(Instant::now());
        let b = &s.shot().unwrap().bitmap;
        assert_eq!((b.width, b.height), (10, 8));
        // A click on the bare left monitor takes all of it.
        s.begin(DESK, (0, 0)).unwrap();
        s.session_mut().unwrap().selector.press(-10, 40);
        s.finish(Instant::now());
        let b = &s.shot().unwrap().bitmap;
        assert_eq!((b.width, b.height), (32, 48));
    }

    #[test]
    fn cancel_and_a_tiny_drag_never_touch_the_clipboard() {
        let (mut s, puts, _) = snapper(vec![]);
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (1, 1), (30, 30));
        s.cancel();
        assert!(s.session().is_none());
        assert_eq!(s.finish(Instant::now()), None);
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (10, 10), (40, 40));
        drag(&mut s, (10, 10), (11, 40));
        assert_eq!(s.finish(Instant::now()), None, "1 px wide");
        assert!(s.session().is_none());
        assert!(puts.borrow().is_empty());
        assert!(s.shot().is_none());
    }

    #[test]
    fn a_failed_copy_keeps_the_shot_for_a_retry() {
        let (mut s, puts, fail) = snapper(vec![]);
        *fail.borrow_mut() = true;
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (0, 0), (8, 8));
        assert_eq!(s.finish(Instant::now()), Some(Err("busy".into())));
        assert_eq!(s.shot().unwrap().status, ThumbStatus::CopyFailed);
        *fail.borrow_mut() = false;
        assert_eq!(s.copy(), Ok(()));
        assert_eq!(s.shot().unwrap().status, ThumbStatus::Copied);
        assert_eq!(puts.borrow().len(), 1);
    }

    #[test]
    fn save_writes_a_png_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Pictures").join("Daijobu");
        let (mut s, _, _) = snapper(vec![]);
        assert!(s.save(&target, STAMP).is_err(), "no shot yet");
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (0, 0), (6, 4));
        s.finish(Instant::now());
        let first = s.save(&target, STAMP).unwrap();
        assert_eq!(first, target.join("snap-2026-10-26_140309.png"));
        assert_eq!(s.shot().unwrap().status, ThumbStatus::Saved);
        assert_eq!(s.shot().unwrap().saved.as_ref(), Some(&first));
        let second = s.save(&target, STAMP).unwrap();
        assert_eq!(second, target.join("snap-2026-10-26_140309-2.png"));
        let bytes = fs::read(&first).unwrap();
        let back = dj_capture::png::decode(&bytes).unwrap();
        assert_eq!(back.pixels, s.shot().unwrap().bitmap.pixels);
        assert_eq!(fs::read(&second).unwrap(), bytes);
    }

    #[test]
    fn a_failed_save_says_so() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be.
        let blocker = dir.path().join("taken");
        fs::write(&blocker, b"x").unwrap();
        let (mut s, _, _) = snapper(vec![]);
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (0, 0), (6, 4));
        s.finish(Instant::now());
        assert!(s.save(&blocker.join("sub"), STAMP).is_err());
        assert_eq!(s.shot().unwrap().status, ThumbStatus::SaveFailed);
    }

    #[test]
    fn the_thumb_times_out_after_six_seconds_and_hover_pauses_it() {
        let (mut s, _, _) = snapper(vec![]);
        let t0 = Instant::now();
        let sec = Duration::from_secs;
        assert_eq!(s.thumb_ms(t0), None, "nothing to time");
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (0, 0), (6, 4));
        s.finish(t0);
        assert_eq!(s.thumb_ms(t0), Some(6_005));
        assert!(!s.expired(t0 + sec(5)));
        // Hovering at 5 s pauses with 1 s left, for as long as it lasts.
        s.set_hover(true, t0 + sec(5));
        assert_eq!(s.thumb_ms(t0 + sec(5)), None);
        assert!(!s.expired(t0 + sec(60)));
        s.set_hover(false, t0 + sec(60));
        assert_eq!(s.thumb_ms(t0 + sec(60)), Some(1_005));
        assert!(s.expired(t0 + sec(61)));
        s.dismiss();
        assert!(s.shot().is_none(), "the crop is freed");
        assert!(!s.expired(t0 + sec(99)));
    }

    #[test]
    fn the_island_returns_to_rest_when_the_thumb_times_out() {
        use dj_core::island::{Island, IslandState, MusicUi};
        use dj_core::theme::{ThemeRegistry, DEFAULT_THEME};
        let tokens = ThemeRegistry::with_builtins()
            .get(DEFAULT_THEME)
            .unwrap()
            .clone();
        let mut island = Island::new(&tokens);
        island.set_music(
            MusicUi {
                signed_in: true,
                has_track: true,
                blocked: false,
            },
            &tokens,
        );
        let (mut s, _, _) = snapper(vec![]);
        let t0 = Instant::now();
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (0, 0), (6, 4));
        s.finish(t0);
        island.show_snap(&tokens);
        assert_eq!(island.state(), IslandState::SnapThumb);
        // What app.rs does when the one-shot timer fires.
        let fired = t0 + Duration::from_secs(THUMB_SECS);
        if s.expired(fired) {
            s.dismiss();
            island.hide_snap(&tokens);
        }
        assert_eq!(island.state(), IslandState::MusicCompact);
        assert!(s.shot().is_none());
    }

    #[test]
    fn a_new_snap_replaces_the_last_shot() {
        let (mut s, puts, _) = snapper(vec![]);
        s.begin(DESK, (0, 0)).unwrap();
        drag(&mut s, (0, 0), (6, 4));
        s.finish(Instant::now());
        s.begin(DESK, (0, 0)).unwrap();
        // While the overlay is up, the old shot is still there for its thumb.
        assert!(s.shot().is_some());
        drag(&mut s, (0, 0), (9, 9));
        s.finish(Instant::now());
        let b = &s.shot().unwrap().bitmap;
        assert_eq!((b.width, b.height), (9, 9));
        assert_eq!(puts.borrow().len(), 2);
    }
}
