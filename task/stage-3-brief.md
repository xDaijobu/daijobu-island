# Stage 3 brief: OSD volume (for the worktree agent)

Your worktree starts from `master` (the Stage 1 app shell). Stage 2 (Music) lives on another branch and is **not** in your tree. Ignore it.

## Read first
1. `AGENTS.md`: the rules.
2. **Contract:** `C:/Users/chris/orca/projects/daijobu-island/task/progress.md`, section **"Stage 3: OSD volume"**. Read it from the main checkout and don't edit it. Every checkbox there is your job, and nothing more.
3. `C:/Users/chris/orca/projects/daijobu-island/docs/plan.html`: OSD size 220×40, radius 20, icon + 4 px track + mono value.
4. `task/stage-1-report.md`: the code layout.

## HEADLESS MODE (overrides everything below)
The user is gaming on this PC right now. You must be **invisible**:
- **Never run `daijobu.exe`** or any exe that opens windows, installs hooks, or registers audio callbacks.
- **No `SendInput`**, no key presses, no mouse, no volume or mute changes, no screenshots, and no opening apps or browsers.
- Build at low priority with few cores: set `CARGO_BUILD_JOBS=2` and avoid back-to-back full rebuilds.
- Verify **only** with `cargo fmt`, `cargo clippy` and `cargo test`. Unit tests must not touch real devices or the real keyboard: test pure logic, and put the Win32/COM parts behind thin wrappers that the tests don't call.
- Every live check in the "Verify" section below becomes **"not verified, user to test"** in your report.

## Hard rules
- Never play audio. You may change the volume to test, but **record the original level and mute state first and restore them** before you finish.
- Don't drive the user's mouse or GUI. Test with synthetic `SendInput` key presses (volume keys) and logs, and keep it short.
- No features beyond the contract, no GPL code, and `// SAFETY:` on every `unsafe`.

## Use subagents (Haiku 5.5)
Delegate independent parts with the Agent tool (`model: haiku`). Each subagent owns its own files. Give each one these hard rules. You are the integrator: you review, wire everything up, and run fmt/clippy/test yourself. Suggested split:
1. `crates/dj-win/src/volume.rs`: `IAudioEndpointVolume` wrapper (get/set level, step up/down, mute) + `IAudioEndpointVolumeCallback` + `IMMNotificationClient` (re-acquire on default-device change). Events go to the UI via `PostMessageW(WM_APP + n)`.
2. `crates/dj-win/src/hook.rs`: the `WH_KEYBOARD_LL` hook for `VK_VOLUME_UP/DOWN/MUTE`. Swallow the key (return 1) only while `replace_volume_flyout` is on. The callback just posts a message.
3. `dj-core` OSD model + tests: a temporary-overlay state, a 1.5 s hide timer reset on every change, icon thresholds (muted / 0–33 / 34–66 / 67–100), clamping.
4. `dj-render` OSD view: 220×40 pill, speaker icon variants, level bar (spring), mono value, click/drag on the bar sets the volume.

You own: island state integration, app wiring (messages → model → redraw), the OSD tile unlock, config `[osd] replace_volume_flyout = true`, and the tray (no tray changes are needed unless trivial).

## Verify
- Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
- Run the release exe:
  - A synthetic volume-up/down shows the pill (check the log or state) and the level changes.
  - Changing the volume via the API from outside triggers the callback.
  - The pill hides after 1.5 s, and idle CPU is about 0.
  - **Restore the original volume and mute.**
- If possible, screenshot only the island showing the OSD to `task/stage-3-osd.png`, cropped to the pill. If that's not possible without capturing other windows, skip it.

## Finish
Commit in small steps on your branch and don't push. Write `task/stage-3-report.md`: a per-item checklist (honest), how the user should test, known issues, and what was not verified. Then stop.

Toolchain: `%USERPROFILE%\.cargo\bin`, `C:\Program Files\LLVM\bin`, `%LOCALAPPDATA%\Microsoft\WinGet\Links`. Prepend them to `PATH` if needed.
