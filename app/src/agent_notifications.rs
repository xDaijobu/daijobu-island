//! Opt-in agent hooks and the shared notification boundary. Hook plans are pure
//! transforms; the serialized setup worker persists them only after user opt-in.
mod setup;
pub use setup::{Runtime, SetupWorker};

use base64::Engine;
use dj_core::agent_notifications::{
    parse_hook, AgentNotice, BridgeEvent, Credential, EventKind, LaunchGrant, Provider, Sessions,
    Target, Ticket, MAX_HOOK_BYTES, WIRE_VERSION,
};
use dj_core::term_tabs::TabId;
use dj_win::agent_notifications::{self as ipc, BridgeServer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;

pub use dj_core::agent_notifications::{ENV_CREDENTIAL, ENV_GENERATION, ENV_PIPE, EXIT_SWITCH};
pub const HOOK_SWITCH: &str = "--agent-notification-hook";
const MAX_CONFIG_BYTES: usize = 256 * 1024;

/// Conservative capability gate: these exact Windows versions were inspected for
/// this stage. Broaden only after verifying the provider's event/config contract.
#[derive(Clone, Copy, Debug)]
pub struct VerifiedProvider(Provider);

impl VerifiedProvider {
    pub fn from_version_output(provider: Provider, output: &str) -> Result<Self, &'static str> {
        if output.trim() == provider.verified_version() {
            Ok(Self(provider))
        } else {
            Err("Agent notifications are unavailable for this unverified provider version.")
        }
    }

    pub fn provider(self) -> Provider {
        self.0
    }
}

pub struct AgentNotifications {
    server: BridgeServer,
    sessions: Sessions,
}

impl AgentNotifications {
    /// Call only after explicit internal-notification opt-in. No global hooks/config changes.
    pub fn start(wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        Ok(Self {
            server: BridgeServer::start(wake)?,
            sessions: Sessions::default(),
        })
    }

    /// Prepare before launching the provider; install these env vars only in that launch.
    /// Never put them in the long-lived parent shell, user environment or persisted pins.
    pub fn prepare(
        &mut self,
        tab: TabId,
        provider: VerifiedProvider,
    ) -> Result<LaunchEnvironment, String> {
        let grant = self
            .sessions
            .register(tab, provider.provider(), ipc::random_credential()?)
            .map_err(str::to_owned)?;
        Ok(LaunchEnvironment {
            endpoint: self.server.endpoint().into(),
            grant,
        })
    }

    /// Required before closing, restarting or reusing a tab and when its agent process exits.
    pub fn revoke(&mut self, tab: TabId) {
        self.sessions.revoke(tab);
    }

    /// The main loop calls this after the IPC worker posts a wake.
    pub fn drain(&mut self) -> Vec<AgentNotice> {
        self.server
            .drain()
            .iter()
            .filter_map(|e| self.sessions.accept(e))
            .collect()
    }

    /// Recheck on click; the terminal must additionally verify its live generation.
    pub fn resolve(&self, ticket: Ticket) -> Option<Target> {
        self.sessions.resolve(ticket)
    }
}

#[derive(Debug)]
pub struct LaunchEnvironment {
    pub endpoint: String,
    pub grant: LaunchGrant,
}

impl LaunchEnvironment {
    #[cfg(test)]
    pub fn variables(&self) -> [(String, String); 3] {
        [
            (ENV_PIPE.into(), self.endpoint.clone()),
            (ENV_CREDENTIAL.into(), self.grant.credential.expose().into()),
            (ENV_GENERATION.into(), self.grant.generation.to_string()),
        ]
    }
}

fn provider(value: &str) -> Option<Provider> {
    match value {
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        _ => None,
    }
}

