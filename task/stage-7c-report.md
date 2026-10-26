# Stage 7c report: Snap stack (preview UI only)

Status: **built and verified headless** on `xDaijobu/stage-7c-snap-stack`. **No live test yet**: nothing in this stage has been run on a desktop, so every acceptance sequence below is still open.

Built 2026-10-26 by the coordinator (Claude Opus 5.5) with four parallel Claude Sonnet 5.5 workers, each in its own worktree with its own target dir. The interface contract is in [`stage-7c-plan.md`](stage-7c-plan.md).

| Part | What | By |
|---|---|---|
| A core | `dj_core::snap_stack`: countdowns and pause rules, capacity, selection, keyboard model, layout and hit testing, springs, placement; `[snap] preview_seconds` | Sonnet worker |
| B native | `dj_win::previews`: the companion window, rounded input regions, `DwmFlush`, work area / DPI; tray item "Focus Snap previews" | Sonnet worker |
| C render | `dj_render::previews`: the Skia surface (cards, ring, PIN badge, check, focus ring, status chips, bar, notice) | Sonnet worker |
| D store | `app/src/preview_store.rs`: shared payloads (`Arc`), one bounded IO thread for saves and encodes, reveal with a missing-file check; `Shot` now shares its image | Sonnet worker |
| F wiring | `app/src/previews.rs` (controller: `Model` + window/surface) and `app.rs`; topmost companion in `dj_win::system`; review and fixes across A–D | coordinator |
| 7b review | read-only review of the 7b follow-up fixes (verdict: ship; one gap fixed, see below) | Sonnet reviewer |
| F review | read-only review of the wiring (no high-severity bug; the medium/low findings are fixed, see "Review of the wiring") | Sonnet reviewer |

## Checklist (Stage 7c scope in `progress.md`)

Legend: ✅ done and covered by headless tests · 🟡 done, needs the live test to confirm · ⬜ not done

- ✅ **7b finish flow kept.** A card is added only in `end_snap` when the shot was taken (Ctrl+C / Enter / Ctrl+S), never on release or cancel. The 6 s thumb is untouched; the card shares the thumb's `Arc<Bitmap>` and PNG (`Snapper::payload`). Expiry and dismissal only drop `Arc`s (tests: `dismiss_and_expiry_free_the_payload_but_leave_files_and_clipboard_alone`).
- 🟡 **Floating previews.** Separate `WS_POPUP` window (`DaijobuSnapPreviews`, `WS_EX_NOACTIVATE | TOOLWINDOW | TOPMOST | NOREDIRECTIONBITMAP`) with its own D3D12 + DComp chain, under the island, not an island activity and not an AppBar. 124×80 r12 cards, aspect-fit thumbnails made once per card, 16 px ring, check mark, text action chips, newest first, gap 10, spring pop-in / slide / fade-out. No fan (optional in the contract) and no hover lift.
- 🟡 **Placement/input.** The window (692×188 logical, sized once per DPI) is centred under the island's current lower edge + 12 px and clamped into that monitor's work area (`snap_stack::place`, tested with negative coordinates, 150 % and clamping). It follows the island every frame the island moves. The window region is only the cards (+2 px, +5 px when selected/focused for the outline), the bar and the notice, so gaps and the rest of the window are click-through. Shown with `SW_SHOWNOACTIVATE`; clicks answer `MA_NOACTIVATE`; no taskbar/Alt-Tab entry. Kept topmost with the island on every foreground change (`system::set_topmost_companion`).
- ✅ **Countdown.** `[snap] preview_seconds` (3–60, else 10; lenient parse). One predicate: running = not hidden, nothing selected, not hovered, not keyboard-focused, not pinned, not busy. Paused cards keep their remainder. Hidden and capture time never counts. Covered by 13 pause tests incl. all 63 combinations of the six reasons.
- ✅ **Pin = keep in stack**, session only; the ring is replaced by a "PIN" badge. No pin windows.
- ✅ **Selection/actions.** Card body toggles selection; buttons never select (`hit` gives actions priority). `copy`, `save`→`open`, `pin`/`unpin`, `✕`. Bar: `N selected`, `Select all`, `Clear selection`, `Save selected` (keeps cards and selection, skips cards whose file still exists, per-card results), `Dismiss selected`. No multi-image clipboard.
- ✅ **Payload/storage.** No recapture; saves reuse `write_new` (no overwrite, `-2`…); `open` reveals the saved file; a missing file shows `file missing` and the card can be saved again; failures stay on the card (`copy failed`, `save failed`) and go to the log; nothing is written to disk without an explicit save.
- ✅ **Bounded retention.** 5 cards / 256 MiB (pixels + PNG, counted once with the thumb). Only unprotected cards are evicted (oldest first); otherwise `Preview stack full` for 3 s and the shot keeps its own copy/save/thumb. Stable `ItemId`s; worker replies for gone cards are dropped (`a_reply_for_a_dismissed_card_is_dropped`).
- 🟡 **Lifecycle/performance.** Hidden with Ctrl+Alt+H (island visibility) and at the start of every `begin_snap` (both the direct and the delayed path) followed by `DwmFlush`, restored after finish/cancel and on every early failure. Our own windows are already excluded from snap targets (`targets.rs`: own PID). Window, surface, thumbnails and timer exist only while there are cards or a notice. Timer 9 runs at 100 ms only while a visible countdown runs, else a one-shot to the notice's end, else nothing. Saves/encodes run on the IO thread. DPI/display change rebuilds the window and thumbnails at the new scale.
- ✅ **Tests.** See below.

