# Stage 7c-2 report: Snap drag-out

Date: 2026-10-28. Branch: `xDaijobu/stage-7c2-snap-drag`.
Status: implemented, source-reviewed and automated/release/runtime checks passed.
Target-app drag and multi-monitor desktop acceptance remain listed below.

## Delivered

- [x] Preview-body drag after the system's physical threshold. Short clicks still
  select; action buttons do not initiate a drag. A selected group preserves display
  order; an unselected origin becomes a one-card selection.
- [x] Stable request IDs, busy/countdown handling, all-or-nothing asynchronous PNG
  preparation, cancellation before readiness, and per-card failure feedback.
- [x] Collision-safe Snap PNG exports, validated saved-file reuse, and retained
  successful files after cancellation, failed batches, expiry and app shutdown.
- [x] Owned copy-only OLE transport with Unicode CF_HDROP; a single image additionally
  offers PNG and CF_DIB. No clipboard write merely from dragging.
- [x] Fresh GetData media, private Shell-helper formats, both SetData ownership modes,
  snapshot enumeration with independent Clone cursors, and balanced STA initialization.
- [x] Bounded thumbnail/count drag image with a standard-cursor fallback if the Shell
  helper cannot initialize. Actual appearance has not been observed.
- [x] Owned outer dispatch across the reentrant OLE loop. Disruptive events cancel
  before deferred App handling; quit prevents subsequent queued Snap dispatch.
- [x] Escape watch across asynchronous preparation and COM setup, matching-generation
  cancellation, physical button/Escape preflight, and cleanup on return/unwind.
- [x] Only confirmed copy drops dismiss unpinned dragged cards. Cancellation/errors
  preserve remaining cards, selection and countdown remainder.
- [x] README, workflow and progress documentation. No new dependency, OCR, recording,
  upload or additional feature unlock.

## Verification

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace --locked`: **1,323 passed**, zero failed, ignored or filtered.
  App 192; capture 8; core 708; render 154; sweep 35; terminal 71; vitals 26; Win32 129.
  Documentation tests also passed.
- `cargo build --release --locked --target-dir .../target/stage-t4`: passed (50.62 s).
- Graceful shutdown, copied-artifact hash match and restart from the standard release
  path passed. The new process (PID 31904) answered native responsiveness probes.
  Posting the app's Snap hotkey opened its real overlay; posting Snap Cancel closed
  it and left the app responsive. Clipboard sequence stayed unchanged. No synthetic
  keyboard/mouse input or clipboard contents were used in this smoke check.
  Release SHA-256: `9DDB0108DA01A3228C70309BB376E577E6F23E1660E0984978D867DC480C6CB5`.
- Independent reviews examined file lifetime, COM resources, reentrancy, gestures,
  cancellation and shutdown. All actionable findings were repaired and re-read.
- Tests use synthetic images, temporary directories, COM objects and fake event flows;
  they do not send desktop input or read/write the system clipboard.

The full-suite run initially exposed an existing ConPTY mouse fixture race: its child
exited before the asynchronous reader consumed the final assertion output. The test
now acknowledges complete output before child exit. Its exact test passed ten runs
and all 71 terminal tests pass. Production terminal code was not changed; this does
not claim a general fix for final-output-at-exit behaviour.

## How to try

Build and run `daijobu.exe`. Finish a screenshot, then drag its preview image into a
temporary Explorer folder or a local browser file input. Select several previews to
drag them together. Escape cancels; a confirmed copy removes only unpinned cards.
Exports remain in the configured Snap save folder (`Pictures/Daijobu` by default).

## Remaining desktop acceptance

- [ ] Explorer one/multiple-file acceptance and decoded final annotated pixels/order.
- [ ] Local browser file-input acceptance without submitting or uploading.
- [ ] Visible Shell thumbnail/count ghost and fallback appearance.
- [ ] Escape/reject/fast release/failed write in the actual gesture flow.
- [ ] Stationary cancellation during native OLE on hide, quit, second Snap or display
  change. The coordinator posts WM_CANCELMODE to the current thread's capture window;
  source/event tests do not prove every native OLE target exits promptly.
- [ ] Real 100/150/200% DPI, negative-origin monitors and cross-monitor drag behaviour.

PNG admission bounds retained input bitmap/cached PNG bytes, not process peak memory;
encoding and native format/ghost buffers add temporary allocations. Exported files
are deliberately retained, including after a cancelled drag. Pin-to-screen and
Clipboard history are the next separate stages and are not part of this change.
