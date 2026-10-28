# Stage 7d brief: Snap pin to screen

Status: implemented, reviewed and verified; see [report](stage-7d-report.md) for
automated/native evidence and remaining desktop checks. Based on Stage 7c-2 (PR #29). Read the
[dispatch contract](stage-7d-dispatch.md) and [coordination plan](stage-snap-clipboard-plan.md).
**OCR was removed by the user and must not be implemented.**

## User flow and labels

- Existing preview pin becomes **Keep in stack / Release from stack**, with its existing
  keep-alive meaning. A separate **Pin to screen** action opens a floating reference.
- Add Pin to screen to the annotation editor finish actions and the latest-shot/stack
  actions. The coordinator adjusts shared layout/hit testing; use an overflow menu if
  the card cannot fit clear labels. Do not cram six unreadable labels into the old row.
- Editor Pin to screen flattens the current annotated selection and opens a pin when
  successful. It closes the editor without writing the clipboard or saving a file.
  If flatten/admission/window creation fails, leave the editor and markup intact.
- Pinning an existing result leaves its stack card and clipboard unchanged. Opening the
  same result twice focuses/reveals its existing pin rather than retaining duplicate
  payloads; copies with different stable screenshot identities remain independent.

## Window behaviour

- Each reference is an independent native Win32/Skia/D3D12 window with theme chrome,
  always on top, no taskbar/Alt-Tab entry and no automatic foreground activation.
- Drag its image/body to move; Copy, Save/Open and Close buttons do not move it.
  There is no click-through toggle in this stage.
- Initial image fit is at most 640 by 480 logical pixels and within the monitor work
  area, preserving aspect ratio and avoiding unnecessary upscaling. Small images are
  centred in chrome large enough for accessible buttons. Original pixels stay unchanged.
- Clamp after monitor removal, resolution/work-area changes or DPI transitions, including
  negative desktop coordinates. Users must always be able to reach Close.
- Use a fixed maximum swap-chain allocation for each pin at its current DPI. Position
  dragging never reallocates it per frame; a DPI/device reset may rebuild resources.
- At most 8 screen pins and 256 MiB of unique retained image payloads in the pin manager,
  counting shared Bitmap/PNG allocations once within that manager. This is an additional
  bounded owner alongside the existing stack. Refuse admission with visible feedback;
  never close an old pin silently.
- PinId and window lifetime are independent of stack ItemId. Stack expiry/dismissal does
  not close a reference; closing a reference does not dismiss its source card or delete
  a saved PNG.
- Copy and Save reuse existing PNG/DIB and collision-safe save services. Busy/failure
  status belongs to the exact pin ID, and late IO replies cannot affect a new pin.
- Screen pins last until closed or the app exits. No automatic persistence across app
  restarts and no silent PNG writes just from pinning.
- Tray gets **Show pinned screenshots** for recovery/keyboard access. Deliberate keyboard
  focus enables Tab/Shift+Tab, Enter, Ctrl+C, Ctrl+S and Escape (close this pin). Restore
  the prior window on closing a deliberately focused pin when it is still valid.
- Ctrl+Alt+H hides/restores pin windows along with the island, preserving their individual
  lifetime. Device loss recreates drawing resources without discarding image content.

## Capture integration

Hide reference windows together with preview cards before acquiring the next frozen
desktop and wait for composition as the current preview implementation does. Keep the
existing open-terminal/panel preservation flow unchanged. Track exactly which pins were
visible, and restore them after capture completes, cancels or fails; a pin explicitly
closed during that period must not reappear. Do not rely only on display-affinity hints
for exclusion. Window-selection candidates must exclude the reference HWNDs.

## Ownership

Pure state/geometry: dj-core/snap_pin. Native per-HWND routing: dj-win/snap_pins. Drawing:
dj-render/snap_pins. Controller/resource ownership: app/snap_pins. Never use a single
thread-local controller slot for all pin windows. The coordinator owns stack/editor action
enums, entry points, app capture integration, tray and shared renderer exports.

## Automated acceptance

- Synthetic small/tall/wide images, fit/clamp and hit targets at 100/150/200% DPI, work
  areas with a top strip, negative origins and a removed monitor.
- Stable IDs, duplicate reveal, count/byte limits, shared-payload accounting, expiry of
  source cards, independent close, late replies and shutdown cleanup.
- Capture begin/end/cancel/error plus close/hide/show during capture; restore only live
  references that should be visible. Regression for the open Terminal staying open.
- Per-window event isolation, pointer-capture loss and destruction sequencing through
  fakes; no real window creation in tests.
- Synthetic raster checks in all four themes: aspect ratio, image fidelity, legible
  chrome, hover/focus/error states and clipping.

## Desktop acceptance for the coordinator

1. Pin an annotated synthetic capture from the editor; verify the system clipboard was
   not changed by this action. Pin a finished stack result as well.
2. Move references on/between monitors; expire/dismiss the source card, then Copy/Save
   from the pin and compare original output pixels.
3. Test two pins independently, duplicate reveal, Close, tray recovery, hotkey hide/show,
   DPI change and capacity feedback.
4. Open Terminal, trigger Snap: Terminal stays available in the frozen image, reference
   windows are absent, and cancel/failure restores references without stealing focus.
5. Close/rerun the app: screen pins end with that session, while saved files and existing
   pinned Terminal tab behaviour remain intact.

Finish with stage-7d-report.md, standard workspace checks, README/progress updates and a
separate PR. Report every desktop case not actually exercised.
