//! Provider events and exact terminal-session correlation. No UI, I/O or transcript access.
use crate::term_tabs::{TabId, MAX_TABS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_WIRE_BYTES: usize = 4096;
pub const MAX_HOOK_BYTES: usize = 64 * 1024;
pub const MAX_ID_BYTES: usize = 128;
pub const WIRE_VERSION: u8 = 1;
pub const ENV_PIPE: &str = "DJ_AGENT_NOTIFY_PIPE";
pub const ENV_CREDENTIAL: &str = "DJ_AGENT_NOTIFY_CREDENTIAL";
pub const ENV_GENERATION: &str = "DJ_AGENT_NOTIFY_GENERATION";
pub const EXIT_SWITCH: &str = "--agent-notification-exit";

// Never reuse identities when the internal source is disabled/re-enabled in one
// app process. Windows destroys all popup state at process exit.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);
static NEXT_TICKET: AtomicU64 = AtomicU64::new(0);

fn next_id(counter: &AtomicU64) -> Option<u64> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .ok()
        .map(|previous| previous + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    pub fn verified_version(self) -> &'static str {
        match self {
            Self::Codex => "codex-cli 0.163.0",
            Self::Claude => "2.1.295 (Claude Code)",
        }
    }

    pub fn agent(self) -> crate::term_agents::Agent {
        match self {
            Self::Claude => crate::term_agents::Agent::Claude,
            Self::Codex => crate::term_agents::Agent::Codex,
        }
    }
}

/// A capability generated with the OS CSPRNG. Never log or persist it in hook config.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Credential(String);

impl Credential {
    pub fn parse(value: String) -> Result<Self, &'static str> {
        if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid launch credential");
        }
        Ok(Self(value))
    }

    /// Only for passing to the provider process's launch-scoped environment.
    pub fn expose(&self) -> &str {
        &self.0
    }

    fn matches(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .bytes()
                .zip(other.0.bytes())
                .fold(0, |d, (a, b)| d | (a ^ b))
                == 0
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Started,
    Activity,
    Completed,
    NeedsApproval,
    NeedsInput,
    Ended,
    LaunchExited,
}

/// This is the entire IPC schema. No commands, URIs, paths, prompts or messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeEvent {
    pub version: u8,
    pub provider: Provider,
    pub generation: u64,
    pub credential: Credential,
    pub session: String,
    pub kind: EventKind,
    pub event_id: Option<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

impl BridgeEvent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != WIRE_VERSION || self.generation == 0 {
            return Err("unsupported bridge version or generation");
        }
        Credential::parse(self.credential.0.clone())?;
        if !valid_id(&self.session) || self.event_id.as_deref().is_some_and(|id| !valid_id(id)) {
            return Err("invalid provider identifier");
        }
        // There is deliberately no claim of reliable generic Codex question hooks.
        if self.provider == Provider::Codex && self.kind == EventKind::NeedsInput {
            return Err("Codex question events require app-server integration");
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_WIRE_BYTES {
            return Err("bridge payload too large");
        }
        let event: Self = serde_json::from_slice(bytes).map_err(|_| "invalid bridge JSON")?;
        event.validate()?;
        Ok(event)
    }

    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "cannot encode bridge event")?;
        if bytes.len() > MAX_WIRE_BYTES {
            return Err("bridge payload too large");
        }
        Ok(bytes)
    }
}

/// Only these metadata fields are deserialized. All transcript/tool/message content is ignored.
#[derive(Deserialize)]
struct HookInput {
    session_id: String,
    hook_event_name: String,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
}

