//! The app's side of Snap: freeze, select, annotate, render, copy, save, and the
//! thumb's countdown. The screen and the clipboard sit behind traits, so this runs in
//! unit tests with synthetic frames and a fake clipboard. The overlay window and its
//! renderer are glue in `app.rs`.
//!
//! Flow: `begin` freezes and starts selecting; `start_editing` (the release) hands the
//! selection to the annotation editor; `edit` feeds it input; `end` closes the session:
//! cancelled, or rendered with its markup and copied (and saved).
//!
//! Memory: the frozen frames live only while the overlay is up (`Session`). After that
//! only the annotated image and its PNG are kept (`Shot`), until the thumb goes. Both
//! are behind `Arc`s: the preview stack's cards share them (`Snapper::payload`) instead
//! of copying, so a shot costs its pixels and its PNG once however many owners it has.

use crate::preview_store::Payload;
use crate::storage;
use dj_core::config::SnapHotkey;
use dj_core::snap::{
    file_name, Bitmap, Countdown, IRect, MonitorFrame, Selector, Stamp, ThumbStatus, THUMB_SECS,
};
use dj_core::snap_edit::{Display, EditInput, Editor, Outcome, Prefs, Rgba, TextMeasure};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Hotkey ids: the app has 1 (toggle) and 2 (theme).
pub const HOTKEY_SNAP: i32 = 3;
/// Esc, registered only while the overlay is up, in case it didn't get the keyboard.
pub const HOTKEY_SNAP_ESC: i32 = 4;
/// `[snap] alt_hotkey`, the second shortcut (Ctrl+Space by default).
pub const HOTKEY_SNAP_ALT: i32 = 5;
/// One-shot: the thumb's countdown ends. App timer ids 1-6 are taken.
pub const TIMER_SNAP_THUMB: usize = 7;
/// One-shot: the island has finished collapsing, so the screen can be frozen.
pub const TIMER_SNAP_START: usize = 8;
/// How long after the collapse settles before the freeze (one composed frame or so).
pub const START_DELAY_MS: u32 = 60;

const VK_SNAPSHOT: u32 = 0x2C;
const VK_S: u32 = 0x53;
const VK_SPACE: u32 = 0x20;
pub const VK_ESCAPE: u32 = 0x1B;
/// A save never overwrites: `-2` … `-999`, then it gives up.
const MAX_NAME_TRIES: u32 = 999;

/// `(modifiers, virtual key)` for `RegisterHotKey`.
pub fn hotkey_keys(k: SnapHotkey) -> (u32, u32) {
    match k {
        SnapHotkey::PrtSc => (dj_win::window::MOD_NONE, VK_SNAPSHOT),
        SnapHotkey::CtrlShiftS => (dj_win::window::MOD_CTRL_SHIFT, VK_S),
        SnapHotkey::CtrlAltS => (dj_win::window::MOD_CTRL_ALT, VK_S),
        SnapHotkey::CtrlSpace => (dj_win::window::MOD_CTRL, VK_SPACE),
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

/// Measures text roughly (0.55 em per char) if the embedded font can't be loaded, so
/// the editor still works; the output is drawn with the real font either way.
pub struct RoughMeasure;

impl TextMeasure for RoughMeasure {
    fn width(&self, text: &str, px: f32) -> f32 {
        text.chars().count() as f32 * px * 0.55
    }

    fn line_height(&self, px: f32) -> f32 {
        px * 1.25
    }
}

/// While the overlay is up: the frozen monitors, the selection, and once it is released
/// the annotation editor.
pub struct Session {
    pub frames: Vec<MonitorFrame>,
    pub selector: Selector,
    pub editor: Option<Editor>,
    /// The tool, colour and size the editor starts from; they are written back only if
    /// the snap changed them.
    prefs: Prefs,
    accent: Rgba,
}

/// How a snap ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// Esc, ✕, a right click while selecting, or a too-small selection: nothing is
    /// copied or saved.
    Cancel,
    /// Ctrl+C, Enter or the copy button.
    Copy,
    /// Ctrl+S or the save button: copied, and saved into `dir`.
    Save { dir: PathBuf, stamp: Stamp },
}

/// What `end` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ended {
    /// The prefs, if this snap changed them: write them to `[snap.annotate]`.
    pub prefs: Option<Prefs>,
    /// A new shot was made (and copied, if the clipboard allowed): show the thumb.
    pub taken: bool,
    /// What went wrong (drawing, copying, saving), for the log.
    pub errors: Vec<String>,
}

