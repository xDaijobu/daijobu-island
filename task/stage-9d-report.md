# Stage 9d — top edge reveal report

Implemented and verified on integrated base `4d2c3de`, including the preceding
Terminal Folders and Windows notifications tracks. Final validation ran exclusively
in the assigned shared target cache on 2026-10-30. The running user app was untouched.

## Scope checklist

- [x] Wrote `stage-9d-brief.md` before implementation and read AGENTS/design context
  (also opened `docs/plan.html` in Orca's embedded browser).
- [x] Extended existing `island.stay_behind`, without another setting. Its default
  remains `false`; the Settings description explains top-edge reveal and fullscreen.
- [x] A capture-excluded, layered, nonactivating tool HWND covers exactly one physical
  pixel vertically, only across the island's horizontal bounds at monitor top.
- [x] Revealed island remains in front while the pointer is inside its native hit
  region, or the existing panel/OSD/activity policy requires front. Leaving starts
  a 450 ms one-shot grace, including a bridge across the floating pill's top margin.
- [x] No global mouse/click hook or idle mouse polling. Foreground/location WinEvents
  invalidate fullscreen state; cursor sampling occurs only at one-shot expiry.
- [x] Native shell D3D-exclusive/presentation state and foreground client/visible DWM
  geometry suppress passive reveal. Captioned maximized resize borders are excluded.
- [x] The existing topmost foreground hook skips passive raises into fullscreen.
- [x] Explicit hide cancels reveal; screenshot preparation disables the sensor before
  freezing. Keyboard/focus/hotkeys and AppBar reservation policy remain existing code.
- [x] Sensor follows existing primary-monitor/DPI relayout and current island width,
  including terminal shortcuts and notch shoulders. No new monitor-selection behavior.
- [x] Relayout clears the previous physical pointer lease and lowers a resting
  stay-behind island before placement, avoiding a passive TOPMOST reassertion during
  a fullscreen display switch. Front-needed panels retain their existing policy.
- [x] Unsafe is confined to `dj-win`, with safety comments. No dependencies added.
- [x] Added the coordinator-requested README usage paragraph explaining retained
  desktop visibility, top-edge reveal and fullscreen suppression. Also reconciled
  the coordinator-requested notifications/lyrics status rows and identified merged
  Windows notifications as current-master/unreleased, with native manual gaps.
- [x] Worker made no notification/Explorer implementation, AGENTS/progress edits,
  Git mutations or subagents. Coordinator-owned progress/integration-plan updates
  and the merged base were preserved.

## Final exclusive validation

Target: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4`.

- PASS: `cargo fmt --all --check`.
- PASS: `cargo clippy --locked --workspace --all-targets -- -D warnings`.
- PASS: `cargo test --locked --workspace -- --test-threads=1`: **1,682 passed**, 19
  suites, 52.76 seconds executing tests. Serial execution avoids the known concurrent
  ConPTY exit flake.
- PASS: all six pure controller tests: bounded bridge/leave grace, reentry,
  panel/OSD hold and release, panel closing under the pointer, cancellation for
  hide/capture/fullscreen/setting-off, and no spontaneous resting reveal.
- PASS: all three native tests: negative-origin one-pixel edge geometry; hidden HWND
  nonactivation/tool-window styles, capture affinity and destruction; actual hidden
  window geometry distinguishing full-monitor, work-area, captioned and F11-style
  custom client bounds. Native fixtures never show or activate their HWNDs.
- PASS: `cargo build --locked --profile terminal-folders`: optimized, 1m 30s.
  The existing `terminal-folders` profile inherits `release`.
- PASS: final staged/working-tree `git diff --check`; executable-copy SHA-256 match.

Earlier overlapping shared-cache test attempts are **discarded as branch evidence**.
They exposed a Settings search hint contract (retain "notch", fixed) and LNK1104 on
another run's renderer test executable. Cargo releases its build lock before all
test suites finish, allowing cross-worktree binary replacement. The coordinator
then updated the base and granted the uninterrupted final sequence above. An early
native fixture assumption about oversized captioned windows was replaced with a
hidden `WM_NCCALCSIZE` fixture that models F11 custom client handling directly;
the corrected fixture passed in the final run.

## Executable

Stable stage copy (not launched):
`C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/stage-9d-edge-reveal/daijobu.exe`

Original optimized output:
`C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/terminal-folders/daijobu.exe`

Size: **27,628,544 bytes**. SHA-256 of both files at handoff:
`0E59D7830D669C18CF963B4D7C7FB252394EF672B94DBA9F40CE00CAA61934FF`.
The stable copy avoids later shared-profile builds replacing the review artifact.

## Integration points

App timer ID `14` is reserved for edge grace; native message `WM_APP + 0x91` delivers
`Event::EdgeReveal { reached }` outside callbacks. `dj-core::edge_reveal::Reveal`
holds the pure lease; `dj-win::edge_reveal::Sensor` owns the HWND and WinEvent hooks.
`system::foreground_changed` consults the passive-fullscreen guard before raising
the island. App wiring covers startup/configuration, input, one-shot expiry,
capture preparation, deliberate hide, display/DPI relayout and shutdown. There is
no config-schema or feature-registry change. Coordinator owns shared-file merges.

## Run and manual verification

Enable **Settings > Notch > Stay behind windows**, or set `[island] stay_behind = true`
in the config and restart manually. The island stays visible on the desktop layer;
covered by an ordinary window, moving to the physical top edge over its width raises
it. Move into the island and use it, then leave: it returns behind after 450 ms once
the existing panel/OSD/activity finishes. Ctrl+Alt+H still deliberately hides/shows it.

The worker did not launch, terminate, restart or automate the user's running app.
Manual pointer feel, real exclusive games, F11/borderless apps, presentation mode,
fullscreen transitions, capture contents, terminal/clipboard focus/hotkeys, monitor
disconnect/reconnect, negative monitor origins and 100/125/150/200% mixed-DPI moves
remain unverified on the interactive desktop. Native tests use hidden test windows
only; they do not establish those interactive claims.

Fullscreen suppression is deliberately conservative: a foreground fullscreen app on
another monitor also suppresses passive edge reveal. Panels and OSD keep their
existing front policy. Failure to create a capture-excluded sensor or native hooks
logs once and leaves the existing stay-behind/panel behavior usable until restart.

## Native references

Fullscreen geometry uses client bounds and visible DWM bounds rather than raw
resize-border-inclusive window rectangles, per [GetWindowRect documentation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowrect).
Exclusive fullscreen and presentation suppression uses the shell's
[notification-state enumeration](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ne-shellapi-query_user_notification_state).
