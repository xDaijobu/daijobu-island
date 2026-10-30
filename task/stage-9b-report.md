# Stage 9b report: agent notifications

Status: full feature implemented and headlessly validated on combined master
`ab4b700`; coordinator owns commit/PR, cleanup and live user acceptance.

## Completed

- [x] Claude/Codex bounded metadata parsers and exact provider/session/launch-generation
  binding to island terminal tabs, with bounded deduplication and opaque tickets.
- [x] Click-time live-generation revalidation through typed `AgentSession(Ticket)`
  targets in the shared center. Close, restart, provider exit, progress, disable,
  controller replacement and changed SessionStart identity invalidate old actions.
- [x] Current-user-only protected native pipe DACL, remote rejection, independent
  random endpoint and 256-bit launch capability, finite overlapped I/O and clean
  cancel-and-drain shutdown. The fixed transport acknowledgement carries no decisions.
- [x] 4 KiB wire frames, 64 KiB hook stdin, 128-byte IDs, 64 queued frames and at most
  the terminal tab limit of registrations. Credentials are redacted from Debug.
- [x] Headless CLI entry before single-instance/storage/UI initialization. Neutral
  `{}` output on delivery success/failure; no approvals, input answers, transcript
  reads, event-derived shell commands, paths or URI activation.
- [x] Settings opt-in/status and serialized setup/removal, exact ownership journals,
  preserved unknown/third-party config, atomic writes with change detection and
  existing DACL preservation. New agent/pinned launches wait for setup.
- [x] Launch wrapper rechecks provider version, scopes/restores environment, preserves
  existing permission arguments and exit code, and revokes in finally on return/error.
- [x] Native version probes start suspended, enter a kill-on-close job before resume,
  use bounded output/wait and terminate descendants before joining the reader.
- [x] Small build reproducibility correction: compile-time tracked manifest directory
  and absolute dependency prevent shared-target linker metadata referencing a removed
  sibling worktree. Embedded DPI/MSIX manifest values are preserved.

## Run and enable

Build with `cargo build --locked --profile terminal-folders`. In the application,
enable **Settings > General > Agent notifications**; setup installs only owned hooks.
Check **Settings > Terminal > Agent notification status**, then open a fresh agent
tab. For Codex, review new hooks through `/hooks` and open a fresh tab afterward.
Windows notification permission is independent. Turning agent notifications off
clears agent alerts, invalidates the bridge immediately and removes only recorded
owned handlers on the serialized worker. A queued disable finishes when quitting.

Setup honours `CLAUDE_CONFIG_DIR` and `CODEX_HOME`; otherwise it uses the normal user
Claude settings.json and Codex hooks.json. Ownership journals are
`~/.daijobu/agent-hooks-{claude,codex}.json`; they contain owned commands/paths, never
launch capabilities or copies of original settings. Edited owned handlers are kept.
Outside an island-owned launch, installed hooks have no capability and return neutral
JSON. No real provider configs were edited during development; fixtures use temp dirs.

## Provider behavior and limitations

Read-only installation checks found **Codex CLI 0.163.0** with stable hooks enabled
and **Claude Code 2.1.295**. Offline Codex schema generation confirmed modern hook
and app-server request types. Only these inspected Windows versions are admitted;
missing/unverified versions show unavailable status. Each actual launch checks again.

Both adapters observe SessionStart, Stop, PermissionRequest and SessionEnd, and clear
pending actions on explicit progress. Claude observes explicit question/plan tools
AskUserQuestion and ExitPlanMode. Codex legacy notify is completion-only and untouched;
modern hooks supply approval requests. Generic Codex question alerts are explicitly
unavailable: no dedicated question lifecycle hook is documented and general tool-hook
coverage is not universal. The documented alternative is app-server
`item/tool/requestUserInput`; replacing interactive CLI is outside this feature.

Opted-in verified Codex launches add only `--no-daemon` for per-tab environment
isolation. Existing yolo/permission flags and hook trust are preserved. Clear/fork or
another changed session ID revokes the original grant and needs a fresh agent tab.
Third-party hooks may answer requests or continue a Stop, so these are explicit
provider observations, not a guarantee of no subsequent work. Managed policy,
disabled hooks and Claude's optional subprocess environment scrub can prevent delivery;
the feature never weakens them. Oversized/invalid events, full queues and unavailable
IPC may drop an alert, always with neutral provider output. No waiting is inferred
from inactivity, transcript scraping, BEL or notification text.

Primary sources checked on 2026-10-30:

- [Codex hooks](https://learn.chatgpt.com/docs/hooks)
- [Codex legacy notify](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications)
- [Codex CLI flags](https://learn.chatgpt.com/docs/cli/reference)
- [Codex app-server](https://learn.chatgpt.com/docs/app-server)
- [Claude hooks](https://code.claude.com/docs/en/hooks)
- [Claude environment overrides/scrub](https://code.claude.com/docs/en/env-vars)
- [Windows pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
  and [I/O cancellation](https://learn.microsoft.com/en-us/windows/win32/fileio/cancelioex-func)

## Validation

All Cargo commands used
`CARGO_TARGET_DIR=C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4` in the
coordinator-assigned exclusive slot.

- `cargo check --workspace --all-targets`: pass; lockfile updated for declared direct deps.
- `cargo fmt --all -- --check`: pass on final source.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: pass, no issues.
- `cargo test --locked --workspace -- --test-threads=1`: **1719 passed**, 20 suites,
  77.24 seconds; none excluded or ignored in the final run.
- `cargo build --locked --profile terminal-folders`: pass, optimized, 2m 19s.
- Preserved executable SHA-256 equals the just-built source executable.
- Extracted embedded manifest contains PerMonitorV2 and DaijobuIsland.Notifications
  sparse-MSIX identity; PE subsystem is 2 (Windows GUI).
- Optimized GUI-subsystem binary headless hook smoke, separately for Claude and Codex,
  with redirected stdin/stdout/stderr and all launch variables removed: both exit 0,
  stdout `{}`, empty stderr. No main UI/provider/config initialization occurred.

Fixtures cover hostile/oversized input, wrong credentials/providers/sessions, stale
and reused tabs, clear/fork, controller replacement, setup/remove/upgrade preservation,
concurrent config edits, supported-version guards, actual local pipes, exact kernel
DACL, queue bounds, stalled clients, fake descendant/timeout cleanup, and real headless
bridge processes. Fake provider functions run under apostrophe/space/Unicode paths
and prove literal quoting, preserved permission flags/exit code, restored environment
and finally revocation on success/error. No real agent or user settings are used.

Earlier attempts are recorded separately: the first test link failed on cached
Explorer-worktree manifest metadata before tests ran, fixed in app/build.rs. The next
run had 1718 passes and one ACL assertion: Windows normalized generic GA to specific
file-all-access FA. The corrected assertion still requires the exact protected sole-user
ACL; production access restrictions were not relaxed. Toolchain deprecation/test-slice
lints were corrected before the final clean clippy run.

## Preserved review artifact

- Executable: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/stage-9b-agent-notifications/daijobu.exe`
- SHA-256: `4473FB4C06AA622F220FFD70F5C888BEAE9D246E509A40E499F60894DA99F486`
- Size: 28,060,160 bytes.
- Same folder: `embedded-manifest.xml` and `release-hook-smoke.json` evidence.

## Not verified live

The main island UI, real provider hook trust/consent, real interactive completion/input
popups and desktop focus behavior remain for coordinator/user acceptance. No existing
app was replaced/restarted, no real provider session/config was changed, and no
AGENTS.md/shared progress/Git lifecycle changes were authored by this worker.
