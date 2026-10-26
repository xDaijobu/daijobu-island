# Stage 7b report: Snap annotations

Built on `stage-7b-work`, starting from `c43b044` (contract and brief). The work was headless. The exe was never run, the real screen was never captured, the real clipboard was never written, and no windows, hooks or input were created in tests. All live and visual behaviour is **not verified, user to test**.

## Verdict

Every scope item in the contract is implemented, and every test the contract lists exists and passes. The editor is a retained shape model with snapshot undo, in a new pure module `dj-core::snap_edit`. The overlay and the copied/saved image are drawn by the same painter, `dj-render::markup::draw_markup`. The image is rendered on a CPU surface the size of the crop; with no markup it returns the 7a crop unchanged. No new crates, dependencies, timers, hotkeys or assets were added. A few small semantic choices the contract leaves open are listed below.

Verification on `stage-7b-work` (MSVC 14.51, Windows SDK 10.0.26100, `dj-build-env.sh`):

- `cargo fmt --all`: clean (`--check` passes).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: **360 passed**, 0 failed. By crate: dj-core 232, dj-sweep 35, app 28, dj-vitals 26, dj-render 23, dj-capture 8, dj-win 8. Stage S ended at 303, so this stage adds 57: dj-core +44, dj-render +7, app +5 (the 7a app tests were rewritten for the new flow), dj-win +1.
- `cargo build --release`: compiles and links (`target/release/deps/daijobu.exe`, 10.7 MB, 2026-10-26 11:10). Cargo's last step, the copy to `target/release/daijobu.exe`, failed with "Access is denied". A running `daijobu.exe` (PID 38724, window "Daijobu Island", from a build at 11:03 that is not this branch) holds the file. I left it running. Quit it and run `cargo build --release` again, or run `target/release/deps/daijobu.exe` directly. Note that another worktree can overwrite `deps/daijobu.exe` in the shared target dir; see "Shared target dir" below.

## Checklist (contract)

