# Windows notification setup

Notifications default off. Open **Home > Notifications**, enable Windows notifications,
then select **Set up notifications** if identity is missing. Setup extracts an embedded
payload and registers it for the current Windows account on a background MTA thread.
No PowerShell, SDK, signing tools, certificate import or elevation runs on the user's PC.
Setup never runs from saved settings, startup, agent hooks or other headless commands.

After registration, select **Restart to finish setup**. A helper first opens the old
process, then waits for its actual exit, including single-instance mutex release,
before launching the same exe and reopening Notifications. Shutdown follows the normal
app path: existing saved settings and pinned terminal tabs survive; shell sessions close.
Unsaved shell state and unpinned sessions are not restored. Registration success alone
never reports readiness. The new process checks real runtime identity, then **Allow
access** invokes `RequestAccessAsync` explicitly on the UI thread. Denied permission
stays denied until changed in Windows Settings; setup never changes it.

Repeated setup clicks coalesce. Leaving/reopening the panel retains status and errors.
**Off** remains available during setup; a late result cannot enable notifications.
Retryable failures retain a bounded Windows HRESULT/message. A deployment timeout
requests cancellation and requires closing/reopening before retry, because Windows may
still complete it. No force shutdown, kill, policy bypass or trust-store fallback occurs.
The restart helper uses readiness/commit events and bounded waits. If it cannot start,
retry restart or close/reopen manually.

## Payloads and build requirements

Both routes require Windows build 19041 or newer, a stable local app folder, and an exe
named `daijobu.exe`. Embedded `<msix>` metadata in `app/daijobu.manifest` must match the
package name, publisher and application ID in `AppxManifest.xml`. Assets are original
Apache-2.0 project artwork. The app extracts logos under the app folder's `Assets/`;
existing different files are not overwritten. Installers using a read-only app folder
must deploy these exact assets there in advance.

- **Distribution:** embed a signed sparse MSIX using `DAIJOBU_NOTIFICATION_MSIX` at
  compile time. Windows validates signature/trust during native `AddPackageByUriAsync`.
  Signature/trust errors never fall back to development registration, even on a developer PC.
- **Development:** builds without that variable embed the loose manifest and three
  logo assets. Only when Windows Developer Mode is already enabled do they use native
  `RegisterPackageByUriAsync` with `DeveloperMode=true` and `ExternalLocationUri`.
  Otherwise setup explains that a signed build or the developer's explicit Windows
  Settings change is required and offers retry. The app never enables Developer Mode.

Extracted payloads live permanently under
`~/.daijobu/notification-identity/0.3.1/<external-folder-hash>/`, because development
registration continues referring to its loose manifest. Keep that folder and the app
installation stable until explicitly unregistering. Setup may leave these small files
after failure. Moving the app requires setup against its new external location;
registration resources/version conflicts are reported for operator resolution.

### Signed distribution build

Only the build machine needs Windows SDK MakeAppx and SignTool. Supply an existing
code-signing certificate with an accessible private key in `CurrentUser/My`, whose
Subject equals the manifest Publisher (`CN=Daijobu Island` here). A production certificate
must chain to trust on recipient machines. If its Subject differs, update both manifests
and rebuild. Test certificate trust is a separate explicit operator action. The app and
script never create/import certificates. Never put private PFX/keys in source or delivery.

```powershell
./packaging/notifications/Build-IdentityPackage.ps1 `
  -OutputDirectory C:/build/daijobu-identity `
  -CertificateThumbprint $env:DAIJOBU_SIGNING_THUMBPRINT `
  -TimestampUrl https://timestamp.digicert.com
