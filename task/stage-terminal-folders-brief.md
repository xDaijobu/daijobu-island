# Stage Terminal Folders brief

Base: `9335f13`; implementation only in `C:/Users/UseR/orca/workspaces/daijobu-island/stage-terminal-folders`.

## Scope

Show a pre-launch chooser for initial empty Terminal open and new-session actions. Reuse existing Shell, Claude and Codex profiles and flags. Provide editable absolute folder, last folder per profile, bounded recent folders, Browse, Open, Cancel and Clear history. Persist only paths under local config. Clear removes last-profile paths and MRU. Existing tabs, restore and restart behavior remain intact; tab switching never launches or moves a session.

Validate existing directory before launch and pass it to SpawnOptions.cwd, never interpolate it into commands. Spaces, Unicode and UNC are path data; OS/ConPTY limits still apply. Browse is explicit-only using a dedicated STA thread and wake-driven completion. Reuse themed Skia rendering and line editor, do not alter GPU caching or idle scheduling.

## Acceptance and checks

- No new chooser-driven process before Open; validation and spawn errors remain visible.
- Keyboard and mouse can choose profile/history, edit, browse, launch, cancel and clear.
- History bounded and deduplicated; successful launch updates only chosen profile; no content/command history access.
- New chooser controller tests use fake spawners or filesystem-only temporary fixtures. The 2026-10-30 verification request supersedes the historical ban on real-shell/window tests: run the full standard workspace suite, including hidden ConPTY shells and hidden popup geometry; never launch real Claude/Codex agents or disturb the running app/desktop.
- Run cargo fmt, workspace clippy all-targets with warnings denied, workspace tests, optimized terminal-folders profile inheriting release with shared target/stage-t4 cache.
- Report exact files, test counts, executable path and unverified native interactions.

Worker keeps all changes uncommitted and performs no desktop input, running-app lifecycle operations or version/release work. Other worktrees remain untouched. The coordinator owns the now-authorized Git integration, merge and worktree removal. Verify actual local ConPTY cwd through hidden native tests; native manual appearance, picker, network-path latency and smoothness checks remain deferred.