/// The last snap, kept for the thumb's copy and save until the thumb goes.
pub struct Shot {
    /// The annotated image. Shared (never copied) with the preview stack's card.
    pub bitmap: Arc<Bitmap>,
    /// None if encoding failed; copy and save try again. Shared like `bitmap`.
    png: Option<Arc<[u8]>>,
    pub status: ThumbStatus,
    /// Where the last save went. The thumb's save button then reads "open" and reveals
    /// this file instead of saving a second copy.
    pub saved: Option<PathBuf>,
    countdown: Countdown,
}

pub struct Snapper {
    screen: Box<dyn Screen>,
    clipboard: Box<dyn Clipboard>,
    /// The editor's text measurer (the font the markup is drawn with).
    measure: Box<dyn TextMeasure>,
    session: Option<Session>,
    shot: Option<Shot>,
}

impl Snapper {
    pub fn new(
        screen: Box<dyn Screen>,
        clipboard: Box<dyn Clipboard>,
        measure: Box<dyn TextMeasure>,
    ) -> Self {
        Self {
            screen,
            clipboard,
            measure,
            session: None,
            shot: None,
        }
    }

    /// Freezes every monitor and starts a selection over `desktop` at `cursor`. Click
    /// targets are the windows, then the monitors (a click on the bare desktop takes
    /// the monitor under it). `prefs` and `accent` are for the editor that follows.
    pub fn begin(
        &mut self,
        desktop: IRect,
        cursor: (i32, i32),
        prefs: Prefs,
        accent: Rgba,
    ) -> Result<&Session, String> {
        let frames = self.screen.freeze()?;
        let mut targets = self.screen.windows();
        targets.extend(frames.iter().map(|f| f.rect));
        let selector = Selector::new(desktop, targets, cursor);
        Ok(self.session.insert(Session {
            frames,
            selector,
            editor: None,
            prefs,
            accent,
        }))
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// The selecting phase's release. True if the editor now has the selection (a drag
    /// of at least 2 x 2, or a window/monitor click); false if it was too small, and
    /// the app ends the snap. Nothing touches the clipboard here.
    pub fn start_editing(&mut self) -> bool {
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        if s.editor.is_some() {
            return true;
        }
        let Some(rect) = s.selector.release() else {
            return false;
        };
        let displays = s
            .frames
            .iter()
            .map(|f| Display {
                rect: f.rect,
                scale: f.dpi as f32 / 96.0,
            })
            .collect();
        s.editor = Some(Editor::new(
            rect,
            s.selector.desktop(),
            displays,
            s.prefs,
            s.accent,
            s.selector.cursor(),
        ));
        true
    }

    /// Editor input (desktop px). Nothing while selecting.
    pub fn edit(&mut self, ev: EditInput) -> Outcome {
        let measure = self.measure.as_ref();
        match self.session.as_mut().and_then(|s| s.editor.as_mut()) {
            Some(e) => e.input(ev, measure),
            None => Outcome::Nothing,
        }
    }

    /// Ends the session (the frozen frames go). A copy or save renders the selection
    /// with its markup, replaces the last shot with it, copies it (PNG + `CF_DIB`) and
    /// starts the thumb's countdown; a save also writes it to `dir`. A cancel copies
    /// and saves nothing. Either way, changed prefs are handed back once.
    pub fn end(&mut self, how: End, now: Instant) -> Ended {
        let mut ended = Ended::default();
        let Some(session) = self.session.take() else {
            return ended;
        };
        let Some(editor) = session.editor.as_ref() else {
            return ended;
        };
        if editor.prefs() != session.prefs {
            ended.prefs = Some(editor.prefs());
        }
        let save = match how {
            End::Cancel => return ended,
            End::Copy => None,
            End::Save { dir, stamp } => Some((dir, stamp)),
        };
        let bitmap =
            match dj_render::render_output(&session.frames, editor.selection(), editor.shapes()) {
                Ok(b) => b,
                Err(e) => {
                    // Never fall back to the bare crop: pixelated secrets would leak.
                    ended.errors.push(format!("cannot draw the markup: {e}"));
                    return ended;
                }
            };
        drop(session);
        let mut shot = Shot {
            bitmap: Arc::new(bitmap),
            png: None,
            status: ThumbStatus::Copied,
            saved: None,
            countdown: Countdown::start(Duration::from_secs(THUMB_SECS), now),
        };
        if let Err(e) = copy_shot(&mut shot, self.clipboard.as_mut()) {
            ended.errors.push(format!("copy failed: {e}"));
        }
        self.shot = Some(shot);
        ended.taken = true;
        if let Some((dir, stamp)) = save {
            if let Err(e) = self.save(&dir, stamp) {
                ended.errors.push(format!("save failed: {e}"));
            }
        }
        ended
    }

    pub fn shot(&self) -> Option<&Shot> {
        self.shot.as_ref()
    }

    /// The current shot's image and PNG for a preview card: clones of the `Arc`s, so
    /// nothing is copied and the bytes are counted once. None without a shot. The card
    /// keeps them after the thumb goes (`dismiss`), and the thumb after the card does.
    pub fn payload(&self) -> Option<Payload> {
        let shot = self.shot.as_ref()?;
        Some(Payload {
            bitmap: Arc::clone(&shot.bitmap),
            png: shot.png.clone(),
        })
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
        shot.png = Some(Arc::from(dj_capture::png::encode(&shot.bitmap)?));
    }
    Ok(shot.png.as_deref().unwrap_or_default())
}

fn copy_shot(shot: &mut Shot, clipboard: &mut dyn Clipboard) -> Result<(), String> {
    let bitmap = Arc::clone(&shot.bitmap);
    let result = ensure_png(shot).and_then(|png| put_image(&bitmap, png, clipboard));
    shot.status = if result.is_ok() {
        ThumbStatus::Copied
    } else {
        ThumbStatus::CopyFailed
    };
    result
}

/// Puts an image on the clipboard as its PNG plus a `CF_DIB` made from `bitmap`. The
/// thumb's copy and the preview cards' copy both end here.
pub(crate) fn put_image(
    bitmap: &Bitmap,
    png: &[u8],
    clipboard: &mut dyn Clipboard,
) -> Result<(), String> {
    clipboard.put(png, &dj_capture::dib::from_bitmap(bitmap))
}

/// Writes `bytes` to the first free `snap-…png` name in `dir`. `create_new` makes the
/// check and the create one step, so nothing is ever overwritten. Used by the thumb's
/// save and by the preview store's worker.
pub(crate) fn write_new(dir: &Path, stamp: Stamp, bytes: &[u8]) -> Result<PathBuf, String> {
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
    use dj_core::snap::crop;
    use dj_core::snap_edit::{Key, KeyCode, Size, Tool, SWATCHES};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Two synthetic monitors, the second left of the first at negative x; pixel (x, y)
    /// of a monitor is [seed, x, y, 255] (wrapping at 256).
    fn layout(primary: (i32, i32), left_w: i32) -> Vec<MonitorFrame> {
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
            mon(IRect::new(0, 0, primary.0, primary.1), 1),
            mon(IRect::new(-left_w, 0, left_w, primary.1), 2),
        ]
    }

