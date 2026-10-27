//! The dj CLI on the island's side: what each request from the pipe does, and the one-shot
//! timer that ends a done message, a notice or a stale progress. No Win32 here, so the
//! tests drive it with plain requests; `App` owns the pipe server and the timer.

use dj_core::dj::{Dj, DjView};
use dj_core::island::DjUi;
use dj_core::theme::ThemeRegistry;
use dj_ipc::{Reply, Request};
use std::time::Instant;

/// One-shot: a done message, a notice or a stale progress is due.
pub const TIMER_DJ: usize = 13;

/// The theme list stops before its reply could outgrow a frame.
const LIST_MAX: usize = 3000;

/// What the app does for a request besides the pills, after it has replied.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Switch to this theme (it exists).
    Theme(String),
    /// Start a snap, as PrtSc does.
    Snap,
}

/// What a request may look at in the app.
pub struct Context<'a> {
    pub themes: &'a ThemeRegistry,
    pub current: &'a str,
    /// A snap is already up or about to start.
    pub snapping: bool,
}

/// What to do with `TIMER_DJ`.
#[derive(Debug, PartialEq, Eq)]
pub enum Arm {
    /// (Re)arm it to fire in this many ms.
    Set(u32),
    Kill,
    Keep,
}

/// The dj activity and the instant the one-shot is armed for.
#[derive(Default)]
pub struct Activity {
    dj: Dj,
    armed_for: Option<Instant>,
}

impl Activity {
    /// Answers `request`. The pills change here (push `ui` to the island after); a theme or
    /// a snap is the app's to do (`Effect`).
    pub fn handle(&mut self, request: Request, cx: &Context, now: Instant) -> (Reply, Effect) {
        match request {
            Request::Progress { percent, label } => {
                self.dj.progress(percent, label, now);
                (Reply::ok(), Effect::None)
            }
            Request::Done { message } => {
                self.dj.done(message, now);
                (Reply::ok(), Effect::None)
            }
            Request::Notify { message } => {
                self.dj.notify(message, now);
                (Reply::ok(), Effect::None)
            }
            Request::Theme { id: None } => (Reply::text(theme_list(cx)), Effect::None),
            Request::Theme { id: Some(id) } => {
                if cx.themes.get(&id).is_some() {
                    (Reply::ok(), Effect::Theme(id))
                } else {
                    let ids = cx.themes.ids().join(", ");
                    (
                        Reply::error(format!("unknown theme \"{id}\" (themes: {ids})")),
                        Effect::None,
                    )
                }
            }
            Request::Snap if cx.snapping => (Reply::error("a snap is already open"), Effect::None),
            Request::Snap => (Reply::ok(), Effect::Snap),
        }
    }

    /// A click on the notice pill. True if one was up.
    pub fn dismiss_notice(&mut self) -> bool {
        self.dj.dismiss_notice()
    }

    /// `TIMER_DJ` fired (it is one-shot: the app killed it): drops what timed out. True if
    /// the pills changed. A timer that fired early is simply armed again by `plan_timer`.
    pub fn fired(&mut self, now: Instant) -> bool {
        self.armed_for = None;
        self.dj.tick(now)
    }

    /// What the island shows now (its state is `DjProgress` / `DjNotice` and it is on
    /// screen): a done message or a notice counts its time from here.
    pub fn on_screen(&mut self, progress: bool, notice: bool, now: Instant) {
        self.dj.on_screen(progress, notice, now);
    }

    /// Arms the one-shot for the next timeout, if it isn't armed for it already. The
    /// timeout is a fixed instant, so events while it is due don't keep pushing it back.
    pub fn plan_timer(&mut self, now: Instant) -> Arm {
        let due = self.dj.next_at();
        if due == self.armed_for {
            return Arm::Keep;
        }
        self.armed_for = due;
        match due {
            // Rounded up, so it never fires before the timeout is due.
            Some(at) => {
                let ms = at.saturating_duration_since(now).as_micros().div_ceil(1000);
                Arm::Set(ms.clamp(1, u128::from(u32::MAX)) as u32)
            }
            None => Arm::Kill,
        }
    }