Also in this branch: the 7b follow-up fixes (pixelate fails closed; double-click stays off the toolbar/picker) and, from the 7b review, **a double-click on a selection handle now grabs the handle** instead of re-editing text under it (`double_click_on_a_selection_handle_grabs_it_instead_of_editing_text`, verified to fail without the fix).

## Review of the wiring

A Sonnet reviewer read part F after it was written (no high-severity bug). Fixed:

- **The 10 Hz tick was re-armed on every input**, so a card could not expire while the mouse kept moving over the stack. The timer is now armed only when what it is for changes (`Armed::Tick` / `Armed::At`, 20 ms slack for one-shots).
- **Hover/focus survived a window rebuild** (DPI or display change, lost device), leaving a card paused for good. `destroy_surface` now clears press, hover and focus.
- **A lost device or a failed window left cards counting down unseen** and never retried. The countdowns now pause exactly while nothing is on screen (`set_hidden(!shown)` after every sync), and a lost/failed window is retried every 500 ms, at most 3 times in a row (reset by the next snap or relayout).
- **A stale `FocusLost`** (queued while the tray menu had the foreground) could clear the focus the tray had just given. It is ignored while the window holds the foreground, and the focus is only set if `SetForegroundWindow` actually worked.
- **Buttons that weren't drawn could be tapped** (touch/pen press before the hover frame). A press on a hidden button now counts as a press on its card.
- The window region is unioned with the previous frame's, so a moving card isn't clipped while the swap chain shows a late frame; the notice gets 3 px of room for its r 8 corners.
- Thumbnails of cards evicted before they were ever drawn, ghosts of a hidden empty stack and a hidden notice-only window are now freed; a DPI change rebuilds once, not twice.
- `busy` now lasts until both a card's save and its copy-encode are done.
- Saving from the thumb marks the card showing the same image as saved (and the other way round), so the second button reads `open` instead of writing a duplicate file.

Left as they are: `open` failing for another reason than a missing file is only logged; a shot whose PNG is encoded later is under-counted by its PNG size.

## Commands and results

Worktree-local target (`CARGO_TARGET_DIR=…/stage-7c-snap-stack/target`), env from `dj-build-env.sh`:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **541 passed, 0 failed** (daijobu 80, dj-core 317, dj-render 54, dj-sweep 35, dj-vitals 26, dj-win 21, dj-capture 8). Base before 7c: 393. |
| `cargo build --release` | clean, `target/release/daijobu.exe` (10.9 MB) |

New tests: core 69 (`snap_stack`) + 3 config, render 21 (CPU raster), win 12, store 19 + 3 snap, controller 19, 7b handle 1.

## How to run

```sh
source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh
export CARGO_TARGET_DIR=C:/Users/UseR/orca/workspaces/daijobu-island/stage-7c-snap-stack/target
cargo build --release
# close the running island first (WM_CLOSE to DaijobuIslandWindow, not a kill, so the AppBar strip goes)
$CARGO_TARGET_DIR/release/daijobu.exe
```

Optional: `[snap] preview_seconds = 20` in `~/.daijobu/config.toml`.

