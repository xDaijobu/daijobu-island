# Stage 7b brief: Snap annotations (for the worktree agent)

Your worktree starts from `master`, which has Stages 1–7a and S (shell, Music, OSD, Vitals + Sweep, Timer, Snap core, Settings). You are adding a **Lightshot-style annotation editor** to the Snap overlay. After release, the selection stays and a toolbar appears. The user draws, then copies or saves. Pin, OCR (now 7d) and the Snap stack (7c) are **out of scope**.

## HEADLESS (overrides everything)
- Never run `daijobu.exe`. Create no windows, hooks or input, take no screenshots of the real screen, and write nothing to the real clipboard.
- Test with the existing fakes (`Screen`, `Clipboard` in `app/src/snap.rs`) and synthetic bitmaps. Drawing tests go on Skia CPU raster surfaces.
- Run `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh` before cargo. Verify only with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and `cargo build --release`.
- In the report, mark every live check "not verified, user to test".

## Read first
1. `AGENTS.md`.
2. **Contract:** `task/progress.md`, section **"Stage 7b: Snap annotations"**. Build exactly that. Don't edit `progress.md`.
3. `task/stage-7a-report.md`, especially "Semantics decided here" and "Ids". Then read the Snap code end to end:
   - `crates/dj-core/src/snap.rs`
   - `crates/dj-render/src/snap.rs`
   - `crates/dj-win/src/overlay.rs`
   - `app/src/snap.rs`
   - the snap part of `app/src/app.rs` (`begin_snap`, `snap_input`, `end_snap`, the `HOTKEY_SNAP_ESC` handling)
4. `docs/plan.html`: the Snap card and the `.snapov .tools` toolbar mockup (look and order).
5. `task/stage-s-report.md` and `app/src/storage.rs` for how config is written back without losing other sections.

## Design guidance
**Pick:** a retained shape model with **snapshot undo**. History is a stack of whole `Vec<Shape>` copies, capped at 100. Shapes are small, so this is cheap, and it avoids per-command inverse bugs. Don't build a command/inverse system, and don't burn strokes into a bitmap.

### `dj-core` (pure, `forbid(unsafe)`, no Skia)
Put this in a new module (e.g. `snap_edit.rs`). `snap.rs` is already ~500 lines.
- **Data:**
  - `Tool` (Select, Pen, Line, Arrow, Rect, Marker, Text, Pixelate, Step)
  - `Size` (S/M/L)
  - `Rgba`
  - `Shape`: an enum with its geometry in desktop px (f32 is fine), its colour and its size
  - `History`
- **The editor state machine** (one struct, e.g. `Editor`):
  - phase, tool, colour, size, selection rect
  - the drag in progress: drawing / moving a shape / moving the selection / resizing by handle `0..8` / picker drag
  - the selected shape index, and the text being edited (string + caret in chars)
  - inputs: press, motion, release, double-click, key, char, wheel, toolbar/picker hits
  - output: an `Outcome` such as `Redraw` / `Copy` / `Save` / `Cancel` / `Nothing`, plus the cursor kind to show

  Keep the 7a `Selector` for the selecting phase. The editor takes over from its release.
- **Text measurement:** add a `TextMeasure` trait in dj-core (line width and line height for a size). dj-render implements it with Skia's Geist Medium, and the tests use a fixed-advance fake. Text bounds (for hit tests) and the caret x both come from it.
- **Pure geometry with tests:**
  - shape hit testing (distance to segment/polyline with tolerance, filled bounds for text/step/pixelate)
  - the 8 handles and their resize maths (flip, clamp, minimum)
  - toolbar and picker layout (below / above / inside / two rows, clamped to a monitor rect) and their hit testing
  - HSV ↔ RGB
  - the step "next number"
  - pen point thinning (drop points closer than ~1.5 px)
- **Config:** `AnnotateConfig` under `SnapConfig` as `[snap.annotate]`, with `#[serde(default)]` and validated accessors, like the existing `SnapConfig::hotkey()`.

### `dj-render`
- **One markup painter**, e.g. `draw_markup(canvas, shapes, base, clip, scale_info)`. Both the overlay and the output use it. That's how "the output is what you see" is kept.
- **Pixelate** reads the frozen screen, not the canvas:
  - downscale the source region with nearest sampling (block size from the contract), then draw it scaled up with nearest sampling
  - anchor the grid at the box's top-left in desktop px, so the overlay and the output match
  - cache the small image per shape and rebuild it only when the shape's rect changes
- **Output:** `render_output(frames, rect, shapes, …) -> Bitmap`.
  - Draw on a CPU raster surface the size of the crop, then convert to the opaque BGRA `Bitmap` that `dj-capture` already encodes.
  - With an empty shape list, return the 7a crop unchanged (byte-identical; test it).
- **Overlay scene:** add the editing phase:
  - selection outline (solid while editing), 8 handle squares, size label, toolbar, picker popover, the selected shape's dashed box, the text caret
  - no loupe while editing
- **Icons** for the toolbar as vector paths, in the same style as `icons.rs`. Licences: draw your own and copy nothing from Lightshot, ShareX (GPL) or icon packs.

### `dj-win` (`overlay.rs`)
- **Input:**
  - Add `CS_DBLCLKS` to the class and a `DoubleClick` input.
  - Add `Char(char)` from `WM_CHAR`. The message loop already calls `TranslateMessage`. Join surrogate pairs and ignore control chars below 0x20, except what the editor needs.
  - Add `Key { vk, ctrl, shift }` for everything the editor needs: Esc, Enter, Delete, Backspace, arrows, Home/End, the tool letters, and C/S/Z/Y with Ctrl.
  - Add `Wheel(delta)`.
- **Right click:** make it a `RightPress` input. The app decides: cancel while selecting, nothing while editing.
- **Cursor:** a settable cursor kind (cross, I-beam, size NWSE/NESW/WE/NS, move, arrow), read in `WM_SETCURSOR`. Use a static atomic, because the window proc can't borrow the app.

### `app`
- **Snapper:** a release now enters editing instead of `finish`. Copy and Save render the output through dj-render, then reuse the 7a copy/save path (PNG + DIB in one clipboard session, the `create_new` save, the thumb).
- **Esc:** `HOTKEY_SNAP_ESC` (id 4) must go through the editor's Esc logic. That means committing the text, then dropping the shape selection, then cancelling. It must not call `end_snap(false)` directly.
- **Prefs:** write them on close, only if they changed, with the existing config writer.
- **Ids:** no new timers or hotkeys should be needed. If you need one, `TIMER_*` 1–8 and hotkeys 1–5 are taken.

## Watch out
- **Desktop coordinates can be negative** (monitors left of or above the primary). Convert overlay client px with the overlay origin, as 7a does.
- **Mixed DPI:** pick the scale from the monitor that holds most of the selection. Use that scale for widths, font sizes and the toolbar.
- **Big virtual desktops:**
  - the overlay redraws the whole frame on each input (as 7a); keep per-frame work proportional to the shapes, not the screen
  - the pixelate cache matters here
- **Pen smoothing must not move a click-dot.** A single point draws a filled circle.
- **Foreground:** if the overlay has no keyboard (Windows refused `SetForegroundWindow`), only the Esc hotkey works. The toolbar's copy/save/✕ buttons must work with the mouse alone.

## Finish
Commit in small steps on your branch and don't push. Write `task/stage-7b-report.md`. It must have:
- an honest per-item checklist against the contract
- how to run
- known issues
- what was not verified
- a short "live test script" for the user, one line per tool and flow

Then stop.