/// Runs before the single-instance mutex and storage/UI initialization. Hook failures
/// fail open, and stdout is always the neutral JSON object required by Codex Stop.
/// No error text, payload content, approval decision or tool result reaches the provider.
pub fn handle_cli(args: &[OsString]) -> bool {
    let switch = args.first().and_then(|a| a.to_str());
    if !matches!(switch, Some(HOOK_SWITCH | EXIT_SWITCH)) {
        return false;
    }
    if args.len() == 2 {
        if let Some(provider) = args[1].to_str().and_then(provider) {
            if switch == Some(EXIT_SWITCH) {
                let _ = forward_exit(provider);
            } else {
                let _ = forward_hook(provider, &mut std::io::stdin().lock());
            }
        }
    }
    let _ = std::io::stdout().lock().write_all(b"{}\n");
    true
}

fn forward_exit(provider: Provider) -> Result<(), String> {
    let endpoint = std::env::var(ENV_PIPE).map_err(|_| "no launch pipe")?;
    let credential = Credential::parse(std::env::var(ENV_CREDENTIAL).map_err(|_| "no credential")?)
        .map_err(str::to_owned)?;
    let generation = std::env::var(ENV_GENERATION)
        .map_err(|_| "no generation")?
        .parse()
        .map_err(|_| "invalid generation")?;
    ipc::send(
        &endpoint,
        &BridgeEvent {
            version: WIRE_VERSION,
            provider,
            generation,
            credential,
            session: "launch".into(),
            kind: EventKind::LaunchExited,
            event_id: None,
        },
    )
}

fn forward_hook(provider: Provider, reader: &mut impl Read) -> Result<(), String> {
    let endpoint = std::env::var(ENV_PIPE).map_err(|_| "no launch pipe")?;
    if !ipc::valid_endpoint(&endpoint) {
        return Err("invalid launch pipe".into());
    }
    let credential = Credential::parse(std::env::var(ENV_CREDENTIAL).map_err(|_| "no credential")?)
        .map_err(str::to_owned)?;
    let generation = std::env::var(ENV_GENERATION)
        .map_err(|_| "no generation")?
        .parse::<u64>()
        .map_err(|_| "invalid generation")?;
    let bytes = read_bounded(reader)?;
    if let Some(event) =
        parse_hook(provider, generation, credential, &bytes).map_err(str::to_owned)?
    {
        ipc::send(&endpoint, &event)?;
    }
    Ok(())
}

fn read_bounded(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_HOOK_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_HOOK_BYTES {
        return Err("hook input too large".into());
    }
    Ok(bytes)
}

#[derive(Clone, Debug)]
pub struct HookPlan {
    entries: Vec<OwnedHook>,
}

/// Persist the first install receipt with the opt-in state. It contains only owned
/// hooks, never the user's original settings. Edited owned handlers are left alone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetupReceipt {
    created_hooks: bool,
    created_events: Vec<String>,
    entries: Vec<OwnedHook>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OwnedHook {
    event: String,
    group: Value,
}

pub struct ConfigEdit {
    pub output: String,
    /// None on an idempotent install: retain the previous receipt.
    pub receipt: Option<SetupReceipt>,
}

