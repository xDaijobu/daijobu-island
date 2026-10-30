//! App-owned notification controller. The native adapter owns Windows identity;
//! internal producers register typed targets and opaque tokens with this controller.

use dj_core::agent_notifications::{AgentNotice, Ticket};
use dj_core::config::NotificationsConfig;
use dj_core::notification_setup::{Failure, Setup};
use dj_core::notifications::{
    Access, Activation, ActivationToken, Admission, NotificationCenter, NotificationInput,
    NotificationKey, HISTORY_LIMIT,
};
use dj_core::term_tabs::TabId;
use dj_win::notification_setup::{Completion, Job, RestartTicket};
use dj_win::notifications::{Listener, NativeNotification};
use std::collections::VecDeque;
use std::time::Instant;

mod setup;

pub const TIMER_TOAST: usize = 17;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // typed integration target; next track owns internal producers
pub enum InternalTarget {
    TerminalTab(TabId),
    AgentSession(Ticket),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationResult {
    OpenedSourceApp,
    Internal(InternalTarget),
    Unavailable,
}

#[derive(Default)]
struct Activations(VecDeque<(NotificationKey, ActivationToken, InternalTarget)>);
impl Activations {
    fn register(&mut self, key: NotificationKey, token: ActivationToken, target: InternalTarget) {
        if !matches!(key, NotificationKey::Internal(_)) {
            return;
        }
        self.0.retain(|(k, t, _)| *k != key && *t != token);
        self.0.push_front((key, token, target));
        self.0.truncate(HISTORY_LIMIT);
    }
    fn resolve(&self, key: NotificationKey, token: ActivationToken) -> Option<InternalTarget> {
        self.0
            .iter()
            .find(|(k, t, _)| *k == key && *t == token)
            .map(|(_, _, target)| *target)
    }
}

pub struct Notifications {
    pub center: NotificationCenter,
    native: Listener,
    config: NotificationsConfig,
    current: Vec<NativeNotification>,
    initial: bool,
    watermark: i64,
    activations: Activations,
    pub notice: Option<&'static str>,
    pub armed: Option<Instant>,
    pub setup: Setup,
    pub setup_notice: Option<String>,
    setup_job: Option<Job>,
    restart_ticket: Option<RestartTicket>,
    hwnd: isize,
}

impl Notifications {
    pub fn new(hwnd: isize, config: &NotificationsConfig) -> Self {
        let mut this = Self {
            center: NotificationCenter::default(),
            native: Listener::new(hwnd),
            config: NotificationsConfig::default(),
            current: Vec::new(),
            initial: true,
            watermark: i64::MIN,
            activations: Activations::default(),
            notice: None,
            armed: None,
            setup: Setup::default(),
            setup_notice: None,
            setup_job: None,
            restart_ticket: None,
            hwnd,
        };
        this.configure(config);
        this
    }

    pub fn configure(&mut self, config: &NotificationsConfig) {
        self.center.configure_windows(config.enabled);
        self.center.configure_internal(config.internal_enabled);
        if !config.internal_enabled {
            self.activations.0.clear();
        }
        if self.config.enabled != config.enabled {
            self.native.configure(config.enabled);
            self.current.clear();
            self.initial = true;
            self.watermark = i64::MIN;
        }
        self.config = config.clone();
        self.notice = None;
    }

    pub fn access(&self) -> Access {
        self.native.access()
    }

    pub fn admit_agent(&mut self, notice: &AgentNotice, now: Instant) -> Admission {
        let key = NotificationKey::Internal(notice.ticket.0);
        let token = ActivationToken(notice.ticket.0);
        self.register_activation(key, token, InternalTarget::AgentSession(notice.ticket));
        self.admit(
            NotificationInput {
                key,
                source: notice.provider.name().into(),
                title: notice.title().into(),
                body: notice.body().into(),
                activation: Activation::Internal(token),
            },
            now,
        )
    }

    /// Keep the history entry, but remove an action as soon as its exact session is stale.
    pub fn expire_agent_actions(&mut self, live: impl Fn(Ticket) -> bool) -> bool {
        let stale: Vec<_> = self
            .activations
            .0
            .iter()
            .filter_map(|(key, _, target)| match target {
                InternalTarget::AgentSession(ticket) if !live(*ticket) => Some(*key),
                _ => None,
            })
            .collect();
        for key in &stale {
            if let Some(mut input) = self.center.get(*key).cloned() {
                input.activation = Activation::None;
                self.center.admit(input, Instant::now());
            }
        }
        self.activations
            .0
            .retain(|(key, _, _)| !stale.contains(key));
        !stale.is_empty()
    }

