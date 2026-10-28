# Stage 7c-2 live coordination

Coordinator: main conversation. Orca run `run_3dda215997bf`.
Status: implementation, source reviews, workspace checks, release build and runtime
smoke check complete. The coordinator is preparing the commit/PR.

## Final handoff

- All workers are settled and released. After the user's model change, every new
  dispatch used `gpt-6.1-sol` with effort `high`; no Astra worker remains active.
- D1: 23 model tests. D2: 22 native transport tests. D3: 39 file/preparation tests.
  D4: 36 controller, 12 native-preview and 22 renderer tests. Focused clippy passed.
- Full coordinator fmt, all-target workspace clippy with warnings denied, and all
  1,323 workspace tests pass without ignored or filtered tests.
- The preparation Escape watcher remains armed through `take_drag` and COM/helper
  setup. `drag_drop::run` disarms it only after final input/cancel preflight, directly
  before `DoDragDrop`; the outer guard also clears it on early return or unwind.
- Source review repairs cover queued cancellation at entry, deferred quit/Snap order,
  capture-loss versus pointer-leave, quick Escape taps and modal notification growth.
  The coordinator checked the final snapshot enumerator after its Clone cursor fix.
- Actual Explorer/browser drops, Shell ghost appearance, stationary OLE cancellation
  and real multi-monitor/DPI changes remain desktop acceptance cases, not claims made
  by the synthetic tests below.

## Model switch and resumption (2026-10-28)

The user explicitly stopped Astra: all further workers MUST use `gpt-6.1-sol` with
effort `high` through Orca. Existing edits are preserved. Resume your component;
do not rewrite working code or assume a prior task completion means integration passed.

- D1 is complete: 23 snap_drag tests and core clippy passed (target/core-check).
- D2 has all transport files and tests. First compile was blocked by D4 native
  GetSystemMetricsForDpi arguments: remove `.0` from SM_CXDRAG/SM_CYDRAG (D4 owns fix).
  Add final primary-button/Escape preflight after helper creation and before OLE.
  Investigate prompt cancellation while stationary; coordinator asked whether
  WM_CANCELMODE to the current thread's GetCapture HWND is appropriate. Do not inject input.
- D3 has async file prep and tests, not yet tested through Cargo. Ghost is top-down
  straight-alpha BGRA <=512x512, with count badge. Do not delete retained export files.
- D4 has gesture/controller/renderer edits and tests. Finish request-scoped Escape
  watcher: window::watch_drag_escape(owner HWND, Option<u64>) -> bool exists; arm while
  preparing; retain it through native setup as clarified above, and disarm on
  cancel/hide/shutdown. Cancel visibly if installing the watcher fails.
  App calls Previews::cancel_drag_request(request:u64, now:Instant); implement matching
  generation cancellation and quick-tap/stale-ID tests. Fix the metric types above.
- Coordinator integrated owned outer dispatch, Close/quit latch, cancellation token
  seeded from queued events, no App borrow during OLE, deferred queue draining, timer
  coalescing and keyboard-hook Escape latch. Shared files remain coordinator-owned.
- Independent early review found entry/shutdown races; fixes are in source and have
  not yet had full workspace verification. Final native/resource and interaction
  reviews are still required. All work is uncommitted; main checkout is untouched.
- A separate verification worker fixed only dj-term/src/tests.rs output/exit race:
  acknowledge result before child exits; exact test 10/10, all 71 terminal tests pass.
  Production final-output-at-exit behavior was not changed.

Build with Git bash, sourcing
`C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh` for MSVC/SDK/shared target.
Request a Cargo slot; no full Skia build per worker, no clean and no live desktop tests.

The user's latest instruction overrides the older scaffold-commit rule: all work,
including interfaces and docs, stays uncommitted until implementation, review and
verification pass. Workers must not commit, push, merge, clean or reset files.

## Ownership

- D1: `crates/dj-core/src/snap_drag.rs` only: pure state machine and tests.
- D2: `crates/dj-win/src/drag_drop.rs` and `drag_drop/` only: OLE and tests.
- D3: `app/src/preview_store.rs`, `app/src/snap_drag_store.rs` only: async files/tests.
- D4: `app/src/previews.rs`, `crates/dj-win/src/previews.rs`,
  `crates/dj-render/src/previews.rs` only: gesture/controller/UI/tests.
- Coordinator: shared exports/manifests, app.rs, main.rs, window.rs, docs and integration.
- Reviewers: read-only; route findings to coordinator and file owners.

## Fixed boundaries

All workers read `stage-7c2-brief.md` and the coordination plan. Treat this file as
the current interface agreement. Ask the coordinator before changing signatures.
No worker opens the app, captures the screen, touches the real clipboard, sends OS
input, launches visible test windows, or starts another worker. No workspace-wide
formatting while other editors work. Request a build slot before Cargo commands.
Source reads and formatting owned files are fine. Do not clean shared build caches.

