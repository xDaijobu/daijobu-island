# Stage 9b: agent notifications

## Desired outcome

Opt-in Claude and Codex terminal sessions notify Daijobu Island when a response
finishes or the provider explicitly requests approval/input. Internal alerts use
the shared notification popup/history independently of Windows notification
permission. Clicking opens only the exact live provider session and terminal launch
generation; closed, restarted, exited or changed sessions lose their action.

The initial dispatch covered adapters. After Stage 9a merged, the coordinator
expanded this same dispatch to complete Settings, setup/removal, terminal launch
and lifecycle wiring, delivery and click-time revalidation. Final base: master
`ab4b700`, including Windows notifications, edge reveal and Explorer folders.

## Scope and acceptance

- [x] Pure bounded provider metadata schema and parsers; no transcript access.
- [x] Own-user native local IPC, typed bounded frames/queue, random launch capability,
  finite I/O and clean shutdown; remote clients rejected.
- [x] Exact provider/session/generation correlation to `TabId`, opaque activation
  tickets and click-time revalidation; fresh identities across controller replacement.
- [x] Session change by clear/fork revokes the old binding; a new tab is required.
- [x] Hook bridge runs before single-instance/UI/storage initialization and always
  emits neutral JSON; no event-driven command/URI execution, approvals or answers.
- [x] Explicit Settings opt-in with provider setup/capability status; existing
  third-party config and provider permission/trust settings are preserved.
- [x] Exact ownership receipts, atomic config edits, config-change detection,
  DACL preservation, safe Windows quoting and fixture-only development writes.
- [x] Supported version checks and Codex isolation; launch credentials are scoped
  and restored, provider return/error revokes the binding and preserves exit code.
- [x] Pinned/new agent launches wait for setup, and close/restart/exit/disable
  invalidate actions. Shared `AgentSession(Ticket)` routing resolves the current
  generation again before opening; stale actions become unavailable.
- [x] Security, correlation, config and native fake-process fixtures pass.
- [x] Format, locked workspace all-targets clippy with warnings denied, all 1719
  serial workspace tests, optimized build and release-mode headless bridge smoke pass.
- [x] Combined optimized artifact preserved and SHA-256 verified; embedded GUI,
  PerMonitorV2 and sparse-MSIX identity checked without launching the main UI.

## Verified capabilities

Windows installation inspected without starting a real agent: Codex CLI **0.163.0**
(stable hooks enabled, offline generated schema) and Claude Code **2.1.295**.
Supported hooks include SessionStart/Stop/PermissionRequest/SessionEnd; explicit
progress clears old actions. Claude also exposes AskUserQuestion/ExitPlanMode tool
hooks. Generic Codex question lifecycle coverage is not established: this feature
reports those alerts unavailable; documented app-server input requests are an
alternative outside this feature. Waiting is never inferred from silence.

Codex legacy notify is completion-only and remains untouched. Opted-in verified
Codex launches add `--no-daemon` and keep their existing permission flags. Hook
trust stays with the provider; Codex users review `/hooks` and open a fresh tab.
Unsupported versions show unavailable status. Third-party hooks/provider policy
can affect delivery or continue a Stop; these observations are advisory.

Primary references checked 2026-10-30:

- [Codex hooks](https://learn.chatgpt.com/docs/hooks)
- [Codex notification config](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications)
- [Codex CLI flags](https://learn.chatgpt.com/docs/cli/reference)
- [Codex app-server input requests](https://learn.chatgpt.com/docs/app-server)
- [Claude hooks](https://code.claude.com/docs/en/hooks)

## Boundaries

No real provider session or user provider configuration was changed during
validation. No existing application was replaced or restarted. The main island
UI, live provider hook trust and manual desktop behavior remain for coordinator/
user acceptance. The optimized binary was invoked only with the headless hook
switch and cleared launch environment. No popup model, Explorer/edge feature,
AGENTS.md or shared progress changes were authored here; coordinator owns Git.