/// Normalize only documented explicit events. Unknown events and idle notifications are ignored.
pub fn parse_hook(
    provider: Provider,
    generation: u64,
    credential: Credential,
    bytes: &[u8],
) -> Result<Option<BridgeEvent>, &'static str> {
    if bytes.len() > MAX_HOOK_BYTES {
        return Err("provider hook input too large");
    }
    let input: HookInput = serde_json::from_slice(bytes).map_err(|_| "invalid hook JSON")?;
    let kind = match input.hook_event_name.as_str() {
        "SessionStart" => EventKind::Started,
        "SessionEnd" => EventKind::Ended,
        "UserPromptSubmit" | "PostToolUse" => EventKind::Activity,
        "Interrupt" if provider == Provider::Codex => EventKind::Activity,
        "PostToolUseFailure" if provider == Provider::Claude => EventKind::Activity,
        "Stop" => EventKind::Completed,
        "PermissionRequest" => EventKind::NeedsApproval,
        "PreToolUse"
            if provider == Provider::Claude
                && matches!(
                    input.tool_name.as_deref(),
                    Some("AskUserQuestion" | "ExitPlanMode")
                ) =>
        {
            EventKind::NeedsInput
        }
        _ => return Ok(None),
    };
    // Notification permission_prompt is intentionally not also registered: PermissionRequest
    // gives an immediate explicit signal without a second delayed duplicate alert.
    let event = BridgeEvent {
        version: WIRE_VERSION,
        provider,
        generation,
        credential,
        session: input.session_id,
        kind,
        event_id: input.tool_use_id.or(input.turn_id),
    };
    event.validate()?;
    Ok(Some(event))
}

#[derive(Clone, Debug)]
pub struct LaunchGrant {
    pub provider: Provider,
    pub tab: TabId,
    pub generation: u64,
    pub credential: Credential,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub tab: TabId,
    pub generation: u64,
}

/// Map this to 9a's ActivationToken only at the integration boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNotice {
    pub ticket: Ticket,
    pub target: Target,
    pub provider: Provider,
    pub kind: EventKind,
}

impl AgentNotice {
    pub fn title(&self) -> &'static str {
        match self.kind {
            EventKind::Completed => "Response finished",
            EventKind::NeedsApproval => "Approval requested",
            EventKind::NeedsInput => "Input requested",
            _ => "Agent update",
        }
    }

    pub fn body(&self) -> &'static str {
        "Open the terminal tab to continue."
    }
}

struct Registration {
    grant: LaunchGrant,
    session: Option<String>,
    last: Option<(EventKind, Option<String>)>,
    ticket: Option<Ticket>,
}

/// In-memory only; bounded by the terminal's tab limit. Closing/restarting/rebinding must
/// revoke first. Never recover a target by cwd, tab order, active tab or provider name.
#[derive(Default)]
pub struct Sessions {
    launches: HashMap<TabId, Registration>,
}

