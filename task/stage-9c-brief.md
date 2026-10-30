# Stage 9c: Explorer starting folders

Base: master after Terminal Folders PR #36 (`a1992e5`). This isolated track adds
Explorer folder suggestions to the existing explicit new-session chooser.

- Add a Settings preference for Explorer first versus per-profile last folder.
- Explicit Spotlight paths always win; Explorer falls back to last/config/home.
- Track the last foreground Explorer HWND through the existing foreground event
  hook, without activation. Resolve filesystem paths on a dedicated STA thread.
- Enumerate at most 128 automation registrations and show at most six unique
  folders. Refresh and selection only change chooser data, never a live shell.
- Read the active shell view, verify its window is visible and belongs to the
  Explorer top-level HWND, and reject ambiguous registrations, virtual namespaces
  and invalid paths. Hidden Windows 11 tabs are intentionally unavailable.
- Discard results when the chooser changes, closes, is edited or validates; keep
  pinned restore/restart and provider permissions unchanged.
- Preserve recent folders, native Browse, validation and explicit Open.

Acceptance: meaningful precedence/fallback/latest-selection/stale-result/path/
menu-bound tests; direct rustfmt during source work; only after coordinator grants
the exclusive shared Cargo slot run fmt, workspace clippy with warnings denied,
workspace tests serially and release build. No provider launches or app restart.
Read-only native enumeration smoke may run without navigation or input. Report
actual checks and remaining Windows 11 tab/native interaction limitations.

Ownership excludes progress.md, AGENTS.md, integration plan and Git operations.
Freeze source edits and notify the coordinator before base integration/final tests.
