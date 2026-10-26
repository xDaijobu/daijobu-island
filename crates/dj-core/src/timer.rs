//! Pomodoro: focus, break and long-break phases on a deadline. Time only enters as an
//! `Instant` argument and nothing ticks inside, so the remaining time is always derived
//! from `ends_at - now` and never drifts.

use std::time::{Duration, Instant};

/// Longest phase the config accepts, in minutes.
pub const MAX_MIN: i64 = 180;
/// Most focus sessions per long break the config accepts.
pub const MAX_EVERY: i64 = 12;

/// The panel's preset chips: (focus, break) minutes. The long break and `every` come
/// from the config.
pub const PRESETS: [(u32, u32); 3] = [(25, 5), (50, 10), (15, 3)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Focus,
    Break,
    LongBreak,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Focus => "focus",
            Phase::Break => "break",
            Phase::LongBreak => "long break",
        }
    }
}

/// Phase lengths and how many focus sessions come before the long break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub focus: Duration,
    pub brk: Duration,
    pub long_break: Duration,
    /// Every `every`-th focus is followed by the long break. At least 1.
    pub every: u32,
}

impl Settings {
    pub const fn minutes(focus: u32, brk: u32, long_break: u32, every: u32) -> Self {
        Self {
            focus: Duration::from_secs(focus as u64 * 60),
            brk: Duration::from_secs(brk as u64 * 60),
            long_break: Duration::from_secs(long_break as u64 * 60),
            every,
        }
    }

    pub fn duration(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Focus => self.focus,
            Phase::Break => self.brk,
            Phase::LongBreak => self.long_break,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::minutes(25, 5, 15, 4)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Running {
        ends_at: Instant,
    },
    Paused {
        left: Duration,
    },
    /// A phase ran out. The island waits for continue (starts `next`) or stop.
    Done {
        finished: Phase,
        next: Phase,
    },
}

/// Returned by `tick` when a phase runs out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Finished {
    pub phase: Phase,
    pub next: Phase,
}

/// The coarse state, for views that don't need the deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Running,
    Paused,
    Done,
}

/// Everything the painter draws for the timer, computed once per frame.
#[derive(Clone, Debug, PartialEq)]
pub struct TimerView {
    pub status: Status,
    /// The phase on the clock. In Done it is the next phase.
    pub phase: Phase,
    /// `mm:ss`, rounded up to the second.
    pub digits: String,
    /// 0..=1 of the phase elapsed.
    pub progress: f32,
    /// `focus 2/4`, `break`, `long break`, `paused` or `focus done`.
    pub label: String,
    /// In Done: `focus done · break 5:00`. Empty otherwise.
    pub alert: String,
    /// The preset chip that matches the settings, if any.
    pub preset: Option<usize>,
}

impl TimerView {
    /// Running or Paused: the timer has something on the clock.
    pub fn is_active(&self) -> bool {
        matches!(self.status, Status::Running | Status::Paused)
    }
}

impl Default for TimerView {
    fn default() -> Self {
        Pomodoro::new(Settings::default()).view(Instant::now())
    }
}

