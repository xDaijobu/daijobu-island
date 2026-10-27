# Stage T report: Terminal

Built on 2026-10-27. The coordinator (Claude Opus 5.5) wrote the contract, the plan and the shared types. Four Claude Sonnet 5.5 workers built parts A–D in parallel worktrees; they were paused once by a Sonnet usage limit and resumed. The coordinator reviewed and merged them, wrote the wiring (part F), and fixed what a Sonnet reviewer and the live check found. Branch `xDaijobu/stage-t-terminal`. **Waiting for the user's live test.**

## Checklist

- [x] **Open/close:**
  - Openers: `Ctrl+Alt+`` (hotkey id 19, `VK_OEM_3`), the **Terminal** tile on Home (the old locked "dj CLI" tile, renamed and unlocked), ✕ in the header, or the hotkey again.
  - The island morphs from the notch into a 760 × 420 r 22 panel. The window grew to 776 × 428 logical to hold it.
- [x] **Keyboard:**
  - While the panel is open the island takes the keyboard (`set_keyboard` drops `WS_EX_NOACTIVATE` and answers `MA_ACTIVATE`).
  - Alt+F4, F10 and Alt+Space go to the shell, never to the island's system menu.
  - On close the keyboard goes back to the window that had it, but only if the island still holds it.
- [x] **Hide on blur:**
  - Clicking another window hides the panel and the shell keeps running.
  - With `hide_on_blur = false` the panel stays with a hollow cursor.
  - Ctrl+Alt+H, a snap or another panel also close it and release the keyboard.
- [x] **Session:**
  - One shell, started on first open and kept while hidden; it ends when the island quits.
  - When the shell exits: "process exited with code N · Enter to restart".
  - When it can't start: "could not start …: shell not found · Enter to retry".
- [x] **Shell:** `""` picks `pwsh.exe` if it is on PATH, otherwise Windows PowerShell (with `-NoLogo`). `cmd`, `wsl` or a path also work, with `args` and `cwd` (home by default). It runs through ConPTY using `alacritty_terminal` 0.26 (Apache-2.0).
- [x] **Panel:**
  - The header has the theme lamp, the shell's own title (oh-my-posh sets "pwsh in UseR") and ✕.
  - The grid is whole cells (about 104 × 26 at 100 %).
  - The cursor is a steady block, hollow when unfocused; selection is shown; a scroll thumb appears when scrolled back.
  - Bold, italic, underline, strike and wide characters (CJK) are drawn.
- [x] **Fonts:**
  - The default is `font = "Cascadia Mono"`, falling back to Geist Mono.
  - A glyph the font lacks comes from a system font.
  - **Prompt icons** (Private Use Area) come from "Symbols Nerd Font Mono" or "Symbols Nerd Font" by name first. That was added after the live check, where oh-my-posh's icons drew as boxes.
- [x] **Colours:**
  - The pill colour is the background, the text colour the default foreground, the accent the cursor and selection.
  - Two tuned ANSI-16 sets (dark and light pills) with measured contrast; truecolor and 256 colours work.
  - A theme may set `terminal_ansi = [16]`.
- [x] **Mouse and clipboard:**
  - Drag to select, double-click for a word.
  - Ctrl+C copies with a selection and interrupts without one. Ctrl+V, Ctrl+Shift+V and Shift+Insert paste; right-click copies or pastes.
  - The wheel scrolls 3 lines, with touchpad steps added up; typing returns to the bottom.
  - A paste is one block (bracketed when asked), and control characters are dropped, so a pasted `ESC[201~` can't break out.
  - OSC 52 clipboard writes from programs are ignored.
- [x] **Idle:** the PTY thread blocks, wakes are coalesced, and nothing polls. Measured 0 ms of CPU over 10 s with the panel open and idle, and 0 ms with it closed.
- [x] **Docs:** README (status, shortcut, a "Terminal" section with the `[terminal]` keys, the crate list), AGENTS.md (`crates/dj-term/`), NOTICE (alacritty_terminal).
- [x] **Tests:** 932 in the workspace. New ones:
  - `dj-core`: 485 in total, including key and char mapping (AltGr), paste, palette contrast, config, theme override, island state and the window size.
  - `dj-term`: 48, including real ConPTY round trips, drop timing, a spawn failure and colour resolution.
  - `dj-render`: 26 terminal and 2 font tests.
  - `dj-win`: 27, for message translation, keyboard mode and the surrogate join.
  - `app::terminal`: 14 controller tests with fakes.

## Verification

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 932 passed.
- `cargo build --release`: OK.

### Live (Windows 11, 1440 × 900 at 100 %, notch)

The panel was driven by posted window messages: the hotkey, typed characters and Enter. Screenshots were crops of the panel only and were deleted afterwards.

| Acceptance | Result |
|---|---|
| 1. Opens with a PowerShell prompt; `git --version`, `cargo --version` and coloured `Write-Host` work | ✅ (the user's oh-my-posh prompt with icons, PSReadLine syntax colours) |
| 2. Close and reopen keeps the session; `ping -n 30` kept running behind the closed panel | ✅ |
| 3. Hide on blur | ⏳ needs a real click into another window (user test) |
| 4. Ctrl+C stops `ping` (as 0x03) | ✅ after a fix (below). Selection copy, Ctrl+V, wheel and right-click need real input (user test) |
| 5. `exit` shows the restart line; Enter restarts | ✅ |
| 6. All four themes read well | ✅ Paper Zine, Clean, Tobacco Tape, Midnight Cassette. `vim`/`less` not installed here |
| 7. Idle CPU ≈ 0, open and closed | ✅ 0 ms / 10 s for both |

`漢字` drew as two wide cells.

### Found live and fixed
- **Ctrl+C was ignored by the shell.** A process passes "ignore Ctrl+C" on to the programs it starts. The island had been started by a script whose process group ignores it, so the shell and `ping` did too. `dj-term` now clears the flag (`SetConsoleCtrlHandler(NULL, FALSE)`) right before it starts a shell, so the shell always gets Ctrl+C however the island was launched.
- **Prompt icons drew as boxes.** DirectWrite's fallback never looks in the Private Use Area (and Windows' Segoe Fluent Icons would draw other icons there). The fix is the Nerd Font name chain above, with a test that is skipped when no Nerd font is installed.

## Review of the wiring

A read-only Sonnet 5.5 reviewer read the wiring and found no serious defects. These were fixed:
1. **Handing back the keyboard:** it now happens only while the island still holds it (and never to the island itself). A collapse from elsewhere (a snap, another panel) also hands it back, so the keyboard is never left on a window that won't take it.
2. **OSD drag:** the volume OSD's bar can be dragged while the terminal is the state.
3. **Selection drag:** dragging up into the header keeps selecting the top row.
4. **Right-click and wheel:** they act only on the panel itself, not on the notch shoulders and not while the OSD is up.
5. **Pending snap:** the hotkey doesn't open the panel while a snap is about to start.
6. **Foreground refused:** if the system refuses the foreground when opening, the cursor shows hollow rather than pretend.

Left as is (low): while the panel is closed, a chatty background command still wakes the UI per output burst (a drain plus the sync checks; cheap). A double click on ✕ can reopen Home with its second press. `VK_OEM_3` is the backtick on US layouts; on others the hotkey is that key position.

## How to run

```powershell
cargo build --release
target\release\daijobu.exe
# Ctrl+Alt+` opens the terminal; ~/.daijobu/config.toml [terminal] changes the shell, font, hide_on_blur
```

## Not verified / known gaps
- The real keyboard (the hotkey taking the foreground, typing, AltGr, Ctrl+C with a selection, Ctrl+V), the clipboard, hide on blur, the wheel and right-click: these need the user's live test.
- Not tried: `vim`/`less`, scales other than 100 %, and a second monitor.
- Mouse reporting to programs is out of scope, so the wheel does nothing in a full-screen program (alternate screen).
- Box-drawing characters come from the font, so Cascadia's `─` may show faint joins; procedural box drawing would be a follow-up.
- alacritty keeps one process handle per session open (an upstream behaviour); restarts leak a handle each.
