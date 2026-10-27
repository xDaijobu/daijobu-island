//! What `dj` scripts ask the island to show: one progress bar and one notice. Pure model:
//! time enters only as an `Instant` argument, nothing ticks inside, and nothing here
//! touches a pipe, a window or the clock (the app owns those; the wire format lives in
//! `dj-ipc`).
//!
//! - **Progress**: one at a time. `progress` sets or updates it (a new label replaces the
//!   old one) and starts its 30 minute staleness clock again, so a script that died does
//!   not leave the pill up for ever.
//! - **Done**: `done` turns the progress into a check mark and a message for `DONE_FOR`,
//!   with or without a progress before it, then everything goes. A later `progress`
//!   cancels a pending done.
//! - **Notice**: one at a time. `notify` replaces any notice and shows it for
//!   `NOTICE_FOR`; `dismiss_notice` ends it early.
//!
//! The view carries no clock: it shows the state as of the last call. The caller calls
//! `tick` when `next_change` is due and redraws when it says the view changed.

use std::time::{Duration, Instant};

/// The check mark and its message stay this long.
pub const DONE_FOR: Duration = Duration::from_secs(4);
/// A notice stays this long (a click dismisses it sooner).
pub const NOTICE_FOR: Duration = Duration::from_secs(5);
/// A progress nobody updated for this long goes away.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);

/// What `done` shows when it is given no message.
const DONE_TEXT: &str = "done";

/// The progress pill's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressView {
    /// Empty: the pill reads "working".
    pub label: String,
    /// 0..=100; None: open-ended (no fill, no percent).
    pub percent: Option<u8>,
    /// Some: the pill shows a check mark and this message instead of the bar.
    pub done: Option<String>,
}

/// Everything the island needs to show for dj.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DjView {
    pub progress: Option<ProgressView>,
    pub notice: Option<String>,
}

/// The progress pill and what ends it.
#[derive(Clone, Debug)]
struct Bar {
    label: String,
    percent: Option<u8>,
    /// The last `progress` or `done` call (the staleness clock).
    updated: Instant,
    done: Option<Done>,
}

#[derive(Clone, Debug)]
struct Done {
    message: String,
    until: Instant,
}

#[derive(Clone, Debug)]
struct Notice {
    message: String,
    until: Instant,
}

impl Bar {
    /// When the pill goes if nothing happens: the end of its check mark, else the moment
    /// it turns stale.
    fn ends_at(&self) -> Instant {
        match &self.done {
            Some(d) => d.until,
            None => self.updated + STALE_AFTER,
        }
    }
}

/// The island's dj activity.
#[derive(Clone, Debug, Default)]
pub struct Dj {
    bar: Option<Bar>,
    notice: Option<Notice>,
}

impl Dj {
    /// Sets or updates the progress. A label replaces the old one; an empty one keeps it
    /// (`dj progress 80` after `dj progress 40 deploy` is still "deploy"). `None` is
    /// open-ended. A pending done is cancelled. Percent is clamped to 100.
    pub fn progress(&mut self, percent: Option<u8>, label: String, now: Instant) {
        let label = match (&self.bar, label.is_empty()) {
            (Some(old), true) if old.done.is_none() => old.label.clone(),
            _ => label,
        };
        self.bar = Some(Bar {
            label,
            percent: percent.map(|p| p.min(100)),
            updated: now,
            done: None,
        });
    }

    /// Shows a check mark and `message` ("done" when blank) for `DONE_FOR`, then nothing.
    /// With a progress, its label and percent stay in the view; without one the view has an
    /// empty label and no percent. A second done replaces the message and starts over.
    pub fn done(&mut self, message: String, now: Instant) {
        let (label, percent) = self
            .bar
            .take()
            .map(|b| (b.label, b.percent))
            .unwrap_or_default();
        let message = if message.trim().is_empty() {
            DONE_TEXT.to_string()
        } else {
            message
        };
        self.bar = Some(Bar {
            label,
            percent,
            updated: now,
            done: Some(Done {
                message,
                until: now + DONE_FOR,
            }),
        });
    }

