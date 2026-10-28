# Terminal wheel report

Brief: [stage-terminal-wheel-brief.md](stage-terminal-wheel-brief.md).

## Delivered

- [x] Wheel events now carry their pointer cell through the app controller into the real session backend. Headers and open tab menus do not send application scroll input.
- [x] Programs requesting xterm mouse tracking receive wheel button reports (64 up / 65 down) using SGR, legacy or UTF-8 coordinates. Coordinates are clamped to the active grid; legacy coordinates outside its protocol range are omitted.
- [x] Alternate-screen programs without mouse tracking receive up/down keys when alternate-scroll mode is enabled, respecting application cursor mode. Disabling alternate-scroll is honored; ordinary shell history stays local.
- [x] Preserve fractional-wheel accumulation and three lines per notch. Bound emitted reports per message. No dependency, unsafe code, polling or agent-specific input shortcuts added.
- [x] README documents the behavior. General mouse click/drag reporting remains outside scope; this supersedes the original terminal report's no-wheel-in-full-screen gap.

## Verification

- fmt, workspace clippy with all targets and warnings denied: pass.
- Workspace tests: **1223 passed**, 0 failed. Six emulator regressions exercise real parsed VT modes, local history, tracking in both screen buffers, protocol formats/ranges, alternate-screen cursor behavior, resets and bounds. One controller regression guards headers/menus; existing wheel test now checks cell coordinates too.
- New real ConPTY integration test starts a local PowerShell/C# VT client, sends a normal three-line wheel notch through `Session::wheel`, reads raw console key records and verifies the first complete SGR report's bytes and coordinates. No agent prompts or external services.
- Locked release build: pass, using main checkout `target/stage-t4` cache. Verified executable copied into `target/release/daijobu.exe` and restarted.
- Desktop smoke: opened Terminal, posted wheel-up/down at a client-translated screen point, collapsed it again. App remained responsive; clipboard unchanged. No real cursor movement, shell commands or agent prompts in the user's terminal.

## Limits and run

Open a Codex/Claude tab with enough content, put the pointer over the terminal text and scroll. Mouse-tracking applications own their content; applications that expose neither mouse scrolling nor alternate-screen navigation still only offer emulator history.

Real Codex/Claude transcripts were not generated or visually scrolled during verification. On this Windows host, the synthetic raw VT client buffered a single isolated mouse report until further input; the normal three-report wheel notch is verified end to end. Very small gestures can therefore wait for another input report in that client. Mouse modifiers and a Shift override to local history are not part of this change.