/// `mm:ss`, with the seconds rounded up so a fresh 25 min shows 25:00 and the clock
/// reads 00:00 only at the end. Minutes grow past two digits when needed.
pub fn fmt_clock(d: Duration) -> String {
    let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// Like `fmt_clock` without the leading zero on the minutes (`5:00`), for prose.
pub fn fmt_short(d: Duration) -> String {
    let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub struct Pomodoro {
    settings: Settings,
    /// The phase on the clock (in Done, the next one).
    phase: Phase,
    /// Focus sessions finished in this cycle. Cleared after the long break and on reset.
    focus_done: u32,
    state: State,
    /// Length of the current run when it was started or resumed. The remaining time is
    /// capped at it, so an `Instant` before the start never adds time.
    span: Duration,
}

impl Pomodoro {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            phase: Phase::Focus,
            focus_done: 0,
            state: State::Stopped,
            span: settings.focus,
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn focus_done(&self) -> u32 {
        self.focus_done
    }

    pub fn status(&self) -> Status {
        match self.state {
            State::Stopped => Status::Stopped,
            State::Running { .. } => Status::Running,
            State::Paused { .. } => Status::Paused,
            State::Done { .. } => Status::Done,
        }
    }

    /// Running or Paused.
    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Running { .. } | State::Paused { .. })
    }

    /// Applies preset `i` (focus and break minutes). Only while Stopped. True if applied.
    pub fn set_preset(&mut self, i: usize) -> bool {
        let Some(&(focus, brk)) = PRESETS.get(i) else {
            return false;
        };
        if self.state != State::Stopped {
            return false;
        }
        self.settings.focus = Duration::from_secs(u64::from(focus) * 60);
        self.settings.brk = Duration::from_secs(u64::from(brk) * 60);
        true
    }

    /// The preset whose focus and break match the settings.
    pub fn preset(&self) -> Option<usize> {
        PRESETS.iter().position(|&(focus, brk)| {
            self.settings.focus.as_secs() == u64::from(focus) * 60
                && self.settings.brk.as_secs() == u64::from(brk) * 60
        })
    }

    /// Stopped -> Running for the full phase.
    pub fn start(&mut self, now: Instant) -> bool {
        if self.state != State::Stopped {
            return false;
        }
        self.run(now, self.settings.duration(self.phase));
        true
    }

    pub fn pause(&mut self, now: Instant) -> bool {
        if !matches!(self.state, State::Running { .. }) {
            return false;
        }
        self.state = State::Paused {
            left: self.remaining(now),
        };
        true
    }

    pub fn resume(&mut self, now: Instant) -> bool {
        let State::Paused { left } = self.state else {
            return false;
        };
        self.run(now, left);
        true
    }

    /// Start, pause or resume, whichever applies. Does nothing in Done.
    pub fn toggle(&mut self, now: Instant) -> bool {
        match self.state {
            State::Stopped => self.start(now),
            State::Running { .. } => self.pause(now),
            State::Paused { .. } => self.resume(now),
            State::Done { .. } => false,
        }
    }

    /// Ends the running or paused phase now, without the alert, and moves on to the
    /// next one in the same run state (a paused skip stays paused).
    pub fn skip(&mut self, now: Instant) -> bool {
        let paused = match self.state {
            State::Running { .. } => false,
            State::Paused { .. } => true,
            State::Stopped | State::Done { .. } => return false,
        };
        self.advance();
        let full = self.settings.duration(self.phase);
        if paused {
            self.span = full;
            self.state = State::Paused { left: full };
        } else {
            self.run(now, full);
        }
        true
    }

    /// Back to Stopped at the first focus, counter cleared.
    pub fn reset(&mut self) -> bool {
        let changed =
            self.state != State::Stopped || self.phase != Phase::Focus || self.focus_done != 0;
        self.state = State::Stopped;
        self.phase = Phase::Focus;
        self.focus_done = 0;
        changed
    }

    /// Done -> Running the next phase.
    pub fn continue_next(&mut self, now: Instant) -> bool {
        if !matches!(self.state, State::Done { .. }) {
            return false;
        }
        // `phase` already holds the next phase while Done.
        self.run(now, self.settings.duration(self.phase));
        true
    }

    /// Done -> Stopped at focus. The counter is kept, unless the long break was next
    /// (then the cycle is over).
    pub fn stop(&mut self) -> bool {
        let State::Done { next, .. } = self.state else {
            return false;
        };
        if next == Phase::LongBreak {
            self.focus_done = 0;
        }
        self.phase = Phase::Focus;
        self.state = State::Stopped;
        true
    }

    /// Running and out of time: moves to Done and reports the phase that finished.
    pub fn tick(&mut self, now: Instant) -> Option<Finished> {
        if !matches!(self.state, State::Running { .. }) || !self.remaining(now).is_zero() {
            return None;
        }
        let done = self.advance();
        self.state = State::Done {
            finished: done.phase,
            next: done.next,
        };
        Some(done)
    }

    /// Time left on the clock, never negative and never more than the run's length.
    pub fn remaining(&self, now: Instant) -> Duration {
        match self.state {
            State::Stopped => self.settings.duration(self.phase),
            State::Running { ends_at } => ends_at.saturating_duration_since(now).min(self.span),
            State::Paused { left } => left,
            State::Done { .. } => Duration::ZERO,
        }
    }

    /// 0..=1 of the phase elapsed. 1 in Done, 0 while Stopped.
    pub fn progress(&self, now: Instant) -> f32 {
        match self.state {
            State::Stopped => 0.0,
            State::Done { .. } => 1.0,
            State::Running { .. } | State::Paused { .. } => {
                let total = self.settings.duration(self.phase).as_secs_f32();
                if total <= 0.0 {
                    return 1.0;
                }
                (1.0 - self.remaining(now).as_secs_f32() / total).clamp(0.0, 1.0)
            }
        }
    }

    pub fn label(&self) -> String {
        match self.state {
            State::Paused { .. } => "paused".to_string(),
            State::Done { finished, .. } => format!("{} done", finished.name()),
            State::Stopped | State::Running { .. } => match self.phase {
                Phase::Focus => {
                    let every = self.settings.every.max(1);
                    let n = (self.focus_done + 1).min(every);
                    format!("focus {n}/{every}")
                }
                other => other.name().to_string(),
            },
        }
    }

    /// `focus done · break 5:00` in Done, empty otherwise.
    pub fn alert_line(&self) -> String {
        match self.state {
            State::Done { finished, next } => format!(
                "{} done · {} {}",
                finished.name(),
                next.name(),
                fmt_short(self.settings.duration(next))
            ),
            _ => String::new(),
        }
    }

    /// When the running phase runs out, for the app's one-shot timer.
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            State::Running { ends_at } => Some(ends_at),
            _ => None,
        }
    }

    /// Runs `left` of the current phase from `now`.
    fn run(&mut self, now: Instant, left: Duration) {
        self.span = left;
        self.state = State::Running {
            ends_at: now + left,
        };
    }

    /// Finishes the current phase: counts a focus and picks the next phase.
    fn advance(&mut self) -> Finished {
        let phase = self.phase;
        let next = match phase {
            Phase::Focus => {
                self.focus_done += 1;
                if self.focus_done >= self.settings.every.max(1) {
                    Phase::LongBreak
                } else {
                    Phase::Break
                }
            }
            Phase::Break => Phase::Focus,
            Phase::LongBreak => {
                self.focus_done = 0;
                Phase::Focus
            }
        };
        self.phase = next;
        Finished { phase, next }
    }


    pub fn view(&self, now: Instant) -> TimerView {
        TimerView {
            status: self.status(),
            phase: self.phase,
            digits: fmt_clock(self.remaining(now)),
            progress: self.progress(now),
            label: self.label(),
            alert: self.alert_line(),
            preset: self.preset(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    /// A start point with room to go back in time.
    fn t0() -> Instant {
        Instant::now() + Duration::from_secs(24 * 3600)
    }

    fn pomo() -> Pomodoro {
        Pomodoro::new(Settings::default())
    }

    /// Runs the current phase to its end and returns what finished.
    fn run_out(p: &mut Pomodoro, now: &mut Instant) -> Finished {
        let end = p.next_deadline().expect("running");
        *now = end;
        p.tick(end).expect("finished at the deadline")
    }

    #[test]
    fn starts_stopped_at_the_first_focus() {
        let p = pomo();
        let now = t0();
        assert_eq!(p.state(), State::Stopped);
        assert_eq!(p.status(), Status::Stopped);
        assert_eq!(p.remaining(now), 25 * MIN);
        assert_eq!(p.progress(now), 0.0);
        assert_eq!(p.label(), "focus 1/4");
        assert_eq!(p.next_deadline(), None);
        assert!(!p.is_active());
        assert_eq!(p.view(now).digits, "25:00");
    }

    #[test]
    fn start_counts_down_from_the_deadline() {
        let mut p = pomo();
        let now = t0();
        assert!(p.start(now));
        assert!(!p.start(now), "already running");
        assert_eq!(p.next_deadline(), Some(now + 25 * MIN));
        let later = now + Duration::from_millis(1500);
        assert_eq!(p.remaining(later), 25 * MIN - Duration::from_millis(1500));
        assert_eq!(fmt_clock(p.remaining(later)), "24:59");
        assert!((p.progress(now + 5 * MIN) - 0.2).abs() < 1e-6);
        assert_eq!(p.tick(later), None);
        assert!(p.is_active());
    }

    #[test]
    fn pause_and_resume_keep_the_remainder() {
        let mut p = pomo();
        let now = t0();
        p.start(now);
        assert!(p.pause(now + 10 * MIN));
        assert_eq!(p.state(), State::Paused { left: 15 * MIN });
        assert_eq!(p.label(), "paused");
        assert_eq!(p.next_deadline(), None);
        // Time passing while paused changes nothing.
        assert_eq!(p.remaining(now + 60 * MIN), 15 * MIN);
        assert!(!p.pause(now + 60 * MIN));
        assert!(p.resume(now + 60 * MIN));
        assert_eq!(p.next_deadline(), Some(now + 75 * MIN));
        assert_eq!(p.remaining(now + 61 * MIN), 14 * MIN);
        // Toggle goes the same way round.
        assert!(p.toggle(now + 70 * MIN));
        assert_eq!(p.status(), Status::Paused);
        assert!(p.toggle(now + 70 * MIN));
        assert_eq!(p.status(), Status::Running);
    }

    #[test]
    fn remaining_is_never_negative_and_tick_finishes_once() {
        let mut p = pomo();
        let now = t0();
        p.start(now);
        assert_eq!(p.remaining(now + 99 * MIN), Duration::ZERO);
        let done = p.tick(now + 99 * MIN);
        assert_eq!(
            done,
            Some(Finished {
                phase: Phase::Focus,
                next: Phase::Break
            })
        );
        assert_eq!(
            p.state(),
            State::Done {
                finished: Phase::Focus,
                next: Phase::Break
            }
        );
        assert_eq!(p.tick(now + 100 * MIN), None);
        assert_eq!(p.progress(now), 1.0);
        assert_eq!(p.remaining(now), Duration::ZERO);
        assert_eq!(p.label(), "focus done");
        assert_eq!(p.alert_line(), "focus done · break 5:00");
        assert_eq!(p.focus_done(), 1);
        assert!(!p.toggle(now), "toggle does nothing in Done");
    }

    #[test]
    fn a_clock_going_backwards_never_adds_time() {
        let mut p = pomo();
        let now = t0();
        p.start(now);
        let before = now - 5 * MIN;
        assert_eq!(p.remaining(before), 25 * MIN);
        assert_eq!(p.progress(before), 0.0);
        assert_eq!(p.tick(before), None);
        // Pausing "before" the start keeps the full run, not more.
        p.pause(before);
        assert_eq!(p.state(), State::Paused { left: 25 * MIN });
        // After a resume, the cap is the remainder at the resume.
        p.resume(now);
        p.pause(now + 20 * MIN);
        p.resume(now + 30 * MIN);
        assert_eq!(p.remaining(now), 5 * MIN);
    }

    #[test]
    fn continue_starts_the_next_phase_and_stop_returns_to_focus() {
        let mut p = pomo();
        let mut now = t0();
        p.start(now);
        run_out(&mut p, &mut now);
        assert!(p.continue_next(now));
        assert_eq!(p.phase(), Phase::Break);
        assert_eq!(p.label(), "break");
        assert_eq!(p.next_deadline(), Some(now + 5 * MIN));
        assert!(!p.continue_next(now), "only from Done");
        assert!(!p.stop(), "only from Done");
        run_out(&mut p, &mut now);
        assert_eq!(p.alert_line(), "break done · focus 25:00");
        assert!(p.stop());
        assert_eq!(p.state(), State::Stopped);
        assert_eq!(p.phase(), Phase::Focus);
        // The counter is kept: the next focus is the second one.
        assert_eq!(p.label(), "focus 2/4");
    }

    #[test]
    fn every_fourth_focus_is_followed_by_the_long_break() {
        let mut p = pomo();
        let mut now = t0();
        p.start(now);
        let mut seen = Vec::new();
        for _ in 0..8 {
            seen.push(p.label());
            let f = run_out(&mut p, &mut now);
            seen.push(format!("{} -> {}", f.phase.name(), f.next.name()));
            p.continue_next(now);
        }
        assert_eq!(
            seen,
            [
                "focus 1/4",
                "focus -> break",
                "break",
                "break -> focus",
                "focus 2/4",
                "focus -> break",
                "break",
                "break -> focus",
                "focus 3/4",
                "focus -> break",
                "break",
                "break -> focus",
                "focus 4/4",
                "focus -> long break",
                "long break",
                "long break -> focus",
            ]
        );
        // A new cycle.
        assert_eq!(p.label(), "focus 1/4");
        assert_eq!(p.focus_done(), 0);
    }

    #[test]
    fn stopping_before_the_long_break_ends_the_cycle() {
        let mut p = Pomodoro::new(Settings::minutes(25, 5, 15, 1));
        let mut now = t0();
        p.start(now);
        let f = run_out(&mut p, &mut now);
        assert_eq!(f.next, Phase::LongBreak);
        assert_eq!(p.alert_line(), "focus done · long break 15:00");
        p.stop();
        assert_eq!(p.label(), "focus 1/1");
    }

    #[test]
    fn skip_moves_on_without_done_and_counts_the_focus() {
        let mut p = pomo();
        let now = t0();
        assert!(!p.skip(now), "nothing to skip while stopped");
        p.start(now);
        assert!(p.skip(now + MIN));
        assert_eq!(p.status(), Status::Running);
        assert_eq!(p.phase(), Phase::Break);
        assert_eq!(p.focus_done(), 1);
        assert_eq!(p.next_deadline(), Some(now + 6 * MIN));
        // A paused skip stays paused, with the full next phase.
        p.pause(now + 2 * MIN);
        assert!(p.skip(now + 2 * MIN));
        assert_eq!(p.state(), State::Paused { left: 25 * MIN });
        assert_eq!(p.phase(), Phase::Focus);
    }

    #[test]
    fn reset_clears_everything() {
        let mut p = pomo();
        let mut now = t0();
        p.start(now);
        run_out(&mut p, &mut now);
        p.continue_next(now);
        assert!(p.reset());
        assert_eq!(p.state(), State::Stopped);
        assert_eq!(p.phase(), Phase::Focus);
        assert_eq!(p.focus_done(), 0);
        assert!(!p.reset(), "already reset");
    }

    #[test]
    fn presets_apply_only_while_stopped_and_keep_the_long_break() {
        let mut p = Pomodoro::new(Settings::minutes(25, 5, 20, 3));
        let now = t0();
        assert_eq!(p.preset(), Some(0));
        assert!(p.set_preset(1));
        assert_eq!(p.settings(), Settings::minutes(50, 10, 20, 3));
        assert_eq!(p.preset(), Some(1));
        assert_eq!(p.remaining(now), 50 * MIN);
        assert!(!p.set_preset(7));
        p.start(now);
        assert!(!p.set_preset(2), "locked while running");
        p.pause(now);
        assert!(!p.set_preset(2), "locked while paused");
        p.reset();
        assert!(p.set_preset(2));
        assert_eq!(p.view(now).digits, "15:00");
        let odd = Pomodoro::new(Settings::minutes(30, 5, 15, 4));
        assert_eq!(odd.preset(), None);
    }

    #[test]
    fn clock_format_rounds_up() {
        assert_eq!(fmt_clock(Duration::ZERO), "00:00");
        assert_eq!(fmt_clock(Duration::from_millis(1)), "00:01");
        assert_eq!(fmt_clock(Duration::from_secs(59)), "00:59");
        assert_eq!(fmt_clock(Duration::from_millis(59_001)), "01:00");
        assert_eq!(fmt_clock(180 * MIN), "180:00");
        assert_eq!(fmt_short(5 * MIN), "5:00");
        assert_eq!(fmt_short(Duration::from_millis(59_001)), "1:00");
    }

    #[test]
    fn view_reports_the_state() {
        let mut p = pomo();
        let now = t0();
        p.start(now);
        let v = p.view(now + 30 * Duration::from_secs(1));
        assert_eq!(v.status, Status::Running);
        assert_eq!(v.digits, "24:30");
        assert_eq!(v.label, "focus 1/4");
        assert!(v.alert.is_empty());
        assert_eq!(v.preset, Some(0));
        assert!(v.is_active());
        assert!(!TimerView::default().is_active());
    }
}
