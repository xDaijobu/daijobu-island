# Stage 9a — Windows notifications

This isolated track belongs to the requested integration stage; the coordinator integrates feature worktrees sequentially. Scope is opt-in Windows toast ingestion, a native Skia toast and history panel, Settings control, safe dismissal/source-app opening, and documented optional package identity. Other roadmap features, agent hooks, shared progress, Git publication and global installation are outside this track.

## Scope and acceptance

- [ ] Read the design and existing island/config/settings architecture; preserve native Rust/Skia/Win32 and sleeping idle behavior.
- [ ] Add a reusable platform-free typed notification admission/activation model with bounded memory-only history/queue and deterministic tests; send the interface to the coordinator early.
- [ ] Default Windows ingestion off; enable/disable from Settings, with explicit access request only after user action and accurate missing identity/denied/unavailable status.
- [ ] Use UserNotificationListener events, not periodic polling; handle removals, duplicates, access checks and worker shutdown without content logs or disk history.
- [ ] Unlock Notifications and route Open to a native panel; display a timed 360 × 72 toast without interrupting open panels, with dismiss and safe source-app action.
- [ ] Never execute notification-provided command lines, arguments, links or URIs; source-app activation uses only OS app identity, and future internal activation is an opaque typed token.
- [ ] Supply a package manifest, embedded identity metadata and explicit sparse packaging instructions/scripts; research official Microsoft APIs and record current-host read-only viability evidence. Do not install/register/trust certificates or grant consent automatically.
- [ ] Run meaningful model/controller/native smoke tests, format check, workspace clippy with warnings denied, workspace tests and optimized build using the assigned target cache; report exact outcomes and remaining native permission gaps.
- [ ] Write an honest `task/stage-9a-report.md`; leave all work uncommitted for coordinator review.

## Privacy and verification limits

Notifications live only in bounded RAM and are cleared on Windows ingestion disable. Windows' own toast remains visible. No telemetry, content logging or persistence is introduced. A successful unpackaged smoke can prove graceful missing-identity handling, but cannot prove listener access, consent, event delivery or activation under package identity; those need an explicitly installed/trusted package and interactive user verification.
