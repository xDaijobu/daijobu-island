# Stage 4 brief: Vitals (compact) (for the worktree agent)

Your worktree is **stacked on Stage 3** (OSD volume). It already contains the OSD work. Build on top of it and don't rewrite it.

## HEADLESS MODE (overrides everything)
The user may be gaming on this PC:
- Never run `daijobu.exe` or anything that shows windows, installs hooks, sends input, changes volume or takes screenshots.
- Set `CARGO_BUILD_JOBS=2`. Verify **only** with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
- Unit tests must not depend on live system values. Test the pure maths with fixed inputs. Reading the real system APIs once inside a test is OK only if the test asserts nothing about the values.
- In the report, mark live checks as "not verified, user to test".

## Read first
1. `AGENTS.md`.
2. **Contract:** `C:/Users/chris/orca/projects/daijobu-island/task/progress.md`, section **"Stage 4: Vitals (compact)"**. Read it from the main checkout and don't edit it. Build exactly that.
3. `C:/Users/chris/orca/projects/daijobu-island/docs/plan.html`: the Vitals panel (400×150, rings 56 px with a 4 px stroke, net sparkline) and the health score section.
4. `task/stage-1-report.md` and `task/stage-3-report.md`: the code layout and how OSD hooked into the island.

## Subagents (Agent tool, model `haiku`)
Delegate these. Each owns its files, and each gets the headless rules.
1. `crates/dj-vitals`: samplers (`GetSystemTimes`, `GlobalMemoryStatusEx`, `GetDiskFreeSpaceExW` on the system drive, `GetIfTable2` up/non-loopback, `GetSystemPowerStatus`, `GetTickCount64`). Pure rate/delta helpers (with counter wrap) live in a separate module with tests.
2. `dj-core` health model: score, penalties, bands, reason tags, plus tests, all following the contract weights exactly.
3. `dj-render` Vitals panel + idle-lamp colour: rings, sparkline (60 samples), net text, battery, score/band, reason tag. Use the existing theme tokens (accent/warn) and fonts.

You (the integrator) own:
- the sampler worker thread and its cadence (1 Hz while the panel is open, otherwise every 30 s, nothing while hidden), using `PostMessageW(WM_APP + n)` to wake the UI
- the island state integration and the Vitals tile unlock
- config `[vitals] health_lamp = true`
- the final fmt/clippy/test

## Rules
No features beyond the contract (no disk I/O, top processes, Sweep or settings UI). No GPL code: Mole is a concept reference only. Add `// SAFETY:` on every `unsafe`. Commit in small steps on your branch and don't push.

## Finish
Write `task/stage-4-report.md`: an honest per-item checklist, how the user should test, known issues, and what was not verified. Then stop.

Toolchain: `%USERPROFILE%\.cargo\bin`, `C:\Program Files\LLVM\bin`, `%LOCALAPPDATA%\Microsoft\WinGet\Links`. Prepend them to `PATH` if needed.