$env:DAIJOBU_NOTIFICATION_MSIX = 'C:/build/daijobu-identity/DaijobuIsland.Notifications.msix'
$env:DAIJOBU_REQUIRE_SIGNED_NOTIFICATION_PAYLOAD = '1'
cargo build --locked --release
```

The script packs, signs and verifies with SignTool, and never installs. Without
`-CertificateThumbprint` it produces an unsigned inspection artifact, which must not be
embedded/distributed. The build script rejects missing files, archives over 16 MiB,
non-ZIP payloads and archives lacking `AppxSignature.p7x`. These are sanity checks,
not cryptographic verification: verify with SignTool before embedding. Windows validates
it again during deployment. The `REQUIRE` variable fails the build if the signed payload
variable is absent. No build-machine paths are needed at runtime. Sign the final exe
separately after embedding if required by your distribution process.

For local delivery on an already configured developer machine, a normal build contains
the complete development payload without a certificate or any runtime SDK:

```powershell
Remove-Item Env:DAIJOBU_NOTIFICATION_MSIX -ErrorAction SilentlyContinue
Remove-Item Env:DAIJOBU_REQUIRE_SIGNED_NOTIFICATION_PAYLOAD -ErrorAction SilentlyContinue
cargo build --locked --profile terminal-folders
```

### Why not AllowUnsigned?

Microsoft's unsigned MSIX route requires a special publisher OID, changing package
identity; executable-content installation generally requires administrator privileges.
The no-admin content-only/hosted-app case does not establish a portable Win32 listener
identity. This app never uses `AllowUnsigned` to bypass signed deployment. The coordinator's
isolated probes on build 26300 with pre-existing Developer Mode proved loose-manifest
registration without certificates and runtime identity after a fresh process launch.
Those probe packages were removed and did not grant real Daijobu notification permission.

## Manual acceptance

1. Open Notifications in an unpackaged app with notifications off. Enable exposes
   **Set up notifications**, with no package or permission prompt before selecting setup.
   Saved enabled settings also offer setup without automatically deploying.
2. Select setup twice, leave/reopen the panel, and confirm one deployment and useful
   status. Verify useful errors/retry. Disable while pending: late success must not
   re-enable the setting or restart the app.
3. Complete setup and select restart. Verify old process exit before replacement,
   saved settings/pins, Notifications reopening, and actual **Allow access** status.
   Missing runtime identity must display guidance and a setup retry action.
4. Only the user selects **Allow access**. Verify deny, allow and externally revoked
   paths. **Connecting** remains until subscription and initial snapshot both succeed.
   Unsupported event delivery reports unavailable.
5. Generate a benign trusted-app toast. Windows keeps its own toast; the island shows
   a six-second toast when no higher-priority panel/OSD/alert is open. History is bounded
   to 64, queue to eight, and clears on disable/exit. Existing history does not replay.
   Source activation uses OS AppInfo and argument-free `IApplicationActivationManager`,
   never toast text/links/protocols/commands. Incompatible activation shows feedback.
   Internal notification routing remains independent.
6. Confirm idle CPU near zero without events/animations. Setup's temporary bounded
   async status wait ends at completion; there is no ongoing setup timer. The listener
   blocks on its channel and rechecks access on interaction/events.

Explicit operator cleanup after closing the app:

```powershell
Get-AppxPackage -Name DaijobuIsland.Notifications | Remove-AppxPackage
```

No background notification task or polling fallback is installed. Microsoft's tracker
has an [unpackaged event-registration report](https://github.com/microsoft/WindowsAppSDK/issues/6172);
actual consent/event delivery requires interactive acceptance with acquired identity.

Official references reviewed 2026-10-30:

- [Sparse identity and matching metadata](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)
- [Sparse signing and external assets](https://learn.microsoft.com/en-us/windows/apps/dev-tools/winapp-cli/guides/sparse)
- [Native RegisterPackageByUriAsync](https://learn.microsoft.com/en-us/uwp/api/windows.management.deployment.packagemanager.registerpackagebyuriasync)
- [Unsigned MSIX restrictions](https://learn.microsoft.com/en-us/windows/msix/package/unsigned-package)
- [Listener and explicit consent](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/notification-listener)
