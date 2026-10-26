//! The app's side of the Pomodoro: applies panel actions, plays the chime once when a
//! session ends, and says which window timers to arm. Win32 stays in `app.rs`; this
//! part is plain logic, so it runs in unit tests with a fake chime.
//!
//! Timers: one one-shot `WM_TIMER` at the deadline (kept while the island is hidden),
//! and a one-shot redraw at the next change of the digits, only while the timer runs
//! and its digits are on screen. Nothing is armed while Stopped, Paused or Done.

use crate::chime::Chime;
use dj_core::island::{TimerAction, TimerUi};
use dj_core::timer::{Finished, Pomodoro, Settings, Status, TimerView};
use std::time::{Duration, Instant};

/// One-shot: the running phase reaches its deadline. App timer ids 1-4 are taken.
pub const TIMER_DEADLINE: usize = 5;
/// One-shot: the digits change (at most once a second).
pub const TIMER_DIGITS: usize = 6;

/// A `WM_TIMER` can come a little late; this much slack also keeps it from landing a
/// hair before the boundary it waits for.
const SLACK_MS: u64 = 5;

/// What to arm: milliseconds from now, or None to kill the timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub deadline_ms: Option<u32>,
    pub digits_ms: Option<u32>,
}

pub struct Timer {
    pomo: Pomodoro,
    chime: Box<dyn Chime>,
    /// Config `[timer] chime`.
    chime_on: bool,
}

impl Timer {
    pub fn new(settings: Settings, chime_on: bool, chime: Box<dyn Chime>) -> Self {
        Self {
            pomo: Pomodoro::new(settings),
            chime,
            chime_on,
        }
    }

    /// What the island needs to know.
    pub fn ui(&self) -> TimerUi {
        TimerUi {
            active: self.pomo.is_active(),
            done: self.pomo.status() == Status::Done,
        }
    }

    pub fn view(&self, now: Instant) -> TimerView {
        self.pomo.view(now)
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.pomo.next_deadline()
    }

    /// Checks the deadline. When the phase just ran out, plays the chime (if on) and
    /// reports it. Safe to call early or twice: it only fires once per session.
    pub fn fire(&mut self, now: Instant) -> Option<Finished> {
        let done = self.pomo.tick(now)?;
        if self.chime_on {
            self.chime.play();
        }
        Some(done)
    }

    /// Applies a panel or alert control. True if anything changed.
    pub fn apply(&mut self, action: TimerAction, now: Instant) -> bool {
        let p = &mut self.pomo;
        match action {
            TimerAction::Toggle => p.toggle(now),
            TimerAction::Skip => p.skip(now),
            TimerAction::Reset => p.reset(),
            TimerAction::Preset(i) => p.set_preset(i),
            TimerAction::Continue => p.continue_next(now),
            TimerAction::Stop => p.stop(),
        }
    }

    /// The window timers for this moment. `digits_on_screen`: the island is visible and
    /// shows the timer's digits (Timer compact or panel, or Music compact with a timer).
    pub fn plan(&self, now: Instant, digits_on_screen: bool) -> Plan {
        let Some(deadline) = self.pomo.next_deadline() else {
            return Plan {
                deadline_ms: None,
                digits_ms: None,
            };
        };
        let to_deadline = deadline.saturating_duration_since(now);
        let digits_ms = digits_on_screen.then(|| {
            // The digits round up, so they change when the remainder crosses a second.
            let left = self.pomo.remaining(now);
            let into_second = Duration::from_nanos(u64::from(left.subsec_nanos()));
            let wait = if into_second.is_zero() {
                Duration::from_secs(1)
            } else {
                into_second
            };
            ms(wait)
        });
        Plan {
            deadline_ms: Some(ms(to_deadline)),
            digits_ms,
        }
    }
}

