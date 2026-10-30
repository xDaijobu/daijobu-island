# Stage 9a — Windows notifications

Implemented and validated on the coordinator-updated Terminal Folders base, commit `a1992e5`. Workspace checks and the optimized build pass; actual registered-identity permission/event flows remain explicitly unverified. All changes remain uncommitted for coordinator review.

## Scope checklist

- [x] Brief written before implementation; AGENTS.md and the interactive design read, existing island/settings/config architecture inspected. No worker edits to shared progress or AGENTS, Git publication, extra worker launch or unrelated feature changes. The coordinator updated AGENTS/progress and added the integration plan while updating this checkout; those shared planning changes are coordinator-owned.
- [x] Notifications unlocked in the feature registry and `ClickOutcome::Open` routed to the real native Skia panel. Added spring-driven notification panel and 360 × 72 six-second toast; open panels, OSD, Snap and timer completion take priority. History paginates three rows at a time, with source/title/body previews, dismissal and source-app action.
- [x] Windows ingestion defaults off, configurable from Settings → General and the panel. Enabling only probes identity/access; **Allow access** explicitly requests consent asynchronously from the UI thread. A saved opt-in never automatically prompts. Missing identity, unspecified consent, requesting, connecting, denied, unavailable and successful listening are distinct states.
- [x] Native UserNotificationListener event subscription wakes a sleeping MTA snapshot worker through a one-slot coalescing channel. No periodic notification/access polling, background task registration, WNF workaround or silent fallback. Listening is only published after subscription and the first successful snapshot. Quiet external access revocation is detected on the next listener event, panel entry or native action; immediate detection without an event is not claimed.
- [x] Project-owned history/snapshot structures cap at 64 entries, queue at eight, and source/title/body text at 96/256/1024 characters. Snapshots retain only newest items; initial hydration does not replay existing Windows toasts. A creation-time watermark prevents older notifications reappearing in the bounded native window from being replayed as fresh toasts. Native query results can temporarily own the full OS collection.
- [x] Notification contents stay in RAM. No content logs, telemetry or disk history. Disabling an origin purges that origin from history/queue; disabling Windows stops its worker and discards stale completion mailboxes. In-flight OS queries may finish asynchronously after stop; no late results are admitted. Exit clears the controller.
- [x] Dismiss removes that one Windows toast (never clears all notifications); failures remain visible. Toast-body and history-entry clicks use the same validated activation path. Open app re-fetches the OS notification, compares OS AppInfo identity and calls `IApplicationActivationManager::ActivateApplication` with empty arguments and a validated AUMID. Successful native activation hides/collapses the toast without reclaiming keyboard focus; missing/unsupported activation opens history with a notice. Home always provides access to history. No links, notification arguments, shell commands or arbitrary protocols are executed.
- [x] Typed internal admission/activation contract sent early and accepted by the coordinator. Independent `[notifications] internal_enabled` switch defaults off. The controller binds opaque tokens to both notification key and typed target, strips unregistered tokens and revalidates them on activation. Only existing terminal-tab activation is exposed; no provider hooks are implemented.
- [x] Optional sparse package manifest, embedded identity association, original project logo assets, build-only script and explicit signing/per-user registration/removal documentation supplied under `packaging/notifications/`. Nothing installs or modifies certificate trust automatically.
- [x] Final workspace format check, locked all-targets Clippy with warnings denied, full single-threaded workspace tests and optimized build passed in the exclusive validation slot.
- [ ] Real registered-identity consent/event delivery/dismiss/source-app activation and live idle CPU: intentionally not verified in this worker run; see native gaps below.

## Files and integration seam

- `crates/dj-core/src/notifications.rs`: pure bounded model, geometry, access/action types and deterministic tests.
- `crates/dj-win/src/notifications.rs`: identity probe, explicit UI-thread consent, native event/snapshot worker, safe native actions and read-only smoke.
- `app/src/notifications.rs`: controller, native reconciliation, activation registry and integration tests.
- `crates/dj-render/src/notifications.rs`: native Skia panel/toast painting, Unicode shaping/fallback, contrast-safe row surfaces and synthetic raster smoke.
- Existing core/config/settings/island, app/main/frame, Win32 manifest and crate manifests wired as needed. `windows-future` is an already-locked MIT/Apache-2.0 dependency used directly for consent completion; no new third-party assets were introduced.

Frozen core interface:

```rust
NotificationKey::{Windows(u32), Internal(u64)}
Activation::{None, OpenSourceApp, Internal(ActivationToken(u64))}
NotificationInput { key, source: String, title: String, body: String, activation }
NotificationCenter::admit(input, now: Instant) -> Admission::{Inserted, Updated, Disabled}
NotificationCenter::remove(key)
NotificationCenter::activate(key) -> Option<Activation>
NotificationCenter::configure_windows(bool)
NotificationCenter::configure_internal(bool)
```

The next provider track integrates through `Notifications::register_activation(key, token, InternalTarget::TerminalTab(tab))` followed by `Notifications::admit(input, now)`. It must verify producer/session ownership before registration. App activation checks that the target tab still exists and never starts a new shell or agent from notification text. Registration is bounded to 64 key/token/target bindings. The controller returns `ActivationResult::{OpenedSourceApp, Internal(InternalTarget), Unavailable}` so routing distinguishes success from failure; this controller-only addition was sent to the coordinator before changing it. Frozen core signatures are unchanged.