    /// The small layout of the 7a tests.
    fn frames() -> Vec<MonitorFrame> {
        layout((64, 48), 32)
    }

    const DESK: IRect = IRect::new(-32, 0, 96, 48);
    /// A layout big enough for the editor's toolbar.
    const BIG_DESK: IRect = IRect::new(-400, 0, 1600, 800);

    struct FakeScreen {
        frames: Vec<MonitorFrame>,
        windows: Vec<IRect>,
        freezes: Rc<RefCell<u32>>,
    }

    impl Screen for FakeScreen {
        fn freeze(&mut self) -> Result<Vec<MonitorFrame>, String> {
            *self.freezes.borrow_mut() += 1;
            Ok(self.frames.clone())
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

    fn snapper_on(
        frames: Vec<MonitorFrame>,
        windows: Vec<IRect>,
    ) -> (Snapper, Puts, Rc<RefCell<bool>>) {
        let clip = FakeClipboard::default();
        let (puts, fail) = (clip.puts.clone(), clip.fail.clone());
        let screen = FakeScreen {
            frames,
            windows,
            freezes: Rc::default(),
        };
        (
            Snapper::new(Box::new(screen), Box::new(clip), Box::new(RoughMeasure)),
            puts,
            fail,
        )
    }

    fn snapper(windows: Vec<IRect>) -> (Snapper, Puts, Rc<RefCell<bool>>) {
        snapper_on(frames(), windows)
    }

    fn begin(s: &mut Snapper, desktop: IRect) {
        s.begin(desktop, (0, 0), Prefs::default(), SWATCHES[3])
            .unwrap();
    }

    fn drag(s: &mut Snapper, from: (i32, i32), to: (i32, i32)) {
        let sel = &mut s.session_mut().unwrap().selector;
        sel.press(from.0, from.1);
        sel.move_to(to.0, to.1);
    }

    /// The release, then a copy: what Ctrl+C right after the release does.
    fn finish(s: &mut Snapper, now: Instant) -> Ended {
        if !s.start_editing() {
            return s.end(End::Cancel, now);
        }
        s.end(End::Copy, now)
    }

    fn key(vk: u16, ctrl: bool) -> EditInput {
        EditInput::Key(Key {
            code: KeyCode::from_vk(vk),
            ctrl,
            shift: false,
        })
    }

    /// Selects (100, 100)-(400, 300) on the big layout and draws a red rect from
    /// (150, 150) to (250, 250) in it.
    fn annotated() -> (Snapper, Puts) {
        let (mut s, puts, _) = snapper_on(layout((1200, 800), 400), vec![]);
        begin(&mut s, BIG_DESK);
        drag(&mut s, (100, 100), (400, 300));
        assert!(s.start_editing());
        assert_eq!(s.edit(key(0x52, false)), Outcome::Redraw, "R: rect");
        s.edit(EditInput::Press { x: 150, y: 150 });
        s.edit(EditInput::Motion { x: 250, y: 250 });
        s.edit(EditInput::Release { x: 250, y: 250 });
        assert_eq!(
            s.session().unwrap().editor.as_ref().unwrap().shapes().len(),
            1
        );
        (s, puts)
    }

    fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width + x) * 4) as usize;
        [
            b.pixels[i],
            b.pixels[i + 1],
            b.pixels[i + 2],
            b.pixels[i + 3],
        ]
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
        assert_eq!(hotkey_keys(SnapHotkey::CtrlSpace), (0x0002, 0x20));
    }

    #[test]
    fn ids_are_free() {
        // Hotkeys 1 and 2 belong to app.rs; timers 1, 2, 4 to app.rs, 3 to vitals.rs and
        // 5, 6 to timer.rs.
        for id in [HOTKEY_SNAP, HOTKEY_SNAP_ESC, HOTKEY_SNAP_ALT] {
            assert!(![1, 2].contains(&id));
        }
        assert_ne!(HOTKEY_SNAP, HOTKEY_SNAP_ESC);
        assert_ne!(HOTKEY_SNAP_ALT, HOTKEY_SNAP);
        assert_ne!(HOTKEY_SNAP_ALT, HOTKEY_SNAP_ESC);
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
    fn release_enters_editing_and_the_clipboard_is_untouched() {
        let (mut s, puts, _) = snapper(vec![]);
        begin(&mut s, DESK);
        drag(&mut s, (-2, 3), (2, 5));
        assert!(s.start_editing());
        let editor = s.session().unwrap().editor.as_ref().unwrap();
        assert_eq!(editor.selection(), IRect::new(-2, 3, 4, 2));
        assert_eq!(editor.tool(), Tool::Arrow, "from the prefs");
        assert!(puts.borrow().is_empty());
        assert!(s.shot().is_none());
        // While editing the release is idempotent and the frames stay.
        assert!(s.start_editing());
        assert!(s.session().is_some());
    }

    #[test]
    fn a_copy_without_markup_puts_the_exact_pixels_as_png_and_dib() {
        let (mut s, puts, _) = snapper(vec![]);
        begin(&mut s, DESK);
        // Across the seam between the two monitors.
        drag(&mut s, (-2, 3), (2, 5));
        let ended = finish(&mut s, Instant::now());
        assert!(ended.taken && ended.errors.is_empty() && ended.prefs.is_none());
        assert!(s.session().is_none(), "the frozen frames are gone");
        let shot = s.shot().unwrap();
        assert_eq!((shot.bitmap.width, shot.bitmap.height), (4, 2));
        assert_eq!(*shot.bitmap, crop(&frames(), IRect::new(-2, 3, 4, 2)));
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
    fn ctrl_c_copies_the_annotated_png_and_dib_and_shows_the_thumb() {
        let (mut s, puts) = annotated();
        assert!(puts.borrow().is_empty(), "drawing touches nothing");
        assert_eq!(s.edit(key(0x43, true)), Outcome::Copy);
        let ended = s.end(End::Copy, Instant::now());
        assert!(ended.taken, "the thumb shows");
        assert!(ended.errors.is_empty(), "{:?}", ended.errors);
        let shot = s.shot().unwrap();
        assert_eq!(shot.status, ThumbStatus::Copied);
        let b = &shot.bitmap;
        assert_eq!((b.width, b.height), (300, 200));
        // The rect's left edge (desktop x 150) is output x 50, in red.
        let red = SWATCHES[0];
        assert_eq!(px(b, 50, 100), [red.b, red.g, red.r, 255]);
        // Inside it: the frozen screen, desktop (200, 200).
        assert_eq!(px(b, 100, 100), [1, 200, 200, 255]);
        let plain = crop(&layout((1200, 800), 400), IRect::new(100, 100, 300, 200));
        assert_ne!(**b, plain);
        assert_eq!(px(b, 10, 10), px(&plain, 10, 10));
        let puts = puts.borrow();
        assert_eq!(puts.len(), 1);
        let (png, dib) = &puts[0];
        assert_eq!(dj_capture::png::decode(png).unwrap().pixels, b.pixels);
        assert_eq!(*dib, dj_capture::dib::from_bitmap(b));
    }

    #[test]
    fn ctrl_s_saves_and_copies() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Pictures").join("Daijobu");
        let (mut s, puts) = annotated();
        assert_eq!(s.edit(key(0x53, true)), Outcome::Save);
        let ended = s.end(
            End::Save {
                dir: target.clone(),
                stamp: STAMP,
            },
            Instant::now(),
        );
        assert!(ended.taken && ended.errors.is_empty(), "{:?}", ended.errors);
        let shot = s.shot().unwrap();
        let path = target.join("snap-2026-10-26_140309.png");
        assert_eq!(shot.saved.as_ref(), Some(&path));
        assert_eq!(shot.status, ThumbStatus::Saved);
        let saved = dj_capture::png::decode(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.pixels, shot.bitmap.pixels, "the annotated image");
        assert_eq!(puts.borrow().len(), 1, "and copied");
        assert_eq!(fs::read(&path).unwrap(), puts.borrow()[0].0);
    }

    #[test]
    fn esc_while_editing_copies_nothing() {
        let (mut s, puts) = annotated();
        assert_eq!(s.edit(key(0x1B, false)), Outcome::Cancel);
        let ended = s.end(End::Cancel, Instant::now());
        assert!(!ended.taken);
        assert!(s.session().is_none());
        assert!(s.shot().is_none());
        assert!(puts.borrow().is_empty());
        // Typing first: Esc commits the text and stays; the second one cancels.
        let (mut s, puts) = annotated();
        s.edit(key(0x54, false));
        s.edit(EditInput::Press { x: 300, y: 200 });
        s.edit(EditInput::Release { x: 300, y: 200 });
        s.edit(EditInput::Char('a'));
        assert_eq!(s.edit(key(0x1B, false)), Outcome::Redraw);
        assert_eq!(s.edit(key(0x1B, false)), Outcome::Cancel);
        s.end(End::Cancel, Instant::now());
        assert!(puts.borrow().is_empty());
    }

    #[test]
    fn prefs_come_back_once_on_close_and_only_when_they_changed() {
        let (mut s, _, _) = snapper_on(layout((1200, 800), 400), vec![]);
        begin(&mut s, BIG_DESK);
        drag(&mut s, (100, 100), (400, 300));
        s.start_editing();
        s.edit(key(0x50, false));
        s.edit(EditInput::Wheel(1));
        let ended = s.end(End::Cancel, Instant::now());
        let want = Prefs {
            tool: Tool::Pen,
            size: Size::L,
            ..Prefs::default()
        };
        assert_eq!(ended.prefs, Some(want), "a cancel still keeps the choice");
        // Only once: the session is gone.
        assert_eq!(s.end(End::Cancel, Instant::now()), Ended::default());
        // The next snap starts from them; no change means nothing to write.
        s.begin(BIG_DESK, (0, 0), want, SWATCHES[3]).unwrap();
        drag(&mut s, (100, 100), (400, 300));
        s.start_editing();
        assert_eq!(s.session().unwrap().editor.as_ref().unwrap().prefs(), want);
        let ended = s.end(End::Copy, Instant::now());
        assert_eq!(ended.prefs, None);
        assert!(ended.taken);
        // A snap cancelled while still selecting has nothing to write either.
        begin(&mut s, BIG_DESK);
        assert_eq!(s.end(End::Cancel, Instant::now()).prefs, None);
    }

    #[test]
    fn a_click_takes_the_window_under_the_cursor_or_the_monitor() {
        let win = IRect::new(4, 4, 10, 8);
        let (mut s, _, _) = snapper(vec![win]);
        begin(&mut s, DESK);
        s.session_mut().unwrap().selector.press(6, 6);
        finish(&mut s, Instant::now());
        let b = &s.shot().unwrap().bitmap;
        assert_eq!((b.width, b.height), (10, 8));
        // A click on the bare left monitor takes all of it.
        begin(&mut s, DESK);
        s.session_mut().unwrap().selector.press(-10, 40);
        finish(&mut s, Instant::now());
        let b = &s.shot().unwrap().bitmap;
        assert_eq!((b.width, b.height), (32, 48));
    }

    #[test]
    fn cancel_and_a_tiny_drag_never_touch_the_clipboard() {
        let (mut s, puts, _) = snapper(vec![]);
        begin(&mut s, DESK);
        drag(&mut s, (1, 1), (30, 30));
        s.end(End::Cancel, Instant::now());
        assert!(s.session().is_none());
        assert_eq!(s.end(End::Copy, Instant::now()), Ended::default());
        begin(&mut s, DESK);
        drag(&mut s, (10, 10), (40, 40));
        drag(&mut s, (10, 10), (11, 40));
        assert!(!s.start_editing(), "1 px wide");
        assert!(!finish(&mut s, Instant::now()).taken);
        assert!(s.session().is_none());
        assert!(puts.borrow().is_empty());
        assert!(s.shot().is_none());
    }

    #[test]
    fn a_failed_copy_keeps_the_shot_for_a_retry() {
        let (mut s, puts, fail) = snapper(vec![]);
        *fail.borrow_mut() = true;
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (8, 8));
        let ended = finish(&mut s, Instant::now());
        assert!(ended.taken);
        assert_eq!(ended.errors, vec!["copy failed: busy".to_string()]);
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
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (6, 4));
        finish(&mut s, Instant::now());
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
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (6, 4));
        assert!(s.start_editing());
        let ended = s.end(
            End::Save {
                dir: blocker.join("sub"),
                stamp: STAMP,
            },
            Instant::now(),
        );
        assert!(ended.taken, "still copied");
        assert!(
            ended.errors[0].starts_with("save failed"),
            "{:?}",
            ended.errors
        );
        assert_eq!(s.shot().unwrap().status, ThumbStatus::SaveFailed);
    }

    #[test]
    fn the_thumb_times_out_after_six_seconds_and_hover_pauses_it() {
        let (mut s, _, _) = snapper(vec![]);
        let t0 = Instant::now();
        let sec = Duration::from_secs;
        assert_eq!(s.thumb_ms(t0), None, "nothing to time");
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (6, 4));
        finish(&mut s, t0);
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
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (6, 4));
        finish(&mut s, t0);
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
    fn the_payload_shares_the_shots_image_and_png() {
        let (mut s, _, _) = snapper(vec![]);
        assert!(s.payload().is_none(), "no shot yet");
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (8, 6));
        finish(&mut s, Instant::now());
        let shot = s.shot().unwrap();
        let png = shot.png.clone().expect("the copy encoded it");
        assert_eq!(Arc::strong_count(&shot.bitmap), 1);
        let p = s.payload().unwrap();
        let shot = s.shot().unwrap();
        assert!(Arc::ptr_eq(&p.bitmap, &shot.bitmap), "same pixels, no copy");
        assert!(Arc::ptr_eq(p.png.as_ref().unwrap(), &png), "same PNG");
        assert_eq!(Arc::strong_count(&shot.bitmap), 2);
        // Counted once: the payload's size is the pixels plus the PNG, not a second set.
        assert_eq!(p.bytes(), 8 * 6 * 4 + png.len() as u64);
        // The card outlives the thumb, and the thumb's copy and save still work.
        let image = Arc::downgrade(&p.bitmap);
        s.dismiss();
        assert!(s.payload().is_none());
        assert!(image.upgrade().is_some(), "the card still holds the image");
        drop(p);
        drop(png);
        assert!(image.upgrade().is_none(), "freed with its last owner");
    }

    #[test]
    fn the_thumbs_save_and_copy_use_the_shared_png() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, puts, _) = snapper(vec![]);
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (8, 6));
        finish(&mut s, Instant::now());
        let p = s.payload().unwrap();
        s.copy().unwrap();
        let path = s.save(dir.path(), STAMP).unwrap();
        let png = p.png.as_ref().unwrap();
        assert_eq!(fs::read(&path).unwrap(), **png);
        assert_eq!(puts.borrow().len(), 2);
        assert_eq!(puts.borrow()[1].0, **png);
        assert!(Arc::ptr_eq(png, s.payload().unwrap().png.as_ref().unwrap()));
    }

    #[test]
    fn a_new_snap_replaces_the_last_shot() {
        let (mut s, puts, _) = snapper(vec![]);
        begin(&mut s, DESK);
        drag(&mut s, (0, 0), (6, 4));
        finish(&mut s, Instant::now());
        begin(&mut s, DESK);
        // While the overlay is up, the old shot is still there for its thumb.
        assert!(s.shot().is_some());
        drag(&mut s, (0, 0), (9, 9));
        finish(&mut s, Instant::now());
        let b = &s.shot().unwrap().bitmap;
        assert_eq!((b.width, b.height), (9, 9));
        assert_eq!(puts.borrow().len(), 2);
    }
}