    pub fn refresh_access(&mut self) {
        if self.native.refresh_access() && self.access() != Access::Listening {
            self.purge_windows();
        }
    }

    fn purge_windows(&mut self) {
        for item in self.current.drain(..) {
            self.center.remove(item.input.key);
        }
        self.initial = true;
    }

    pub fn request_access(&mut self) {
        self.native.request_access();
    }

    pub fn start_setup(&mut self) {
        if !self.setup.begin(self.config.enabled, self.access()) {
            return;
        }
        self.setup_notice = None;
        let root = crate::storage::data_dir();
        let result = std::env::current_exe()
            .map_err(|e| Failure {
                message: e.to_string(),
                retryable: true,
            })
            .and_then(|exe| {
                dj_win::notification_setup::register(self.hwnd, move || setup::prepare(&root, &exe))
            });
        match result {
            Ok(job) => self.setup_job = Some(job),
            Err(error) => self.setup.complete(Err(error)),
        }
    }

    pub fn restart(&mut self) {
        if !self.setup.restart(self.config.enabled) {
            return;
        }
        self.setup_notice = None;
        match dj_win::notification_setup::prepare_restart(self.hwnd) {
            Ok(job) => self.setup_job = Some(job),
            Err(error) => {
                self.setup.restart_failed();
                self.setup_notice = Some(error.message);
            }
        }
    }

    pub fn take_restart(&mut self) -> bool {
        let Some(ticket) = self.restart_ticket.take() else {
            return false;
        };
        if !self.config.enabled || self.setup != Setup::Restarting {
            self.setup.restart_failed();
            return false;
        }
        match ticket.commit() {
            Ok(()) => true,
            Err(error) => {
                self.setup.restart_failed();
                self.setup_notice = Some(error.message);
                false
            }
        }
    }

    pub fn shutdown_setup(&mut self) {
        self.setup_job = None;
        self.restart_ticket = None;
    }

    pub fn drain(&mut self, now: Instant) -> bool {
        let completion = self.setup_job.as_ref().and_then(Job::take);
        let setup_changed = completion.is_some();
        if let Some(completion) = completion {
            self.setup_job = None;
            match completion {
                Completion::Registered(result) => self.setup.complete(result),
                Completion::Restart(Ok(ticket)) => self.restart_ticket = Some(ticket),
                Completion::Restart(Err(error)) => {
                    self.setup.restart_failed();
                    self.setup_notice = Some(error.message);
                }
            }
        }
        let update = self.native.drain();
        let changed = setup_changed || update.access.is_some() || update.snapshot.is_some();
        if self.access() != Access::Listening {
            self.purge_windows();
        }
        if let Some(snapshot) = update
            .snapshot
            .filter(|_| self.access() == Access::Listening)
        {
            for old in &self.current {
                if !snapshot.iter().any(|n| n.input.key == old.input.key) {
                    self.center.remove(old.input.key);
                }
            }
            for n in &snapshot {
                if self.initial
                    || (n.created() <= self.watermark && self.center.get(n.input.key).is_none())
                {
                    self.center.admit_history(n.input.clone(), now);
                } else {
                    self.center.admit(n.input.clone(), now);
                }
            }
            if let Some(latest) = snapshot.iter().map(NativeNotification::created).max() {
                self.watermark = self.watermark.max(latest);
            }
            self.current = snapshot;
            self.initial = false;
        }
        changed
    }

    /// Integration seam: register trusted key/token/target before admission.
    /// The caller must own the target; activation checks the key-token pair again.
    #[allow(dead_code)] // consumed by the next integration track
    pub fn register_activation(
        &mut self,
        key: NotificationKey,
        token: ActivationToken,
        target: InternalTarget,
    ) {
        self.activations.register(key, token, target);
    }

    #[allow(dead_code)] // consumed by the next integration track
    pub fn admit(&mut self, mut input: NotificationInput, now: Instant) -> Admission {
        if let Activation::Internal(token) = input.activation {
            if self.activations.resolve(input.key, token).is_none() {
                input.activation = Activation::None;
            }
        }
        self.center.admit(input, now)
    }

    pub fn dismiss(&mut self, key: NotificationKey) {
        if let NotificationKey::Windows(id) = key {
            if !self.native.dismiss(id) {
                self.notice = Some("Windows could not dismiss this notification");
                self.refresh_access();
                return;
            }
        }
        self.center.remove(key);
        self.current.retain(|n| n.input.key != key);
        self.activations.0.retain(|(k, _, _)| *k != key);
        self.notice = None;
    }

