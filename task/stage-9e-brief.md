# Stage 9e: In-app Windows notification setup

## Scope
User requests notification setup inside the exe so enabling Windows notifications does not require manual PowerShell/package installation. Preserve native Rust/Skia UI and explicit Windows notification consent. Implement a complete asynchronous setup/retry/status/restart flow, with signed embedded sparse identity payload for distribution and an honest development build path. No private signing keys in source or embedded binaries. Never silently add broad certificate trust or bypass OS consent. Establish whether unsigned per-user identity support on this Windows version can avoid test certificate trust, using official documentation and real probes. If test signing is required for this local build, report concrete requirements to coordinator before changing trust.

## Ownership
One worker owns implementation, packaging, relevant docs/tests and final report in this worktree. Coordinator owns scope, review, integration/Git, local installation and app restart. Worker must not commit/push/merge, install packages, modify certificate trust, stop/restart the running app, or edit real provider configs.

## Acceptance
- Missing identity offers an actionable setup UI; no packaging guide dead end.
- Setup runs off UI thread, coalesces duplicate clicks, reports errors and allows retry.
- Successful registration requires restart/acquisition of runtime identity; never reports ready from registration alone. Preserve settings and terminal state via normal graceful exit.
- Existing identity and denial paths behave correctly; RequestAccessAsync remains explicit and UI-thread based.
- Distributed exe contains required payload; no SDK/runtime build tools required on end-user PC. Signing is build-time only. Default developer builds work with a clear unavailable payload state if unavoidable; coordinator delivery build must include payload.
- No silent trust-store expansion, shell injection, unbounded output/process waits, or setup from headless agent-hook commands.
- Meaningful regression tests for state/error/retry paths; fmt, workspace clippy/tests, optimized build. Record exact commands and remaining live verification in task/stage-9e-report.md.
- Inspect docs/plan.html and existing stage reports. Update packaging docs and AGENTS.md if workflow changes.
## Coordinator deployment evidence (2026-10-30)
Windows 10.0.26300.0 has Developer Mode already enabled, current process is not elevated, and no CurrentUser/My code-signing certificates exist. An isolated `DaijobuIsland.SetupProbe` loose manifest was successfully registered per-user using `Add-AppxPackage -Register <manifest> -ExternalLocation <isolated external dir>`; status 0, IsDevelopmentMode true. Probe registration was removed immediately afterward. No trust or OS developer settings changed.
Accepted local delivery path: embed loose manifest/assets and use native RegisterPackageByUriAsync when existing Developer Mode permits it. Do not turn on Developer Mode. Production builds may embed a signed MSIX instead. Make this distinction explicit without sending users to a packaging guide as the only action.