impl Sessions {
    pub fn register(
        &mut self,
        tab: TabId,
        provider: Provider,
        credential: Credential,
    ) -> Result<LaunchGrant, &'static str> {
        if self.launches.len() >= MAX_TABS && !self.launches.contains_key(&tab) {
            return Err("agent launch limit reached");
        }
        // Reject credential reuse, including re-registering the same tab.
        if self
            .launches
            .values()
            .any(|r| r.grant.credential.matches(&credential))
        {
            return Err("launch credential already registered");
        }
        let generation = next_id(&NEXT_GENERATION).ok_or("generation exhausted")?;
        let grant = LaunchGrant {
            provider,
            tab,
            generation,
            credential,
        };
        self.launches.insert(
            tab,
            Registration {
                grant: grant.clone(),
                session: None,
                last: None,
                ticket: None,
            },
        );
        Ok(grant)
    }

    pub fn revoke(&mut self, tab: TabId) {
        self.launches.remove(&tab);
    }

    pub fn resolve(&self, ticket: Ticket) -> Option<Target> {
        self.launches
            .values()
            .find(|r| r.ticket == Some(ticket) && r.session.is_some())
            .map(|r| Target {
                tab: r.grant.tab,
                generation: r.grant.generation,
            })
    }

    pub fn accept(&mut self, event: &BridgeEvent) -> Option<AgentNotice> {
        event.validate().ok()?;
        let r = self.launches.values_mut().find(|r| {
            r.grant.generation == event.generation
                && r.grant.provider == event.provider
                && r.grant.credential.matches(&event.credential)
        })?;
        if event.kind == EventKind::Started {
            // First explicit SessionStart binds exactly once. clear/fork needs a new grant.
            if r.session.is_none() {
                r.session = Some(event.session.clone());
            } else if r.session.as_deref() != Some(event.session.as_str()) {
                // The client changed conversations: an old ticket must not open the
                // newly selected session, even though it occupies the same tab/process.
                let tab = r.grant.tab;
                self.revoke(tab);
            }
            return None;
        }
        // The trusted launch wrapper knows its capability/generation, even if the
        // provider crashed before SessionStart or never ran any SessionEnd hook.
        if event.kind == EventKind::LaunchExited {
            let tab = r.grant.tab;
            self.revoke(tab);
            return None;
        }
        if r.session.as_deref() != Some(event.session.as_str()) {
            return None;
        }
        if event.kind == EventKind::Ended {
            let tab = r.grant.tab;
            self.revoke(tab);
            return None;
        }
        if event.kind == EventKind::Activity {
            r.last = None;
            r.ticket = None;
            return None;
        }
        let identity = (event.kind, event.event_id.clone());
        if r.last.as_ref() == Some(&identity) {
            return None;
        }
        let ticket = Ticket(next_id(&NEXT_TICKET)?);
        r.last = Some(identity);
        r.ticket = Some(ticket);
        Some(AgentNotice {
            ticket,
            target: Target {
                tab: r.grant.tab,
                generation: r.grant.generation,
            },
            provider: event.provider,
            kind: event.kind,
        })
    }
}

