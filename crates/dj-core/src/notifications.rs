//! Bounded, memory-only notification admission. Platform handles and executable
//! actions never enter this model. Internal producers own activation token routing.

use crate::layout::Rect;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const HISTORY_LIMIT: usize = 64;
pub const QUEUE_LIMIT: usize = 8;
pub const TOAST_DURATION: Duration = Duration::from_secs(6);
pub const TOAST_W: f32 = 360.0;
pub const TOAST_H: f32 = 72.0;
pub const PANEL_W: f32 = 440.0;
pub const PANEL_H: f32 = 340.0;
pub const CLOSE: Rect = Rect::new(394.0, 12.0, 32.0, 28.0);
pub const ENABLE: Rect = Rect::new(16.0, 48.0, 408.0, 32.0);
pub const PREV: Rect = Rect::new(16.0, 304.0, 64.0, 24.0);
pub const NEXT: Rect = Rect::new(360.0, 304.0, 64.0, 24.0);
pub const PAGE_SIZE: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NotificationKey {
    Windows(u32),
    Internal(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivationToken(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Activation {
    #[default]
    None,
    /// The platform adapter resolves the key to OS-provided app identity.
    OpenSourceApp,
    /// An opaque token resolved by trusted app code, never by notification text.
    Internal(ActivationToken),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationInput {
    pub key: NotificationKey,
    pub source: String,
    pub title: String,
    pub body: String,
    pub activation: Activation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Inserted,
    Updated,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    #[default]
    Disabled,
    MissingIdentity,
    NeedsConsent,
    Requesting,
    Connecting,
    Denied,
    Listening,
    Unavailable,
}

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Windows notifications are off",
            Self::MissingIdentity => "Windows notification setup required",
            Self::NeedsConsent => "Select Allow access to request Windows permission",
            Self::Requesting => "Waiting for Windows permission",
            Self::Connecting => "Connecting to Windows notification events",
            Self::Denied => "Denied: allow notification access in Windows Settings",
            Self::Listening => "Listening · memory only · Windows toasts still appear",
            Self::Unavailable => "Listener unavailable; disable and enable to retry",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Close,
    Toggle,
    RequestAccess,
    Setup,
    Restart,
    Dismiss(NotificationKey),
    Activate(NotificationKey),
    Previous,
    Next,
}

pub fn row(index: usize) -> Rect {
    Rect::new(16.0, 92.0 + index as f32 * 68.0, 408.0, 62.0)
}

pub fn dismiss(index: usize) -> Rect {
    let r = row(index);
    Rect::new(r.x + r.w - 32.0, r.y, 32.0, r.h)
}

#[derive(Default)]
pub struct NotificationCenter {
    windows_enabled: bool,
    internal_enabled: bool,
    history: VecDeque<NotificationInput>,
    queue: VecDeque<NotificationKey>,
    current: Option<(NotificationKey, Instant)>,
    pub page: usize,
}

impl NotificationCenter {
    pub fn configure_internal(&mut self, enabled: bool) {
        self.internal_enabled = enabled;
        if !enabled {
            self.history
                .retain(|n| !matches!(n.key, NotificationKey::Internal(_)));
            self.queue
                .retain(|k| !matches!(k, NotificationKey::Internal(_)));
            if self
                .current
                .is_some_and(|(k, _)| matches!(k, NotificationKey::Internal(_)))
            {
                self.current = None;
            }
            self.clamp_page();
        }
    }

    pub fn configure_windows(&mut self, enabled: bool) {
        self.windows_enabled = enabled;
        if !enabled {
            self.history
                .retain(|n| !matches!(n.key, NotificationKey::Windows(_)));
            self.queue
                .retain(|k| !matches!(k, NotificationKey::Windows(_)));
            if self
                .current
                .is_some_and(|(k, _)| matches!(k, NotificationKey::Windows(_)))
            {
                self.current = None;
            }
            self.clamp_page();
        }
    }

    pub fn admit(&mut self, mut input: NotificationInput, _now: Instant) -> Admission {
        if (!self.windows_enabled && matches!(input.key, NotificationKey::Windows(_)))
            || (!self.internal_enabled && matches!(input.key, NotificationKey::Internal(_)))
        {
            return Admission::Disabled;
        }
        // Text is untrusted presentation only. Bound each field, remove controls
        // including directional overrides, and never interpret markup/links.
        input.source = bounded_text(&input.source, 96);
        input.title = bounded_text(&input.title, 256);
        input.body = bounded_text(&input.body, 1024);
        // Internal producers cannot manufacture OS activation and Windows cannot
        // manufacture internal activation tokens.
        input.activation = match (input.key, input.activation) {
            (NotificationKey::Internal(_), Activation::OpenSourceApp)
            | (NotificationKey::Windows(_), Activation::Internal(_)) => Activation::None,
            (_, a) => a,
        };
        if let Some(existing) = self.history.iter_mut().find(|n| n.key == input.key) {
            *existing = input;
            return Admission::Updated;
        }
        self.queue.push_back(input.key);
        self.history.push_front(input);
        while self.queue.len() > QUEUE_LIMIT {
            self.queue.pop_front();
        }
        while self.history.len() > HISTORY_LIMIT {
            let key = self.history.back().unwrap().key;
            self.remove(key);
        }
        Admission::Inserted
    }

    /// Hydrate current Windows history on startup without replaying old toasts.
    pub fn admit_history(&mut self, input: NotificationInput, now: Instant) -> Admission {
        let key = input.key;
        let result = self.admit(input, now);
        self.queue.retain(|k| *k != key);
        result
    }

    pub fn remove(&mut self, key: NotificationKey) {
        self.history.retain(|n| n.key != key);
        self.queue.retain(|k| *k != key);
        if self.current.is_some_and(|(k, _)| k == key) {
            self.current = None;
        }
        self.clamp_page();
    }

    /// Hide a toast without dismissing its history entry or the Windows toast.
    pub fn hide_toast(&mut self) {
        self.current = None;
    }

    /// Only run while the island can present a toast. Panels leave queued items
    /// waiting; no per-frame timer or idle polling is necessary.
    pub fn advance(&mut self, now: Instant, can_present: bool) {
        if self.current.is_some_and(|(_, end)| now >= end) {
            self.current = None;
        }
        if can_present && self.current.is_none() {
            while let Some(key) = self.queue.pop_front() {
                if self.get(key).is_some() {
                    self.current = Some((key, now + TOAST_DURATION));
                    break;
                }
            }
        }
    }

    pub fn toast(&self) -> Option<&NotificationInput> {
        self.current.and_then(|(key, _)| self.get(key))
    }

    pub fn toast_hit(&self, x: f32, y: f32) -> Option<Action> {
        if !Rect::new(0.0, 0.0, TOAST_W, TOAST_H).contains(x, y) {
            return None;
        }
        let key = self.toast()?.key;
        Some(if x >= TOAST_W - 34.0 {
            Action::Dismiss(key)
        } else {
            Action::Activate(key)
        })
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.current.map(|(_, end)| end)
    }

    pub fn get(&self, key: NotificationKey) -> Option<&NotificationInput> {
        self.history.iter().find(|n| n.key == key)
    }

    pub fn activate(&self, key: NotificationKey) -> Option<Activation> {
        self.get(key)
            .map(|n| n.activation)
            .filter(|a| *a != Activation::None)
    }

    pub fn history(&self) -> &VecDeque<NotificationInput> {
        &self.history
    }

    pub fn pages(&self) -> usize {
        self.history.len().div_ceil(PAGE_SIZE).max(1)
    }

    pub fn clamp_page(&mut self) {
        self.page = self.page.min(self.pages() - 1);
    }

    pub fn hit(
        &self,
        x: f32,
        y: f32,
        access: Access,
        setup: &crate::notification_setup::Setup,
    ) -> Option<Action> {
        if CLOSE.contains(x, y) {
            return Some(Action::Close);
        }
        if self.windows_enabled && Rect::new(366.0, 48.0, 58.0, 32.0).contains(x, y) {
            return Some(Action::Toggle);
        }
        if ENABLE.contains(x, y) {
            return setup.primary(self.windows_enabled, access);
        }
        if self.windows_enabled && access == Access::MissingIdentity {
            return None; // The setup explanation covers history while identity is missing.
        }
        if PREV.contains(x, y) {
            return Some(Action::Previous);
        }
        if NEXT.contains(x, y) {
            return Some(Action::Next);
        }
        for (i, n) in self
            .history
            .iter()
            .skip(self.page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .enumerate()
        {
            if dismiss(i).contains(x, y) {
                return Some(Action::Dismiss(n.key));
            }
            if row(i).contains(x, y) {
                return Some(Action::Activate(n.key));
            }
        }
        None
    }
}

pub fn bounded_text(text: &str, max_chars: usize) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_control() && !matches!(*ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(max_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_view_blocks_covered_internal_history_and_preserves_visible_controls() {
        use crate::notification_setup::Setup;
        let mut center = NotificationCenter::default();
        center.configure_windows(true);
        center.configure_internal(true);
        let key = NotificationKey::Internal(123);
        center.admit(input(key), Instant::now());
        let mut setup = Setup::default();
        assert_eq!(
            center.hit(40.0, 65.0, Access::MissingIdentity, &setup),
            Some(Action::Setup)
        );
        assert_eq!(
            center.hit(380.0, 65.0, Access::MissingIdentity, &setup),
            Some(Action::Toggle)
        );
        for x in [30.0, 415.0] {
            assert_eq!(center.hit(x, 115.0, Access::MissingIdentity, &setup), None);
        }
        assert_eq!(
            center.hit(405.0, 25.0, Access::MissingIdentity, &setup),
            Some(Action::Close)
        );
        setup.begin(true, Access::MissingIdentity);
        assert_eq!(
            center.hit(40.0, 65.0, Access::MissingIdentity, &setup),
            None
        );
        setup.complete(Ok(()));
        assert_eq!(
            center.hit(40.0, 65.0, Access::MissingIdentity, &setup),
            Some(Action::Restart)
        );
        assert_eq!(
            center.hit(30.0, 115.0, Access::NeedsConsent, &setup),
            Some(Action::Activate(key))
        );
        assert_eq!(
            center.hit(40.0, 65.0, Access::NeedsConsent, &setup),
            Some(Action::RequestAccess)
        );
    }

    #[test]
    fn independent_origins_and_initial_history_never_replay() {
        let now = Instant::now();
        let mut c = NotificationCenter::default();
        assert_eq!(
            c.admit(input(NotificationKey::Internal(1)), now),
            Admission::Disabled
        );
        c.configure_internal(true);
        assert_eq!(
            c.admit(input(NotificationKey::Internal(1)), now),
            Admission::Inserted
        );
        c.configure_windows(true);
        c.admit_history(input(NotificationKey::Windows(2)), now);
        assert_eq!(c.queue.len(), 1);
        c.configure_internal(false);
        c.advance(now, true);
        assert!(c.toast().is_none());
        assert_eq!(c.history.len(), 1);
        assert_eq!(c.history[0].key, NotificationKey::Windows(2));
        c.page = 999;
        c.clamp_page();
        assert_eq!(c.page, 0);
        c.configure_windows(false);
        assert!(c.history.is_empty());
    }

    #[test]
    fn toast_body_activates_its_exact_key_and_close_dismisses() {
        let now = Instant::now();
        let mut c = NotificationCenter::default();
        c.configure_internal(true);
        let key = NotificationKey::Internal(12);
        c.admit(input(key), now);
        c.advance(now, true);
        assert_eq!(c.toast_hit(20.0, 30.0), Some(Action::Activate(key)));
        assert_eq!(c.toast_hit(340.0, 30.0), Some(Action::Dismiss(key)));
        assert_eq!(c.toast_hit(400.0, 30.0), None);
        c.hide_toast();
        assert_eq!(c.toast_hit(20.0, 30.0), None);
        assert_eq!(c.history.len(), 1);
    }
    fn input(key: NotificationKey) -> NotificationInput {
        NotificationInput {
            key,
            source: "App".into(),
            title: "Title".into(),
            body: "Body".into(),
            activation: Activation::None,
        }
    }
    #[test]
    fn opt_in_bounds_dedup_and_disabling_purge_windows_only() {
        let now = Instant::now();
        let mut c = NotificationCenter::default();
        c.configure_internal(true);
        assert_eq!(
            c.admit(input(NotificationKey::Windows(1)), now),
            Admission::Disabled
        );
        c.configure_windows(true);
        for n in 0..100 {
            c.admit(input(NotificationKey::Windows(n)), now);
        }
        assert_eq!(c.history.len(), HISTORY_LIMIT);
        assert_eq!(c.queue.len(), QUEUE_LIMIT);
        assert_eq!(
            c.admit(input(NotificationKey::Windows(99)), now),
            Admission::Updated
        );
        c.admit(input(NotificationKey::Internal(1)), now);
        c.advance(now, true);
        c.configure_windows(false);
        assert_eq!(c.history.len(), 1);
        assert!(c
            .queue
            .iter()
            .all(|k| matches!(k, NotificationKey::Internal(_))));
        assert!(c.toast().is_none());
    }
    #[test]
    fn queue_waits_for_panels_and_expires_without_removing_history() {
        let now = Instant::now();
        let mut c = NotificationCenter::default();
        c.configure_internal(true);
        c.admit(input(NotificationKey::Internal(1)), now);
        c.advance(now, false);
        assert_eq!(c.deadline(), None);
        c.advance(now + Duration::from_secs(90), true);
        assert_eq!(c.toast().unwrap().key, NotificationKey::Internal(1));
        c.advance(now + Duration::from_secs(96), true);
        assert!(c.toast().is_none());
        assert_eq!(c.history.len(), 1);
    }
    #[test]
    fn text_is_bounded_and_activation_origins_cannot_cross() {
        let now = Instant::now();
        let mut c = NotificationCenter::default();
        c.configure_internal(true);
        let mut n = input(NotificationKey::Internal(1));
        n.title = "\0\u{202e}字".repeat(1000);
        n.activation = Activation::OpenSourceApp;
        c.admit(n, now);
        assert_eq!(c.history[0].title.chars().count(), 256);
        assert!(c.history[0].title.chars().all(|ch| ch == '字'));
        assert_eq!(c.activate(NotificationKey::Internal(1)), None);
        let mut n = input(NotificationKey::Internal(2));
        n.activation = Activation::Internal(ActivationToken(12));
        c.admit(n, now);
        assert_eq!(
            c.activate(NotificationKey::Internal(2)),
            Some(Activation::Internal(ActivationToken(12)))
        );
    }
}
