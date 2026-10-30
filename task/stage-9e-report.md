# Stage 9e — in-app notification setup

Implemented in the stage-9e-notification-setup checkout. The worker did not commit,
push, install/register packages or certificates, change trust/Developer Mode, change
real provider configurations, or stop/restart the running app. The coordinator owns
live installation/restart/consent verification and Git integration.

## Scope and behavior

- [x] Read the frozen brief, AGENTS/RTK, existing Stage 9 reports and design source;
  opened `docs/plan.html` in Orca's browser and inspected the rendered document.
- [x] Missing runtime identity offers **Set up notifications** inside the native
  Skia panel. Saved opt-in alone, startup and headless hook commands never deploy.
- [x] Setup uses a dedicated MTA worker, a one-result mailbox and the existing wake
  message. Duplicate clicks coalesce; panel re-entry retains status/error/retry.
- [x] Native `RegisterPackageByUriAsync` handles the embedded development manifest
  only with existing Windows Developer Mode. Production uses native
  `AddPackageByUriAsync` with an optionally embedded signed MSIX. Signature/trust
  failure never falls back to development deployment, certificate import or elevation.
- [x] Development manifest and original logo assets are embedded in default builds.
  Files extract to a stable per-installation user-data directory; logos also extract
  at the external app location. Different existing logos are not overwritten and
  comparisons use metadata length plus a bounded read. Unwritable app folders have
  actionable errors; installers can predeploy exact logos.
- [x] Builds without signed payload on non-developer machines explain the limitation
  in-app. Retrying can observe a user-selected Developer Mode change. The app never
  enables that setting or changes trust. No runtime SDK, shell or signing tool exists.
- [x] Successful registration means **Restart to finish setup**, never listening.
  Restart prepares a same-exe helper off-thread. Readiness proves it holds the old
  process; a separate commit permits relaunch only after actual process exit and
  single-instance mutex release. Handshake/process waits are bounded, with no kill.
- [x] Restart calls the existing normal `App::quit` path, retaining saved settings
  and pinned terminal tabs while closing shells. It reopens Notifications and checks
  actual identity. Missing identity after restart displays retry guidance. Live shell
  process state and unpinned sessions are not restored, as explained before restart.
- [x] `RequestAccessAsync` remains a separate explicit UI-thread action. Existing
  identity, denial and listener subscription behavior remain intact. Disabling while
  setup runs cannot be undone by a late result and cannot silently trigger restart.
- [x] Setup timeouts request cancellation and disallow same-process deployment retry
  because the OS operation may still finish. Restart-worker failures retain successful
  registration so restart can retry without registering again. Lost worker channels
  produce useful failures rather than leaving the UI busy forever.
- [x] Setup view suppresses covered internal-history painting and hit targets while
  preserving setup/restart/Off/Close. This resolves the coordinator's review finding.
- [x] Updated build/signing documentation, main README and AGENTS build workflow.
  Coordinator-owned brief/progress changes were preserved and are not worker edits.

## Packaging research and coordinator evidence

Official Microsoft sparse guidance requires a signed MSIX with a trusted signer whose
Subject matches the manifest Publisher; the exe's embedded package/application metadata
must also match. SDK MakeAppx/SignTool belong only on the developer/build machine.
`Build-IdentityPackage.ps1` optionally signs with an existing `CurrentUser/My` certificate
thumbprint and verifies with SignTool. It never creates a key, imports a certificate,
changes trust or installs anything. `DAIJOBU_NOTIFICATION_MSIX` embeds that prepared
archive; `DAIJOBU_REQUIRE_SIGNED_NOTIFICATION_PAYLOAD=1` makes a missing payload fatal.
Archive signature-entry checks at embedding are sanity checks, not cryptographic
verification: the packaging script and OS deployment perform verification.

Microsoft's unsigned MSIX route requires a special publisher OID and does not share
the signed identity. Executable-content installation generally requires administrator
privileges; content-only hosted packages do not establish a suitable portable listener
identity. It is explicitly a testing route rather than wide distribution. The worker
reported these concrete requirements to the coordinator before any live action.

The coordinator independently reported two successful, cleaned-up isolated probes on
Windows `10.0.26300.0`, with Developer Mode already enabled and no code-signing cert:

- Loose manifest registration with external location returned `Status=0` and
  `IsDevelopmentMode=true`, without elevation or certificate changes.
- A separate C# fixture with matching embedded `<msix>` metadata launched directly
  from the external folder and returned `GetCurrentPackageFullName RESULT=0`, package
  `DaijobuIsland.SetupProbe_0.3.1.0_neutral__x1xqmdx6tfxf6`.

These are coordinator-provided probe results, not worker-run installation or proof of
the final app's consent/event delivery. Probe evidence was reported under
`C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/identity-setup-probe/`.

References reviewed 2026-10-30:

- [Sparse identity](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)
- [Sparse build/sign/external assets](https://learn.microsoft.com/en-us/windows/apps/dev-tools/winapp-cli/guides/sparse)
- [Native registration](https://learn.microsoft.com/en-us/uwp/api/windows.management.deployment.packagemanager.registerpackagebyuriasync)
- [Development registration option](https://learn.microsoft.com/en-us/uwp/api/windows.management.deployment.registerpackageoptions.developermode)
- [Unsigned package limitations](https://learn.microsoft.com/en-us/windows/msix/package/unsigned-package)

## Verification

All Cargo commands use
`CARGO_TARGET_DIR=C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4`.
The coordinator granted exclusive cache ownership for the entire sequence, including
test execution. The final source passed the complete sequence below on 2026-10-30.

- PASS: `cargo check --locked --workspace --all-targets`.
- PASS: `cargo fmt --all --check`.
- PASS: `cargo clippy --locked --workspace --all-targets -- -D warnings`.
- PASS: `./packaging/notifications/Build-IdentityPackage.ps1 -OutputDirectory C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/stage-9e-identity-build`.
  MakeAppx `10.0.26100.8249` produced an unsigned inspection MSIX only; it was not
  registered, signed, trusted or embedded in the delivered local exe.
- PASS: `cargo test --locked --workspace -- --test-threads=1`: **1,734 passed**,
  20 suites, 55.00 seconds executing tests; none ignored/excluded.
- PASS: directly executed the current compiled `app/build.rs` script under an isolated
  `OUT_DIR`: default development embedding emits `SIGNED_MSIX=None`; distribution
  required-payload mode rejects missing input; the actual unsigned inspection MSIX is
  rejected for lacking a package signature. These probes did not invoke Cargo or
  modify the delivery build's generated files.
- PASS: `cargo build --locked --profile terminal-folders`: optimized, 2m 11s.
- PASS: final `git diff --check`, source/copy SHA-256 comparison, SDK MT extraction
  of embedded package/publisher/application metadata and PerMonitorV2, and inspection
  confirming the optimized exe contains the embedded developer capability manifest.

An intermediate check ran while a method signature was being edited and saw old
dependency metadata; the completed source was rechecked successfully before final
clippy/tests. No failing tests were excluded or relaxed.

The 15 new regression tests cover explicit setup/consent separation, duplicate-click
coalescing, opt-out during setup, retry/timeout/stale completion, restart retry, hidden
history hit suppression, saved enabled startup without deployment/permission request,
off-thread extraction failure, worker cancellation/disconnection, commit/exit ordering,
launch errors, malformed helper commands, URI escaping, bounded signing errors,
complete/stable development extraction in apostrophe/space/Unicode paths, signed route
selection without fallback, unavailable builds and foreign-logo preservation. Fixtures
do not register packages, start the restart helper, or manipulate real provider configs.

## Preserved review executable

- Stable coordinator-requested executable (not launched by this worker):
  `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/stage-9e-notification-setup/daijobu.exe`.
- Original optimized output:
  `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/terminal-folders/daijobu.exe`.
- Size: **28,146,688 bytes**.
- SHA-256 of both files:
  `703606C120E2C1824F3DF3B5DCA187D90E38E9F9187207543BDD166FB3D721E0`.
- Extracted application metadata:
  `target/stage-t4/stage-9e-notification-setup/stage-9e-embedded-manifest.xml`
  under the project's target root. Existing coordinator probe scripts/JSON in that
  directory were not overwritten.

This local delivery exe embeds the complete development payload and no private key.
Launch that stable path with `--notifications`, then select setup/restart in the UI.
The host's pre-existing Developer Mode is sufficient for this local route. For general
distribution, follow the signed payload build procedure; that path has not been
end-to-end exercised with a production certificate in this task.

## Remaining interactive verification

Coordinator must deliver/launch the final exe, invoke actual in-app setup, verify normal
restart/order/settings/pins and acquired runtime identity, then let the user explicitly
choose Windows notification consent. Real listener event delivery, permission deny/
revoke, source-app activation and UI timing remain interactive checks. No production
signing certificate was available to the worker, so a genuinely signed distribution
package's end-to-end deployment is not claimed. Packaging/trust errors intentionally
remain visible rather than triggering a broader installation path.

## Coordinator live verification (2026-10-30)
- Started the preserved optimized executable (SHA-256 703606C120E2C1824F3DF3B5DCA187D90E38E9F9187207543BDD166FB3D721E0) with Notifications visible. PID 23048 initially had no package identity, and setup/retry UI was visible.
- During interactive use, the package became registered and the app restarted to PID 25836 from the same preserved executable path with --notifications. Coordinator did not manually register the real package through PowerShell or change certificate trust; in-app setup handled registration. The user's interactions occurred before the coordinator's attempted click (that stale-PID click was rejected).
- GetPackageFullName on PID 25836 returned 0 and DaijobuIsland.Notifications_0.3.1.0_neutral__x1xqmdx6tfxf6. Get-AppxPackage reports Status=0, IsDevelopmentMode=true. Existing Developer Mode was used without modification.
- The real Notifications panel shows Listening and populated Windows notification history. This proves acquired runtime identity, granted consent, successful event subscription and initial snapshot. Coordinator did not click the OS Allow dialog; the interactive user completed consent.
- Realtime arrival of a newly generated toast, source-app activation, deny/revoke and terminal pin restoration are not individually verified here. Existing native listener tests remain passing. No genuine production certificate/signing deployment is claimed.
