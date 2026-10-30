# Optional Windows notification identity

The normal unpackaged executable works without notification access. Notifications default off. Windows ingestion additionally needs a registered package identity, the `userNotificationListener` capability, and explicit Windows consent. Neither building nor toggling the feature installs anything or grants access.

This is an external-location identity package (sparse MSIX), for Windows build 19041 or newer. It leaves `daijobu.exe` and its dependencies in their existing directory. Embedded `<msix>` metadata associates the rebuilt executable with this package; all three values (package name, publisher and application ID) must match `AppxManifest.xml`. Changing the publisher for a production certificate requires rebuilding the executable with matching metadata. The provided PNG assets are original project artwork, Apache-2.0.

## Build and explicit per-user registration

1. Build the executable with the embedded identity manifest: `cargo build --release`. Use a stable installation directory containing that executable. Do not replace or restart an existing running instance during a worker run.
2. Build the unsigned identity file, without installation:

   ```powershell
   ./packaging/notifications/Build-IdentityPackage.ps1 -OutputDirectory C:/temp/daijobu-identity
   ```

3. Sign the MSIX using Windows SDK SignTool and a certificate whose Subject exactly matches `CN=Daijobu Island` (or change both manifests and rebuild). Use a trusted production certificate for distribution. For local development, explicitly create a test signing certificate with that subject, export its public `.cer`, and explicitly trust that public certificate in `Cert:/CurrentUser/TrustedPeople` before installation. Keep its private `.pfx` private; do not commit it. Building does not create a certificate or modify trust.
4. Only when the user chooses installation, register for the current user. Substitute the real signed package and stable executable directory:

   ```powershell
   Add-AppxPackage -Path C:/temp/daijobu-identity/DaijobuIsland.Notifications.msix -ExternalLocation C:/Apps/Daijobu
   Get-AppxPackage -Name DaijobuIsland.Notifications | Select-Object Name,PackageFullName,Status
   ```

   Registration and certificate trust are manual user operations; no app code or build script invokes them. Do not use machine-wide provisioning. Updating the same package version may require explicit removal of the old version first; prefer incrementing the four-part version for updates.
5. After explicitly closing the prior app, launch the rebuilt executable from the registered external directory (or the registered Start entry). Open Home → Notifications. Verify the actual runtime status; registration alone is not proof that the launched process acquired identity. Enable ingestion, then select **Allow access**. This invokes `RequestAccessAsync` asynchronously from the UI thread; closing or denying consent keeps the appropriate status visible. A saved enabled setting never automatically reopens a permission prompt.
6. If denied, allow notification access for Daijobu Island in Windows Settings, then reopen the Notifications panel. The app rechecks access on panel entry and native actions. A fresh event also checks before querying. There is no periodic access polling; quiet external revocation may only be noticed at the next interaction/event. Disable clears Windows history/queue and stops ingestion; the independent internal toggle controls only internal producers.

To unregister explicitly when finished, first close the app yourself and run:

```powershell
Get-AppxPackage -Name DaijobuIsland.Notifications | Remove-AppxPackage
```

## Behavior and manual acceptance

- Open Notifications while disabled/unpackaged: it must say off, then package identity required after opt-in. No permission or installation should occur.
- After registered identity and Allow: generate a benign toast from an existing trusted app. Its Windows toast remains visible; the island shows a six-second 360 × 72 toast if no panel/higher-priority alert is open. Click its body to open the validated source app or exact internal target, X to dismiss it from both places. Successful activation hides the toast; unavailable activation opens history with feedback. History is always accessible from Home. Only the most recent 64 current Windows entries are retained. Startup hydrates history without replaying old notifications.
- A burst queues at most eight fresh toasts. Open panels keep queued items waiting. OSD, Snap and timer-completion alerts have priority. A toast expires without removing its history entry.
- In history, click an entry marked **Open app** to open the source app's default view. The adapter re-fetches the notification and compares OS AppInfo identity, then calls `IApplicationActivationManager::ActivateApplication` with no arguments. Desktop apps without compatible app activation show a failure notice; there is no shell, protocol or command-line fallback. Notification links, actions and launch arguments are never read or executed.
- Remove a toast from Windows Notification Center: the matching island entry goes on the next listener event. Disable/re-enable and verify purge. Revoke permission externally and verify status/purge at the next panel entry/action/event. Quit/relaunch: no island history survives.
- Check idle CPU with no notification events and no active animations. The worker blocks on a channel and the UI uses the existing message loop; only displayed toast expiry arms a one-shot timer. The OS async API may temporarily own a full notification collection, while all project-owned snapshot/history/queue structures stay bounded.

## Integration surface

`dj_core::notifications` exports `NotificationKey::{Windows(u32), Internal(u64)}`, `Activation::{None, OpenSourceApp, Internal(ActivationToken(u64))}`, `NotificationInput { key, source, title, body, activation }`, `NotificationCenter::admit(input, now) -> Admission`, `remove(key)`, `activate(key) -> Option<Activation>`, `configure_windows(bool)` and `configure_internal(bool)`. Both origins start disabled, and disabling either clears only that origin. This model is platform-free and has no persistence.

`app::notifications::Notifications` supplies the integration seam: `register_activation(key, token, InternalTarget::TerminalTab(tab))` then `admit(input, now)`. Admission strips unregistered activation tokens; activation resolves both key and token again. The app validates that the terminal tab still exists before opening it. These methods never spawn an agent or execute text. The upcoming provider track plugs in through this seam, with session/tab ownership checked by its trusted adapter.

## Evidence and remaining verification

Stage 9a's report records the exact host, package build and tests performed. An unsigned package build and an unpackaged missing-identity probe cannot establish actual package registration, consent UX, foreground event delivery or source-app activation. Those require the interactive manual checks above after the user explicitly chooses registration. The foreground listener API is documented by Microsoft; no background task or polling fallback is installed.

Microsoft's tracker also has an [open unpackaged event-registration report](https://github.com/microsoft/WindowsAppSDK/issues/6172), despite other listener calls apparently succeeding. This implementation requires actual runtime identity before probing permission and reports **Connecting** until both event subscription and the first snapshot succeed. A subscription failure reports unavailable; it never silently falls back to polling. Sparse identity support is the documented deployment route, while successful event delivery under that identity still needs on-device verification.

Official references (reviewed 2026-10-30):

- [Notification listener, access checks, text bindings and foreground events](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/notification-listener)
- [External-location identity package and embedded identity metadata](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)
- [uap3 userNotificationListener capability](https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-uap3-capability-manual)
- [IApplicationActivationManager::ActivateApplication](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-iapplicationactivationmanager-activateapplication)