    /// Which pills are up, for the island.
    pub fn ui(&self) -> DjUi {
        let v = self.dj.view();
        DjUi {
            progress: v.progress.is_some(),
            notice: v.notice.is_some(),
        }
    }

    pub fn view(&self) -> DjView {
        self.dj.view()
    }
}

/// `dj theme`: one line per theme, the current one marked with `*`.
fn theme_list(cx: &Context) -> String {
    let ids = cx.themes.ids();
    let width = ids.iter().map(|i| i.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for (n, id) in ids.iter().enumerate() {
        let mark = if id == cx.current { '*' } else { ' ' };
        let name = cx.themes.name_of(id).unwrap_or("");
        let line = format!("{mark} {id:<width$}  {name}");
        if out.len() + line.len() > LIST_MAX {
            out.push_str(&format!("  ... and {} more", ids.len() - n));
            break;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// What the pills draw: the live view, except that a pill fading out keeps what it showed
/// (`layers` are the island's `dj_progress` and `dj_notice` layer values).
pub fn hold(shown: &mut DjView, live: DjView, layers: (f32, f32)) {
    if live.progress.is_some() || layers.0 <= 0.0 {
        shown.progress = live.progress;
    }
    if live.notice.is_some() || layers.1 <= 0.0 {
        shown.notice = live.notice;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::dj::{ProgressView, DONE_FOR, NOTICE_FOR, STALE_AFTER};
    use dj_core::island::{Island, IslandState};
    use dj_core::theme::DEFAULT_THEME;
    use std::time::Duration;

    fn registry() -> ThemeRegistry {
        ThemeRegistry::with_builtins()
    }

    fn cx(themes: &ThemeRegistry) -> Context<'_> {
        Context {
            themes,
            current: DEFAULT_THEME,
            snapping: false,
        }
    }

    fn progress(percent: Option<u8>, label: &str) -> Request {
        Request::Progress {
            percent,
            label: label.to_string(),
        }
    }

    /// The island after pushing `a`'s pills, settled.
    fn island_for(a: &Activity) -> Island {
        let t = registry().get(DEFAULT_THEME).unwrap().clone();
        let mut i = Island::new(&t);
        i.set_dj(a.ui(), &t);
        i
    }

    #[test]
    fn progress_shows_the_progress_pill_and_updates_it() {
        let themes = registry();
        let mut a = Activity::default();
        let now = Instant::now();
        let (reply, effect) = a.handle(progress(Some(40), "deploy"), &cx(&themes), now);
        assert_eq!((reply, effect), (Reply::ok(), Effect::None));
        assert_eq!(island_for(&a).state(), IslandState::DjProgress);
        a.handle(progress(Some(80), ""), &cx(&themes), now);
        assert_eq!(
            a.view().progress,
            Some(ProgressView {
                label: "deploy".into(),
                percent: Some(80),
                done: None
            })
        );
    }

    #[test]
    fn done_shows_the_check_then_goes_on_the_timer() {
        let themes = registry();
        let mut a = Activity::default();
        let t0 = Instant::now();
        a.handle(progress(Some(40), "build"), &cx(&themes), t0);
        a.handle(
            Request::Done {
                message: "build ok".into(),
            },
            &cx(&themes),
            t0,
        );
        assert_eq!(
            a.view().progress.and_then(|p| p.done).as_deref(),
            Some("build ok")
        );
        // Until the pill is on screen only the stale cap is pending.
        assert_eq!(a.plan_timer(t0), Arm::Set(STALE_AFTER.as_millis() as u32));
        // On screen: the one-shot is armed for the done timeout, and only once.
        a.on_screen(true, false, t0);
        assert_eq!(a.plan_timer(t0), Arm::Set(DONE_FOR.as_millis() as u32));
        assert_eq!(a.plan_timer(t0 + Duration::from_millis(5)), Arm::Keep);
        // Fired early: nothing changes and it is armed again for the rest.
        let early = t0 + DONE_FOR - Duration::from_millis(20);
        assert!(!a.fired(early));
        assert_eq!(a.plan_timer(early), Arm::Set(20));
        assert!(a.fired(t0 + DONE_FOR));
        assert_eq!(a.ui(), DjUi::default());
        assert_eq!(island_for(&a).state(), IslandState::Idle);
        // The app killed the one-shot when it fired, and nothing is due: leave it.
        assert_eq!(a.plan_timer(t0 + DONE_FOR), Arm::Keep);
    }

    #[test]
    fn the_timer_is_killed_when_nothing_is_left_to_time_out() {
        let themes = registry();
        let mut a = Activity::default();
        let t0 = Instant::now();
        a.handle(
            Request::Notify {
                message: "hello".into(),
            },
            &cx(&themes),
            t0,
        );
        a.on_screen(false, true, t0);
        assert_eq!(a.plan_timer(t0), Arm::Set(NOTICE_FOR.as_millis() as u32));
        // Dismissed by a click before it timed out.
        assert!(a.dismiss_notice());
        assert_eq!(a.plan_timer(t0), Arm::Kill);
        assert_eq!(a.plan_timer(t0), Arm::Keep);
    }

    #[test]
    fn a_due_timeout_is_armed_once_however_many_events_come() {
        let themes = registry();
        let mut a = Activity::default();
        let t0 = Instant::now();
        a.handle(
            Request::Notify {
                message: "hello".into(),
            },
            &cx(&themes),
            t0,
        );
        a.on_screen(false, true, t0);
        assert_eq!(a.plan_timer(t0), Arm::Set(NOTICE_FOR.as_millis() as u32));
        // Mouse moves (events) after the notice is due, before WM_TIMER arrives.
        for ms in [5_000, 5_004, 5_020, 6_000] {
            assert_eq!(
                a.plan_timer(t0 + Duration::from_millis(ms)),
                Arm::Keep,
                "{ms}"
            );
        }
        assert!(a.fired(t0 + Duration::from_millis(6_001)));
    }

    #[test]
    fn a_notice_behind_a_panel_keeps_its_time() {
        let themes = registry();
        let mut a = Activity::default();
        let t0 = Instant::now();
        a.handle(
            Request::Notify {
                message: "the agent needs input".into(),
            },
            &cx(&themes),
            t0,
        );
        // Home is open for 20 s: the notice waits under it and isn't on screen.
        a.on_screen(false, false, t0 + Duration::from_secs(20));
        assert!(!a.fired(t0 + Duration::from_secs(20)));
        // The panel collapses and the notice shows for its full 5 s.
        let shown = t0 + Duration::from_secs(21);
        a.on_screen(false, true, shown);
        assert_eq!(a.plan_timer(shown), Arm::Set(NOTICE_FOR.as_millis() as u32));
        assert!(!a.fired(shown + NOTICE_FOR - Duration::from_millis(1)));
        assert!(a.fired(shown + NOTICE_FOR));
    }

    #[test]
    fn done_without_a_progress_still_shows_the_pill() {
        let themes = registry();
        let mut a = Activity::default();
        a.handle(
            Request::Done {
                message: String::new(),
            },
            &cx(&themes),
            Instant::now(),
        );
        assert!(a.ui().progress);
        assert_eq!(island_for(&a).state(), IslandState::DjProgress);
    }

    #[test]
    fn notify_shows_the_notice_and_a_click_dismisses_it() {
        let themes = registry();
        let mut a = Activity::default();
        let t0 = Instant::now();
        a.handle(progress(Some(10), "sync"), &cx(&themes), t0);
        a.handle(
            Request::Notify {
                message: "tests failed".into(),
            },
            &cx(&themes),
            t0,
        );
        // The notice outranks the progress.
        assert_eq!(island_for(&a).state(), IslandState::DjNotice);
        a.on_screen(false, true, t0);
        assert_eq!(a.plan_timer(t0), Arm::Set(NOTICE_FOR.as_millis() as u32));
        assert!(a.dismiss_notice());
        assert!(!a.dismiss_notice());
        assert_eq!(island_for(&a).state(), IslandState::DjProgress);
        // Only the stale progress is left to time out.
        assert_eq!(a.plan_timer(t0), Arm::Set(STALE_AFTER.as_millis() as u32));
    }

    #[test]
    fn theme_switches_lists_and_rejects() {
        let themes = registry();
        let mut a = Activity::default();
        let now = Instant::now();
        let other = themes
            .ids()
            .iter()
            .find(|i| *i != DEFAULT_THEME)
            .unwrap()
            .clone();
        let (reply, effect) = a.handle(
            Request::Theme {
                id: Some(other.clone()),
            },
            &cx(&themes),
            now,
        );
        assert_eq!((reply, effect), (Reply::ok(), Effect::Theme(other)));

        let (reply, effect) = a.handle(
            Request::Theme {
                id: Some("nope".into()),
            },
            &cx(&themes),
            now,
        );
        assert_eq!(effect, Effect::None);
        assert!(!reply.ok);
        let error = reply.error.unwrap();
        assert!(error.contains("\"nope\""), "{error}");
        for id in themes.ids() {
            assert!(error.contains(id.as_str()), "{error}");
        }

        let (reply, effect) = a.handle(Request::Theme { id: None }, &cx(&themes), now);
        assert_eq!(effect, Effect::None);
        assert!(reply.ok);
        let text = reply.text.unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), themes.ids().len());
        for (line, id) in lines.iter().zip(themes.ids()) {
            let marked = line.starts_with('*');
            assert_eq!(marked, id == DEFAULT_THEME, "{line}");
            assert!(line[2..].starts_with(id.as_str()), "{line}");
            assert!(line.ends_with(themes.name_of(id).unwrap()), "{line}");
        }
        // Nothing the pills show changed.
        assert_eq!(a.ui(), DjUi::default());
    }

    #[test]
    fn a_long_theme_list_stays_inside_a_frame() {
        let src = include_str!("../../crates/dj-core/themes/clean.toml");
        let files =
            (0..400).map(|n| (format!("user-theme-with-a-long-id-{n:03}"), src.to_string()));
        let mut themes = registry();
        themes.set_user_themes(files);
        assert!(themes.ids().len() > 100, "{:?}", themes.rejected().first());
        let text = theme_list(&cx(&themes));
        assert!(text.len() <= LIST_MAX + 40, "{}", text.len());
        assert!(text.ends_with("more"), "{text}");
        let frame = dj_ipc::proto::encode_reply(&Reply::text(text));
        assert!(frame.len() <= dj_ipc::proto::MAX_FRAME, "{}", frame.len());
    }

    #[test]
    fn snap_starts_a_snap_unless_one_is_open() {
        let themes = registry();
        let mut a = Activity::default();
        let now = Instant::now();
        assert_eq!(
            a.handle(Request::Snap, &cx(&themes), now),
            (Reply::ok(), Effect::Snap)
        );
        let busy = Context {
            snapping: true,
            ..cx(&themes)
        };
        let (reply, effect) = a.handle(Request::Snap, &busy, now);
        assert_eq!(effect, Effect::None);
        assert!(!reply.ok && reply.error.is_some());
    }

    #[test]
    fn a_fading_pill_keeps_what_it_showed() {
        let shown_progress = ProgressView {
            label: "deploy".into(),
            percent: Some(100),
            done: Some("ok".into()),
        };
        let mut shown = DjView {
            progress: Some(shown_progress.clone()),
            notice: Some("hello".into()),
        };
        // Both ended, both layers still fading: keep them.
        hold(&mut shown, DjView::default(), (0.4, 0.2));
        assert_eq!(shown.progress, Some(shown_progress));
        assert_eq!(shown.notice.as_deref(), Some("hello"));
        // Faded out: dropped.
        hold(&mut shown, DjView::default(), (0.0, 0.0));
        assert_eq!(shown, DjView::default());
        // A live one always wins.
        let live = DjView {
            progress: None,
            notice: Some("new".into()),
        };
        hold(&mut shown, live.clone(), (0.0, 0.5));
        assert_eq!(shown, live);
    }
}