### D1: dj_core::snap_drag

`RequestId(pub u64)`, `Request { pub id: RequestId, pub items: Vec<ItemId> }`.
`Outcome::{DroppedCopy, Cancelled, Failed}`.
`DragState: Default` with:
- `press(&mut self, origin: Option<ItemId>, point: (i32,i32))` (None = action row).
- `motion(&mut self, stack: &mut Stack, point: (i32,i32), threshold: (i32,i32), now: Instant) -> Option<Request>`.
- `release(&mut self, stack: &mut Stack, now: Instant) -> bool`: true suppresses a click;
  cancels preparation, leaves a modal drag to finish. Below threshold returns false.
- `ready(&mut self, id: RequestId) -> bool`: once only, preparation -> modal.
- `cancel(&mut self, stack: &mut Stack, now: Instant)`.
- `finish(&mut self, id: RequestId, outcome: Outcome, stack: &mut Stack, now: Instant) -> Vec<ItemId>`: removed IDs.
- `request(&self) -> Option<&Request>`, `is_dragging(&self) -> bool`.

Threshold units physical screen pixels; use checked/widened deltas. Preserve stable
display order and selected pinned cards. Busy cards are ineligible; stale IDs fail
the whole group, never substitute. Core tests all cancellation/countdown paths.

### D2: dj_win::drag_drop

`ImageData { pub png: Arc<[u8]>, pub dib: Arc<[u8]> }`.
`Ghost { pub width: u32, pub height: u32, pub bgra: Arc<[u8]>, pub count: usize }`.
`DragPayload { pub paths: Vec<PathBuf>, pub image: Option<ImageData>, pub ghost: Option<Ghost> }`.
`DragOutcome::{DroppedCopy, Cancelled}`; errors use `Result<DragOutcome,String>`.
`run(payload: DragPayload, cancelled: Arc<AtomicBool>) -> Result<DragOutcome,String>`
owns all inputs, runs on UI STA, checks cancel in IDropSource. No App reference.
Transport independent from Snap/core. Full Shell helper SetData handling is required.
Coordinator arranges outer dispatch, sets cancel on disruptive native events, and
defers their app handling until after this call. Report helper fallback honestly.

### D3: app::snap_drag_store

`DragItem { pub id: u64, pub payload: Payload, pub saved: Option<PathBuf> }`.
`PreparedItem { pub id: u64, pub payload: Payload, pub path: PathBuf }`.
`Completion { pub request: u64, pub saved: Vec<PreparedItem>, pub failed: Vec<(u64,String)>, pub cancelled: bool, pub transport: Option<dj_win::drag_drop::DragPayload> }`.
`DragStore::new(wake: Box<dyn Fn() + Send>) -> Self`.
`submit(&mut self, request: u64, items: Vec<DragItem>, dir: PathBuf, stamp: Stamp) -> Result<(),String>`.
`cancel(&mut self, request: u64)` and `drain(&mut self) -> Vec<Completion>`.
Transport is Some only for a complete successful batch; prepared images and DIB
bytes must be assembled on the worker. Build a bounded downscaled BGRA ghost with
count. No native OLE or clipboard calls from worker. Saved paths persist even on
cancel/failure. Existing valid saved path reuse; missing path recreated safely.
Any preview_store helper added must not change existing save/copy semantics.

### D4: app::previews and native inputs

Use the D1/D3 interfaces above. Own drag controller inside Previews/Model. Add:
`Previews::take_drag(&mut self, now: Instant) -> Option<(u64, dj_win::drag_drop::DragPayload)>`
which validates current pointer/cancellation, takes one ready request, marks modal,
and releases capture before returning. No DoDragDrop from a method with App borrowed.
`Previews::finish_drag(&mut self, request: u64, result: Result<dj_win::drag_drop::DragOutcome,String>, now: Instant)`.
`Previews::cancel_drag(&mut self, now: Instant)` cancels preparation and clears busy.
Use real screen coordinates/system physical thresholds; expose native helpers as
needed. Distinguish normal pointer leave from capture cancellation while held.
Hide, capture, shutdown and relayout invalidate preparation. Preserve all ordinary
preview copy/save, keyboard and timer behavior. Rendering may say Preparing drag.

## Verification

Owners report files changed and test evidence via Orca lifecycle. Coordinator runs
full fmt, clippy, workspace tests and release build, then independent reviews and
appropriate native checks. Honest report records unavailable target/DPI checks.
Only after accepted review/tests: commit, PR to master, merge, rerun and clean stage
worktree. Stage 7d and Stage 8 follow separately.
