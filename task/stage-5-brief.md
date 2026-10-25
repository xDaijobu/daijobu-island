# Stage 5 brief: Vitals + Sweep (for the worktree agent)

Your worktree starts from `master`, which includes Stages 1–4 (shell, Music, OSD, Vitals). You are adding **Sweep inside the Vitals panel**, plus full vitals.

## HEADLESS + NO REAL FILES (overrides everything)
- Never run `daijobu.exe` or anything that shows windows, installs hooks, sends input, changes volume, plays audio or takes screenshots.
- **Never scan, clean, recycle or delete anything on the user's real disk**, not even a "dry run" against real paths. Every scan and delete in tests happens inside `tempfile` directories that you create. Don't call `DnsFlushResolverCache` or `SHEmptyRecycleBinW` in tests. Put them behind a trait and test the wiring with a fake.
- `CARGO_BUILD_JOBS=2`. Verify only with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and `cargo build --release`.
- In the report, mark live checks as "not verified, user to test".

## Read first
1. `AGENTS.md`.
2. **Contract:** `C:/Users/chris/orca/projects/daijobu-island/task/progress.md`, section **"Stage 5: Vitals + Sweep"**. Read it from the main checkout and don't edit it. Build exactly that.
3. `C:/Users/chris/orca/projects/daijobu-island/docs/plan.html`: the Sweep mockup (400×200 checklist + "Clean" button), the safety rules and the health score.
4. `task/stage-4-report.md`: how Vitals is wired (worker, `WM_VITALS`, coalescable timer, panel painter).

## Design guidance
- `crates/dj-sweep` is split in three: **pure** (rules, guard, age filter, purge context, size aggregation, oplog formatting, all unit-tested), **fs** (walker with time budget, reparse skip, final-path re-verify), and **ops** (permanent delete, `IFileOperation` recycle, DNS flush, empty bin, behind a trait).
- Scans run on a worker thread and post progress/results to the UI with `PostMessageW(WM_APP + 8…)`. **Check existing WM_APP ids first: +1…+7 are taken.**
- Reclaimable cache goes in `~/.daijobu/sweep-cache.toml` (per-category bytes + timestamp).
- Island: Sweep is a sub-view of Vitals (not a new tile). Vitals → button → Sweep → back. Keep the OSD overlay working on top. The Sweep view may be taller than 150 (up to about 400×236). Keep the swap chain size rule from AGENTS.md: grow the max size constant only if needed.
- Category lists are **our own**. Mole (GPL) is a concept reference only. Don't copy its lists or code.
- WinIsland is irrelevant here.

## Finish
Commit in small steps on your branch and don't push. Write `task/stage-5-report.md`: an honest per-item checklist, **how the user should test safely** (for example, start with the preview only, and check the Recycle Bin), known issues, and what was not verified. Then stop.

Toolchain: `%USERPROFILE%\.cargo\bin`, `C:\Program Files\LLVM\bin`, `%LOCALAPPDATA%\Microsoft\WinGet\Links`. Prepend them to `PATH` if needed.
