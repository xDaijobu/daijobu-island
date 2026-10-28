# Stage 7d report: Snap pin to screen

Date: 2026-10-28. Branch: `xDaijobu/stage-7d-snap-pin`, base `fc8d025` (PR #29).
Status: implemented, independently reviewed, automated/release checks and native
smoke passed. Remaining desktop scenarios are explicitly listed below.

## Delivered

- [x] Separate **Keep in stack / Release from stack** and **Pin to screen** actions.
  Preview actions use an overflow menu; latest-shot and annotation-editor actions
  also open screen pins. Editor pin flattens annotations and commits active text,
  closes only after successful admission, and never copies or saves automatically.
- [x] Independent topmost Win32/Skia reference windows with Copy, Save/Open and Close,
  body dragging, no taskbar/Alt-Tab entry, no activation on creation, theme chrome,
  keyboard actions and tray recovery. Screen pins end with the application session.
- [x] Stable PinIds and original shared image/PNG payloads independent of preview
  expiry, dismissal, keep-alive and saved-file lifetime. Duplicate sources reveal the
  existing pin. Earlier source errors survive successful preview pinning.
- [x] Eight-window and 256 MiB unique retained-payload limits with explicit failure
  feedback. Queued IO and undrained replies retain allocation reservations after
  their window closes; shared allocations count once. Cached Save replies retain
  their PNG Arc until drain so reservation keys cannot outlive those allocations.
- [x] Collision-safe saves, exact-ID async results, queue refusal/failure recovery,
  shared saved-path propagation, renderer recovery and ordered resource teardown.
- [x] Work-area/DPI fit and clamping, fixed maximum GPU allocation per pin/DPI,
  per-window capture/events, press-only sampling with generation/dedup guards, and
  preservation of rapid button clicks and newer presses through queued older events.
- [x] Pins hide before the frozen desktop is acquired and return after capture,
  cancellation or failure. Global hide/show preserves their lifetime. Capture keeps
  existing island/Terminal panel behavior; keyboard restoration respects visibility,
  valid targets, denied activation and user foreground changes during preparation.
- [x] Tray foreground snapshots account for Explorer's notification-area activation.
  Pin errors clear on new editor interaction and are placed away from the selection.
- [x] README/progress documentation. No new dependencies, OCR, recording, automatic
  screenshot persistence or unrelated feature unlock. Clipboard remains the next stage.

## Verification

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace --locked`: **1,445 passed**, zero failed/ignored/filtered.
  App 249; capture 8; core 728; render 162; sweep 35; terminal 71; vitals 26; Win32 166.
  Documentation tests also passed. The full suite ran without exclusions.
- `cargo build --release --locked --target-dir .../target/stage-t4`: passed (26.81 s).
- Three independent final source reviews approved memory/IO ownership, native input
  and resources, and entry-point/regression behavior. A follow-up review approved
  the cached-PNG lifetime repair and gated panic regressions. All workers released.
- Regression coverage includes admission and IO holds across close/reopen, failure
  and panic for Copy/Save, cached PNG Weak-Arc lifetime and stale-source guards,
  annotation pixel preservation, duplicate reveal, hidden/capture lifecycle, queued
  pointer/cancel ordering, denied focus, tray cycling and theme/DPI raster geometry.
- Graceful restart from the standard `target/release/daijobu.exe` path succeeded.
  Installed artifact SHA256:
  `C7F429B999C1B4411F3FF22262F010B9A5DFA3F93853446EFC051FC3C338E263`.
  New process PID 29716 answered native responsiveness probes.
- Native smoke used a disposable, labelled synthetic colour fixture and posted
  messages to Daijobu windows, without global SendInput. Editor pin created two
  independent 344x268 reference windows from 320x180 selections, with the expected
  native title and topmost/tool-window/no-activate styles. The rendered crop and
  Copy/Save/Close chrome were visually inspected.
- The existing pin was hidden during creation of the second capture, both returned
  afterward, global hide/show preserved both, and a subsequent capture/cancel hid
  and restored both. A fast posted Close-button click removed only its pin. The
  remaining pin stayed visible and the application responsive.
- Clipboard sequence was unchanged across restart/overlay smoke and pin-only smoke.
  No copy/save action or clipboard content inspection was used in those checks.
  Temporary fixture windows and created pins were closed afterward.

The native harness initially hit PowerShell's `lp` alias rather than its coordinate
packer; renaming the helper fixed the harness and the subsequent smoke passed.
That harness failure did not indicate an application defect.

## How to try

Trigger Snap, select and annotate, then choose **Pin to screen**. Alternatively use
the latest screenshot or a preview's **More** menu. Drag the reference image, use
its actions, or choose **Show pinned screenshots** in the tray for keyboard access.
Tab/Shift+Tab chooses an action; Enter activates it; Ctrl+C copies, Ctrl+S saves,
and Escape closes that pin. `Ctrl+Alt+H` hides/restores pins with the island.

## Unperformed desktop checks and limits

- [ ] Actual mouse dragging/release outside the window, capture loss, swapped buttons
  and an elevated foreground application. Posted message/fake tests do not prove
  those physical-input cases, especially when async button state is inaccessible.
- [ ] Actual notification-area menu cycling and external-window focus restoration;
  deterministic adapters cover routing and rejected activation, not Windows timing.
- [ ] Real cross-monitor drag, negative monitor origins and 150/200% DPI transitions.
  This host has one 3440x1440 monitor at 100% with the AppBar work-area top at y=42.
- [ ] Desktop Copy/Save/Open output comparison after source-preview expiry, preview
  and latest-shot entry activation, and visible capacity rejection with eight pins.
  Synthetic controller/store/editor tests cover these data/lifetime paths.
- [ ] Capture preparation failure while the user changes foreground, GPU/device loss,
  and Terminal-in-frozen-image acceptance on the current native build. Source review
  and regression tests preserve the existing Terminal behavior.

The 256 MiB limit covers unique retained bitmap/PNG allocations, including accepted
IO holds. GPU resources and temporary encoder/native format buffers are additional
memory. A theoretical panic outside the Store's guarded work path could strand busy
state; the production wake callback only posts a native message and no practical
unhandled failure trigger was found. Pins intentionally do not persist across restart.