## Validation evidence

All Cargo commands use `CARGO_TARGET_DIR=C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4`. Shared-cache validation is serialized with the coordinator; no existing Daijobu process was stopped or restarted. Read-only process inspection found an existing executable under the `memory-opt` profile, so a normal release build does not target its running file.

| Check | Result |
| --- | --- |
| Initial `cargo check -p dj-win -p dj-core` | Passed, 13 crates, 34.02 s; this preceded later UI/controller refinements |
| `cargo fmt --all -- --check` | Passed on final source |
| `git diff --check` | Passed; Git emitted only CRLF normalization notices |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Passed on final source, no issues |
| `cargo test --locked --workspace --quiet -- --test-threads=1` | Passed: **1,673 tests, 19 suites**, 53.73 s reported test time |
| `cargo build --locked --profile terminal-folders` | Passed: **optimized**, inherits release settings, 15 crates, **1 min 35 s** |
| Read-only native missing-identity smoke | Passed in full workspace suite: opt-in reports MissingIdentity, creates no worker or consent operation, disable restores Disabled |
| Synthetic Skia raster panel smoke | Passed in full suite; generated PNG visually inspected, no window or real notification content used |
| Final executable embedded manifest | SDK `mt.exe` extraction passed; `DaijobuIsland.Notifications` / `CN=Daijobu Island` / `Daijobu` exactly match package metadata |
| Build-only sparse MSIX script | Passed with MakeAppx SDK 10.0.26100.8249; four payload files |
| SDK manifest/schema validation without `/nv` | Expected missing external executable error on real sparse payload; passed on separate schema-only payload with a two-byte `MZ` placeholder satisfying the path. The schema-only output is explicitly not installable and was never installed |
| Package registration | `Get-AppxPackage -Name DaijobuIsland*` returned no package; no registration/trust/consent was attempted |

Host read-only evidence: Windows build **26300.9711**, DisplayVersion **26H2** (the ProductName registry value retains the legacy “Windows 10 Pro” label). Windows SDK MakeAppx 10.0.26100.8249 and Appx module 2.0.1.0 are present. The unsigned sparse output is `target/stage-t4/notification-identity/DaijobuIsland.Notifications.msix` outside this worktree.

Initial validation found old test assumptions about 14 island layers and a mistaken renderer surface field name; these were corrected before the final passing runs. No outstanding compile, lint or test failures remain. The Cargo slot was explicitly returned to the coordinator immediately after the optimized build; no subsequent Cargo commands were run.

Preserved review artifacts, outside the worktree:

- Executable: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/notification-identity/review/daijobu.exe`, **27,613,696 bytes**.
- Executable SHA-256: `22195ac199b90eb484fe0abf0db503fca5153a32470d034e86b255b6c96138ad`.
- Extracted embedded manifest: `notification-identity/review/embedded.manifest` under the same target cache.
- Synthetic native panel image: `notification-identity/native-panel-smoke.png` under the same target cache. This contains only fixed synthetic test text.

## How to run and native gaps

Build normally and open Home → Notifications. With no opt-in it shows off; enable while unpackaged to see the package-identity requirement without consent or installation. Settings → General has separate Windows and internal notification toggles. The feature works as a panel even when native access is unavailable.

For actual Windows ingestion, follow `packaging/notifications/README.md`: build a matching executable/identity package, explicitly sign/trust/register for the current user, explicitly close the old instance, launch from the registered external directory, then enable and select Allow access. The worker did **not** do these operations. Therefore actual process identity under external-location registration, consent UX in this native Win32 host, event subscription/delivery, removal, external revocation behavior, source-app activation and real GPU/window appearance/idle CPU remain interactive coordinator/user acceptance work. A successful package file build is not proof of any of those behaviors.

The official foreground event API is used; Microsoft's tracker contains an [unpackaged event-registration failure report](https://github.com/microsoft/WindowsAppSDK/issues/6172). This implementation fails visibly on subscription errors and introduces no polling workaround. Some desktop apps do not support IApplicationActivationManager; their notifications intentionally have no shell/URI fallback.

Notification UI displays bounded source/title/body previews rather than replaying original toast markup, inline actions or images. Provider hooks, Explorer folders, edge reveal and other roadmap items remain outside this track.

## Official API research

Reviewed 2026-10-30:

- [Microsoft notification listener guide](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/notification-listener): user consent, GetAccessStatus checks, ToastGeneric text, per-item removal and foreground NotificationChanged.
- [Microsoft manual sparse identity guide](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps): external-location support from build 19041, MakeAppx `/nv`, signing/trust, embedded `<msix>` association and explicit per-user registration.
- [Microsoft uap3 capability schema](https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-uap3-capability-manual): `userNotificationListener` declaration.
- [Microsoft desktop WinRT support caveats](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/winrt-api-desktop-app-support): package identity and desktop consent/UI limitations; actual native consent must be verified rather than inferred from package construction.
- [Microsoft activation-manager method](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-iapplicationactivationmanager-activateapplication): typed AUMID activation with no notification arguments.
