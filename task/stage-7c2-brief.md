# Stage 7c-2 brief: Snap stack drag-out

Status: implemented and reviewed; automated/release/runtime checks passed. See the
[report](stage-7c2-report.md) for remaining desktop acceptance. Read the [coordination plan](stage-snap-clipboard-plan.md)
for worker ownership and shared contracts. Start from master after the existing 7c
stack; no new feature-registry unlock is needed.

## User flow

Finish an annotated screenshot, press its preview body and drag into Explorer, a local
browser file input or a compatible app. Selecting several previews first drags the
group. The target receives the final annotated PNGs. The ghost shows a thumbnail/count.
OCR, uploads, recording and editing a finished preview are outside this stage.

## Behaviour contract

- A short press/release still toggles selection. Buttons keep their existing actions.
  A body press becomes drag preparation only after the system drag threshold, scaled
  correctly in physical screen coordinates. A drag never generates a trailing click.
- Dragging a selected card uses the current selected group in stable displayed order.
  Dragging an unselected card selects it alone and drags only that card. Action-row
  presses never initiate a drag.
- Snapshot IDs/payloads at request creation. Mark involved cards busy and pause their
  countdowns while preparing/dragging. Do not substitute another card when an ID goes stale.
- Prepare files asynchronously. If the button is released or capture is cancelled before
  readiness, cancel the request; a later IO completion must not start an unsolicited drag.
  Release preview pointer capture immediately before entering the OLE loop.
- After a confirmed DroppedCopy, dismiss dragged cards that are not kept in stack;
  keep stack-pinned cards and their existing selection. Cancel, rejected drop or error
  keeps all remaining cards and selection and resumes each countdown from its remainder.
- Escape cancels the drag. Capture loss, app hide/quit, display changes and a second Snap
  request cannot leave a stuck drag/busy flag. Defer Snap/quit until the OLE loop exits
  when necessary; show no new capture overlay from inside that loop.
- No clipboard write occurs merely because the user dragged a preview.

## Files and transport

- Reuse the final shared Bitmap/PNG in preview_store; never recapture or save the frozen
  desktop. Materialize through the existing collision-safe Pictures/Daijobu save path.
  A valid saved PNG may be reused; a missing file is recreated under a new collision-safe
  name without overwriting the previous path.
- Preparation is all-or-nothing for the requested group: if one image cannot be encoded
  or written, do not silently drag a subset. Report which cards failed; keep successful
  materialized files and record their paths for retries.
- Once exposed, PNG files remain after cancel, accepted drop, card expiry and app exit.
  Never delete a user-saved file or a file another app may still consume. Partial failed
  writes may be removed only when this operation created them.
- Offer CF_HDROP for every drag. For exactly one image also offer registered PNG and
  CF_DIB; a multi-image drag must not expose the first image as the whole bundle.
- Build Unicode DROPFILES data with checked lengths, correct offsets and double-NUL
  termination; reject empty/relative/embedded-NUL paths. Transport owns its copies.
- Copy is the only allowed effect. Never implement MOVE, delete-on-paste or a source-file
  deletion callback.
- Native code lives in dj-win/drag_drop. OleInitialize/cleanup are balanced on the
  appropriate STA thread; handle an incompatible apartment as an error.
- Implement IDataObject and IDropSource with checked FORMATETC/TYMED/aspect/index handling,
  coherent enumeration, fresh ownership of returned media and correct release behaviour.
  Support arbitrary Shell-helper private formats through SetData/GetData, including
  replacement and both fRelease cases. Use IDragSourceHelper for the ghost.
- Helper/ghost failure may fall back to the standard drag cursor, while a transport
  failure remains visible. Do not claim the ghost worked unless tested.
- The transport takes owned immutable data. Dispatch it only after releasing App/state
  borrows; no RefCell/mutex guard survives DoDragDrop's reentrant message loop.

## Architecture and dependencies

Use snap_drag in dj-core, drag_drop in dj-win, the existing preview_store plus a small
snap_drag_store adapter, and the existing preview renderer/controller. Shared exports,
event IDs and the outer application dispatcher are coordinator-owned. No new crate or
third-party drag library is required. The transport must accept owned file/image payloads
so Clipboard can reuse it in Stage 8 without depending on Snap card types.

## Automated acceptance

- Below/above-threshold gesture, selected/unselected origin, action-row exclusion,
  release-before-ready, duplicate completion, stale ID and cancel-at-every-phase tests.
- Countdown/selection preservation and success cleanup, including mixed pinned groups.
- Synthetic one/multiple-file format bytes; Unicode names, NUL rejection, size overflow,
  supported/unsupported FORMATETC, independent GetData lifetimes and helper SetData
  replacement. Test abstractions/COM data objects without starting a desktop drag.
- Temp-dir encoding, collision-safe naming, disappeared saved file, partial batch
  failure and cancellation; verify completed PNGs remain intact.
- Reentrant event simulation for hide, quit, capture request and unrelated timer events.
  No test reads/writes the real clipboard or sends OS input.

## Desktop acceptance for the coordinator

1. One and three annotated synthetic images into a temporary Explorer folder: files
   and decoded pixels match, order is stable and no original is moved/deleted.
2. Local browser file-upload fixture: one/multiple files are accepted without network
   submission. If testing Discord, stop at the attachment preview and remove the draft;
   never post/send. Record unsupported targets separately from successful ones.
3. Observe thumbnail/count ghost, Esc, rejected drop, fast mouse release during
   preparation and a failed file write. Cards, selection and countdowns remain coherent.
4. Repeat at 100/150/200% DPI and across a negative-origin monitor. Verify ordinary card
   selection, Copy/Save and keyboard navigation still work.
5. Trigger Snap/hide while preparing or dragging, then verify no stuck capture or focus.

Completion requires the usual fmt/clippy/tests/release checks and an honest
stage-7c2-report.md. Do not mark target-app behaviour verified by byte-level tests alone.
