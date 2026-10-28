# Stage T3: New-tab menu with Claude and Codex (yolo) — plan

> Written by the coordinator (Claude Opus 5.5). Same branch and PR as T/T2 (`xDaijobu/stage-t-terminal`, PR #20). Read `task/stage-t2-plan.md` first: this builds on its tab model, menu and controller.

**Goal:** clicking "+" in the terminal tab bar opens an island-styled menu with three rows:

```
[ pwsh ][ claude ✕ ][ + ]                    ✕
                     ┌──────────────────┐
                     │ PowerShell       │   ← the configured shell, as "+" did before
                     │ Claude  · yolo   │
                     │ Codex   · yolo   │
                     └──────────────────┘
```

An agent tab starts **in the active tab's folder** (falling back to the configured `[terminal] cwd`).

**Decided with the user (2026-10-28):**
- "+" always opens the menu. **Ctrl+Shift+T stays a plain shell tab**, and there are no new keyboard shortcuts.
- The yolo flags are Orca's own (`agentDefaultArgs` in Orca's `orca-data.json`):
  - `claude --dangerously-skip-permissions`
  - `codex --dangerously-bypass-approvals-and-sandbox`
- The agent tab starts in the active tab's current folder.

## How an agent tab runs

`claude` and `codex` are npm `.ps1` shims on this machine (`%APPDATA%\npm`), so they cannot be the tab's program. An agent tab is a PowerShell tab with a command:

```
powershell.exe -NoLogo -NoExit -Command "claude --dangerously-skip-permissions"
```

- `-NoExit`: when the agent quits, the tab stays at a normal prompt in the same folder, so the session doesn't vanish with its output.
- The shell is the configured one if it is PowerShell (5.1 or 7), else `powershell.exe`.
- The tab is named `claude` / `codex` until the program sets its own title (Claude does).
- **Pinning works for free:** `Spec { shell, args, cwd }` already holds the whole command, so a pinned agent tab is saved to `terminal-tabs.toml` and relaunched in yolo mode after a restart. (Worth knowing: a pinned yolo tab starts an agent with no permission prompts every time the island starts. That matches what the user asked for, and the report will say it.)
- The agents and their flags are built-in constants in `dj-core`, not new settings, because nobody asked for more than these two. Gemini, Amp and the rest are left out.

## The active tab's folder

PowerShell's `Set-Location` does not change the process's Win32 current directory, so reading it from the process (the PEB) gives the folder it started in, not where the user is now. The shell has to report it:

1. **Prompt hook.** Every PowerShell tab dj starts (plain or agent) gets one extra startup command that wraps whatever `prompt` the profile set (oh-my-posh included). The wrapper adds an invisible `OSC 9;9;"<path>"` before the prompt. This is the sequence Windows Terminal documents for "duplicate tab", and its zero width doesn't disturb oh-my-posh. Wrapping happens after the profile has run, so the user's prompt is kept. For a plain tab:
   `-NoExit -Command "<hook>"`
   For an agent tab:
   `-NoExit -Command "<hook>; claude …"`
2. **Reading it.** vte 0.15 drops OSC 7 and OSC 9;9 (`[unhandled osc_dispatch]`), and Alacritty's `EventLoop` reads the pty itself. `dj-term` therefore wraps the ConPTY in a thin `EventedPty`/`EventedReadWrite` adapter. The adapter passes every byte through unchanged and runs a small state machine that picks out `ESC ] 9 ; 9 ; "path" (BEL | ESC \)` and `ESC ] 7 ; file://host/path …`. A path it finds becomes `TermEvent::Cwd(String)`.
3. **Using it.** `app/src/terminal.rs` keeps `cwd: Option<String>` per tab, updated from `TermEvent::Cwd`. A new agent tab uses the active tab's `cwd` if that folder still exists, else the configured `cwd`. The PowerShell row in the menu keeps today's behaviour: it opens in the configured folder.
4. **Other shells** (cmd, WSL, Git Bash) get no hook. Their tabs have no known cwd, so an agent opened from them starts in the configured folder.

