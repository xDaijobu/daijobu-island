# Snap + Clipboard: implementation and subagent plan

Date: 2026-10-28. Status: **planned; no implementation workers dispatched**.

The user chose Snap and Clipboard as the next work and explicitly removed OCR.
This document prepares the work for a later multi-agent implementation. Music remains
outside this batch. Existing capture, annotations, preview stack and Terminal behaviour
are the baseline, not features to rebuild.

## Delivery order

| Order | Stage / proposed worktree | User-visible result | Contract |
|---|---|---|---|
| 1 | 7c-2 / stage-7c2-snap-drag | Drag one or several screenshots into another app | [7c-2 brief](stage-7c2-brief.md) |
| 2 | 7d / stage-7d-snap-pin | Keep a screenshot in an independently movable reference window | [7d brief](stage-7d-brief.md) |
| 3 | 8 / stage-8-clipboard | Searchable, persistent clipboard history with pin, copy and paste | [8 brief](stage-8-brief.md) |

One stage runs at a time, in its own Orca worktree based on the merged preceding stage.
Each stage has parallel workers, a coordinator integration pass and its own PR into
master. Stage 8 reuses the drag transport delivered by 7c-2. No worker is started by
merely reading this plan.

OCR is **removed**, not deferred: no button, shortcut, recognizer, language downloads,
WinRT OCR dependency or text extraction from screenshot pixels. Old stage reports and
historical logs may mention it; those are not implementation instructions.

## Product decisions

| Area | Decision |
|---|---|
| Stack pin | Label it **Keep in stack / Release from stack**; it pauses that card's countdown |
| Screen pin | A separate **Pin to screen** action; the reference window outlives the stack card |
| Pin lifetime | Screen pins last for the app session; explicit Copy/Save remains available |
| Drag completion | Dismiss only the dragged, unpinned cards after a confirmed copy drop; keep stack-pinned cards |
| Drag cancellation | Keep cards, selection and existing files; resume countdowns from their remainder |
| Clipboard hotkey | **Ctrl+Alt+V** plus Home tile and tray; Ctrl+Shift+V stays usable in Terminal and editors |
| Windows history | Leave Win+V and Windows clipboard history configuration alone |
| Clipboard content | Unicode text, HTTP(S) links, hex colours, images, file/folder lists; preserve compatible rich-text alternatives |
| Paste | Enter/double-click pastes; Shift+Enter pastes text only; Ctrl+C copies without pasting |
| Quick slots | Alt+1 through Alt+9 paste visible rows; plain digits continue typing into search |
| History retention | 500 unpinned items / 30 days; pinned items survive age/count pruning and restart |
| Resource limits | At most 100 pins and 512 MiB encrypted payloads total; refuse new items rather than evict pins silently |
| Privacy | Respect clipboard exclusion flags and excluded apps; DPAPI before database writes; Pause/Resume and Clear history |
| Capture | Preserve open island panels during Snap as the existing fix does; hide only transient previews/reference pins before the freeze |

The shortcut and quick-slot choices supersede the earlier Ctrl+Shift+V / bare 1–9
mockup. Screenshots and clipboard exports stay local. Uploads, recording, drag-in,
cloud sync and a Win+V replacement hook are outside this batch.

## Coordinator-owned integration

Before spawning a stage's implementation workers, commit a compiling skeleton with
the types below, empty modules, fake backend traits and module exports. Workers must
agree on that commit and contracts before editing consumers. The coordinator owns:

- Workspace/app Cargo manifests and lockfile, crate lib.rs exports, NOTICE, AGENTS.md.
- app/src/app.rs, app/src/main.rs and app/src/storage.rs.
- Shared dj-core island/layout/config/feature-registry/settings definitions.
- Shared dj-win event enum, message IDs, timer IDs, hotkeys, tray and popup routing.
- Shared dj-render frame/painter exports and renderer construction.
- docs/plan.html, README, progress and the final stage report.

Workers edit only the paths in their row. Cross-owner requests go to the coordinator;
they do not silently change another worker's interface. Workers report changes and
validation; only the coordinator stages/commits shared-tree work and pushes/merges.
If isolated child worktrees are used instead, create them through orca-cli from the
explicit stage scaffold commit and integrate their commits one at a time.

### Contracts to freeze in the skeleton

These are semantic contracts; the scaffold supplies exact Rust signatures and derives
before parallel implementation. Keep domain types safe and independent of Win32/Skia.