    /// Shows `message` for `NOTICE_FOR`, replacing any notice and starting over.
    pub fn notify(&mut self, message: String, now: Instant) {
        self.notice = Some(Notice {
            message,
            until: now + NOTICE_FOR,
        });
    }

    /// Ends the notice now. True if there was one.
    pub fn dismiss_notice(&mut self) -> bool {
        self.notice.take().is_some()
    }

    /// Drops the progress (or done) and the notice that timed out by `now`. True if the
    /// view changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let bar_due = self.bar.as_ref().is_some_and(|b| now >= b.ends_at());
        let notice_due = self.notice.as_ref().is_some_and(|n| now >= n.until);
        if bar_due {
            self.bar = None;
        }
        if notice_due {
            self.notice = None;
        }
        bar_due || notice_due
    }

    /// Time until something goes by itself: the soonest end of the progress (or done) and
    /// the notice. None when nothing will. Zero when one is already due: call `tick`.
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        let bar = self.bar.as_ref().map(Bar::ends_at);
        let notice = self.notice.as_ref().map(|n| n.until);
        bar.into_iter()
            .chain(notice)
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    /// What to show, as of the last call.
    pub fn view(&self) -> DjView {
        DjView {
            progress: self.bar.as_ref().map(|b| ProgressView {
                label: b.label.clone(),
                percent: b.percent,
                done: b.done.as_ref().map(|d| d.message.clone()),
            }),
            notice: self.notice.as_ref().map(|n| n.message.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn mins(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    fn label(s: &str) -> String {
        s.to_string()
    }

    /// The progress part of the view.
    fn bar(dj: &Dj) -> Option<ProgressView> {
        dj.view().progress
    }

    fn running(label: &str, percent: Option<u8>) -> Option<ProgressView> {
        Some(ProgressView {
            label: label.to_string(),
            percent,
            done: None,
        })
    }

    fn finished(label: &str, percent: Option<u8>, message: &str) -> Option<ProgressView> {
        Some(ProgressView {
            label: label.to_string(),
            percent,
            done: Some(message.to_string()),
        })
    }

    // ----- the plan's numbers ---------------------------------------------------------

    #[test]
    fn the_durations_are_the_contract() {
        assert_eq!(DONE_FOR, secs(4));
        assert_eq!(NOTICE_FOR, secs(5));
        assert_eq!(STALE_AFTER, mins(30));
    }

    #[test]
    fn a_new_model_shows_nothing_and_has_nothing_to_wait_for() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        assert_eq!(dj.view(), DjView::default());
        assert_eq!(dj.next_change(t0), None);
        assert!(!dj.tick(t0 + mins(500)));
        assert!(!dj.dismiss_notice());
    }

    // ----- progress -------------------------------------------------------------------

    #[test]
    fn progress_shows_the_bar() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("deploy"), t0);
        assert_eq!(bar(&dj), running("deploy", Some(40)));
        assert_eq!(dj.view().notice, None);
    }

    #[test]
    fn progress_updates_the_one_bar_in_place() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("deploy"), t0);
        dj.progress(Some(80), label("deploy"), t0 + secs(3));
        assert_eq!(bar(&dj), running("deploy", Some(80)));
        dj.progress(Some(100), label("deploy"), t0 + secs(6));
        assert_eq!(bar(&dj), running("deploy", Some(100)));
        assert!(!dj.tick(t0 + secs(7)), "an update is not a change to tick");
    }

    #[test]
    fn a_new_label_replaces_the_old_one() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("build"), t0);
        dj.progress(Some(10), label("test"), t0 + secs(1));
        assert_eq!(bar(&dj), running("test", Some(10)));
    }

    #[test]
    fn open_ended_replaces_a_percent_and_back() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("build"), t0);
        dj.progress(None, label("linking"), t0 + secs(1));
        assert_eq!(bar(&dj), running("linking", None));
        dj.progress(Some(5), label("linking"), t0 + secs(2));
        assert_eq!(bar(&dj), running("linking", Some(5)));
    }

    #[test]
    fn an_empty_label_keeps_the_running_one() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("deploy"), t0);
        dj.progress(Some(60), String::new(), t0 + secs(1));
        assert_eq!(bar(&dj), running("deploy", Some(60)));
        dj.progress(None, String::new(), t0 + secs(2));
        assert_eq!(bar(&dj), running("deploy", None));
        // With nothing running (or after a done), an empty label stays empty ("working").
        let mut fresh = Dj::default();
        fresh.progress(Some(10), String::new(), t0);
        assert_eq!(bar(&fresh), running("", Some(10)));
        fresh.done(String::new(), t0 + secs(1));
        fresh.progress(Some(20), String::new(), t0 + secs(2));
        assert_eq!(bar(&fresh), running("", Some(20)));
    }

    #[test]
    fn the_bounds_of_percent_are_kept_and_the_top_is_clamped() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(0), label("a"), t0);
        assert_eq!(bar(&dj), running("a", Some(0)));
        dj.progress(Some(100), label("a"), t0);
        assert_eq!(bar(&dj), running("a", Some(100)));
        dj.progress(Some(250), label("a"), t0);
        assert_eq!(bar(&dj), running("a", Some(100)), "over 100 is 100");
    }

    // ----- done -----------------------------------------------------------------------

    #[test]
    fn done_after_a_progress_shows_the_check_and_keeps_the_bar_facts() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(90), label("deploy"), t0);
        dj.done(label("build ok"), t0 + secs(2));
        assert_eq!(bar(&dj), finished("deploy", Some(90), "build ok"));
    }

    #[test]
    fn done_without_a_progress_still_shows_the_check() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.done(label("build ok"), t0);
        assert_eq!(bar(&dj), finished("", None, "build ok"));
        assert_eq!(dj.view().notice, None);
    }

    #[test]
    fn a_blank_done_message_reads_done() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.done(String::new(), t0);
        assert_eq!(bar(&dj), finished("", None, "done"));
        dj.done("  ".to_string(), t0);
        assert_eq!(bar(&dj), finished("", None, "done"));
    }

    #[test]
    fn done_lasts_exactly_done_for_then_nothing() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(100), label("deploy"), t0);
        dj.done(label("ok"), t0 + secs(10));
        assert!(!dj.tick(t0 + secs(10) + ms(3_999)));
        assert_eq!(bar(&dj), finished("deploy", Some(100), "ok"));
        assert!(dj.tick(t0 + secs(14)), "the view changed");
        assert_eq!(bar(&dj), None);
        assert_eq!(dj.next_change(t0 + secs(14)), None);
        assert!(!dj.tick(t0 + secs(99)), "and only once");
    }

    #[test]
    fn done_without_a_progress_times_out_the_same() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.done(label("ok"), t0);
        assert!(!dj.tick(t0 + ms(3_999)));
        assert!(dj.tick(t0 + secs(4)));
        assert_eq!(dj.view(), DjView::default());
    }

    #[test]
    fn a_second_done_replaces_the_message_and_starts_over() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.done(label("one"), t0);
        dj.done(label("two"), t0 + secs(3));
        assert_eq!(bar(&dj), finished("", None, "two"));
        assert!(!dj.tick(t0 + secs(4)), "the first done's end is gone");
        assert!(!dj.tick(t0 + ms(6_999)));
        assert!(dj.tick(t0 + secs(7)));
        assert_eq!(bar(&dj), None);
    }

    #[test]
    fn a_done_is_not_cut_short_by_the_stale_clock() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(99), label("deploy"), t0);
        // 1 s before the progress would have gone stale.
        dj.done(label("ok"), t0 + STALE_AFTER - secs(1));
        assert!(!dj.tick(t0 + STALE_AFTER + secs(1)), "still inside its 4 s");
        assert_eq!(bar(&dj), finished("deploy", Some(99), "ok"));
        assert!(dj.tick(t0 + STALE_AFTER + secs(3)));
        assert_eq!(bar(&dj), None);
    }

    #[test]
    fn progress_after_done_cancels_it() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(100), label("build"), t0);
        dj.done(label("ok"), t0 + secs(1));
        dj.progress(Some(5), label("deploy"), t0 + secs(2));
        assert_eq!(bar(&dj), running("deploy", Some(5)), "the check is gone");
        // The done's 4 s run out at t0 + 5 s: the progress must survive them.
        assert!(!dj.tick(t0 + secs(5)));
        assert!(!dj.tick(t0 + secs(60)));
        assert_eq!(bar(&dj), running("deploy", Some(5)));
        assert_eq!(dj.next_change(t0 + secs(60)), Some(STALE_AFTER - secs(58)));
    }

    #[test]
    fn progress_after_a_finished_done_starts_a_new_bar() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.done(label("ok"), t0);
        assert!(dj.tick(t0 + secs(4)));
        dj.progress(None, label("next"), t0 + secs(5));
        assert_eq!(bar(&dj), running("next", None));
    }

    // ----- notice ---------------------------------------------------------------------

    #[test]
    fn a_notice_shows_for_exactly_notice_for() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.notify(label("tests failed"), t0);
        assert_eq!(dj.view().notice.as_deref(), Some("tests failed"));
        assert_eq!(bar(&dj), None, "a notice is not a progress");
        assert!(!dj.tick(t0 + ms(4_999)));
        assert_eq!(dj.view().notice.as_deref(), Some("tests failed"));
        assert!(dj.tick(t0 + secs(5)));
        assert_eq!(dj.view(), DjView::default());
        assert!(!dj.tick(t0 + secs(50)), "and only once");
    }

    #[test]
    fn a_new_notice_replaces_the_old_one_and_restarts_the_clock() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.notify(label("first"), t0);
        dj.notify(label("second"), t0 + secs(3));
        assert_eq!(dj.view().notice.as_deref(), Some("second"));
        assert!(!dj.tick(t0 + secs(5)), "the first one's end is gone");
        assert!(!dj.tick(t0 + ms(7_999)));
        assert!(dj.tick(t0 + secs(8)));
        assert_eq!(dj.view().notice, None);
    }

    #[test]
    fn dismiss_ends_the_notice_early_and_says_so() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.notify(label("hello"), t0);
        assert!(dj.dismiss_notice());
        assert_eq!(dj.view().notice, None);
        assert!(!dj.dismiss_notice(), "nothing left to dismiss");
        assert!(!dj.tick(t0 + secs(5)), "dismissed: nothing times out later");
        assert_eq!(dj.next_change(t0), None);
    }

    #[test]
    fn dismiss_leaves_the_progress_alone() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("deploy"), t0);
        dj.notify(label("hello"), t0 + secs(1));
        assert!(dj.dismiss_notice());
        assert_eq!(bar(&dj), running("deploy", Some(40)));
        assert_eq!(dj.next_change(t0 + secs(1)), Some(STALE_AFTER - secs(1)));
    }

    #[test]
    fn a_notice_and_a_progress_do_not_touch_each_other() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("deploy"), t0);
        dj.notify(label("hello"), t0 + secs(1));
        dj.progress(Some(60), label("deploy"), t0 + secs(2));
        assert_eq!(dj.view().notice.as_deref(), Some("hello"));
        dj.done(label("ok"), t0 + secs(3));
        assert_eq!(dj.view().notice.as_deref(), Some("hello"));
        // The notice goes first (t0 + 6 s) and the done at t0 + 7 s.
        assert!(dj.tick(t0 + secs(6)));
        assert_eq!(dj.view().notice, None);
        assert_eq!(bar(&dj), finished("deploy", Some(60), "ok"));
        assert!(dj.tick(t0 + secs(7)));
        assert_eq!(dj.view(), DjView::default());
    }

    #[test]
    fn one_tick_can_drop_both() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.done(label("ok"), t0);
        dj.notify(label("hello"), t0);
        assert!(dj.tick(t0 + secs(10)));
        assert_eq!(dj.view(), DjView::default());
    }

    // ----- stale expiry ---------------------------------------------------------------

    #[test]
    fn a_progress_nobody_updates_goes_after_thirty_minutes() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(40), label("deploy"), t0);
        assert!(!dj.tick(t0 + STALE_AFTER - ms(1)));
        assert_eq!(bar(&dj), running("deploy", Some(40)));
        assert!(dj.tick(t0 + STALE_AFTER));
        assert_eq!(dj.view(), DjView::default());
        assert!(!dj.tick(t0 + STALE_AFTER + secs(1)), "and only once");
    }

    #[test]
    fn an_update_restarts_the_stale_clock() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(None, label("watch"), t0);
        dj.progress(None, label("watch"), t0 + mins(29));
        assert!(!dj.tick(t0 + mins(30)), "the first call's end is gone");
        assert!(!dj.tick(t0 + mins(59) - ms(1)));
        assert!(dj.tick(t0 + mins(59)));
        assert_eq!(bar(&dj), None);
    }

    #[test]
    fn open_ended_progress_goes_stale_too() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(None, label("watch"), t0);
        assert!(dj.tick(t0 + STALE_AFTER));
        assert_eq!(bar(&dj), None);
    }

    // ----- next_change ----------------------------------------------------------------

    #[test]
    fn next_change_of_a_progress_is_its_stale_time() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(10), label("a"), t0);
        assert_eq!(dj.next_change(t0), Some(STALE_AFTER));
        assert_eq!(dj.next_change(t0 + mins(10)), Some(mins(20)));
        dj.progress(Some(20), label("a"), t0 + mins(10));
        assert_eq!(dj.next_change(t0 + mins(10)), Some(STALE_AFTER));
    }

    #[test]
    fn next_change_of_a_done_and_of_a_notice() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.done(label("ok"), t0);
        assert_eq!(dj.next_change(t0), Some(DONE_FOR));
        assert_eq!(dj.next_change(t0 + ms(1_500)), Some(ms(2_500)));
        let mut dj = Dj::default();
        dj.notify(label("hi"), t0);
        assert_eq!(dj.next_change(t0), Some(NOTICE_FOR));
        assert_eq!(dj.next_change(t0 + secs(2)), Some(secs(3)));
    }

    #[test]
    fn next_change_is_the_soonest_end_and_moves_on_as_they_pass() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.progress(Some(50), label("deploy"), t0);
        // The notice (ends at t0 + 6 s) is sooner than the stale progress.
        dj.notify(label("hi"), t0 + secs(1));
        assert_eq!(dj.next_change(t0 + secs(1)), Some(NOTICE_FOR));
        // The done ends at t0 + 7 s, after the notice.
        dj.done(label("ok"), t0 + secs(3));
        assert_eq!(dj.next_change(t0 + secs(3)), Some(secs(3)), "the notice");
        assert!(dj.tick(t0 + secs(6)));
        assert_eq!(dj.next_change(t0 + secs(6)), Some(secs(1)), "then the done");
        assert!(dj.tick(t0 + secs(7)));
        assert_eq!(dj.next_change(t0 + secs(7)), None);
    }

    #[test]
    fn next_change_prefers_a_done_that_ends_before_the_notice() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.notify(label("hi"), t0 + secs(2));
        dj.done(label("ok"), t0);
        // Done ends t0 + 4 s, the notice t0 + 7 s.
        assert_eq!(dj.next_change(t0 + secs(2)), Some(secs(2)));
        assert!(dj.tick(t0 + secs(4)));
        assert_eq!(dj.next_change(t0 + secs(4)), Some(secs(3)));
    }

    #[test]
    fn next_change_is_zero_when_something_is_already_due() {
        let t0 = Instant::now();
        let mut dj = Dj::default();
        dj.notify(label("hi"), t0);
        assert_eq!(dj.next_change(t0 + secs(5)), Some(Duration::ZERO));
        assert_eq!(dj.next_change(t0 + secs(500)), Some(Duration::ZERO));
        assert!(dj.tick(t0 + secs(500)));
        assert_eq!(dj.next_change(t0 + secs(500)), None);
    }

    #[test]
    fn a_clock_that_seems_to_go_backwards_changes_nothing() {
        let t0 = Instant::now() + secs(100);
        let mut dj = Dj::default();
        dj.progress(Some(10), label("a"), t0);
        dj.notify(label("hi"), t0);
        assert!(!dj.tick(t0 - secs(50)));
        assert_eq!(dj.next_change(t0 - secs(50)), Some(NOTICE_FOR + secs(50)));
        assert_eq!(dj.view().notice.as_deref(), Some("hi"));
    }
}