**Spike: done and it passed (2026-10-28).** The test used raw `tty::new` and read the pty directly, with PowerShell 5.1 and the user's oh-my-posh profile.
- ConPTY forwards `ESC]9;9;<path>BEL` and `ESC]9;9;<path>ESC\` unchanged.
- The hook below fires on the first prompt (`…;C:\Users\UseR\AppData\Local\Temp`) and again after `cd C:\Windows` (`…;C:\Windows`).
- The oh-my-posh prompt renders exactly as before.

The hook doesn't touch the prompt string. It writes the sequence straight to the console, then calls the original prompt:

```powershell
$global:__dj_prompt = $function:prompt; function global:prompt { $l = $executionContext.SessionState.Path.CurrentLocation; if ($l.Provider.Name -eq 'FileSystem') { [Console]::Write([char]27 + ']9;9;' + $l.ProviderPath + [char]27 + '\') }; & $global:__dj_prompt }
```

It contains no `"`, so `escape_args` has nothing to mangle. `EventedReadWrite::reader()` returns `&mut Self::Reader`, so the adapter can be `Reader = Self`. Its `io::Read` calls `pty.reader().read()` and then scans the bytes it read.

**Where the args come from:** the agent is stored rather than baked in. `Spec` and `PinnedTab` gain `agent` (`""`, `"claude"` or `"codex"`; serde default `""`). The app builds the args each time a tab starts:
- Take `resolve_shell(shell, args)`.
- If the program is PowerShell and the user's args have no `-Command`/`-File`/`-c`, append `-NoExit -Command <hook>[; <agent command line>]`.
- An agent tab whose configured shell isn't PowerShell runs `powershell.exe`.

So a pinned agent tab never doubles the hook.

## Parts

### Part A — model (`crates/dj-core/src/term_tabs.rs`, `crates/dj-core/src/term_agents.rs`)

- `term_agents.rs` (new):
  - `enum NewTab { Shell, Agent(Agent) }`
  - `enum Agent { Claude, Codex }` with `label()` ("Claude", "Codex"), `command()` ("claude", "codex") and `yolo_args()` (Orca's flags).
  - `fn agent_command_line(agent) -> String`
  - `fn hook_script() -> &'static str` (the prompt wrapper)
  - `fn powershell_args(hook: bool, agent: Option<Agent>) -> Vec<String>`, which quotes arguments correctly for `-Command`
- `term_tabs.rs`:
  - "+" opens a **new-tab menu**: the same `Menu` and `MenuView`, anchored under "+" and kept inside the panel.
  - `MenuAction` gains `NewShell`, `NewClaude` and `NewCodex`.
  - `Press::Plus` goes away. The menu press reports `Press::Menu(action, _)`, whose id is ignored for the new-tab actions.
  - At `MAX_TABS` there is no "+", so there is no menu either.
- **Tests:**
  - The menu opens under "+" with the right rows and stays inside `TERM_W` when "+" sits far right.
  - An outside press closes it without acting.
  - The arguments: the exact flags, and quoting a hook that contains `"` and `$`.

### Part B — capturing the cwd (`crates/dj-term`)

- The OSC scanner is a pure function over byte chunks, so it is testable headless. It handles a sequence split across reads, both terminators, quoted and unquoted paths, `file://` URLs with `%20`, and garbage. Nothing it does changes the byte stream.
- The pty adapter feeds the scanner and pushes `TermEvent::Cwd` through the existing listener queue.
- `resolve_shell` tells the app whether the shell is PowerShell, so the hook is added only there.
- **Tests:** the scanner cases above, and that a stream with the sequence in it reaches the emulator unchanged (same screen as without the adapter).

### Part C — controller and drawing (coordinator)

- `app/src/terminal.rs`:
  - Handle `NewShell`, `NewClaude` and `NewCodex`.
  - Store `cwd` from `TermEvent::Cwd`.
  - Choose the agent tab's folder.
  - Name the tab `claude` / `codex` until the program sets a title.
  - `TabKey::New` (Ctrl+Shift+T) still makes a plain shell tab.
- `crates/dj-render/src/terminal.rs`: the menu already draws `MenuView`. Check that the "· yolo" hint reads well, dim compared with the label.
- When `claude` / `codex` is not installed, the tab shows PowerShell's own "not recognized" error and stays at a prompt (`-NoExit`). No extra handling is needed.

## Review Focus

1. **Quoting:** the hook and the agent command reach PowerShell intact through ConPTY's `escape_args`. Paths with spaces and `'` are covered.
2. **The scanner never alters or delays output.** The scan runs on bytes already read; it does not buffer them.
3. **The user's prompt survives:** oh-my-posh still renders, and transient prompts and right-side segments still work.
4. **A stale cwd** (the folder was deleted, or the tab is cmd) falls back to the configured folder.
5. **Pinned agent tabs** restore with their args and the hook isn't doubled.

## Live test (on this machine)

1. Click "+": the menu shows PowerShell / Claude · yolo / Codex · yolo. A click outside closes it.
2. In a tab, `cd C:\Users\UseR\orca\projects\daijobu-island`, then choose "+" → Claude. Claude starts in that folder with no permission prompt (check its "bypass permissions" banner). `/exit` leaves a PowerShell prompt in the same folder.
3. Do the same for Codex (check that it shows full access / no approvals).
4. Pin the Claude tab, restart the island, open the terminal: Claude starts again.
5. Ctrl+Shift+T still opens a plain PowerShell tab, and the oh-my-posh prompt looks unchanged.
6. Rename `claude` out of PATH (or test from cmd as the shell): the tab shows the error and stays usable.

## Out of scope

- Other agents (Gemini, Amp, …).
- Editing the flags in Settings.
- Keyboard shortcuts for agent tabs.
- An Orca-style worktree picker.