impl HookPlan {
    /// Claude uses documented exec form (no shell quoting). Codex uses an encoded
    /// PowerShell command so outer cmd/PowerShell parsing cannot interpret the path.
    /// Both call a fixed executable/switch; event data is only ever read from stdin.
    pub fn new(provider: VerifiedProvider, executable: &Path) -> Result<Self, String> {
        let path = executable
            .to_str()
            .ok_or("executable path is not Unicode")?;
        if !executable.is_absolute()
            || !executable
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
            || path.chars().any(char::is_control)
            || path.contains("${")
        {
            return Err("hook bridge must be an absolute Windows exe path".into());
        }
        let handler = match provider.provider() {
            Provider::Claude => json!({"type":"command", "command":path,
                "args":[HOOK_SWITCH,"claude"], "timeout":3}),
            Provider::Codex => {
                let script = format!("& '{}' {HOOK_SWITCH} codex", path.replace('\'', "''"));
                let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
                let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
                let command = format!(
                    "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {encoded}"
                );
                json!({"type":"command", "command":command, "commandWindows":command, "timeout":3})
            }
        };
        let mut entries = Vec::new();
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "Stop",
            "PermissionRequest",
            "PostToolUse",
            "SessionEnd",
        ] {
            entries.push(OwnedHook {
                event: event.into(),
                group: json!({"hooks":[handler.clone()]}),
            });
        }
        if provider.provider() == Provider::Claude {
            entries.push(OwnedHook {
                event: "PostToolUseFailure".into(),
                group: json!({"hooks":[handler.clone()]}),
            });
            entries.push(OwnedHook {
                event: "PreToolUse".into(),
                group: json!({"matcher":"^(AskUserQuestion|ExitPlanMode)$", "hooks":[handler]}),
            });
        } else {
            entries.push(OwnedHook {
                event: "Interrupt".into(),
                group: json!({"hooks":[handler]}),
            });
        }
        Ok(Self { entries })
    }

    /// For Claude settings.json or Codex hooks.json. Codex config.toml/legacy notify,
    /// security policy and hook trust are deliberately untouched.
    pub fn install(&self, input: &str) -> Result<ConfigEdit, String> {
        let mut root = parse_config(input)?;
        let object = root.as_object_mut().ok_or("config must be an object")?;
        let created_hooks = !object.contains_key("hooks");
        let hooks = object
            .entry("hooks")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or("hooks must be an object")?;
        let mut receipt = SetupReceipt {
            created_hooks,
            created_events: Vec::new(),
            entries: Vec::new(),
        };
        for entry in &self.entries {
            let created_event = !hooks.contains_key(&entry.event);
            let groups = hooks
                .entry(&entry.event)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or("hook event must be an array")?;
            if groups.iter().any(|group| group == &entry.group) {
                continue;
            }
            if created_event {
                receipt.created_events.push(entry.event.clone());
            }
            groups.push(entry.group.clone());
            receipt.entries.push(entry.clone());
        }
        let changed = !receipt.entries.is_empty();
        Ok(ConfigEdit {
            output: if changed {
                pretty(&root)?
            } else {
                input.into()
            },
            receipt: changed.then_some(receipt),
        })
    }
}

/// Remove only exact handlers recorded by our install. New third-party handlers,
/// unknown group fields and user edits remain intact, including edits made later.
pub fn uninstall_hooks(input: &str, receipt: &SetupReceipt) -> Result<String, String> {
    let mut root = parse_config(input)?;
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(input.into());
    };
    let hooks = hooks.as_object_mut().ok_or("hooks must be an object")?;
    for entry in &receipt.entries {
        let Some(groups) = hooks.get_mut(&entry.event) else {
            continue;
        };
        let groups = groups.as_array_mut().ok_or("hook event must be an array")?;
        let owned_handler = &entry.group["hooks"][0];
        groups.retain_mut(|group| {
            if group == &entry.group {
                return false;
            }
            // If a user appended a third-party handler to our group, keep that handler.
            let mut metadata = group.clone();
            if let Some(object) = metadata.as_object_mut() {
                object.remove("hooks");
            }
            let mut owned_metadata = entry.group.clone();
            owned_metadata.as_object_mut().unwrap().remove("hooks");
            if metadata == owned_metadata {
                if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    handlers.retain(|handler| handler != owned_handler);
                }
            }
            true
        });
    }
    for event in &receipt.created_events {
        if hooks
            .get(event)
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            hooks.remove(event);
        }
    }
    if receipt.created_hooks && hooks.is_empty() {
        root.as_object_mut().unwrap().remove("hooks");
    }
    pretty(&root)
}

fn parse_config(input: &str) -> Result<Value, String> {
    if input.len() > MAX_CONFIG_BYTES {
        return Err("provider config too large".into());
    }
    // No filesystem access, fallback, permission reset or destructive repair on invalid config.
    serde_json::from_str(input).map_err(|_| "provider config is not valid JSON".into())
}

