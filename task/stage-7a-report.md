# Stage 7a report: Snap core

Built on `stage-7a-snap`, starting from `363c293` (contract and brief). The work was headless: the exe was never run, the real screen was never captured, the real clipboard was never written, and no windows, hooks or input were created in tests. All live and visual behaviour is **not verified, user to test**.

## Verdict

Every scope item is implemented, and the pure parts have unit tests. Nothing in the contract was skipped. The overlay renders on the existing Skia/D3D12/DirectComposition path (its own device and swap chain, made per snap), not through the CPU fallback. The deviations are small and listed below.

Verification on `stage-7a-snap` (MSVC 14.51, Windows SDK 10.0.26100, `dj-build-env.sh`):

- `cargo fmt --all`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: **284 passed**, 0 failed (dj-core 171, dj-sweep 35, dj-vitals 26, app 23, dj-render 16, dj-capture 8, dj-win 5). Stage 6 ended at 234, so this stage adds 50: dj-core +28, app +11, dj-capture +8 (new), dj-render +2, dj-win +1.
- `cargo build --release`: compiles and links (`target/release/deps/daijobu.exe`, 10.5 MB, 2026-10-26 08:13). Cargo's last step, the copy to `target/release/daijobu.exe`, failed with "Access is denied". An older `daijobu.exe` (PID 42012, built 07:23, before this stage) is running from the shared target dir and locks the file. I left it running. Quit it and run `cargo build --release` again to refresh `target/release/daijobu.exe`, or run `target/release/deps/daijobu.exe` directly.

## Checklist (contract)