/// Pure, trusted launch wrapper. No provider event content is accepted here.
/// The parent shell never keeps a launch capability after the provider returns.
pub fn powershell_launch(grant: &LaunchGrant, endpoint: &str, executable: &str) -> String {
    fn literal(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
    let agent = grant.provider.agent();
    let command = crate::term_agents::command_line(agent);
    let variables = [
        (ENV_PIPE, endpoint.to_string()),
        (ENV_CREDENTIAL, grant.credential.expose().to_string()),
        (ENV_GENERATION, grant.generation.to_string()),
    ];
    let save = variables.iter().map(|(name, _)| format!(
        "$__dj_saved[{0}]=[Environment]::GetEnvironmentVariable({0}); [Environment]::SetEnvironmentVariable({0},$null)", literal(name)))
        .collect::<Vec<_>>().join("; ");
    let set = variables
        .iter()
        .map(|(name, value)| {
            format!(
                "[Environment]::SetEnvironmentVariable({},{})",
                literal(name),
                literal(value)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let restore = variables
        .iter()
        .map(|(name, _)| {
            format!(
                "[Environment]::SetEnvironmentVariable({0},$__dj_saved[{0}])",
                literal(name)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let extra = if grant.provider == Provider::Codex {
        " --no-daemon"
    } else {
        ""
    };
    format!("& {{ $__dj_saved=@{{}}; {save}; try {{ $__dj_version=(& {} --version | Out-String).Trim(); if ($__dj_version -ceq {}) {{ {set}; {command}{extra} }} else {{ Write-Host 'Agent notifications unavailable for this provider version.'; {command} }} }} finally {{ $__dj_exit=$global:LASTEXITCODE; try {{ {set}; & {} {EXIT_SWITCH} {} > $null 2>$null }} catch {{ }} finally {{ {restore}; $global:LASTEXITCODE=$__dj_exit }} }} }}",
        agent.command(), literal(grant.provider.verified_version()), literal(executable), agent.key())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(n: u8) -> Credential {
        Credential::parse(format!("{n:064x}")).unwrap()
    }
    fn event(g: &LaunchGrant, session: &str, kind: EventKind) -> BridgeEvent {
        BridgeEvent {
            version: WIRE_VERSION,
            provider: g.provider,
            generation: g.generation,
            credential: g.credential.clone(),
            session: session.into(),
            kind,
            event_id: None,
        }
    }

    #[test]
    fn session_and_generation_are_required_even_for_reused_tabs() {
        let mut sessions = Sessions::default();
        let first = sessions
            .register(7, Provider::Claude, credential(1))
            .unwrap();
        assert!(sessions
            .accept(&event(&first, "s1", EventKind::Completed))
            .is_none());
        sessions.accept(&event(&first, "s1", EventKind::Started));
        assert!(sessions
            .accept(&event(&first, "s2", EventKind::Completed))
            .is_none());
        let notice = sessions
            .accept(&event(&first, "s1", EventKind::Completed))
            .unwrap();
        assert_eq!(sessions.resolve(notice.ticket), Some(notice.target));
        assert!(sessions
            .accept(&event(&first, "s1", EventKind::Completed))
            .is_none());
        sessions.accept(&event(&first, "s2", EventKind::Started));
        assert!(sessions.resolve(notice.ticket).is_none());
        assert!(sessions
            .accept(&event(&first, "s1", EventKind::Completed))
            .is_none());
        assert!(sessions
            .accept(&event(&first, "s2", EventKind::Completed))
            .is_none());
        let second = sessions
            .register(7, Provider::Claude, credential(2))
            .unwrap();
        assert!(sessions.resolve(notice.ticket).is_none());
        assert!(sessions
            .accept(&event(&first, "s1", EventKind::NeedsInput))
            .is_none());
        sessions.accept(&event(&second, "s1", EventKind::Started));
        let new = sessions
            .accept(&event(&second, "s1", EventKind::Completed))
            .unwrap();
        assert_ne!(notice.ticket, new.ticket);
        sessions.revoke(7);
        assert!(sessions.resolve(new.ticket).is_none());
    }

    #[test]
    fn provider_and_secret_cannot_cross_sessions() {
        let mut sessions = Sessions::default();
        let a = sessions
            .register(1, Provider::Codex, credential(1))
            .unwrap();
        let b = sessions
            .register(2, Provider::Claude, credential(2))
            .unwrap();
        sessions.accept(&event(&a, "same", EventKind::Started));
        sessions.accept(&event(&b, "same", EventKind::Started));
        let mut forged = event(&a, "same", EventKind::Completed);
        forged.credential = b.credential;
        assert!(sessions.accept(&forged).is_none());
        forged.credential = a.credential.clone();
        forged.provider = Provider::Claude;
        assert!(sessions.accept(&forged).is_none());
        assert!(sessions
            .register(3, Provider::Claude, a.credential)
            .is_err());
    }

    #[test]
    fn activity_and_end_invalidate_activation() {
        let mut sessions = Sessions::default();
        let g = sessions
            .register(1, Provider::Claude, credential(1))
            .unwrap();
        sessions.accept(&event(&g, "s", EventKind::Started));
        let first = sessions
            .accept(&event(&g, "s", EventKind::NeedsApproval))
            .unwrap();
        sessions.accept(&event(&g, "s", EventKind::Activity));
        assert!(sessions.resolve(first.ticket).is_none());
        let second = sessions
            .accept(&event(&g, "s", EventKind::NeedsApproval))
            .unwrap();
        sessions.accept(&event(&g, "s", EventKind::Ended));
        assert!(sessions.resolve(second.ticket).is_none());
        assert!(sessions
            .accept(&event(&g, "s", EventKind::Started))
            .is_none());
        assert!(sessions
            .accept(&event(&g, "s", EventKind::Completed))
            .is_none());
    }

    #[test]
    fn replacing_the_controller_cannot_reuse_an_old_activation() {
        let mut first = Sessions::default();
        let g1 = first.register(1, Provider::Claude, credential(1)).unwrap();
        first.accept(&event(&g1, "s", EventKind::Started));
        let old = first
            .accept(&event(&g1, "s", EventKind::Completed))
            .unwrap();
        drop(first);
        let mut second = Sessions::default();
        let g2 = second.register(1, Provider::Claude, credential(2)).unwrap();
        second.accept(&event(&g2, "s", EventKind::Started));
        let current = second
            .accept(&event(&g2, "s", EventKind::Completed))
            .unwrap();
        assert_ne!(g1.generation, g2.generation);
        assert_ne!(old.ticket, current.ticket);
        assert!(second.resolve(old.ticket).is_none());
        assert_eq!(second.resolve(current.ticket), Some(current.target));
    }

    #[test]
    fn hostile_wire_fields_and_sizes_are_rejected() {
        assert!(BridgeEvent::decode(&vec![b' '; MAX_WIRE_BYTES + 1]).is_err());
        let g = LaunchGrant {
            provider: Provider::Claude,
            tab: 1,
            generation: 1,
            credential: credential(1),
        };
        for session in ["", "a/b", "s\ncmd", "https://host", &"x".repeat(129)] {
            assert!(event(&g, session, EventKind::Completed).encode().is_err());
        }
        let mut json = serde_json::to_value(event(&g, "s", EventKind::Completed)).unwrap();
        json["command"] = "calc.exe".into();
        assert!(BridgeEvent::decode(&serde_json::to_vec(&json).unwrap()).is_err());
        assert!(!format!("{:?}", g.credential).contains(g.credential.expose()));
    }

    #[test]
    fn provider_fixtures_ignore_content_and_idle_and_subagents() {
        let fixtures = [
            (
                Provider::Claude,
                r#"{"session_id":"c-1","hook_event_name":"Stop","last_assistant_message":"SECRET","transcript_path":"C:/private"}"#,
                Some(EventKind::Completed),
            ),
            (
                Provider::Codex,
                r#"{"session_id":"thr_1","hook_event_name":"PermissionRequest","turn_id":"t1","tool_input":{"command":"calc.exe"}}"#,
                Some(EventKind::NeedsApproval),
            ),
            (
                Provider::Claude,
                r#"{"session_id":"c1","hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_use_id":"toolu_1"}"#,
                Some(EventKind::NeedsInput),
            ),
            (
                Provider::Claude,
                r#"{"session_id":"c1","hook_event_name":"PreToolUse","tool_name":"ExitPlanMode"}"#,
                Some(EventKind::NeedsInput),
            ),
            (
                Provider::Claude,
                r#"{"session_id":"c1","hook_event_name":"Notification","notification_type":"idle_prompt"}"#,
                None,
            ),
            (
                Provider::Codex,
                r#"{"session_id":"c1","hook_event_name":"PreToolUse","tool_name":"request_user_input"}"#,
                None,
            ),
            (
                Provider::Claude,
                r#"{"session_id":"c1","hook_event_name":"SubagentStop"}"#,
                None,
            ),
            (
                Provider::Codex,
                r#"{"session_id":"c1","hook_event_name":"Stop","stop_hook_active":true}"#,
                Some(EventKind::Completed),
            ),
        ];
        for (provider, fixture, expected) in fixtures {
            let parsed = parse_hook(provider, 1, credential(1), fixture.as_bytes()).unwrap();
            assert_eq!(parsed.as_ref().map(|e| e.kind), expected);
            if let Some(parsed) = parsed {
                let wire = String::from_utf8(parsed.encode().unwrap()).unwrap();
                assert!(
                    !wire.contains("SECRET")
                        && !wire.contains("calc.exe")
                        && !wire.contains("private")
                );
            }
        }
        assert!(parse_hook(
            Provider::Claude,
            1,
            credential(1),
            &vec![b' '; MAX_HOOK_BYTES + 1]
        )
        .is_err());
    }

    #[test]
    fn registrations_are_bounded() {
        let mut sessions = Sessions::default();
        for n in 0..MAX_TABS {
            sessions
                .register(n as u64, Provider::Claude, credential(n as u8))
                .unwrap();
        }
        assert!(sessions
            .register(100, Provider::Claude, credential(200))
            .is_err());
        sessions.revoke(0);
        assert!(sessions
            .register(100, Provider::Claude, credential(200))
            .is_ok());
    }
}