| Item | State | Notes |
|---|---|---|
| **Flow.** Selecting is unchanged from 7a; Esc or a right click cancels | Done | `App::select_input` keeps the 7a logic (hover, drag, window click, arrow nudge, loupe). Esc (overlay key or the `HOTKEY_SNAP_ESC` hotkey) and `RightPress` end the snap with `End::Cancel`. |
| … editing starts on release (drag ≥ 2×2, or a window/monitor click); selection stays with 8 handles and its size label, toolbar appears, loupe hides; the clipboard is untouched; right click does nothing | Done | `Snapper::start_editing` runs `Selector::release` and builds the `Editor`. A too-small selection cancels (as 7a). `SnapScene::edit` switches the overlay to the editing scene, which draws no loupe. Nothing is copied before a finish (test `release_enters_editing_and_the_clipboard_is_untouched`). A right click while editing is dropped in `edit_input`. |
| … finish: Ctrl+C, Enter or copy renders with markup, copies PNG + `CF_DIB`, closes, shows the thumb | Done | `Outcome::Copy` → `end_snap(End::Copy)`. The overlay closes first, then `Snapper::end` renders through `dj_render::render_output` and reuses the 7a `copy_shot` (one clipboard session) and thumb. Test `ctrl_c_copies_the_annotated_png_and_dib_and_shows_the_thumb`. |
| … Ctrl+S or save: the same plus a save (7a naming); thumb opens as `saved`, its button reads `open` | Done | `End::Save { dir, stamp }` copies, then calls the 7a `Snapper::save` (`create_new`, `-2`…), so `shot.saved` is set and the thumb's button reads `open`. Test `ctrl_s_saves_and_copies`. |
| … Esc or ✕ cancels, nothing copied or saved, no confirm | Done | Test `esc_while_editing_copies_nothing`. |
| … the thumb's copy/save work as in 7a, on the annotated image | Done | `Shot::bitmap` is the annotated render, and the thumb's buttons use the unchanged 7a paths. |
| **Toolbar**: pill colour, r 12, 26 px buttons r 8, active in accent; contents in order | Done | `snap_edit::toolbar` (layout) and `dj-render::snap::draw_toolbar` (paint). Order: select, pen, line, arrow, rect, marker, text, pixelate, step · 8 swatches + custom · S M L · undo redo · copy save ✕, with a 1 px rule between groups. The active tool and size are filled with the accent. The active swatch gets an accent ring, because an accent fill would hide the swatch's colour. Undo and redo dim when there is nothing to undo or redo. A hovered button sits on surface2. Icons are new stroke paths in `icons::draw_toolbar_icon`, drawn for this project (nothing copied). |
| … placement: under the selection left-aligned, else above, else inside the bottom edge; clamped to the monitor holding most of the selection; two rows if too wide | Done | Tested: below, above, inside, clamped at the right edge, on a negative 150% monitor, and the two-row wrap. The wrap splits between groups, at the split that gives the narrowest bar, and keeps the order. |
| **Keys**: V P L A R H T X N except while typing; wheel steps S/M/L; Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z | Done | `snap_edit::key_action` (pure, tested) maps keys to actions. `KeyCode::from_vk` maps virtual keys. |
| … while typing: Enter is a new line, Backspace/Delete edit, Ctrl+C/S commit then finish, Esc commits and stays, a second Esc cancels | Done | Tests `text_typing_editing_keys_and_commit` and `ctrl_z_while_typing_commits_then_undoes_and_ctrl_c_commits_then_copies`. |
| **Tools**: physical desktop px; sizes logical × scale of the selection's main monitor; drawing starts only inside the selection; markup clipped on screen and in the output | Done | Each shape stores the scale it was drawn at. `main_display` picks the monitor with the largest overlap. Presses outside the selection draw nothing (test). `draw_markup` clips to the selection. In the output the surface is the crop, so that is the clip (test `markup_is_clipped_to_the_selection`). |
| … pen: round caps/joins, smoothed, a click leaves a dot, 2/4/8 | Done | Points closer than 1.5 px are thinned in dj-core. The painter smooths through midpoints (quadratic curves), so stored points never move. One point is drawn as a filled circle. |
| … line, arrow: press to release, filled head scaled with width, < 3 px dropped | Done | The head is 3w + 6·scale long and 0.6 of that half-wide, shrunk to fit short arrows (`arrow_geom`, shared by the painter and the hit test). |
| … rect: outline only | Done | Mitred corners. A box under 3 px both ways is dropped. |
| … marker: pen × 4 width, 40% alpha, never darkens where it crosses itself | Done | Drawn opaque into a layer that is blended at 40% (test checks a crossing equals a single pass). |
| … text: caret on click; typing, Backspace, Delete, ←/→, Home/End, Enter; commits on click elsewhere, Esc or tool change; empty dropped; Geist Medium 14/20/28; no blink; `WM_CHAR` with surrogates, no IME | Done | Caret positions are in chars. The first line is centred on the click. `TextMetrics` (Geist Medium) measures and draws, and the tests use a fixed-advance fake. `dj_win::overlay::join_utf16` joins surrogate pairs (tested). ↑/↓ are not handled; the contract doesn't list them. |
| … pixelate: blocks of max(12, 10/14/20 × scale) on a grid at the box's top-left, from the frozen screen, covering earlier markup | Done | `snap_edit::pixelate_cells` samples each block's centre pixel from the frozen frames (pure, tested). The painter draws it scaled up with nearest sampling at whole-pixel offsets, so the overlay and the output match. The small image is cached per (box, block size) and rebuilt only when the box changes. Test `pixelate_gives_uniform_blocks_anchored_at_the_box`. |
| … step: filled circle 20/28/38 with largest + 1, black or white by contrast | Done | Placed on press. After an undo the number comes back (test). The ink is `step_ink`, a WCAG contrast pick. Red, blue, yellow and white get black; purple and black get white. |
| **Colour**: 8 swatches (incl. accent) + custom swatch showing the last custom colour | Done | The custom swatch shows its colour inside a thin hue ring. |
| … HSV picker: SV square, hue strip, preview, hex; live drag; click outside closes | Done | `snap_edit::picker` (layout, hits, HSV mapping) and `draw_picker`. It opens below the toolbar if there is room, else above, aligned with the custom swatch and clamped to the monitor. A click outside both the picker and the toolbar only closes it. Esc closes it too (see semantics). |
| **Select tool** (V, or a click on the active tool) | Done | Clicking the active drawing tool goes back to select. Keyboard letters only pick a tool and never toggle. |
| … selection: handles resize, empty spot moves; clamped, 2×2 min, flips; not undoable | Done | `chrome::resize` / `move_rect` (tested: flip, clamp, minimum, negative desktops). |
| … shapes: click selects (topmost, ~6 px tolerance), dashed box, drag moves, Delete/Backspace removes, swatch/size restyles; no resize/rotate | Done | Only shapes inside the selection can be hit. Pixelate ignores colour restyles, so no empty undo step is made. |
| … text: double-click re-edits; the text tool also edits an existing text on click | Done | The caret goes where you click. |
| … markup stays put when the selection moves; outside is clipped | Done | Tested. |
| **Undo/redo**: snapshot history, cap 100; one step each for add, move, delete, restyle, one text session, one picker drag on a selected shape; new step clears redo; Ctrl+Z while typing commits then undoes; selection/tool/colour/size not in history | Done | `model::History`. All listed cases are tested, including the cap and redo being cleared. |
| **Prefs** in `[snap.annotate]` (`tool`, `color`, `custom`, `size`; defaults arrow, red, `#FF7AC6`, `M`; invalid → default; written on close only if changed, keeping other sections) | Done | `config::AnnotateConfig` uses `#[serde(default)]` and per-key validated accessors (tested). `Snapper::end` hands back changed prefs once. `end_snap` puts them in `config.snap.annotate` and calls the existing `storage::save_config`. A cancel also keeps the choice, and so does quitting mid-snap. `color` is a swatch name (`red` … `black`, `accent`) or `custom`. |
| **Output is what you see**: same markup code, CPU surface the size of the crop, physical px; no handles/outline/toolbar/dim/loupe; no markup = byte-identical crop | Done | `render_output` draws only the crop plus `draw_markup`. Tests: byte-identical with no shapes, and pixels away from a shape unchanged. If drawing fails, nothing is copied (never the bare crop, so pixelated secrets can't leak), and the reason goes to the log. |
| **Cursors**: crosshair selecting and with drawing tools; I-beam for text; move/size over shapes and handles with select; arrow over toolbar and picker | Done | `Editor::cursor_kind` → `dj_win::overlay::set_cursor` (a static atomic, read in `WM_SETCURSOR`). The cursor also changes at once on a key, not only on the next mouse move. Outside the selection the editing cursor is the arrow (see semantics). |
| **Idle CPU ≈ 0**: draws only on input, no caret blink or animation; frames live until the overlay closes | Done (by design) | No timers. Mouse motion redraws only while dragging or when the hovered toolbar button changes. **Not verified live.** |
| **Tests** | Done | See below. |

### Tests added (57)

- **dj-core `snap_edit`** (43):
  - model (11): tools, sizes and names; colours and choices; HSV round trip; text layout through the fake measurer; stroke tolerance and fills; topmost hit; arrow head; pen thinning and step numbers; bounds and translate; pixelate blocks and floor; history (undo, redo, cap, redo cleared).
  - chrome (8): main display; handles; resize (flip, clamp, minimum, negative desktop); selection move; toolbar below, above, inside and clamped; two-row wrap; toolbar hits; picker placement and hits.
  - editor (24): key codes and actions (letters off while typing); prefs in and out; pen, marker and dot; short drags dropped for line, arrow, rect and pixelate; drawing only inside; step numbers after undo and delete; per-monitor scale; select, move (one step) and delete; topmost select; restyle (one step each, wheel); typing, editing keys and commit; empty text dropped and click-elsewhere commit; re-edit is one step; Ctrl+Z, Ctrl+C and Ctrl+S while typing; colour and size while typing; the toolbar beats the typed text where they overlap; selection resize, move and abort without undo; active-tool click; picker (live, one step per drag, Esc order); picker with nothing selected; copy, save and ✕ by mouse; history cap and redo cleared; Esc aborts a drag; cursors and hover redraw.
- **dj-core `config`** (1): `[snap.annotate]` defaults, reading, invalid values one by one, and a round trip that keeps the other sections.
- **dj-render** (7): no markup equals the crop byte for byte; a rect colours its border and leaves the inside alone; pixelate gives uniform blocks anchored at the box; markup is clipped to the selection; every shape draws and the marker doesn't darken where it crosses; Geist metrics; the editing scene on a CPU surface (frame inside, dim outside, handle, toolbar body, active tool in accent, no loupe).
- **app** (5 new, 11 kept and rewritten for the new flow): release enters editing with the clipboard untouched; Ctrl+C copies the annotated PNG + DIB and makes the thumb's shot; Ctrl+S saves and copies; Esc while editing copies nothing (also after typing); prefs come back once, only when changed.
- **dj-win** (1): `WM_CHAR` surrogate joining and control-char filtering.

## Semantics decided here (the contract leaves them open)

- **Esc order**: abort a drag in progress, then commit the text, then close the picker, then drop the shape selection, then cancel. The contract lists text → shape selection → cancel, and I added the picker step.
- **Keys during a drag** (mouse held while drawing, moving or resizing) are ignored, except Esc, which puts things back as they were at the press. The wheel is ignored during a drag too.
- **Toolbar clicks while typing**: a swatch, the custom swatch, a size or a picker drag restyles the text being typed, and typing goes on (it is part of the same undo step). A tool, undo, redo, copy or save commits the text first.
- **Restyle from the wheel**: the wheel acts like clicking a size button, so it also restyles the selected shape (one step per notch that changes it).
- **Handles** are drawn in every tool but only work with the select tool, as the contract places them under "Select tool".
- **Cursor outside the selection** while editing is the arrow, because drawing can't start there. Inside, the cursor follows the tool. During a drag it stays on the drag's cursor (crosshair while drawing, even outside).
- **Text placement**: a click puts the caret there, with the first line's box centred on the click. Starting a new text and leaving it empty adds no undo step.
- **Rect and pixelate drops**: a box under 3 px both ways is dropped, and so is a pixelate box with no width or height.
- **Picker closing click** is used up (it doesn't also draw). A click on the toolbar while the picker is open works, and closes the picker unless it is the custom swatch, which toggles it.
- **A failed render** copies nothing (logged) rather than the bare crop, because the crop could show what was pixelated.
- **Ctrl detection** ignores Ctrl while Alt is down, so AltGr characters (Ctrl+Alt) never trigger Ctrl+S, Ctrl+Z and so on.
- **Alt+F4 on the overlay** (`WM_CLOSE`) now ends the snap as a cancel. Before, the window could be destroyed under the app.

## Deviations

1. **`SnapInput` was reshaped.** `Arrow` and `Cancel` are gone. The overlay now reports raw `Key { vk, ctrl, shift }`, `RightPress`, `DoubleClick`, `Char`, `Wheel` and `Close`, and the app decides per phase (arrows still nudge while selecting).
2. **The toolbar has 26 buttons** (9 tools, 9 swatches, 3 sizes, 2 history, 3 actions), so one row is 806 logical px. On a monitor narrower than that it wraps to two rows, as the contract allows. Below 531 logical px even two rows overflow and are clamped at the monitor's left edge.
3. **↑/↓ in a text** and **text selection** (shift-arrows, mouse drag) are not implemented. The contract doesn't list them.
4. **`[snap.annotate]` values are strings.** A wrong *type* (e.g. `size = 3`) makes the whole config fall back to defaults, as for every other section (`Config::parse` is lenient as a whole). A wrong *value* falls back per key.

## Shared target dir (heads-up for the coordinator)

`dj-build-env.sh` points every worktree at one `CARGO_TARGET_DIR`. Cargo hashes workspace members by their path relative to the workspace root, so `dj-core` in `stage-7b-work` and in `stage-n-notch` use the **same** artifact and fingerprint. During this stage, a `stage-n-notch` build made my `cargo build` reuse its `dj-core`. The result was a spurious "could not find `snap_edit` in `dj_core`". It can also go silently the other way: one worktree can link the other's crate when its own sources are older than the last build. My workaround was to `touch` every crate root (`crates/*/src/lib.rs`, `app/src/main.rs`) before each cargo command, which forces a rebuild from this worktree. All the numbers above come from builds run that way. The coordinator's re-check should do the same, or use a per-worktree target dir.

## How to run (this machine)

```sh
source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh
cd C:/Users/UseR/orca/workspaces/daijobu-island/stage-7b-work
touch crates/*/src/lib.rs app/src/main.rs   # shared target dir, see above
cargo build --release                       # target/release/daijobu.exe
```

Quit the running `daijobu.exe` first, or the final copy fails (see Verification). The prefs land in `~/.daijobu/config.toml`:

```toml
[snap.annotate]
tool = "arrow"      # select, pen, line, arrow, rect, marker, text, pixelate, step
color = "red"       # red, yellow, green, blue, purple, white, black, accent, custom
custom = "#FF7AC6"
size = "M"          # S, M, L
```

To drive the overlay without the real mouse (as in 7a), post client-px mouse messages to the `DaijobuSnapOverlay` window. After the release the toolbar sits 8 px under the selection's bottom-left (or above it, or inside it), 5 px padding, 26 px buttons 4 px apart, with 9 px between groups. With a single 100% monitor at the origin, button *i*'s left edge is `sel.x + 5 + 30·i + 5·g`, where *g* is how many group rules come before it: 0 for the tools (i 0–8), 1 for the swatches (9–17), 2 for the sizes (18–20), 3 for undo/redo (21–22) and 4 for copy/save/✕ (23–25). Its top is `sel.bottom + 13`. Keys need the overlay to have the keyboard; Esc always works through the hotkey.

## Live test script (not run)

1. **Release**: PrtSc, drag a region, release. The selection stays with 8 handles and a size label, the toolbar appears below it (the arrow tool lit), there is no loupe, and the clipboard still holds what it held before (paste somewhere to check).
2. **Pen**: P, scribble, and click once without moving. You get a smooth stroke and a dot.
3. **Line / arrow**: L, drag; A, drag. A tiny drag (under 3 px) leaves nothing, and the arrow head grows with S/M/L.
4. **Rect**: R, drag. An outline only, with sharp corners.
5. **Marker**: H, drag a stroke that crosses itself. It is translucent and no darker at the crossing.
6. **Text**: T, click, type `hello`, Enter, type `é😀`. Then try ← → Home End Backspace Delete. Esc commits, and a second Esc would cancel (don't yet). Letters typed while typing don't switch tools.
7. **Pixelate**: X, drag over some text or a password. You see square blocks, and they cover any earlier markup under them.
8. **Step**: N, click three times (1 2 3). Ctrl+Z, then click again: it is 3 again. The number is black or white for contrast.
9. **Colour**: click a swatch, then draw. Click the custom swatch and drag in the square and the strip: the preview and hex update live. Click outside to close the picker.
10. **Sizes**: click S/M/L, or use the wheel; new shapes use the size.
11. **Select**: V (or click the lit tool). Click a shape: it gets a dashed box. Drag it, then Delete. Select another and click a swatch and L to restyle it.
12. **Re-edit text**: with V, double-click a text and type; with T, click a text and type.
13. **Undo/redo**: Ctrl+Z through everything above, then Ctrl+Y / Ctrl+Shift+Z back. The buttons dim when there's nothing left.
14. **Selection**: with V, drag a corner and an edge handle (past the opposite edge flips it), and drag an empty spot to move it. Markup stays put on screen and is clipped at the edge. Ctrl+Z doesn't undo this.
15. **Cursors**: crosshair with drawing tools, I-beam with text, move/size over shapes and handles with V, arrow over the toolbar and the picker.
16. **Copy**: Ctrl+C (or Enter, or the copy button). The overlay closes and the thumb shows `✓ copied`. Paste into Paint: the image matches the overlay inside the selection, markup included, with no handles, outline or toolbar.
17. **Save**: snap again, mark it up, Ctrl+S (or the save button). The thumb says `saved` and its button reads `open`, and `Pictures\Daijobu\snap-….png` is the same image.
18. **Cancel**: snap, draw, press Esc (twice if typing) or click ✕. Nothing is copied, and right click does nothing while editing.
19. **Prefs**: change the tool, colour and size, then close. The next snap starts with them, and after a restart they are still there (`[snap.annotate]` in the config, other sections intact).
20. **No keyboard**: start a snap by a posted hotkey (no foreground). The copy/save/✕ buttons still work with the mouse, and Esc still cancels.
21. **Multi-monitor / mixed DPI**: select across two monitors with different scales. Widths, text and the toolbar follow the monitor holding most of the selection, and the toolbar stays on it.
22. **Idle CPU** with the editor open and the mouse still: about 0%.

## Not verified (user to test)

- Everything live and visual: the toolbar's look and icons, the picker, the handles, the caret, the cursors, keyboard focus on the overlay, typing through `WM_CHAR` (including AltGr layouts and emoji), double-click timing, wheel steps, the pasted and saved images, the thumb, prefs persistence across a restart, multi-monitor and mixed DPI, idle CPU.
- That the GPU overlay and the CPU output look the same in practice. They share the same code and pixelate path, but anti-aliasing of strokes and text can differ by a shade between Skia's GPU and CPU backends.
- Overlay frame pacing while drawing fast with a high-rate mouse (one full-frame redraw per input, as in 7a).

## Known issues and risks

- **Big crops are rendered and encoded on the UI thread** (as in 7a). Markup adds one CPU draw of the crop's size before the PNG encode.
- **Text input needs the keyboard.** If Windows refused `SetForegroundWindow`, typing, letters and Ctrl shortcuts don't reach the overlay. Only the Esc hotkey and the mouse work (the toolbar covers copy, save and ✕).
- **Global hotkeys still win.** If `[snap] hotkey` / `alt_hotkey` is Ctrl+Shift+S or Ctrl+Alt+S, that combination never reaches the editor while the overlay is up (plain Ctrl+S still saves).
- **Very narrow monitors** (under 531 logical px) clip the two-row toolbar at the monitor's edge.
- **The shared target dir** can mix crates between worktrees (see above).

## Follow-up verification on master (2026-10-26)

Base: `master` at `f7f9215` (includes 7b, N and N2), plus the uncommitted fixes below. The original verdict/results above describe the earlier stage delivery, not live verification. This follow-up is also **headless**: no app launch, GUI input, real screen capture or real clipboard writes. Desktop-control tools were unavailable, so the live test script remains pending.

### Confirmed regressions fixed

- **Pixelate raster failure:** a failed pixelate image creation previously returned `None`, which the shared painter silently skipped; output could still be returned as successful with underlying pixels exposed. `draw_shape`/`draw_markup` now return an error while restoring the canvas/cache state. The overlay covers the failed pixelate rectangle with opaque black, clipped to the selection, and still paints other shapes. Output propagates the error to the existing `Snapper::end` failure path instead of copying/saving an unredacted result. Normal pixelate rendering is unchanged.
- **Double-click through toolbar/picker:** the Select tool's committed-text fast path now runs only with the picker closed and pointer outside the entire toolbar. Controls, padding and the picker-dismissal click go through the existing `press()` priority rules; normal text re-edit still works.

Five new deterministic regressions were run against the old behavior and failed, then passed after the fixes:

```text
pixelate_failure_refuses_output
pixelate_failure_covers_preview_and_restores_canvas
double_click_on_toolbar_does_not_edit_committed_text
double_click_on_picker_does_not_edit_committed_text
double_click_outside_picker_closes_it_before_re_editing_text
```

Pixelate failure is injected at the cache/raster boundary in test builds, not reproduced by exhausting real machine memory. The output test exercises the same render implementation as `render_output`; the preview test checks opaque coverage, clipping and canvas restoration. Existing fake-clipboard tests cover Ctrl+C, Ctrl+S, annotated PNG/DIB output, cancellation and text commit/re-edit. They do not exercise native keyboard focus or real clipboard consumers.

### Fresh validation

Used MSVC/SDK configuration from `dj-build-env.sh`, overriding the shared target with this checkout's **`target/verify-7b`** and `CARGO_BUILD_JOBS=2`. No shared-target cleanup and no running process was stopped.

```sh
source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh
export CARGO_TARGET_DIR=C:/Users/UseR/orca/projects/daijobu-island/target/verify-7b
export CARGO_BUILD_JOBS=2
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

- fmt/check and clippy: passed.
- Workspace tests: **393 passed, 0 failed**. dj-core 244, dj-render 33, app 39, dj-capture 8, dj-win 8, dj-sweep 35, dj-vitals 26, dj-music 0. Five more than the merged N2 baseline (388). Test binaries were also run directly to confirm counts because command output summarized Cargo results.
- Release build: passed, including final executable at `target/verify-7b/release/daijobu.exe`. Not launched; the ordinary shared `target/release/daijobu.exe` was not replaced and is not this verification artifact.
- `git diff --check`: passed. No new dependencies, unsafe blocks, runtime timers or 7c implementation. Changes remain uncommitted for review.

### Review / live checks still required

- Run the existing 22-step live script, especially Ctrl+C after typing, double-clicks over toolbar/picker text overlaps, pixelate output pasted into Paint, and Ctrl+S output. No live or GPU correctness claim is made by these CPU/unit tests.
- Potential pre-existing lost-capture case remains unverified: overlay handling has no `WM_CAPTURECHANGED`/`WM_CANCELMODE` recovery, while active editor drags intentionally suppress non-Esc keys. Losing a mouse release could leave Ctrl+C ignored until Esc aborts the drag. Confirm with a bounded real-input test before choosing a platform-level fix; this session did not change input capture semantics.
- Stage 7c planning is in `task/progress.md` and `task/stage-7c-brief.md`; OLE is deferred to 7c-2. This is not a claim that 7c has started or that 7b live acceptance passed.

### Startup-only smoke test (2026-10-26, subsequent user request)

The user subsequently requested an app launch. Started `target/verify-7b/release/daijobu.exe`, PID 48364, and left it running for user inspection. Process inspection found a `Daijobu Island` main window and `Responding = True`. Two 5-second samples consumed 31.25 ms then 0 ms CPU; final working set was approximately 117.3 MiB. No crash was observed during those checks.

The only new app-log line was `snap: Ctrl+Space is not available: Hot key is already registered.` The conflicting owner was not identified and no other application's hotkey/settings were changed. This verifies startup/process responsiveness only: no screenshots or automated GUI actions were available, and capture, typing, double-click, copy/paste and saved-image behavior remain unverified live.