/// Rounded-up milliseconds plus slack, clamped to what `SetTimer` takes.
fn ms(d: Duration) -> u32 {
    let whole = d.as_millis() as u64 + u64::from(d.subsec_nanos() % 1_000_000 > 0);
    (whole + SLACK_MS).min(u64::from(u32::MAX / 2)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    const MIN: Duration = Duration::from_secs(60);

    struct FakeChime(Rc<Cell<u32>>);

    impl Chime for FakeChime {
        fn play(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn timer(chime_on: bool) -> (Timer, Rc<Cell<u32>>) {
        let plays = Rc::new(Cell::new(0));
        let t = Timer::new(
            Settings::default(),
            chime_on,
            Box::new(FakeChime(plays.clone())),
        );
        (t, plays)
    }

    fn t0() -> Instant {
        Instant::now() + Duration::from_secs(3600)
    }

    #[test]
    fn chime_plays_once_when_a_session_ends() {
        let (mut t, plays) = timer(true);
        let now = t0();
        t.apply(TimerAction::Toggle, now);
        assert_eq!(t.fire(now + MIN), None, "not yet");
        assert_eq!(plays.get(), 0);
        let end = t.deadline().unwrap();
        assert!(t.fire(end).is_some());
        assert_eq!(t.fire(end + MIN), None);
        assert_eq!(plays.get(), 1);
        assert_eq!(
            t.ui(),
            TimerUi {
                active: false,
                done: true
            }
        );
        t.apply(TimerAction::Continue, end);
        assert_eq!(
            t.ui(),
            TimerUi {
                active: true,
                done: false
            }
        );
    }

    #[test]
    fn no_chime_when_turned_off_or_skipped() {
        let (mut t, plays) = timer(false);
        let now = t0();
        t.apply(TimerAction::Toggle, now);
        assert!(t.fire(now + 30 * MIN).is_some());
        assert_eq!(plays.get(), 0);

        let (mut t, plays) = timer(true);
        t.apply(TimerAction::Toggle, now);
        assert!(t.apply(TimerAction::Skip, now));
        assert_eq!(t.fire(now), None);
        assert_eq!(plays.get(), 0);
    }

    #[test]
    fn nothing_is_armed_unless_running() {
        let (mut t, _) = timer(true);
        let now = t0();
        let none = Plan {
            deadline_ms: None,
            digits_ms: None,
        };
        assert_eq!(t.plan(now, true), none, "stopped");
        t.apply(TimerAction::Toggle, now);
        t.apply(TimerAction::Toggle, now + MIN);
        assert_eq!(t.plan(now + MIN, true), none, "paused on screen");
        assert_eq!(t.plan(now + MIN, false), none, "paused off screen");
        t.apply(TimerAction::Toggle, now + MIN);
        t.fire(now + 30 * MIN);
        assert_eq!(t.plan(now + 30 * MIN, true), none, "done");
    }

    #[test]
    fn running_arms_the_deadline_and_digits_only_on_screen() {
        let (mut t, _) = timer(true);
        let now = t0();
        t.apply(TimerAction::Toggle, now);
        let at = now + Duration::from_millis(1_250);
        let hidden = t.plan(at, false);
        assert_eq!(hidden.deadline_ms, Some(25 * 60_000 - 1_250 + 5));
        assert_eq!(hidden.digits_ms, None);
        // 24:58.75 left: the digits turn to 24:58 in 750 ms.
        let shown = t.plan(at, true);
        assert_eq!(shown.digits_ms, Some(755));
        // On a whole second, the next change is a full second away.
        assert_eq!(t.plan(now, true).digits_ms, Some(1_005));
    }

    #[test]
    fn timer_ids_are_free() {
        // app.rs uses 1 (clock), 2 (VU) and 4 (Sweep done); vitals.rs uses 3.
        let taken = [1, 2, 4, crate::vitals::TIMER];
        for id in [TIMER_DEADLINE, TIMER_DIGITS] {
            assert!(!taken.contains(&id), "{id}");
        }
        assert_ne!(TIMER_DEADLINE, TIMER_DIGITS);
    }

    #[test]
    fn presets_only_change_while_stopped() {
        let (mut t, _) = timer(true);
        let now = t0();
        assert!(t.apply(TimerAction::Preset(1), now));
        assert_eq!(t.view(now).digits, "50:00");
        t.apply(TimerAction::Toggle, now);
        assert!(!t.apply(TimerAction::Preset(0), now));
        assert!(t.apply(TimerAction::Reset, now));
        assert_eq!(t.view(now).digits, "50:00", "the preset stays until restart");
    }
}