fn pretty(value: &Value) -> Result<String, String> {
    let text = serde_json::to_string_pretty(value).map_err(|_| "cannot serialize config")?;
    if text.len() > MAX_CONFIG_BYTES {
        return Err("provider config too large".into());
    }
    Ok(format!("{text}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_core::agent_notifications::{BridgeEvent, EventKind, WIRE_VERSION};

    fn plan(provider: Provider) -> HookPlan {
        HookPlan::new(
            verified(provider),
            Path::new(r"C:\Program Files\O'Brien & Co\daijobu.exe"),
        )
        .unwrap()
    }

    fn verified(provider: Provider) -> VerifiedProvider {
        let version = match provider {
            Provider::Claude => "2.1.295 (Claude Code)",
            Provider::Codex => "codex-cli 0.163.0",
        };
        VerifiedProvider::from_version_output(provider, version).unwrap()
    }

    #[test]
    fn unverified_versions_fail_closed_and_supported_flags_do_not_change_permissions() {
        for (provider, version) in [
            (Provider::Codex, "codex-cli 0.100.0"),
            (Provider::Claude, "2.1.1 (Claude Code)"),
            (Provider::Codex, "codex-cli 99.0.0"),
        ] {
            assert!(VerifiedProvider::from_version_output(provider, version).is_err());
        }
    }

    #[test]
    fn opt_in_install_remove_preserve_config_and_third_party_hooks() {
        let original = r#"{"permissions":{"allow":["Read(*)"],"deny":["Bash(rm *)"]},"disableAllHooks":false,"custom":{"opaque":7},"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"third-party.exe"}]}],"FutureEvent":[]}}"#;
        for provider in [Provider::Claude, Provider::Codex] {
            let plan = plan(provider);
            let first = plan.install(original).unwrap();
            let receipt = first.receipt.unwrap();
            let second = plan.install(&first.output).unwrap();
            assert!(second.receipt.is_none());
            assert_eq!(first.output, second.output);
            let removed = uninstall_hooks(&second.output, &receipt).unwrap();
            assert_eq!(
                parse_config(original).unwrap(),
                parse_config(&removed).unwrap()
            );
            assert_eq!(
                parse_config(&uninstall_hooks(&removed, &receipt).unwrap()).unwrap(),
                parse_config(original).unwrap()
            );
        }
    }

    #[test]
    fn removal_preserves_later_handlers_and_edited_owned_commands() {
        let plan = plan(Provider::Claude);
        let edit = plan.install("{}").unwrap();
        let receipt = edit.receipt.unwrap();
        let mut config = parse_config(&edit.output).unwrap();
        config["hooks"]["Stop"][0]["hooks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"command","command":"keep.exe"}));
        config["hooks"]["SessionStart"][0]["hooks"][0]["command"] = "user-edited.exe".into();
        config["unrelated"] = true.into();
        let removed =
            parse_config(&uninstall_hooks(&pretty(&config).unwrap(), &receipt).unwrap()).unwrap();
        assert_eq!(
            removed["hooks"]["Stop"][0]["hooks"],
            json!([{"type":"command","command":"keep.exe"}])
        );
        assert_eq!(
            removed["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "user-edited.exe"
        );
        assert_eq!(removed["unrelated"], true);
    }

    #[test]
    fn exact_ownership_does_not_remove_similar_commands() {
        let plan = plan(Provider::Claude);
        let edit = plan.install("{}").unwrap();
        let receipt = edit.receipt.unwrap();
        let mut config = parse_config(&edit.output).unwrap();
        let mut other = config["hooks"]["Stop"][0].clone();
        other["hooks"][0]["args"] = json!([HOOK_SWITCH, "claude", "--user-added"]);
        config["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(other.clone());
        let removed =
            parse_config(&uninstall_hooks(&pretty(&config).unwrap(), &receipt).unwrap()).unwrap();
        assert_eq!(removed["hooks"]["Stop"], json!([other]));
    }

    #[test]
    fn windows_paths_are_literal_for_claude_and_encoded_for_codex() {
        let claude = plan(Provider::Claude);
        let handler = &claude.entries[0].group["hooks"][0];
        assert_eq!(
            handler["command"],
            r"C:\Program Files\O'Brien & Co\daijobu.exe"
        );
        assert_eq!(handler["args"], json!([HOOK_SWITCH, "claude"]));
        let codex = plan(Provider::Codex);
        let command = codex.entries[0].group["hooks"][0]["commandWindows"]
            .as_str()
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(command.split_whitespace().last().unwrap())
            .unwrap();
        let words: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(
            String::from_utf16(&words).unwrap(),
            format!("& 'C:\\Program Files\\O''Brien & Co\\daijobu.exe' {HOOK_SWITCH} codex")
        );
        assert!(!command.contains('&') && !command.contains('\''));
        for invalid in [
            "relative.exe",
            "C:\\bin\\bridge.cmd",
            "C:\\bad\npath.exe",
            "C:\\${EVIL}\\bridge.exe",
        ] {
            assert!(HookPlan::new(verified(Provider::Codex), Path::new(invalid)).is_err());
        }
    }

    #[test]
    fn malformed_and_oversized_config_and_input_fail_without_repair() {
        for bad in [
            "",
            "[]",
            "null",
            r#"{"hooks":null}"#,
            r#"{"hooks":{"Stop":{}}}"#,
        ] {
            assert!(plan(Provider::Claude).install(bad).is_err());
        }
        assert!(plan(Provider::Claude)
            .install(&" ".repeat(MAX_CONFIG_BYTES + 1))
            .is_err());
        assert!(read_bounded(&mut &vec![b' '; MAX_HOOK_BYTES + 1][..]).is_err());
        assert_eq!(read_bounded(&mut &b"{}"[..]).unwrap(), b"{}");
    }

    #[test]
    fn encoded_powershell_wrapper_preserves_native_stdin_bytes() {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let encode = |script: &str| {
            base64::engine::general_purpose::STANDARD.encode(
                script
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
        };
        // A native child reads raw stdin just as the Rust bridge does. Its output
        // is ASCII hex so outer PowerShell output encoding cannot hide input damage.
        let inner = encode("$s=New-Object IO.MemoryStream; [Console]::OpenStandardInput().CopyTo($s); [Console]::Out.Write([BitConverter]::ToString($s.ToArray()))");
        let exe = Path::new(&std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let script = format!(
            "& '{}' -NoLogo -NoProfile -NonInteractive -EncodedCommand {inner}",
            exe.to_str().unwrap().replace('\'', "''")
        );
        let mut process = Command::new(&exe)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encode(&script),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000) // CREATE_NO_WINDOW: fixture only, no agent or UI.
            .spawn()
            .unwrap();
        let input = "{\"session_id\":\"fixture\",\"ignored\":\"世界\"}".as_bytes();
        process.stdin.take().unwrap().write_all(input).unwrap();
        let output = process.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = input
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join("-");
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), expected);
    }

    #[test]
    fn native_controller_fake_provider_lifecycle() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut controller = AgentNotifications::start(move || {
            let _ = tx.send(());
        })
        .unwrap();
        let launch = controller.prepare(42, verified(Provider::Claude)).unwrap();
        assert_eq!(launch.variables()[2].1, launch.grant.generation.to_string());
        let send = |kind| {
            ipc::send(
                &launch.endpoint,
                &BridgeEvent {
                    version: WIRE_VERSION,
                    provider: launch.grant.provider,
                    generation: launch.grant.generation,
                    credential: launch.grant.credential.clone(),
                    session: "fixture-session".into(),
                    kind,
                    event_id: None,
                },
            )
            .unwrap();
            rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        };
        send(EventKind::Started);
        assert!(controller.drain().is_empty());
        send(EventKind::NeedsInput);
        let notice = controller.drain().pop().unwrap();
        assert_eq!(controller.resolve(notice.ticket).unwrap().tab, 42);
        controller.revoke(42);
        controller.prepare(42, verified(Provider::Claude)).unwrap();
        assert!(controller.resolve(notice.ticket).is_none());
        send(EventKind::Completed);
        assert!(controller.drain().is_empty());
    }
}
