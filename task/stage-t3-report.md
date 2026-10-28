# Stage T3 report: new-tab menu with Claude and Codex (yolo)

Plan: `task/stage-t3-plan.md`. Branch `xDaijobu/stage-t-terminal`, same PR as T/T2 (#20).

## Checklist

- [x] "+" opens a menu with three rows: Terminal / Claude · yolo / Codex · yolo. A second press on "+" or a press outside closes it without acting. The menu is placed under "+" and kept inside the panel. (`dj_core::term_tabs`)
- [x] Ctrl+Shift+T still opens a plain shell tab, and there are no new shortcuts.
- [x] Claude runs `claude --dangerously-skip-permissions` and Codex runs `codex --dangerously-bypass-approvals-and-sandbox`. These are Orca's `agentDefaultArgs`. (`dj_core::term_agents`)
- [x] An agent tab runs as `powershell -NoLogo -NoExit -Command "<hook>; claude …"`. It uses the configured shell if that is PowerShell, else `powershell.exe`. When the agent ends, the tab stays at a prompt.
- [x] An agent tab starts in the active tab's folder.
  - A prompt hook in every PowerShell tab reports the folder with OSC 9;9.
  - `dj-term` reads it from a scanner on the pty's read side (`tap.rs`, `osc.rs`) and turns it into `TermEvent::Cwd`.
  - If the folder is unknown or no longer exists, the tab uses the configured folder.
- [x] A tab is named `claude` / `codex` until the program sets its own title.
- [x] A pinned agent tab is saved with `agent = "claude"|"codex"` in `terminal-tabs.toml`. After a restart it comes back as the same agent, and the hook is not doubled.
- [x] Each agent row in the menu draws a dim "yolo" hint on the right.

## Verification

- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 1084 passed, 0 failed. New tests:
  - `term_agents`: 7.
  - The plus menu in `term_tabs`: 11.
  - The OSC scanner: 12, plus `is_powershell`.
  - One real-PowerShell test through ConPTY: the `Cwd` event arrives, and the sequence doesn't show on screen.
  - The controller: 6 (the active-tab start folder, an exe-path title, the hook on a plain tab, Claude in the active tab's folder, Codex falling back from a deleted folder, a pinned agent restored).
- Spike before building: PowerShell 5.1 with the user's oh-my-posh, raw ConPTY. OSC 9;9 comes through with both terminators, and the prompt looks the same.
- `cargo build --release` works, and the exe is running from this worktree.

## Live test (coordinator, 2026-10-28, synthetic input and screenshots)

- [x] "+" opens the menu, and "yolo" is drawn dim on the right.
- [x] `cd` to the project, then "+" → Claude. Claude Code starts in `~\orca\projects\daijobu-island` and shows "bypass permissions on".
- [x] "+" → Codex. The process command line is `codex.exe --dangerously-bypass-approvals-and-sandbox` and its cwd (read from the PEB) is the project folder. Codex isn't signed in on this machine, so its own UI showed only the login screen.
- [x] Pin the Claude tab. `terminal-tabs.toml` gets `agent = "claude"` and the folder. After a restart (WM_CLOSE, then start), Claude is back in that folder with bypass on. Closing the pinned tab removes it from the file again.
- [x] Ctrl+Shift+T opens a plain tab, and the oh-my-posh prompt looks unchanged.
- [ ] An agent that isn't installed: not tried.

Found live and fixed (commit after `45c399c`):
- **Codex launched from a Claude tab went to home.** The agent tab had not reached a prompt, so it hadn't reported a folder. Now the tab's start folder is used before the configured one.
- **The Codex tab was titled `C:\WINDOWS\system32\cmd.exe`.** That is conhost titling the console after an exe Codex ran. A title that is only an absolute `.exe` path is now ignored, so the tab stays `codex`. This also helps plain PowerShell tabs, which get their own path as a title.

## Notes and known issues

- **Every PowerShell tab gets the prompt hook.** If the user later re-runs `oh-my-posh init` by hand in a tab, the folder stops being reported for that tab, and agents opened from it fall back to the configured folder.
- **User args that take over startup** (`-Command`, `-File`, …) mean no hook; an agent from such a tab starts without it too.
- **A pinned yolo tab** starts an agent with no permission prompts on every island start, as asked.
- **New dependency:** `polling = "3"` in `dj-term`. It was already in the lock at 3.11 through `alacritty_terminal`, and no second version was added.
- **Claude's title starts with `✳` (U+2733)**, which the tab font lacks, so it draws as a box. Not fixed here; it's a font-fallback issue in the tab strip.
- **Starting the island from a Claude Code session** passes `CLAUDE_CODE_*` env vars down, and Claude then warns about an inherited child-session marker. A normal start (autostart or Explorer) doesn't have them.
- **The shell row is labelled "Terminal"**, not "PowerShell", because the configured shell may be cmd or WSL.
