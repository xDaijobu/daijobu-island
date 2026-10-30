//! User-driven identity setup, independent of Windows permission and opt-in.
use crate::notifications::{Access, Action};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Setup {
    #[default]
    Idle,
    Registering,
    RestartRequired,
    Restarting,
    Failed(Failure),
}

impl Setup {
    pub fn begin(&mut self, enabled: bool, access: Access) -> bool {
        if !enabled || access != Access::MissingIdentity || !self.can_retry() {
            return false;
        }
        *self = Self::Registering;
        true
    }

    pub fn can_retry(&self) -> bool {
        matches!(
            self,
            Self::Idle
                | Self::Failed(Failure {
                    retryable: true,
                    ..
                })
        )
    }

    pub fn complete(&mut self, result: Result<(), Failure>) {
        if *self == Self::Registering {
            *self = match result {
                Ok(()) => Self::RestartRequired,
                Err(error) => Self::Failed(error),
            };
        }
    }

    pub fn restart(&mut self, enabled: bool) -> bool {
        if !enabled || *self != Self::RestartRequired {
            return false;
        }
        *self = Self::Restarting;
        true
    }

    pub fn restart_failed(&mut self) {
        if *self == Self::Restarting {
            // Keep registration success: retry restart without re-registering.
            *self = Self::RestartRequired;
        }
    }

    pub fn primary(&self, enabled: bool, access: Access) -> Option<Action> {
        if !enabled {
            return Some(Action::Toggle);
        }
        if access != Access::MissingIdentity {
            return Some(if access == Access::NeedsConsent {
                Action::RequestAccess
            } else {
                Action::Toggle
            });
        }
        match self {
            Self::RestartRequired => Some(Action::Restart),
            _ if self.can_retry() => Some(Action::Setup),
            _ => None,
        }
    }

    pub fn button(&self, enabled: bool, access: Access) -> &'static str {
        match self.primary(enabled, access) {
            Some(Action::Toggle) if !enabled => "Enable Windows notifications",
            Some(Action::Toggle) => "Disable Windows notifications",
            Some(Action::RequestAccess) => "Allow access",
            Some(Action::Setup) if matches!(self, Self::Failed(_)) => "Retry notification setup",
            Some(Action::Setup) => "Set up notifications",
            Some(Action::Restart) => "Restart to finish setup",
            _ if *self == Self::Registering => "Setting up notifications...",
            _ if *self == Self::Restarting => "Preparing graceful restart...",
            _ => "Setup unavailable in this build",
        }
    }

    pub fn explanation(&self) -> &str {
        match self {
            Self::Idle => "Set up this app for Windows notifications. Windows permission is requested separately after restart.",
            Self::Registering => "Registering this app for your Windows account. You can keep using the island while setup runs.",
            Self::RestartRequired => "Registration finished. Restart to acquire identity, then select Allow access. Restart closes shell sessions and preserves your saved settings and pinned tabs.",
            Self::Restarting => "Waiting for the restart helper. Settings and pinned tabs are saved through the normal shutdown path.",
            Self::Failed(failure) => &failure.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_coalesces_requires_opt_in_and_never_grants_access() {
        let mut setup = Setup::default();
        assert!(!setup.begin(false, Access::MissingIdentity));
        for access in [
            Access::Disabled,
            Access::NeedsConsent,
            Access::Denied,
            Access::Listening,
        ] {
            assert!(!setup.begin(true, access));
        }
        assert!(setup.begin(true, Access::MissingIdentity));
        assert!(!setup.begin(true, Access::MissingIdentity));
        assert_eq!(setup.primary(true, Access::MissingIdentity), None);
        setup.complete(Ok(()));
        assert_eq!(setup, Setup::RestartRequired);
        assert_eq!(
            setup.primary(true, Access::MissingIdentity),
            Some(Action::Restart)
        );
        assert_eq!(
            setup.primary(true, Access::NeedsConsent),
            Some(Action::RequestAccess)
        );
        assert_eq!(setup.primary(true, Access::Denied), Some(Action::Toggle));
    }
    #[test]
    fn disable_and_reentry_do_not_reset_inflight_or_enable_notifications() {
        let mut setup = Setup::default();
        setup.begin(true, Access::MissingIdentity);
        assert_eq!(setup.primary(false, Access::Disabled), Some(Action::Toggle));
        setup.complete(Ok(()));
        assert!(!setup.restart(false));
        assert!(setup.restart(true));
        assert!(!setup.restart(true));
        setup.restart_failed();
        assert_eq!(setup, Setup::RestartRequired);
        assert!(!setup.begin(true, Access::MissingIdentity));
    }
    #[test]
    fn retries_only_after_terminal_failure_and_ignores_stale_completion() {
        let mut setup = Setup::default();
        setup.complete(Ok(()));
        assert_eq!(setup, Setup::Idle);
        setup.begin(true, Access::MissingIdentity);
        setup.complete(Err(Failure {
            message: "0x800B0109: signing certificate is not trusted".into(),
            retryable: true,
        }));
        assert!(setup.begin(true, Access::MissingIdentity));
        setup.complete(Err(Failure {
            message: "Windows did not finish cancellation; restart before retrying".into(),
            retryable: false,
        }));
        assert!(!setup.begin(true, Access::MissingIdentity));
        assert_eq!(setup.primary(true, Access::MissingIdentity), None);
    }
}