    pub fn activate(&mut self, key: NotificationKey) -> ActivationResult {
        match self.center.activate(key) {
            Some(Activation::OpenSourceApp) => {
                let opened = self
                    .current
                    .iter()
                    .find(|n| n.input.key == key)
                    .cloned()
                    .is_some_and(|n| self.native.open_source(&n));
                self.notice = (!opened).then_some(
                    "Source app could not be opened; desktop apps may not support activation",
                );
                if opened {
                    return ActivationResult::OpenedSourceApp;
                }
            }
            Some(Activation::Internal(token)) => {
                if let Some(target) = self.activations.resolve(key, token) {
                    self.notice = None;
                    return ActivationResult::Internal(target);
                }
                self.notice = Some("The notification's session is no longer available");
            }
            _ => self.notice = Some("No source-app action is available"),
        }
        ActivationResult::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_opt_in_and_panel_reentry_never_start_setup_or_permission() {
        let config = NotificationsConfig {
            enabled: true,
            internal_enabled: false,
        };
        let mut controller = Notifications::new(0, &config);
        assert_eq!(controller.setup, Setup::Idle);
        assert!(controller.setup_job.is_none());
        controller.refresh_access();
        controller.drain(Instant::now());
        assert_eq!(controller.setup, Setup::Idle);
        assert!(controller.setup_job.is_none());
        assert_ne!(controller.access(), Access::Requesting);
    }

    #[test]
    fn late_success_after_disable_preserves_opt_out_and_setup_on_reentry() {
        let mut controller = Notifications::new(
            0,
            &NotificationsConfig {
                enabled: true,
                internal_enabled: true,
            },
        );
        // Synthetic completion only: never start a real deployment from a test.
        controller.setup = Setup::Registering;
        controller.configure(&NotificationsConfig {
            enabled: false,
            internal_enabled: true,
        });
        controller.setup.complete(Ok(()));
        assert_eq!(controller.access(), Access::Disabled);
        assert!(!controller.config.enabled);
        assert_eq!(controller.setup, Setup::RestartRequired);
        controller.restart();
        assert!(controller.setup_job.is_none());
        controller.configure(&NotificationsConfig {
            enabled: true,
            internal_enabled: true,
        });
        assert_eq!(controller.setup, Setup::RestartRequired);
        assert!(controller.setup_job.is_none());
    }

    #[test]
    fn controller_admits_internal_without_windows_and_rejects_unregistered_tokens() {
        let config = NotificationsConfig {
            enabled: false,
            internal_enabled: true,
        };
        let mut controller = Notifications::new(0, &config);
        let now = Instant::now();
        let input = |id, token| NotificationInput {
            key: NotificationKey::Internal(id),
            source: "Trusted adapter".into(),
            title: "Ready".into(),
            body: "cmd /c unsafe-text-is-presentation".into(),
            activation: Activation::Internal(ActivationToken(token)),
        };
        assert_eq!(controller.access(), Access::Disabled);
        controller.admit(input(1, 5), now);
        assert_eq!(
            controller.center.activate(NotificationKey::Internal(1)),
            None
        );
        assert_eq!(
            controller.activate(NotificationKey::Internal(1)),
            ActivationResult::Unavailable
        );
        assert!(controller.notice.is_some());
        controller.register_activation(
            NotificationKey::Internal(2),
            ActivationToken(6),
            InternalTarget::TerminalTab(9),
        );
        controller.admit(input(2, 6), now);
        assert_eq!(
            controller.activate(NotificationKey::Internal(2)),
            ActivationResult::Internal(InternalTarget::TerminalTab(9))
        );
        controller.configure(&NotificationsConfig::default());
        assert!(controller.center.history().is_empty());
        assert!(controller.activations.0.is_empty());
    }
    #[test]
    fn internal_activation_is_bounded_and_bound_to_both_key_and_token() {
        let mut registry = Activations::default();
        let key = NotificationKey::Internal(1);
        registry.register(key, ActivationToken(2), InternalTarget::TerminalTab(3));
        assert_eq!(
            registry.resolve(key, ActivationToken(2)),
            Some(InternalTarget::TerminalTab(3))
        );
        assert_eq!(
            registry.resolve(NotificationKey::Internal(4), ActivationToken(2)),
            None
        );
        registry.register(
            NotificationKey::Windows(1),
            ActivationToken(5),
            InternalTarget::TerminalTab(3),
        );
        assert_eq!(
            registry.resolve(NotificationKey::Windows(1), ActivationToken(5)),
            None
        );
        for n in 10..200 {
            registry.register(
                NotificationKey::Internal(n),
                ActivationToken(n),
                InternalTarget::TerminalTab(3),
            );
        }
        assert_eq!(registry.0.len(), HISTORY_LIMIT);
        assert_eq!(registry.resolve(key, ActivationToken(2)), None);
    }
}
