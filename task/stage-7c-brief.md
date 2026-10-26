# Stage 7c brief: Snap stack (preview UI only)

Status: built 2026-10-26 on `xDaijobu/stage-7c-snap-stack` (see [`stage-7c-report.md`](stage-7c-report.md)); live acceptance pending. Stage 7b live input/clipboard checks are still pending too; do not call them passed from unit tests.

## Contract and scope

Read `AGENTS.md`, then `task/progress.md` sections **Stage 7b**, **Stage 7c** and **Stage 7c-2**. Stage 7c's scope/acceptance is authoritative for this delivery. Also read `task/stage-7b-report.md`, `docs/plan.html`'s `.shots`/`.shot`/`.shotbar` styles and `addShot`/`updateShots` mockup, plus Stage N/N2 contracts for placement and AppBar behavior. The HTML shows the eventual product, including features deliberately deferred here.

Deliver floating annotated previews below the island, countdown, keep-in-stack pin, multi-selection and copy/save/dismiss actions. Do not implement OLE drag-out, CF_HDROP, drag ghosts, OCR, standalone pin windows, edit-from-card, uploads or clipboard history. Do not add crates, dependencies or speculative shared drag infrastructure. Snap is already available in the registry.

## Decisions already captured

- Keep 7b's select → annotate → explicit finish flow and the existing 6 s latest-shot thumb. Stack entries have independent lifetimes and may share immutable final-image payloads with the thumb.
- Countdown defaults to 10 s; `[snap] preview_seconds` accepts integer 3–60, otherwise 10. No Settings row in this stage. Hover pauses that card; any selection pauses the whole stack; pin pauses that card. Hidden/capture time does not count. Resume the remainder rather than restarting the timer.
- Pin means keep a card until explicit dismissal or app exit, not a movable reference image. That other kind of pin stays in 7d.
- Cards: 124×80 logical px, radius 12, ring 16, gap 10, newest first, aspect-fit. Place 12 px below the current island edge; clamp/reflow on its monitor. Four/five cards may fan at rest, but hover/selection must expose individually usable cards and controls.
- Per-card actions: `Copy`, `Save`/`Open`, `Pin`/`Unpin`, `Dismiss`. Clicking card body toggles membership without clearing others; action clicks never also select. Bar: selected count, `Select all`, `Clear selection`, `Save selected`, `Dismiss selected`. `Copy` operates on one image only; do not claim multiple PNG/DIB images fit in one clipboard payload.
- Keep at most 5 cards and 256 MiB of retained full-resolution BGRA + PNG data, counting shared data once. Evict oldest unprotected cards only; pinned/selected/hovered/busy cards are protected. If admission cannot fit, preserve current cards and the incoming shot's normal clipboard/save/thumb result; report `Preview stack full`. No automatic preview/pin disk writes.
- Expiry/dismissal never clears clipboard or deletes files. Batch save retains cards and per-item results. Save uses existing no-overwrite names; Open reveals the existing saved path. Missing files and clipboard/save errors are explicit, retryable failures.
- No focus theft on appearance. Mouse-independent access to actions is required, without global shortcuts or relying on color alone. Transparent gaps/shadows pass input through. No Alt-Tab/taskbar entry and no second AppBar.

## Implementation boundaries

- `crates/dj-core`: pure item IDs, metadata, timing/selection/capacity policy, config and layout/hit testing. Inject time; no Win32, Skia or unsafe.
- `crates/dj-render`: themed thumbnails, countdown and selected/pinned/error states on the existing Skia path. Cache thumbnails; keep physical output pixels unchanged. Use bounded animation and countdown updates, not an idle render loop.
- `crates/dj-win`: native companion window/input/visibility support only where existing helpers do not suffice. Preserve Win32 lifetime and safety conventions.
- `crates/dj-capture`: reuse existing WIC and clipboard format handling. Do not add OLE or another capture backend.
- `app`: own payloads and stack lifecycle; reuse `app/src/snap.rs` no-overwrite save paths and existing reveal support. Wire completed 7b output, per-item actions, hide/show and timers. Keep stable IDs for async results; never address completion by current card index. Do not hold duplicate full-desktop captures for previews.

Inspect current code before choosing helper APIs, timer/message IDs or extracting code. Prefer extending existing functions over new abstractions. Use a separate DComp surface sized once for maximum stack layout at current DPI; do not resize the island swap chain per animation frame. Hide the stack before freeze and wait for composition visibility to settle, exclude its windows from snap selection, and restore after cancel/finish. Do not change AppBar's reserved strip to fit cards.

Only visible, running countdowns need ticks (maximum 10 Hz); springs use the existing animation scheduling. Once springs settle, an empty, hidden or fully paused stack has no recurring rendering. Free resources when the last card disappears. Save/encode must not block input for large images.

## Headless verification

Never run `daijobu.exe`, capture the user's screen, write their clipboard, send input, install hooks, create visible windows or change their settings. Test through synthetic frames, fake actions/clipboard and temp directories. Do not use or clean a shared worktree build directory blindly; verify `CARGO_TARGET_DIR` and use a worktree-local target for authoritative results. Keep build parallelism at 2 unless the environment contract says otherwise. Use the configured MSVC/Windows SDK environment; do not silently fall back to a different target toolchain.

Run:

```text
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

Required checks are listed in the Stage 7c contract: combined pause reasons and remaining time, expiry, selection and batch actions, capacity/byte budget with all-protected/oversized inputs, stale completions, DPI/negative-coordinate layout and hit regions, config fallback, synthetic rendering, no-overwrite saves, missing paths and partial failures. Existing 7b output/text/pixelate tests must remain green. Do not replace product semantics just to make tests pass.

## Live acceptance and handoff

Coordinator/user performs the five acceptance sequences in `task/progress.md`. Report exact card/action rectangles and coordinate spaces for driving those checks. Include clipboard/save output verification, exclusion from the next capture, pause/resume, full protected stack, keyboard access, click-through, themes, notch/pill/AppBar coexistence, DPI/displays and idle behavior.

When implementation is complete, write `task/stage-7c-report.md` with an honest per-item checklist, exact commands/results, known issues, paths and every live check not performed. Do not mark progress acceptance complete without evidence. Commit only if the dispatch explicitly requests it; never push or open a PR unless requested. OLE remains a separate 7c-2 brief and delivery, even if this stage finishes early.
