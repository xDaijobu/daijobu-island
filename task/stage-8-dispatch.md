# Stage 8 dispatch contract

Base: `282899d` (Stage 7d merged, PR #30); worktree `stage-8-clipboard`.
Coordinator is Kilo following the user's full ownership handover on 2026-10-28.
Read AGENTS.md, stage-8-brief.md and stage-snap-clipboard-plan.md. The user's latest
instruction requires Kilo only; no Codex, Astra or Claude workers.
No commits (including scaffold/docs) until implementation, reviews and tests pass.

## Ownership

- C1 model/UI: `crates/dj-core/src/clipboard.rs`, `clipboard_ui.rs`.
- C2 listener: `crates/dj-clip/src/native.rs`, `monitor.rs`.
- C3 formats: `crates/dj-clip/src/formats.rs`, `image.rs`.
- C4 storage: `crates/dj-clip/src/store.rs`, `crypto.rs`.
- C5 drawing: `crates/dj-render/src/clipboard.rs`.
- C6 transfer: `crates/dj-win/src/clipboard_transfer.rs`, `app/src/clipboard_transfer.rs`.
- C7 integration foundation: core `island.rs`, `layout.rs`, `config.rs`, render
  `paint.rs`/`lib.rs`, and `app/src/terminal.rs`. Add Clipboard state/config/render
  plumbing and exact terminal-tab adapter; do not unlock the feature yet.
- C8 controller: `app/src/clipboard.rs`, service coordination and owned transfer events.
- C9 taskbar/tray latency: `crates/dj-win/src/tray.rs`; request cross-owner edits.
  Added by the user's report during implementation; preserve all menu commands.
- Head: shared scaffold types/protocol, manifests, remaining exports, app
  controller/integration, feature unlock, hotkeys/tray and docs.
- Reviewers are read-only. Ask head for cross-owner changes; do not edit them yourself.

No worker starts the app/listener, reads or writes the real clipboard, sends native
input, captures a screen, opens windows, modifies user settings, or spawns agents.
Use fake adapters, synthetic bytes/images and temporary directories. No Git mutations,
workspace formatting or Cargo until a slot is granted. Use rustfmt skip_children for
owned files only. Shared build helper is
`C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh` (Git bash).

## Frozen shared types

`dj_core::clipboard_types` owns the scaffold data; C1 re-exports it through
`dj_core::clipboard`. Keep those public fields and enums stable. Add helpers in C1:
`Payload::kind() -> Kind`, `Payload::retained_bytes() -> usize`,
`Payload::plain_text() -> Option<String>`, `Payload::summary() -> String`, and
`Payload::validate() -> Result<(), String>`. Content equality includes all rich
alternatives and no_cloud; source executable metadata is separate in Captured/Row.
No content-bearing logs. Retained image BGRA is straight-alpha, tightly packed top-down.

`dj_clip` root owns RawFormat/RawPart/RawSnapshot, Captured, MonitorPolicy/MonitorEvent,
StoreRequest/StoreCommand/StoreReply/StoreResult, Generation, Wake and ReplayTracker.
Workers import those types, and request changes from head. Generation is shared
`Arc<GenerationGate>`; pause/disable/clear increment it before invalidating pending work.
Storage holds `commit_guard()` for final generation validation and commit. UI uses
`try_fetch_add(1, Ordering::SeqCst)` and defers invalidation while that gate is held;
it never waits for a storage commit. Construct with `Arc::new(GenerationGate::new(1))`.
Every worker event/request carries its generation; stale results never enter the UI.

## C1 model and C5 drawing interface

Implement pure `clipboard_ui` with `View` public fields:
`rows: Vec<Row>, query: String, filter: Filter, selected: Option<ItemId>, scroll: usize,
enabled: bool, paused: bool, loading: bool, status: Option<String>, preview: bool,
confirm_clear_all: bool, focus: Focus` (Focus enum under C1), plus Default.
Preserve search editing selection/caret privately or via extra fields. Methods:
`set_rows(Vec<Row>)`, `selected_row()->Option<&Row>`, `key(vk:u16,ctrl:bool,shift:bool,alt:bool)->Action`,
`character(char)->Action`, `insert_search(&str)->Action`,
`click(x:f32,y:f32,double:bool)->Action`, `wheel(delta:i32)->Action`.
Actions: None, Redraw, SearchChanged, Enable, TogglePause, Close, Copy{plain:bool},
Paste{plain:bool}, TogglePin, Delete, ClearUnpinned, RequestClearAll, ConfirmClearAll,
CancelClearAll, Preview, Drag, PasteSearch. Actions read the stable selected ItemId;
head captures it before fetching payloads asynchronously. Alt slots address visible
rows only, digits edit search. Add shared layout/hit helpers under C1 and tell C5.

Geometry is 400x360. Suggested header y=10, search y=36 h=28, filter chips y=70 h=24,
five rows y=100 h=44, footer y=328 h=24. Header owns Pause/Clear/Close; footer owns
Pin/Delete/Copy/Paste/Preview/Drag. In-panel preview and clear-all confirmation may
replace the list. Keep labels readable; use tooltips/keyboard access where needed.

C5 defines `ClipboardPanel<'a> { pub view: &'a View, pub payload: Option<&'a Payload>,
pub hover: Option<(f32,f32)> }` and
`pub(crate) fn paint_clipboard(canvas:&Canvas, fonts:&mut Fonts,
tokens:&ResolvedTokens, at:(f32,f32), scale:f32, panel:ClipboardPanel<'_>)`.
Head adds module export, Frame field and painter call. Render clipped plain text,
font fallback, contained images/swatches/file metadata, visible selected/pinned and
disabled/error states. Avoid large per-frame image copies; previews can use bounded
thumbnail pixels initially, with no disk/network reads. Synthetic raster tests required.

## C2 listener and C3 normalizer interface

C3 exports `formats::normalize(&RawSnapshot)->Result<Payload,String>` with sanitized
error categories. Inspect privacy flags before taking raw snapshots (C2), then apply
Files > Image > Text precedence in C3. `image` exports
`decode_png(&[u8])->Result<ImageData,String>`, `decode_dib(&[u8])->Result<ImageData,String>`,
`encode_dib(&ImageData)->Result<Vec<u8>,String>`. C3 can add internal helpers/tests.
Validate all byte limits, dimensions, offsets, UTF-16 terminators, paths, rich formats
and orientation/masks before allocating. PNG crate 0.18.1 is available, no Skia needed.

C2 exports `monitor::Monitor::new(generation:Generation,replay:ReplayTracker,wake:Wake)
->Result<Monitor,String>`, `configure(&self,policy:MonitorPolicy)->Result<(),String>`,
`drain(&mut self)->Vec<MonitorEvent>` and `shutdown(&mut self)`; Drop stops safely too.
Initial policy is disabled; configure starts from future sequence events only.
Native message thread owns listener/window and bounded retry timers, reads native
memory while open then closes before normalize. No UI polling or blocking join.
Read exclusion formats first, resolve owner executable without titles, preserve
no-cloud flag, use process+private-token+sequence checks with ReplayTracker for only
Clipboard's own replay. Snap and Terminal writes must still be captured once.
Bound both queue counts and retained bytes; coalesce overload to latest event.
Wake callback must not run after shutdown. Generation changes invalidate slow reads.

## C4 store interface

`store::Store::new(path:PathBuf,generation:Generation,wake:Wake)->Result<Store,String>`
spawns the worker; DB open/load happens off UI, and request 0 yields Ready or error.
`send(&self,StoreRequest)->Result<(),String>`, `drain(&mut self)->Vec<StoreReply>`,
`shutdown(&mut self)` plus safe Drop. Bounded requests/replies and retained bytes;
full queues return honest errors. No DB/DPAPI work on the island thread.
Use rusqlite 0.41.0 (default features off, bundled) and current-user DPAPI; fake
crypto for tests. Schema/version handling and encrypted JSON/binary representation
are yours, but source/text/paths/rich data/image/thumbnail metadata must be encrypted
before SQLite. Allowed plaintext fields are IDs/times/type/pin flags/sizes only.
Exact dedupe preserves stable ID/pin, promotes used_ms, and checks bytes after hash
candidate; hashes remain RAM-only. Enforce all retention limits transactionally.
Query filters and searches full text off UI with bounded decrypted memory; do not
load all original images into the index. Row previews should be at most 4096 UTF-8
bytes (truncate only presentation), thumbnail at most 160x90, total metadata/cache
under 128 MiB. Fetch returns one owned Arc lease. Plaintext payload is never truncated.
Generation checks must prevent invalidated inserts/clear races; coordinate with head
if stronger synchronization than the shared atomic is needed for the commit boundary.
Reply acknowledges transactions before UI claims changes. No silent corrupt DB reset.
Use a bounded shutdown deadline and suppress stale/dead-window callbacks.

## C6 transfer interface

Own native publication/focus/input in dj-win and the app adapter only. Request needed
Windows crate features from head. The app adapter may import dj_clip/dj_core and
native exports; dj-win must not depend on dj-clip. Native API internals are yours.
Provide app `Transfer` with `new(owner:HWND,replay:ReplayTracker,wake:Wake)->Self`,
`capture_external()->Option<Target>`, and Target variants External(native identity)
and Terminal(dj_core::term_tabs::TabId). Target must be Clone.
`begin(payload:Arc<Payload>,plain:bool,paste:bool,target:Option<Target>,now:Instant)
->Result<(),String>`, `tick(now:Instant)->Vec<TransferEvent>`, `is_pending()->bool`,
`cancel()`, `shutdown()`. Events: `Copied`, `ManualPaste(String)`,
`TerminalPaste{tab:TabId,text:String}`, `Failed(String)`; add others only with head.
Prepare expensive format buffers off UI; one publication session validates/allocates
before EmptyClipboard, releases untransferred ownership, accurately reports partial
failure. No paste on failed write; no automatic previous-clipboard restoration.
External paste restores exact target, waits boundedly for physical modifiers without
sleeping on UI, verifies foreground/process identity immediately before Ctrl+V, never
Enter. Elevated/closed/focus-stolen cases stay copied with manual feedback. Internal
Terminal emits an exact-tab event; head routes via existing bracketed paste semantics.
Expose `read_search_text()->Result<Option<String>,String>` only for deliberate search
paste. File text uses existing terminal semantics; image-only Terminal paste refuses.
Expose `prepare_drag(payload:Arc<Payload>,export_dir:PathBuf)->Result<DragPayload,String>`
for an off-UI explicit drag preparation. Reuse native OLE transport; no native drag
inside App borrow. Image exports collision-safe PNG under Pictures/Daijobu/Clipboard,
kept after cancel/delete; file lists validate all members only on explicit transfer.
Head integrates owned outer drag dispatch and cancellation. No text drag.

## Completion

Send saved checkpoints plus exact tests/files and limitations. Do not mark unrun
native checks as passed. Head runs full fmt/clippy/tests/release and native acceptance,
then report/README/progress, PR to master, merge and remove the clean worktree.