| Item | State | Notes |
|---|---|---|
| Unlock Snap (5 of 12 ready); the tile starts a snap after the island collapses | Done | `features.rs` marks Snap `Available`. `ClickOutcome::Open(snap)` calls `App::request_snap`, which collapses the island and freezes once it is still (`TIMER_SNAP_START`, 60 ms after the last frame). The locked-tile island test moved from Snap to Clipboard. Live: **not verified, user to test**. |
| Hotkey: PrtSc via `RegisterHotKey`, fallback Ctrl+Shift+S with the reason logged; `[snap] hotkey` takes `"PrtSc"`, `"Ctrl+Shift+S"`, `"Ctrl+Alt+S"` | Done | `SnapHotkey::parse` / `candidates()` in `dj-core::config`; `App::register_snap_hotkey` logs each failure with the Win32 message (plus a hint about Windows' Print screen setting for PrtSc). An unknown name falls back to PrtSc, with a log line. Ctrl+Alt+S also falls back to Ctrl+Shift+S. Live: **not verified, user to test**. |
| `crates/dj-capture` (the only new crate) | Done | See below. All `unsafe` is in `dj-capture`, `dj-win` and `dj-render`, each block with `// SAFETY:`. |
| Freeze: every monitor once before the overlay shows; DXGI Desktop Duplication per output, GDI `BitBlt` fallback; BGRA per monitor with its rect and DPI | Done | `dj_capture::freeze`. A D3D11 device per adapter, made for this snap and dropped after it. `AcquireNextFrame` (60 ms, up to 4 tries, waiting for `LastPresentTime != 0`), copy to a staging texture, map, copy rows. A monitor falls back to GDI (`SRCCOPY \| CAPTUREBLT` into a top-down DIB section) if it has no output, is rotated, isn't BGRA8, or duplication fails or times out. Alpha is forced to 255. GDI use is logged. Live: **not verified, user to test**. |
| Window rects: `EnumWindows`, visible, not minimised, not cloaked, not tool windows, not ours, `DWMWA_EXTENDED_FRAME_BOUNDS`, top to bottom | Done | `dj_capture::window_rects`. "Ours" means the same process id, so the island, the overlay and the Music windows are all excluded. It also skips click-through windows (`WS_EX_TRANSPARENT`) and the desktop (`Progman`, `WorkerW`); the app appends the monitor rects last, so a click on the bare desktop takes that monitor. |
| PNG through WIC | Done | `dj_capture::png::encode` (an HGLOBAL `IStream`, the PNG encoder, 32bpp BGRA). `decode` exists for the round-trip test. |
| Clipboard: `CF_DIB` (bottom-up, 32-bit) + registered `"PNG"` in one `OpenClipboard`/`EmptyClipboard` session | Done | `dj_capture::put_image`. Retries `OpenClipboard` 10 times, 15 ms apart. `dj_capture::dib` builds the DIB bytes (pure, tested). Live paste: **not verified, user to test**. |
| `dj-core::snap` (pure, tested): selection geometry in physical desktop px | Done | `Selector`, `Loupe`, `crop`, `pixel_at`, labels, `file_name`/`unique_name`, `Countdown`, `aspect_fit`. |
| … drag in any direction, normalised | Done | Tested for all four directions. |
| … click with no drag selects the topmost window under the cursor | Done | Movement under `DRAG_SLOP = 3` px still counts as a click. |
| … arrows nudge 1 px (Shift 10 px) | Done | The nudge moves the cursor, and with it the dragged corner. The app moves the real pointer too (`SetCursorPos`), so the next mouse move agrees. |
| … clamp to the virtual desktop | Done | The cursor, the drag rect and a picked window rect are all clamped. |
| … minimum 2×2, smaller cancels | Done | |
| … loupe: 15×15 grid around the cursor, clamped at the edges | Done | The grid stays on the desktop; at an edge the cursor cell moves off the centre. |
| … size label and hex text | Done | `612 × 388`, `snap · 612×388`, `#E0A458 · 812,344` (a gap between monitors shows `#------`). |
| … file name `snap-YYYY-MM-DD_HHMMSS.png`, `-2`, `-3`… | Done | The save uses `create_new`, so the check and the create are one step and nothing is overwritten. |
| Overlay window: topmost `WS_POPUP` over the whole virtual desktop, Skia on DComp/D3D12 | Done | `dj_win::overlay` (class `DaijobuSnapOverlay`, `WS_EX_NOREDIRECTIONBITMAP \| WS_EX_TOOLWINDOW \| WS_EX_TOPMOST`, no `NOACTIVATE`) and `dj_render::SnapOverlay` (its own D3D12 device, Skia context and composition swap chain the size of the virtual desktop; the frozen monitors are uploaded once as GPU images). Created hidden; the first frame is drawn before `ShowWindow`. Visual: **not verified, user to test**. |
| … frozen frame, ~55% dim outside, dashed outline, size label, loupe 8× pixelated with crosshair and `#E0A458 · 812,344` | Done | Dim is `rgba(0,0,0,.55)` from plan.html; the outline is the theme `highlight`, dashed 6/4 px while dragging. The loupe is 120 px (15 × 8) at 100%, nearest sampling, a faint crosshair band and the cursor cell outlined, with the caption below it. It sits 24 px right and below the cursor and flips at the cursor monitor's edges. UI sizes follow the DPI of the monitor under the cursor. A dj-render test draws the scene onto a CPU surface and checks the dim and the cut-out. |
| … hover highlights the window under the cursor before any drag | Done | Cut out of the dim with a solid 2 px outline and its size label. |
| … Esc or right click cancels; crosshair cursor; renders only on input; created per snap and destroyed after | Done | Esc is also registered as a global hotkey while the overlay is up, in case it didn't get the keyboard. The overlay draws only when an input changed the selection, the cursor or the hover (paced by `Present(1)`); there is no loop. The island's swap chain is untouched. |
| On release: crop across monitors, copy (PNG + CF_DIB), close the overlay, show the thumb | Done | `Snapper::finish`. The overlay (window, GPU images, swap chain) is closed first, then the frames are cropped and dropped, then PNG + DIB go to the clipboard. Live: **not verified, user to test**. |
| Snap thumb 330×72 r 26: 76×48 r 8 aspect-fit shot, `snap · 612×388`, `✓ copied`, copy and save | Done | `IslandState::SnapThumb` (layer 9). The thumb's small copy of the shot is made once (mipmapped downscale to fit 152×96). Hovered buttons turn accent. Visual: **not verified, user to test**. |
| … priority right below the OSD; 6 s, hover pauses; click elsewhere dismisses; then the rest state | Done | `rest_state`: snap thumb > timer done > media > timer running > idle; the OSD still only covers the state. `TIMER_SNAP_THUMB = 7` is a one-shot at the time left; it is killed while hovered and re-armed with what is left on leave. |
| Save: `[snap] save_dir` (default `%USERPROFILE%\Pictures\Daijobu`, created), thumb shows `saved`; never overwrites; failures show `save failed` and are logged | Done | `SnapConfig::save_dir`: `~` is the profile; a relative or blank path falls back to the default. |
| Multi-monitor and DPI: overlay spans every monitor, selection may cross them, output in physical px | Done | The overlay ignores `WM_DPICHANGED`, so Windows never resizes it. Crops across monitors are tested; gaps are opaque black. Live: **not verified, user to test** (only one monitor shape was ever modelled, in tests). |
| Idle CPU ≈ 0; frames only during a snap; only the last crop kept until the thumb goes | Done (by design) | Nothing is captured, cached or armed outside a snap except the thumb's one-shot. The crop and its PNG are freed when the thumb goes (timeout or dismiss); the small thumbnail copy is dropped once the thumb has faded out. **Not verified, user to test.** |
| Tests | Done | See below. |

### Tests added (50)

- **dj-core `snap`** (14): rect basics; drag in all four directions; negative coordinates across monitors; click-to-window with z-order and the monitor fallback; a picked window clamped to the desktop; click on nothing and tiny drags cancel, exactly 2×2 is kept; drag clamped at both corners; arrow nudges (1, 10, edges, slop); loupe centring and clamping at every corner; `pixel_at` and `crop` across two monitors with a gap; `aspect_fit`; labels; file names and collisions; the countdown (pause, resume, backwards clock).
- **dj-core `config`** (4): `[snap]` defaults and reading; hotkey names (case, spaces, modifier order, unknown names); fallback candidates; `save_dir` expansion and fallbacks.
- **dj-core `layout`** (1): the thumb fits the window and its parts don't overlap; the window size is unchanged.
- **dj-core `island`** (9): the tile reports `Open`; the thumb's size, layer and return to Idle; it outranks Idle, Music compact, Timer compact and the timer alert and hands back to each; facts arriving under the thumb wait for it; it takes over an open panel; copy / save / dismiss clicks; dismiss over music returns to Music compact; hover tracking with its buttons; OSD over the thumb returns to it.
- **dj-capture** (8): DIB header fields, row flip, padded source stride, short input; WIC PNG round trip on a synthetic gradient and on one pixel; empty/short bitmaps refused; `opaque`.
- **dj-render `snap`** (2): the thumb's copy is aspect-fit at 2×; the overlay scene drawn on a CPU surface dims outside the selection only, across a negative-x monitor seam.
- **app `snap`** (11, fake screen and clipboard): hotkey → virtual key; ids are free; a drag across two monitors puts the exact pixels on the clipboard as PNG and bottom-up DIB in one call, and frees the frames; click-to-window and click-to-monitor; cancel and a tiny drag never touch the clipboard; a failed copy keeps the shot for a retry; save writes a PNG and never overwrites (`-2`); a failed save says so; the 6 s timeout with hover pause; the island returns to rest when the thumb times out; a new snap replaces the last shot.
- **dj-win `system`** (1): `local_datetime` is sane.

## Semantics decided here (the contract leaves them open)

- **What "collapses first" means.** An open panel (Home, Music, Vitals, Sweep, Timer) or a thumb still up morphs back to the rest state, and the freeze waits for that. A resting island (the idle pill, Music or Timer compact) is frozen at once and is in the shot, like the taskbar. PrtSc with a resting island has no delay.
- **Arrow keys** move the cursor, so before a drag they move the hover point and during a drag the moving corner. There is no post-release adjust step, because the release copies at once (per the contract).
- **A click on the bare desktop** takes the monitor under the cursor (monitors are the last click targets). Without this it would cancel.
- **The thumb takes over an open panel** if one is somehow open when a shot finishes (normally impossible: the island collapses before the freeze and is hidden while the overlay is up).
- **A new snap dismisses the old thumb** before the freeze, so the old thumb isn't in the new shot.
- **Copy and save don't restart the countdown.** It is paused while the pointer is on the thumb anyway.
- **The island window hides while the overlay is up.** It is topmost too, and its foreground hook would raise it over the overlay. The user's visible/hidden choice is kept and restored after.
- **A display change during a snap cancels it** (the frozen frames no longer match the monitors).

## Deviations

1. **The overlay has its own GPU device and Skia context** (per snap), rather than sharing the island renderer's. The island renderer can be rebuilt on a DPI change or device loss, which would leave a shared overlay with dead surfaces; an independent one avoids that. D3D12 devices are per-adapter singletons, so the extra cost is mainly the Skia context. `gpu.rs` was refactored so both use the same `Gpu` and `Chain` helpers; the island's behaviour and drop order are unchanged in intent (surfaces cleared and the GPU synced in `Drop`, the context dropped before the swap chain).
2. **Esc is registered as a global hotkey while the overlay is up** (id 4), in addition to `WM_KEYDOWN`. It is unregistered when the overlay closes. This guards against `SetForegroundWindow` being refused. Arrow keys still need the overlay to have the keyboard.
3. **`WS_EX_TRANSPARENT` windows and the desktop (`Progman`, `WorkerW`) are not snap targets**, beyond the contract's filter list.
4. **Build parallelism:** I used the coordinator's `dj-build-env.sh` (`CARGO_BUILD_JOBS=8`, shared target dir). The release build wrote this branch's binary to `target/release/deps/daijobu.exe` in the main checkout's target dir. It could not replace `target/release/daijobu.exe` (see Verification).

## Ids

- Hotkeys: `HOTKEY_SNAP = 3`, `HOTKEY_SNAP_ESC = 4` (only while the overlay is up).
- Timers: `TIMER_SNAP_THUMB = 7` (the thumb's one-shot), `TIMER_SNAP_START = 8` (the freeze after a collapse). A test checks they are free.
- No new `WM_APP` messages: the overlay's input goes on the existing event queue as `Event::Snap`.

## Pill-local rects (logical px at 100%, from `layout.rs`)

The island window is 520 logical px wide, at the top centre of the primary monitor. Each view is centred in the window at y = 6 (`TOP_MARGIN`):

screen x = monitor left + (monitor width − 520·s)/2 + ((520 − view w)/2 + local x)·s, and screen y = monitor top + (6 + local y)·s.

For a click posted to the island window, the client coordinates are ((520 − view w)/2 + local x)·s and (6 + local y)·s.

| View | Size | Element | Rect (x, y, w, h) | Client centre at s = 1 | Screen centre at 3440 px wide, s = 1 |
|---|---|---|---|---|---|
| Home | 520×340 | Snap tile (index 4) | (408, 52, 88, 76) | (452, 96) | (1912, 96) |
| Snap thumb | 330×72, r 26 | copy | `SN_COPY` (98, 38, 56, 22) | (221, 55) | (1681, 55) |
| | | save | `SN_SAVE` (160, 38, 56, 22) | (283, 55) | (1743, 55) |
| | | dismiss (the shot; anywhere else on the thumb works too) | `SN_SHOT` (10, 12, 76, 48) | (143, 42) | (1603, 42) |
| | | caption (dismisses too) | `SN_CAPTION` (98, 14, 218, 16) | (302, 28) | (1762, 28) |

The thumb's view x offset is (520 − 330)/2 = 95.

**The overlay** (class `DaijobuSnapOverlay`, title "Daijobu Island Snap") covers the virtual screen. Its client px = desktop px − (`SM_XVIRTUALSCREEN`, `SM_YVIRTUALSCREEN`); with a single monitor at the origin they are the same. To drive it without the real mouse, post `WM_MOUSEMOVE` (hover), `WM_LBUTTONDOWN` at the start, `WM_MOUSEMOVE` to the end, then `WM_LBUTTONUP` at the end (a drag), or down/up at one spot (a window click). `WM_RBUTTONDOWN` or `WM_KEYDOWN VK_ESCAPE` cancels. PrtSc can be simulated with `PostMessage(island, WM_HOTKEY, 3, 0)`; in that case Windows may refuse `SetForegroundWindow`, so the overlay may not get arrow keys (Esc still works through the hotkey).

## How to run (this machine)

```sh
source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh
cd C:/Users/UseR/orca/workspaces/daijobu-island/stage-7a-snap
cargo build --release   # target/release/daijobu.exe (in the shared target dir)
```

Quit any running `daijobu.exe` first, or the final copy fails (see Verification). Only one instance can run (single-instance mutex), so the old one must go before the new one starts anyway.

Optional `~/.daijobu/config.toml`:

```toml
[snap]
hotkey = "PrtSc"            # or "Ctrl+Shift+S", "Ctrl+Alt+S"
save_dir = "~/Pictures/Daijobu"
```

If PrtSc is taken, `~/.daijobu/daijobu.log` gets a line such as `snap: PrtSc is not available (…), using Ctrl+Shift+S`.

## Live test (2026-10-26, user's machine, 3440×1440, scale 1.0)

The coordinator re-verified fmt, clippy (`-D warnings`), 284/284 tests and the release build. The user ran the snap flow by hand. The overlay can't be checked by the coordinator's automated screenshot (a GDI `CopyFromScreen` doesn't see the fullscreen DComp overlay), and it captures all input, so it can't be driven while the user is working.

| Check | Result |
|---|---|
| PrtSc registered (no fallback logged) and starts a snap | Pass |
| Screen freezes and dims, the loupe follows the cursor, hovering outlines windows | Pass (seen by the user) |
| Drag a region and release → snap thumb with the size and `✓ copied` | Pass |
| Paste into another app gives exactly the selected region; the clipboard holds `PNG` + `DeviceIndependentBitmap` (checked: 485×292) | Pass |
| Save writes `Pictures\Daijobu\snap-….png`, never overwriting (same-second saves got `-2`) | Pass |
| A single click on a hovered window captures that window | Pass |
| **Added after the live test, at the user's request:** once saved, the save button reads **open** and reveals the file in Explorer (`explorer /select`) | Pass (user) |

Still not verified live: multi-monitor or mixed DPI (single monitor here), the GDI fallback, the Ctrl+Shift+S fallback, HDR, and large crops' encode time.

## Suggested live test (not run)

1. Press PrtSc (or Ctrl+Shift+S, see the log). The screen freezes under a dim; the loupe follows the cursor with the hex and coordinates; hovering a window outlines it with its size.
2. Drag a region and release. Paste into Paint: the pixels match exactly (compare a 1 px line or text edges). The island shows the thumb with the size and `✓ copied`.
3. Click **save**: the thumb shows `saved`, and `Pictures\Daijobu\snap-….png` exists. Save again within the same second: a `-2` file appears.
4. Hover the thumb for more than 6 s: it stays. Move away: it goes after the rest of its 6 s, back to Idle (or Music/Timer compact).
5. Snap again and press Esc (and right click): the overlay closes and the clipboard still holds the previous content.
6. Click a window without dragging: exactly that window's frame (no invisible border).
7. Home → Snap tile: the panel collapses, then the freeze; the panel is not in the shot.
8. Arrow keys during a drag move the corner by 1 px (Shift: 10 px).
9. Multi-monitor and mixed DPI: drag across two monitors; the PNG has the physical size of the selection.
10. Idle CPU before, during (still mouse) and after a snap.

## Not verified (user to test)

- Everything live: the hotkey (and whether Windows' PrtSc setting blocks it), the overlay's look (dim, outline, labels, loupe, crosshair cursor), hover outlines, keyboard focus on the overlay, the clipboard content in other apps, the thumb's look and timing, save on disk, multi-monitor and mixed DPI, rotated monitors (GDI path), HDR monitors, idle CPU.
- DXGI Desktop Duplication on real hardware (which path each monitor takes is logged only when GDI is used).
- Whether the first `AcquireNextFrame` always carries a desktop image on this machine (otherwise that monitor uses GDI after about 240 ms).
- Overlay frame pacing on a high-rate mouse (it draws at most once per vsync).

## Known issues and risks

- **Big crops are encoded on the UI thread.** A full 3440×1440 or multi-monitor PNG may take a few hundred ms in WIC after the release (the overlay is already gone; the thumb appears when the copy is done).
- **Overlay memory:** a virtual desktop of W×H costs about 4·W·H for the frozen frames on the CPU, the same on the GPU, and 8·W·H for the two swap-chain buffers, for the length of a snap only. A virtual desktop over 16384 px on a side exceeds D3D12's texture limit: the frames then stay CPU images (still drawn), but the swap chain itself may fail, in which case the snap is cancelled and logged.
- **Foreground:** if Windows refuses `SetForegroundWindow` (e.g. a posted, not a real, hotkey), the overlay has no keyboard: arrows don't work, Esc and right click still do.
- **HDR:** `DuplicateOutput` (not `DuplicateOutput1`) is used; on an HDR monitor the image is what DXGI's SDR conversion gives, or GDI if the format isn't BGRA8.
- **Contrast:** the overlay's labels (highlight on pill) and the thumb's hovered button (pill ink on accent) aren't checked by the theme validator, which only checks text on the pill. The Timer and Vitals buttons already work the same way.
- **PrtSc while a game has exclusive fullscreen** is not handled specially.
- `snap thumb > timer done` means a session that ends while a thumb is up waits up to 6 s (plus hover) for the alert; the chime still plays at once.
