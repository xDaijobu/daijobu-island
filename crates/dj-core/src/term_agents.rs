//! The coding agents the terminal's new-tab menu can start (Claude Code, Codex), their
//! "yolo" flags, and the PowerShell start-up command that also reports the working
//! directory to the panel (OSC 9;9). Pure strings: the app spawns the process.

/// A coding agent that can be started in a new tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

    /// The name shown in the menu.
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "Codex",
        }
    }

    /// The program to run.
    pub fn command(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// The flags that skip permission prompts (the same ones Orca uses).
    pub fn yolo_args(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &["--dangerously-skip-permissions"],
            Agent::Codex => &["--dangerously-bypass-approvals-and-sandbox"],
        }
    }

    /// The stable key saved in the pinned-tabs file.
    pub fn key(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// The agent for a saved key (exact, lowercase only).
    pub fn from_key(key: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|a| a.key() == key)
    }
}

/// The command line that starts the agent in yolo mode, e.g.
/// `claude --dangerously-skip-permissions`.
pub fn command_line(agent: Agent) -> String {
    let mut s = agent.command().to_string();
    for a in agent.yolo_args() {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// PowerShell that wraps `prompt` so every prompt reports the current folder as
/// `ESC ] 9;9;<path> ESC \`. It has no double quote, so it survives Windows argument
/// escaping.
pub const CWD_HOOK: &str = r#"$global:__dj_prompt = $function:prompt; function global:prompt { $l = $executionContext.SessionState.Path.CurrentLocation; if ($l.Provider.Name -eq 'FileSystem') { [Console]::Write([char]27 + ']9;9;' + $l.ProviderPath + [char]27 + '\') }; & $global:__dj_prompt }"#;

/// The `-Command` text: the hook, then (for an agent) its command line.
pub fn powershell_command(agent: Option<Agent>) -> String {
    match agent {
        None => CWD_HOOK.to_string(),
        Some(a) => format!("{CWD_HOOK}; {}", command_line(a)),
    }
}

/// Arguments to append to a PowerShell start so it installs the folder hook and optionally
/// starts an agent: `-NoExit -Command <powershell_command>`.
///
/// When `user_args` already takes control of start-up (`-Command`, `-c`, `-File`, `-f`,
/// `-EncodedCommand`, `-e`, `-ec`, any case) the hook is left out: this returns an empty
/// vec for no agent, and `-NoExit -Command <command_line>` (no hook) for an agent.
pub fn powershell_extra_args(user_args: &[String], agent: Option<Agent>) -> Vec<String> {
    const TAKEN: [&str; 7] = [
        "-command",
        "-c",
        "-file",
        "-f",
        "-encodedcommand",
        "-e",
        "-ec",
    ];
    let taken = user_args
        .iter()
        .any(|a| TAKEN.contains(&a.to_ascii_lowercase().as_str()));
    match (taken, agent) {
        (false, a) => vec!["-NoExit".into(), "-Command".into(), powershell_command(a)],
        (true, None) => Vec::new(),
        (true, Some(a)) => vec!["-NoExit".into(), "-Command".into(), command_line(a)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_and_their_flags() {
        assert_eq!(Agent::ALL, [Agent::Claude, Agent::Codex]);
        assert_eq!(Agent::Claude.label(), "Claude");
        assert_eq!(Agent::Codex.label(), "Codex");
        assert_eq!(Agent::Claude.command(), "claude");
        assert_eq!(Agent::Codex.command(), "codex");
        assert_eq!(
            Agent::Claude.yolo_args(),
            &["--dangerously-skip-permissions"]
        );
        assert_eq!(
            Agent::Codex.yolo_args(),
            &["--dangerously-bypass-approvals-and-sandbox"]
        );
    }

    #[test]
    fn keys_round_trip_and_are_exact() {
        for a in Agent::ALL {
            assert_eq!(Agent::from_key(a.key()), Some(a));
        }
        assert_eq!(Agent::from_key("Claude"), None);
        assert_eq!(Agent::from_key("CODEX"), None);
        assert_eq!(Agent::from_key(""), None);
        assert_eq!(Agent::from_key("gemini"), None);
    }

    #[test]
    fn command_lines() {
        assert_eq!(
            command_line(Agent::Claude),
            "claude --dangerously-skip-permissions"
        );
        assert_eq!(
            command_line(Agent::Codex),
            "codex --dangerously-bypass-approvals-and-sandbox"
        );
    }

    #[test]
    fn the_hook_is_one_line_without_double_quotes() {
        assert!(!CWD_HOOK.contains('"'));
        assert!(!CWD_HOOK.contains('\n'));
        assert!(CWD_HOOK.starts_with("$global:__dj_prompt = $function:prompt; "));
        assert!(CWD_HOOK.contains("']9;9;'"));
        assert!(CWD_HOOK.ends_with("& $global:__dj_prompt }"));
    }

    #[test]
    fn powershell_command_with_and_without_an_agent() {
        assert_eq!(powershell_command(None), CWD_HOOK);
        assert_eq!(
            powershell_command(Some(Agent::Claude)),
            format!("{CWD_HOOK}; claude --dangerously-skip-permissions")
        );
    }

    #[test]
    fn extra_args_add_the_hook() {
        let none: Vec<String> = Vec::new();
        assert_eq!(
            powershell_extra_args(&none, None),
            vec!["-NoExit", "-Command", CWD_HOOK]
        );
        let user = vec!["-NoLogo".to_string(), "-NoProfile".to_string()];
        let got = powershell_extra_args(&user, Some(Agent::Codex));
        assert_eq!(got[..2], ["-NoExit", "-Command"]);
        assert_eq!(got[2], powershell_command(Some(Agent::Codex)));
    }

    #[test]
    fn user_startup_flags_win() {
        for flag in [
            "-Command",
            "-command",
            "-C",
            "-File",
            "-f",
            "-EncodedCommand",
            "-E",
            "-ec",
        ] {
            let user = vec!["-NoLogo".to_string(), flag.to_string()];
            assert!(powershell_extra_args(&user, None).is_empty(), "{flag}");
            assert_eq!(
                powershell_extra_args(&user, Some(Agent::Claude)),
                vec![
                    "-NoExit".to_string(),
                    "-Command".to_string(),
                    "claude --dangerously-skip-permissions".to_string()
                ],
                "{flag}"
            );
        }
        // Not flags: a value that merely looks similar.
        let user = vec!["-NoProfile".to_string()];
        assert_eq!(powershell_extra_args(&user, None).len(), 3);
    }
}
