//! Temporary front-layer lease for stay-behind windows. No platform or polling.
use std::time::{Duration, Instant};

pub const LEAVE_GRACE: Duration = Duration::from_millis(450);

#[derive(Debug, Default)]
pub struct Reveal {
    active: bool,
    inside: bool,
    deadline: Option<Instant>,
}

impl Reveal {
    pub fn reached(&mut self, now: Instant) {
        self.active = true;
        // Allow crossing the pill style's small top margin without immediately sinking.
        self.deadline = Some(now + LEAVE_GRACE);
    }

    pub fn pointer(&mut self, inside: bool, now: Instant) {
        if self.inside == inside {
            return;
        }
        self.inside = inside;
        if self.active {
            self.deadline = (!inside).then_some(now + LEAVE_GRACE);
        }
    }

    /// Existing panel/OSD policy retains priority; a pointer inside a panel also
    /// retains the front after that panel finishes collapsing.
    pub fn sync(&mut self, allowed: bool, needs_front: bool, now: Instant) -> bool {
        if !allowed {
            *self = Self::default();
            return false;
        }
        if needs_front {
            self.active = true;
            self.deadline = None;
        } else if self.active {
            if self.inside {
                self.deadline = None;
            } else {
                let deadline = *self.deadline.get_or_insert(now + LEAVE_GRACE);
                if now >= deadline {
                    self.active = false;
                    self.deadline = None;
                }
            }
        }
        self.active
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resting_desktop_is_never_hidden_or_spontaneously_raised() {
        let mut r = Reveal::default();
        let now = Instant::now();
        r.pointer(true, now);
        assert!(!r.sync(true, false, now));
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn edge_bridge_enter_leave_reenter_and_bounded_grace() {
        let mut r = Reveal::default();
        let now = Instant::now();
        r.reached(now);
        assert!(r.sync(true, false, now));
        r.pointer(true, now);
        assert!(r.sync(true, false, now + LEAVE_GRACE * 2));
        assert_eq!(r.deadline(), None);
        r.pointer(false, now);
        assert!(r.sync(true, false, now + LEAVE_GRACE / 2));
        r.pointer(true, now + LEAVE_GRACE / 2);
        assert!(r.sync(true, false, now + LEAVE_GRACE));
        r.pointer(false, now + LEAVE_GRACE);
        assert!(!r.sync(true, false, now + LEAVE_GRACE * 2));
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn edge_without_entry_expires_and_repeated_sync_does_not_extend() {
        let mut r = Reveal::default();
        let now = Instant::now();
        r.reached(now);
        for i in 0..450 {
            assert!(r.sync(true, false, now + Duration::from_millis(i)));
            assert_eq!(r.deadline(), Some(now + LEAVE_GRACE));
        }
        assert!(!r.sync(true, false, now + LEAVE_GRACE));
    }

    #[test]
    fn activity_holds_then_grace_starts_when_finished() {
        let mut r = Reveal::default();
        let now = Instant::now();
        assert!(r.sync(true, true, now));
        assert_eq!(r.deadline(), None);
        assert!(r.sync(true, false, now + LEAVE_GRACE));
        assert!(!r.sync(true, false, now + LEAVE_GRACE * 2));
    }

    #[test]
    fn closing_panel_under_pointer_holds_until_leave_then_expires() {
        let mut r = Reveal::default();
        let now = Instant::now();
        assert!(r.sync(true, true, now));
        r.pointer(true, now);
        assert!(r.sync(true, false, now + LEAVE_GRACE * 10));
        assert_eq!(r.deadline(), None);
        r.pointer(false, now + LEAVE_GRACE * 10);
        assert!(!r.sync(true, false, now + LEAVE_GRACE * 11));
    }

    #[test]
    fn hide_capture_fullscreen_and_setting_off_cancel_the_lease() {
        let now = Instant::now();
        let mut r = Reveal::default();
        r.reached(now);
        r.pointer(true, now);
        assert!(!r.sync(false, true, now));
        assert_eq!(r.deadline(), None);
        assert!(!r.sync(true, false, now));
    }
}
