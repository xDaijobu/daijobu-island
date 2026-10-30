# Stage 9 integration: notifications, Explorer folders and edge reveal

User authorized implementation on 2026-10-30, separate Orca worktrees per feature,
Codex `gpt-6.1-sol` with `high` effort, and supervision from the main conversation.
This is one integration stage with isolated feature tracks. The coordinator owns
shared planning, Git, review, integration, PRs and worktree cleanup. Workers do not
commit or merge. No release/version bump is requested.

## Prerequisite

Finish reviewing the existing uncommitted `stage-terminal-folders`, run workspace
checks, resolve findings, merge through a PR, then remove that worktree only after
the remote merge and preservation of all stage changes are confirmed. Its existing
native interaction limitations must remain explicit.

Completed: PR #36 merged at `a1992e5` after exclusive coordinator fmt, clippy,
1,661 tests and optimized build passed. The old worktree was removed and its path
confirmed absent. The current running application was not restarted.

## Tracks and ownership

| Track | Worktree | Scope | Dependency |
|---|---|---|---|
| 9a | stage-9a-windows-notifications | Windows notification permission/identity, bounded notification model, native panel/toast, Settings and app activation | None |
| 9b | stage-9b-agent-notifications | Claude/Codex completion and needs-response events, safe session identity and activation | 9a notification interface |
| 9c | stage-9c-explorer-folders | Prefill a new terminal chooser from last active Explorer; bounded Explorer folder menu | Terminal Folders merge |
| 9d | stage-9d-edge-reveal | Reveal a behind-windows notch at its top edge; hide behind again after leaving; fullscreen suppression | Existing stay-behind setting |

Workers own implementation in their separate checkouts and their own brief/report.
Shared wiring files may differ between branches; no worker edits another checkout.
Coordinator integrates these shared files sequentially and reruns affected checks.
Workers do not edit shared `task/progress.md` or `AGENTS.md`. Each must write its
stage brief before implementation and report actual checks and limitations.

## Integration contracts

- 9a owns notification admission/presentation. Publish a bounded typed interface
  before 9b starts; 9b uses that surface rather than a second popup/history system.
  Native toasts and agent events remain distinct sources with typed activation.
  Frozen proposal: `NotificationKey::{Windows(u32), Internal(u64)}`,
  `Activation::{None, OpenSourceApp, Internal(ActivationToken(u64))}`,
  `NotificationInput { key, source: String, title: String, body: String, activation }`.
  `NotificationCenter::admit(input, now: Instant)` yields Inserted/Updated/Disabled;
  remove/activate/configure_windows complete the surface. Internal enable state is
  independent of Windows permission. Source-app identities stay in the adapter;
  internal tokens resolve through an app-owned registry. Changes require coordination.
- Agent status comes from supported documented events, not guesses from terminal
  output. Preserve existing provider config/hooks and correlate exact sessions.
  Never execute a command or arbitrary URI carried in notification text.
- Explorer only supplies a starting folder or an explicit selection for a new
  session. Existing shells never receive an injected `cd`. Spotlight explicit
  paths take precedence. Keep chooser confirmation and path-as-data semantics.
- Edge reveal means the Vale-style behind-window behavior, not disappearance on
  an otherwise empty desktop. Respect explicit hide, capture, fullscreen, focus,
  panel interaction, monitor/DPI changes and AppBar policy.
- No permanent high-frequency polling, notification-content logging, automatic
  permission consent, package registration or real agent startup in tests.

## Acceptance

- All requested feature paths have implementation and meaningful tests, including
  denial/unavailable/stale-session/cancellation and hide/reveal edge cases.
- fmt, workspace clippy/all-targets with warnings denied, workspace tests and
  optimized build pass for each integrated result. Known environmental flakes
  and omitted native checks are recorded rather than presented as passing.
- The shared `target/stage-t4` requires exclusive ownership of the entire Cargo
  validation sequence, including test execution, not just compilation. Cargo's
  build lock does not prevent another worktree replacing a later test executable.
  A 2026-10-30 overlapping run demonstrated mismatched test counts and executed an
  edge-reveal test from the Terminal Folders run; that run is invalid evidence.
  Coordinator grants one slot at a time; workers explicitly report it free before
  another starts. No concurrently produced result counts as final verification.
- Native smoke is non-disruptive; do not terminate the user's running build.
- Each PR describes final behavior and remaining native acceptance limits.
- Merge/cleanup happen only after verification. No worker worktree is removed
  while uncommitted/unmerged work or an unsettled worker remains.

## Runtime tracking

Orca run: `run_28e8c71571e7`. Launch receipts must confirm effective model and effort.
The coordinator processes questions and completion reports, routes fixes, and
settles/releases each dispatched worker before ending supervision.

Initial dispatches: Terminal Folders `ctx_12e5960fe43f`; Windows notifications
`ctx_897f20e5cb61`; agent adapters `ctx_df4fd284834a`; edge reveal
`ctx_505793d1222d`. The edge worker launch initially left its prompt in the composer;
an Enter-only send submitted the existing prompt, and live working state was
verified without creating a duplicate. Explorer starts after the prerequisite merge.

Explorer dispatched after the prerequisite merge as `ctx_17cab01871d8` in
`stage-9c-explorer-folders`, with effective Sol 6.1 high confirmed. The agent
notification task was extended before settlement to include complete Settings,
terminal launch and shared-popup integration after the 9a base update; an
adapter-only result is no longer its completion condition. Windows notifications
holds the first exclusive validation slot, then edge reveal, then the remaining
ready track after integrating already-merged work.

9a completed exclusive validation on `a1992e5`: fmt, locked workspace all-targets
Clippy, 1,673 serial workspace tests and optimized build passed. Coordinator
review covered toast activation/fallback, bounded typed targets, consent status,
source-app activation and deployment limitations. Registered-identity consent
and live notification delivery remain manual acceptance, not passing evidence.

9a merged as PR #37 (`4d2c3de`); its worker was released and worktree removed.
9d and 9b were updated to that base with all local changes preserved. 9d owns
the next exclusive validation slot; 9b resumes complete feature integration
without Cargo while 9c waits for the integrated 9d base.