| Boundary | Required data and ownership |
|---|---|
| Drag request | Stable request ID; ordered item IDs; immutable Arc image payloads and resolved paths; optional single-image formats; owned thumbnail |
| Drag result | Request ID plus DroppedCopy, Cancelled, Rejected or Failed; never infer success from mouse-up alone |
| Drag preparation | Explicit Preparing, Ready, Dragging and Finished states; readiness rechecks pointer button, capture validity and request generation |
| Pin controller | PinId distinct from stack ItemId; shared annotated Bitmap/PNG; placement and visibility state; Copy/Save/Close actions |
| Clipboard input | Sequence number, capture generation, source-app identity, privacy decision and bounded owned bytes; no borrowed global handles |
| Clipboard item | Stable ClipId; one primary type and compatible alternative formats; timestamps, pin state and encrypted storage metadata |
| Clipboard store | Bounded async commands/replies keyed by operation ID and generation; ContentReady, Stored, Removed and Failed responses |
| Clipboard view | Search/filter/focus/scroll state plus bounded row and preview snapshots; no DB or native handles in renderer |
| Paste target | External window handle + process identity, or exact island Terminal TabId; never just the current foreground window |
| Paste result | Copied, PasteRequested or Failed; dispatch is not proof that a foreign app consumed the content |

No app/controller borrow, global mutable reference or mutex guard may survive a native
OLE modal loop. Platform callbacks enqueue owned events; the outer dispatcher releases
application borrows before performing the drag and reapplies the result afterwards.

## Parallel work allocation

The same two review agents can be reused after each stage. Maximum planned concurrency
is **one coordinator plus eight subagents** in Stage 8. Build/test jobs are scheduled
separately so eight workers do not each start a full Rust/Skia build.

### Stage 7c-2: four implementation workers, then two reviewers

| ID | Owned paths | Deliverable and focused verification |
|---|---|---|
| D1: drag model | crates/dj-core/src/snap_drag.rs | Press/threshold/release state, selected-group rules, cancellation, stale generations; pure event-sequence tests |
| D2: OLE transport | crates/dj-win/src/drag_drop.rs; crates/dj-win/src/drag_drop/ | IDataObject/IDropSource, format enumeration, medium ownership, helper private formats and result mapping; synthetic ownership tests |
| D3: drag files | app/src/preview_store.rs; app/src/snap_drag_store.rs | Bounded async PNG preparation, stable ordered paths, missing-file and partial-failure handling; temp-dir tests |
| D4: preview interaction | app/src/previews.rs; crates/dj-win/src/previews.rs; crates/dj-render/src/previews.rs | Body-drag versus click/actions, Preparing feedback, retained selection/countdowns, native input lifecycle |

Dependency graph: scaffold -> D1/D2/D3/D4 -> coordinator dispatcher wiring ->
R1 and R2 -> fixes -> validation/report/PR.
D4 uses the frozen model and fake transport until D1/D2 are ready; it does not call
DoDragDrop from inside its window procedure or a borrowed App method.

### Stage 7d: four implementation workers, then two reviewers

| ID | Owned paths | Deliverable and focused verification |
|---|---|---|
| P1: pin model | crates/dj-core/src/snap_pin.rs | Stable IDs, admission, placement/clamping, visibility restoration and actions; multi-monitor/DPI geometry tests |
| P2: native windows | crates/dj-win/src/snap_pins.rs | Per-window routing and resources, no-activate creation, move/capture loss, hide/restore for Snap |
| P3: pin drawing | crates/dj-render/src/snap_pins.rs | Image fit, theme chrome, hover/buttons/errors; synthetic rendering at 100/150/200% |
| P4: pin controller | app/src/snap_pins.rs | Shared image lifetime, copy/save jobs, errors, capacity, renderer/window teardown; fake-platform tests |

Dependency graph: scaffold -> P1/P2/P3/P4 -> coordinator stack/editor entry points
and capture integration -> R1 and R2 -> fixes -> validation/report/PR.
P4 targets fake window/render services until P2/P3 land.

### Stage 8: six implementation workers, then two reviewers

| ID | Owned paths | Deliverable and focused verification |
|---|---|---|
| C1: history model | crates/dj-core/src/clipboard.rs; crates/dj-core/src/clipboard_ui.rs | Types, search/filter, dedupe policy, retention decisions, keyboard/mouse actions and layout; pure tests |
| C2: monitor | crates/dj-clip/src/native.rs; crates/dj-clip/src/monitor.rs | Listener lifetime, bounded snapshots, sequence/privacy handling, source identity, pause/shutdown; fake native adapter tests |
| C3: formats | crates/dj-clip/src/formats.rs; crates/dj-clip/src/image.rs | PNG/DIB validation, normalized text/files/images, thumbnail generation, rich alternatives; adversarial/synthetic fixtures |
| C4: persistence | crates/dj-clip/src/store.rs; crates/dj-clip/src/crypto.rs | SQLite worker, user-scoped DPAPI, schema/transactions, encrypted search metadata and limits; temporary DB/fake crypto tests |
| C5: rendering | crates/dj-render/src/clipboard.rs | Search/filter/list, in-panel preview, buttons and empty/loading/error states; raster checks all themes/scales |
| C6: transfer | crates/dj-win/src/clipboard_transfer.rs; app/src/clipboard_transfer.rs | Clipboard publication, exact-target paste state, internal Terminal routing contract, image/file drag reuse; fake focus/input/clipboard tests |

