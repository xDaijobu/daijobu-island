# Stage 7d coordination: screen pins

Coordinator: Codex resumed at the user's explicit request after Claude quota exhaustion.
Active Orca run: `run_c736a1266e74`; previous Codex run `run_3dda215997bf` is historical.
Base: `fc8d025` (Stage 7c-2, PR #29). Worktree: `stage-7d-snap-pin`.
Latest user instruction: resume/retry/continue with Codex now. New workers use
`gpt-6.1-sol` with `high` effort; do not use Astra. This supersedes the earlier
Claude-only handover. Claude's source repairs are preserved and its terminals closed.
Read [Claude handover](handover-claude-2026-10-28.md) as historical context; its
unfinished-fix/test statuses predate Claude's repairs and require fresh verification.

Read AGENTS.md, stage-7d-brief.md and stage-snap-clipboard-plan.md. Implement only
Stage 7d. No OCR, recording, clipboard history, startup persistence or automatic saves.
All work, including scaffold/docs, stays uncommitted until integration, review/tests.
Only the coordinator commits, pushes, opens/merges PRs and removes the worktree.

## Ownership and execution

- P1: `crates/dj-core/src/snap_pin.rs` only.
- P2: `crates/dj-win/src/snap_pins.rs` only.
- P3: `crates/dj-render/src/snap_pins.rs` only.
- P4: `app/src/snap_pins.rs` only.
- P5: `app/src/snap.rs`, `crates/dj-core/src/snap_edit/`,
  `crates/dj-render/src/snap.rs`: editor Pin transaction and error rendering only.
- P6: `app/src/previews.rs`, `crates/dj-core/src/snap_stack.rs`,
  `crates/dj-win/src/previews.rs`, `crates/dj-render/src/previews.rs`: stack overflow
  and distinct Keep in stack / Pin to screen entry points only.
- Coordinator: exports/manifests, app/main, latest-shot actions, capture,
  tray/event integration and docs. Reviewers are read-only.

Do not edit another owner's file; send cross-owner requests to the coordinator.
No worker opens the app, captures the screen, reads/writes the real clipboard,
sends OS input, launches visible test windows, changes user settings or spawns agents.
Use synthetic images, fake platform adapters and temporary directories for tests.
No workspace-wide formatting or Cargo without a granted slot; do not clean caches.
P1 may use the separate existing `target/core-check` for focused pure-core checks.
Other Cargo commands use Git bash and source
`C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh` after requesting a slot.

## Frozen P1 domain API

All public types are safe and independent of Win32/Skia. Use existing `snap::IRect`
and `layout::Rect`. IDs and actions derive Clone/Copy/Debug/PartialEq/Eq/Hash.

```text
PinId(pub u64); SourceId(pub u64);
Allocation { pub key:u64, pub bytes:u64 } // Clone Copy Debug PartialEq Eq
Admission::{New(PinId), Existing(PinId)}
PinAction::{Copy, Save, Close}
Hit::{Body, Action(PinAction)}
Layout { pub w:f32, pub h:f32, pub image:Rect,
         pub actions:[(PinAction,Rect);3], pub status:Rect }
Layout::hit(&self, x:f32, y:f32)->Option<Hit>
layout(image:(u32,u32), scale:f32, work:IRect)->Layout
place(layout:&Layout, scale:f32, work:IRect, anchor:(i32,i32))->IRect
clamp(rect:IRect, work:IRect)->IRect
allocation_size(scale:f32)->(u32,u32)
```

Image fit <=640x480 logical, no needless physical-pixel upscaling. Chrome has legible
Copy/Save/Close buttons even for tiny images, within monitor work area; Close remains
reachable. Layout w/h are logical, work/anchor/placed rectangle physical. Fixed maximum
allocation must contain every layout at that scale, with checked/sane bounds.

```text
Pins::default() // allowed=true, not capturing; monotonically increasing IDs
Pins::admit(&mut self, source:SourceId, allocations:Vec<Allocation>)->Result<Admission,String>
Pins::close(&mut self, id:PinId)->bool
Pins::contains(&self, id:PinId)->bool
Pins::ids(&self)->Vec<PinId> // stable insertion order
Pins::count(&self)->usize; Pins::bytes(&self)->u64
Pins::replace_allocations(&mut self, id:PinId, allocations:Vec<Allocation>)->Result<(),String>
Pins::reveal(&mut self, id:PinId)
Pins::set_visible(&mut self, id:PinId, visible:bool)
Pins::is_visible(&self, id:PinId)->bool
Pins::set_allowed(&mut self, allowed:bool)
Pins::begin_capture(&mut self); Pins::end_capture(&mut self)
```

Max 8 pins /256 MiB unique allocations. Reject overflows/inconsistent keys atomically,
never evict. Duplicate source reveals an existing pin. Desired per-pin visibility is
separate from global hide and capture suppression: end_capture cannot resurrect a
closed pin, override a later hide, or reveal a pin individually hidden before capture.
New editor pins admitted during capture become visible only after capture ends.

P4 derives SourceId from the live Arc<Bitmap> identity, which stays retained while the
pin exists. Shared source results therefore reveal; separately allocated results stay
distinct even with identical pixels. Allocation keys identify live pixel/PNG allocations
with byte lengths. IDs are not recycled, and screenshot identity is not a stack ItemId.

## Frozen P2 native API

No dj-core dependency. Rectangles are `(i32,i32,i32,i32)` = x,y,w,h in physical pixels.
`PinInput` derives Clone/Copy/Debug/PartialEq and has:

```text
Move {x:i32,y:i32,screen_x:i32,screen_y:i32}
Down {x:i32,y:i32,screen_x:i32,screen_y:i32}
Up {x:i32,y:i32,screen_x:i32,screen_y:i32}
Leave, CancelCapture, FocusLost, Paint, Close, Destroyed, DisplayChanged
Key {vk:u16,ctrl:bool,shift:bool,alt:bool}
Dpi {dpi:u32,suggested:(i32,i32,i32,i32)}
```

Queue `window::Event::SnapPin { id:u64, input:PinInput }` only; never call the app.
Use per-HWND owned state, no single controller TLS. Callbacks must not retain a mutable
state reference across reentrant Win32 calls. Pair create/destroy and capture cleanup.

```text
Window::new(id:u64, bounds:(i32,i32,i32,i32))->Result<Window,String>
Window::hwnd(&self)->windows::Win32::Foundation::HWND
Window::show(&self); Window::hide(&self)
Window::place(&self, bounds:(i32,i32,i32,i32))
Window::capture(&self)->bool; Window::release_capture(&self)
Window::has_foreground(&self)->bool
Window::focus(&self)->Option<FocusTarget>
Window::drop // releases capture/destroys exact owned HWND; !Send
work_area_at(point:(i32,i32))->((i32,i32,i32,i32),u32) // bounds + DPI
work_area_for(hwnd:HWND)->((i32,i32,i32,i32),u32)
restore_focus(target:FocusTarget) // validate HWND and process identity before restoring
focus_target()->Option<FocusTarget> // snapshot BEFORE a tray popup activates its owner
FocusTarget::is_window(&self, hwnd:HWND)->bool // distinguish a prior pin from an external target
```

FocusTarget is Copy/Clone/Debug and opaque. Creating/showing/moving is topmost,
TOOLWINDOW/NOACTIVATE, without taskbar/Alt-Tab entry. Only deliberate tray recovery
calls focus; do not activate on mouse clicks. P4 performs manual captured body movement
from queued screen coordinates, avoiding a native moving modal loop. Dpi changes are
queued and applied by P4. Validate WM_PAINT without spinning; per-window tracking and
WM_NCDESTROY teardown must remain isolated and testable without native windows.
Since a nonforeground HWND cannot rely on SetCapture for outside-window delivery,
use a bounded per-HWND sampler only while pressed to observe physical cursor/button
changes without activation. Disarm on release/cancel/capture loss/hide/close/destroy.

## Frozen P3 rendering API

Uses P1 geometry. Public exports will be `PinScene` and `PinSurface`.

```text
PinScene<'a> {
  pub tokens:&'a ResolvedTokens, pub scale:f32, pub layout:&'a Layout,
  pub hover:Option<PinAction>, pub focus:Option<PinAction>,
  pub status:Option<(&'a str,bool)>, pub saved:bool, pub busy:bool
}
PinSurface::new(hwnd:isize, width:u32, height:u32)->Result<Self,RenderError>
PinSurface::size(&self)->(u32,u32)
PinSurface::set_image(&mut self, bitmap:&Bitmap, scale:f32)->bool
PinSurface::render(&mut self, scene:&PinScene)->Result<(),RenderError>
```

Own D3D12/DirectComposition chain, sized once to P1 allocation_size for current DPI.
Cache only the display-sized image; original pixels remain in P4. Use existing GPU,
fonts, token and raster helpers. Surface position changes do not resize/recreate it.
Clip to the actual layout; legible Copy/Save-or-Open/Close, hover/focus/error/busy states.
Draw synthetic raster tests in all four themes and 1/1.5/2 scales without creating HWNDs.

## Frozen P4 controller API

`Pins` is the app controller and uses `preview_store::{Payload,Store}` and
`snap::SystemClipboard`. Keep all unsafe/native code in dj-win or dj-render.

```text
Pins::new(owner:HWND, tokens:&ResolvedTokens, save_dir:PathBuf)->Self
Pins::open(&mut self, payload:Payload, saved:Option<PathBuf>, anchor:(i32,i32))->Result<PinId,String>
Pins::input(&mut self, id:u64, input:PinInput)
Pins::set_tokens(&mut self, tokens:&ResolvedTokens)
Pins::set_save_dir(&mut self, dir:PathBuf)
Pins::set_allowed(&mut self, allowed:bool)
Pins::begin_capture(&mut self); Pins::end_capture(&mut self)
Pins::display_changed(&mut self)
Pins::show_keyboard(&mut self, previous:Option<dj_win::snap_pins::FocusTarget>)
Pins::can_focus(&self)->bool
Pins::wants_frame(&self)->bool; Pins::frame(&mut self)
Pins::drain(&mut self)->Vec<(Arc<Bitmap>,PathBuf)> // completed saves for thumb/stack sync
Pins::mark_saved(&mut self, bitmap:&Arc<Bitmap>, path:&Path) // source saves update existing pins
Pins::shutdown(&mut self)
```

Open is transactional: retain existing pins if a new native/GPU allocation fails and
roll back the new admission. Do not copy/save merely from open. Duplicate reveals the
same live pin. Original payloads survive stack expiry independently. Drop renderer
before its native window. Dirty/event-driven frames only; bound device-loss retries,
retain original image and allow later retry without an idle render loop.

Save/Copy/Save-as-Open reuse Store, using PinId for async replies. Ignore closed IDs and
avoid applying stale replies. Reaccount cached PNGs; if caching would exceed the unique
retention budget, discard that optional cache rather than silently exceeding the bound
or closing an existing pin. Temporary encoding/native buffers are not retained budget.

Body drag excludes actions; clamp on release/work-area/DPI changes, including negative
origins. Keyboard recovery supports Tab/Shift+Tab, Enter, Ctrl+C/Ctrl+S, Escape close.
Restore previous valid focus only if closing the deliberately focused pin still owns
foreground. Capture suppression tracks live desired visibility through hide/close/open.
The previous target is captured before tray::show_menu, which activates the island;
recovery from an owned pin preserves its remembered external target, while recovery
from a different external app replaces it with that new app. Native Window::focus is
only a fallback when no override exists. `can_focus` means recovery is available:
live pins and no active capture, even when global hide currently suppresses them.
P4 hides before coordinator composition flush and does not restore while capture is up.
The existing capture window sampler excludes this process, including all reference
HWNDs; no extra raw-HWND enumeration API is needed in the app controller.

## P5 editor entry point contract

Add `dj_core::snap_edit::Outcome::Pin` and an explicit Pin to screen finish action
with usable label/tooltip and wrapped/clamped toolbar layout. Commit active text as
Copy/Save do, but no clipboard/file operation. Preserve existing shortcuts and tools.
`Snapper::pin_payload(&self)->Result<preview_store::Payload,String>` flattens the
current selection and committed markup into a new shared Bitmap with png=None;
it must not end/take the session or replace the previous Shot. No fallback to a bare
crop if rendering fails. The coordinator calls Pins::open, and only on success calls
the existing End::Cancel cleanup to close the editor and persist changed preferences.
Failure leaves the session usable. Add `pub error:Option<&'a str>` to `EditScene<'a>`
for a readable error chip, and update owned renderer tests; coordinator supplies it.
Also draw the latest-shot `SnapAction::PinScreen` button at `layout::SN_PIN` and add
`SnapFrame::pin_failed:bool` for a visible failed-admission caption. These two shared
definitions and app initializers are coordinator-owned; the renderer is P5-owned.
Tests prove pin preparation changes neither clipboard nor saved files/last shot,
markup pixels survive, and preparation/failure leaves the session intact.

## P6 preview entry point contract

Keep compact Copy, Save/Open, More, Dismiss actions. More opens a native overflow
with exact labels Keep in stack / Release from stack and Pin to screen. Do not
squeeze extra tiny labels into the old row. Preserve selection/drag/copy/save/countdown
semantics and keyboard navigation; menu invocation must not initiate a drag.

```text
pub struct PinRequest { pub id:u64, pub payload:Payload, pub saved:Option<PathBuf> }
Previews::take_pin_requests(&mut self)->Vec<PinRequest>
Previews::pin_result(&mut self, id:u64, result:Result<(),String>, now:Instant)
```

The coordinator drains requests outside preview handling, calls Pins::open using the
existing stack anchor and returns success/error by the original item ID. Failed pin
admission/creation needs visible card feedback; successful pinning leaves the card,
its keep-alive state and system clipboard unchanged. `Payload` Arc identity is used
for deduplication across card/latest-shot/pin. Add focused tests with synthetic actions,
including the overflow choices and stale result IDs. P6 may define additional private
model/native menu helpers but must not edit lib.rs, window.rs or app.rs.

## Claude run additions (2026-10-28)

Run `run_c736a1266e74`. W-A owns `snap_pin.rs`, `app/src/snap_pins.rs` and the
`Store::with_parts` visibility in `preview_store.rs`; W-B owns `dj-render/src/previews.rs`;
R1 (native sampler) and R3 (integration) are read-only reviewers.

Pending IO keeps payload references alive after a pin closes, so the domain ledger also
counts IO holds:

```text
HoldId(pub u64) // Clone Copy Debug PartialEq Eq Hash; never recycled
Pins::hold(&mut self, allocations:Vec<Allocation>)->Result<HoldId,String> // atomic
Pins::release(&mut self, hold:HoldId)->bool // idempotent
Pins::holds(&self)->usize
```

Unique keys across live pins and holds count toward 256 MiB; holds never count toward
the 8-pin limit, never reveal and survive `close`. The controller holds the payload's
allocations before queueing Copy/Save, releases immediately if nothing was queued, and
releases on every drained reply, including replies for closed pins.

A pre-overlay capture failure gives the keyboard back to a deliberately focused pin:

```text
app Pins::keyboard_owner(&self)->Option<PinId> // live pin that owns the foreground
app Pins::refocus(&mut self, id:PinId)->bool   // keeps the remembered external target
```

`begin_snap` records the owner before hiding and the foreground after hiding; a failure
path refocuses only if the app is not quitting and the foreground has not changed since.

## Coordinator integration

- Add editor Pin to screen with two-phase flatten/admit then editor-close: no clipboard
  write or file save, and errors leave the editor/markup intact with visible feedback.
- Add latest-shot and preview overflow actions; replace the ambiguous stack pin label
  with Keep in stack / Release from stack. Shared image identity preserves duplicates.
- Wire native events, frame/wake/theme/display/hide/shutdown and tray recovery.
- Hide pins before previews' composition flush/freeze; restore on every finish/cancel/
  failure path without closing the existing open Terminal/panel.
- Run independent reviews, full workspace checks, release/runtime smoke and honest
  report, then commit/PR/merge/cleanup. Stage 8 follows separately.
