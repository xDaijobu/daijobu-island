//! Real headless bridge processes and fake provider functions. No real agent/config/UI.
use base64::Engine;
use dj_core::agent_notifications::{self as agents, BridgeEvent, EventKind, Provider, Sessions};
use dj_win::agent_notifications::{random_credential, BridgeServer};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[test]
fn headless_hook_cli_normalizes_payload_and_returns_neutral_json() {
    let server = BridgeServer::start(|| {}).unwrap();
    for provider in [Provider::Claude, Provider::Codex] {
        let mut sessions = Sessions::default();
        let grant = sessions
            .register(4, provider, random_credential().unwrap())
            .unwrap();
        for (event, expected) in [
            ("SessionStart", EventKind::Started),
            ("PermissionRequest", EventKind::NeedsApproval),
        ] {
            let mut child = Command::new(env!("CARGO_BIN_EXE_daijobu"))
                .args(["--agent-notification-hook", provider.agent().key()])
                .env(agents::ENV_PIPE, server.endpoint())
                .env(agents::ENV_CREDENTIAL, grant.credential.expose())
                .env(agents::ENV_GENERATION, grant.generation.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(0x08000000)
                .spawn()
                .unwrap();
            let input = serde_json::json!({"session_id":"fixture-session", "hook_event_name":event,
                "transcript_path":"never-read-this-path", "tool_input":{"command":"calc.exe"},
                "last_assistant_message":"https://untrusted.invalid/not-executed"});
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.to_string().as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "{}");
            assert!(output.stderr.is_empty());
            let received = server.drain();
            assert_eq!(received.len(), 1);
            assert_eq!(received[0].kind, expected);
            let wire = String::from_utf8(received[0].encode().unwrap()).unwrap();
            assert!(
                !wire.contains("calc.exe")
                    && !wire.contains("never-read")
                    && !wire.contains("https://")
            );
            let notice = sessions.accept(&received[0]);
            assert_eq!(notice.is_some(), expected == EventKind::NeedsApproval);
        }
    }
}

#[test]
fn real_wrapper_finally_revokes_and_preserves_exit_environment_paths_and_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("O'Brien 你好 & (fixture)");
    std::fs::create_dir(&dir).unwrap();
    let exe = dir.join("daijobu.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_daijobu"), &exe).unwrap();
    for (version, fails, isolated) in [
        (Provider::Codex.verified_version(), false, true),
        (Provider::Codex.verified_version(), true, true),
        ("codex-cli 0.100.0", false, false),
    ] {
        let server = BridgeServer::start(|| {}).unwrap();
        let mut sessions = Sessions::default();
        let grant = sessions
            .register(9, Provider::Codex, random_credential().unwrap())
            .unwrap();
        let bridge = literal(exe.to_str().unwrap());
        let fail = if fails {
            "throw 'fixture-provider-failed'"
        } else {
            ""
        };
        let fake = format!("function global:codex {{ if ($args.Count -eq 1 -and $args[0] -eq '--version') {{ {}; return }}; [Console]::Out.Write('FLAGS='+($args -join ',')+'|ENV='+(-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('{}')))+'|'); '{{\"session_id\":\"fixture-session\",\"hook_event_name\":\"SessionStart\"}}' | & {bridge} --agent-notification-hook codex | Out-Null; $global:LASTEXITCODE=37; {fail}; '{{\"session_id\":\"fixture-session\",\"hook_event_name\":\"Stop\"}}' | & {bridge} --agent-notification-hook codex | Out-Null; $global:LASTEXITCODE=37 }}; ", literal(version), agents::ENV_CREDENTIAL);
        let wrapper = agents::powershell_launch(&grant, server.endpoint(), exe.to_str().unwrap());
        let script = format!("{fake} try {{ {wrapper} }} catch {{ [Console]::Out.Write('CAUGHT|') }}; [Console]::Out.Write('EXIT='+$global:LASTEXITCODE+'|CLEARED='+[string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('{}')))", agents::ENV_CREDENTIAL);
        let encoded = base64::engine::general_purpose::STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let output = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encoded,
            ])
            .env_remove(agents::ENV_PIPE)
            .env_remove(agents::ENV_CREDENTIAL)
            .env_remove(agents::ENV_GENERATION)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        let shown = String::from_utf8(output.stdout).unwrap();
        assert!(
            shown.contains("--dangerously-bypass-approvals-and-sandbox"),
            "{shown}"
        );
        assert_eq!(shown.contains(",--no-daemon"), isolated, "{shown}");
        assert!(shown.contains("EXIT=37|CLEARED=True"), "{shown}");
        assert_eq!(shown.contains("ENV=True"), isolated, "{shown}");
        assert_eq!(shown.contains("CAUGHT"), fails, "{shown}");
        assert!(!shown.contains(grant.credential.expose()));
        let events = server.drain();
        assert_eq!(events.last().map(|e| e.kind), Some(EventKind::LaunchExited));
        if !isolated {
            assert_eq!(events.len(), 1);
        }
        let mut ticket = None;
        for event in &events {
            if let Some(notice) = sessions.accept(event) {
                ticket = Some(notice.ticket);
            }
        }
        if let Some(ticket) = ticket {
            assert!(sessions.resolve(ticket).is_none());
        }
        // Late events cannot resurrect the launch after finally revoked its capability.
        let late = BridgeEvent {
            version: agents::WIRE_VERSION,
            provider: grant.provider,
            generation: grant.generation,
            credential: grant.credential,
            session: "fixture-session".into(),
            kind: EventKind::Completed,
            event_id: None,
        };
        assert!(sessions.accept(&late).is_none());
    }
    assert!(Path::new(&exe).exists());
}