The coordinator writes app/src/clipboard.rs and all shared integration. C2/C3/C4/C6
consume scaffold types without editing C1's files. The coordinator defines the monitor
and store queue endpoints and reconnects the implemented endpoints after they land.

Dependency graph:

    stage 7c-2 merged -> stage 7d merged -> stage 8 scaffold
                                            |
                                C1 C2 C3 C4 C5 C6 (parallel)
                                            |
                              controller + Settings + capture/paste wiring
                                            |
                                    R1 + R2 (parallel)
                                            |
                                  fixes, validation, report, PR

### Review workers

| ID | Review ownership | Required output |
|---|---|---|
| R1: native/resource/privacy | Read-only review of COM/global memory, reentrancy, image parsers, capture restoration, DPAPI and content logging | Findings with file/line, trigger, impact and suggested repair; verify each repaired issue |
| R2: interaction/regression | Read-only review of user flows, focus, drag cancellation, lifecycle, limits and regressions | Acceptance coverage matrix; distinguish automated evidence from unperformed desktop checks |

Reviewers do not run live capture, clipboard or input tests and do not edit overlapping
files. The coordinator routes repairs to the owning worker.

## Ready-to-use dispatch prompt

Fill the role ID, stage, worktree, scaffold commit and owned files from the tables:

    Implement ROLE for STAGE in WORKTREE, based on SCAFFOLD_COMMIT.
    Read AGENTS.md, task/stage-snap-clipboard-plan.md and the stage brief.
    Edit only OWNED_FILES. Keep the scaffold's public types and signatures stable.
    Use fakes/synthetic payloads/temp directories for tests.
    Do not run daijobu.exe, inspect the real clipboard, capture the screen, send input,
    open visible test windows, change settings, spawn more agents, push or merge.
    Tell the coordinator immediately about cross-owner changes or blocked dependencies.
    Return changed files, implemented acceptance items, focused checks and exact results,
    known limitations, and any remaining integration work. Do not claim live verification.

Review prompt: replace "Implement" with "Review read-only", select R1 or R2 and request
findings plus the relevant stage acceptance matrix.

## Verification and delivery

1. Worker checks are focused and deterministic. Coordinate build slots; use separate
   target directories for concurrent builds. Never clean a shared target or start a
   full workspace build from every worker.
2. Coordinator runs fmt check, workspace clippy with all targets / warnings denied,
   workspace tests and a locked release build with the configured MSVC environment.
3. Run the full suite without filters first. The existing
   conpty_delivers_wheel_to_a_mouse_tracking_program test has previously been flaky
   on this host; record any recurrence separately. A diagnostic filtered rerun is
   not a substitute for reporting an unfiltered failure.
4. Desktop acceptance uses synthetic screenshots/content and local test targets.
   No service uploads, real messages or unattended agent-terminal paste tests.
   Preserve the user's working windows; the coordinator handles any necessary app
   restart after the new build is ready.
5. Write task/stage-7c2-report.md, stage-7d-report.md or stage-8-report.md with an honest
   checklist, check results, manual evidence, known issues and items not verified.
   Update progress and README only to the state actually achieved.
6. Commit/push the feature branch, create and merge its PR into master after review,
   sync the main checkout, then remove the clean merged worktree and branches. Do not
   remove an active worker checkout or the pre-existing main-checkout stash.
7. No release/version bump is included in this planning batch. Runtime builds and
   reruns belong to the implementation stages.

## Technical references

Implementation choices above are ours; check API contracts against primary sources:

- [OLE DoDragDrop](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-dodragdrop):
  OLE initialization, the modal drag loop and result/effect handling.
- [IDragSourceHelper](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-idragsourcehelper):
  use the Shell helper and support its private SetData/GetData formats.
- [Shell clipboard formats](https://learn.microsoft.com/en-us/windows/win32/shell/clipboard):
  file-list transport and copy/move communication.
- [Clipboard listener](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener)
  and [clipboard exclusion formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats):
  event-driven updates and source privacy flags.
- [HTML clipboard format](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format):
  the registered name is HTML Format and offsets are UTF-8 byte offsets.
- [CryptProtectData](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata):
  user-scoped data protection, non-interactive errors and output ownership.
- [SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput):
  modifier state, return values and integrity-level restrictions.
- [rusqlite](https://github.com/rusqlite/rusqlite): MIT SQLite bindings; resolve and
  lock the chosen dependency during Stage 8, then update third-party notices.