## Coordinates for driving the live checks

The stack window is class `DaijobuSnapPreviews`. Its input (`WM_MOUSEMOVE`, `WM_LBUTTONDOWN/UP`, `WM_KEYDOWN`) is in **client px = logical px × scale** (scale = monitor DPI / 96). Window size 692×188 logical. Placement: `x = island_centre − w/2`, `y = island_bottom + round(12·s) − round(16·s)`, clamped to the work area (island bottom = 6 + pill height logical; 78 with the snap thumb up).

Logical rects in the window (x, y, w, h):

- Cards, newest first: `x0 = (692 − (n·124 + (n−1)·10)) / 2` → n=1: 284, 2: 217, 3: 150, 4: 83, 5: 16; card i at `(x0 + 134·i, 16, 124, 80)`.
- Inside a card (relative to its top-left): check `(6, 6, 18, 18)`, ring/PIN `(103, 5, 16, 16)`, status `(28, 6, 71, 18)`; buttons at y 59, h 16: copy `(5, 27)`, save/open `(35, 27)`, pin/unpin `(65, 33)`, ✕ `(101, 18)` (x, w).
- Bar (only with a selection): `(74, 106, 544, 32)`; count `(82, 106, 84, 32)`; buttons at y 110, h 24: Select all `x 172 w 84`, Clear selection `x 262 w 108`, Save selected `x 376 w 104`, Dismiss selected `x 486 w 124`.
- Notice `Preview stack full`: `(262, 106, 168, 24)`, or `(262, 148, 168, 24)` under the bar.

Keyboard: tray menu → **Focus Snap previews** (greyed without cards). Without a mouse, Win+B → arrows to the icon → Shift+F10 / the menu key should open the same menu; that path is unverified (the tray icon doesn't set `NOTIFYICON_VERSION_4`). Then ←/→ move, Tab/Shift+Tab cycle cards then bar buttons, Space/Enter toggle (Enter presses a bar button), C copy, S save/open, P pin, Delete dismiss, Ctrl+A select all, Esc releases (the focus ring goes; the window keeps the keyboard until you click elsewhere).

## Known issues and decisions

- **No drop shadow, focus ring inside the card.** The window region that makes the gaps click-through also clips what the window shows (as with the island), so anything outside a card would either be cut off or take clicks. The selection outline (2 px, 2 px off) fits in the +5 px region of a selected card.
- **No fan and no hover lift.** One row, always spread out (the contract makes the fan optional).
- **Esc doesn't hand the keyboard back** to the previous app; the foreground before the tray menu is the tray itself, so there is nothing useful to restore. The focus ring and its pause go away at once.
- A notice can show on an otherwise empty window (an oversized shot with no cards).
- 7b review leftovers (not changed here): a pixelate that can't be drawn shows solid black in the editor and Ctrl+C/S then refuse with only a log line; the error branch of `Snapper::end` has no test.
- Win32 lost-capture recovery for the **overlay** (7b) is still unverified; the stack window turns `WM_CAPTURECHANGED`/`WM_CANCELMODE` into a leave.

## Not verified (needs the live test)

Everything that needs a desktop, a GPU or the real clipboard:

1. Three quick annotated captures: clipboard/PNG correct, cards appear newest-first without stealing focus, **the stack is not in the next capture** (hide + `DwmFlush` before the freeze).
2. Countdown on screen: one expires at 10 active seconds; hover/select/pin pause and resume from the remainder; hidden time (Ctrl+Alt+H) doesn't count.
3. Copy an older card (paste in Paint), save one, select all + Save selected, `open` reveals, delete a saved file then `open` → `file missing` → save again; expiry/dismiss leave files and clipboard alone.
4. Five pinned cards + a sixth capture → cards survive, the new shot still copies/saves and has its thumb, `Preview stack full` shows.
5. Tray keyboard access, click-through in the gaps and around the cards, all four themes, notch/pill, expanded panels (stack moves down), OSD/timer, 100 %/150 % DPI, a second monitor, display changes, and idle CPU ≈ 0 once the stack is empty or fully paused.

Also first-run checks: the GPU path of `PreviewSurface` (only the CPU paint is tested), that `SetForegroundWindow` on the `WS_EX_NOACTIVATE` window really delivers keys, and that `WM_MOUSELEAVE` fires when the pointer moves from a card into a click-through gap.
