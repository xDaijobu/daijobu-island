# Stage 7a brief: Snap core (for the worktree agent)

Your worktree starts from `master`, which includes Stages 1–6 (shell, Music, OSD, Vitals + Sweep, Timer). You are unlocking **Snap**: a PrtSc screen freeze, region/window selection with a loupe, auto-copy on release, save, and the snap thumb in the island. Annotations, pin, OCR (7b) and the Snap stack (7c) are **out of scope**.

## HEADLESS (overrides everything)
- Never run `daijobu.exe`. No windows, hooks, input, screenshots of the real screen, or real clipboard writes.
- Put the clipboard and the screen capture behind traits, and test with fakes and synthetic bitmaps. WIC encode/decode of an in-memory bitmap is fine in tests.
- Run `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh` before cargo. Verify only with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and `cargo build --release`.
- In the report, mark live checks as "not verified, user to test".

## Read first
1. `AGENTS.md`. Add `crates/dj-capture/` to its Layout section in this stage, because it is the new crate.
2. **Contract:** `task/progress.md`, section **"Stage 7a: Snap core"**. Build exactly that. Don't edit progress.md.
3. `docs/plan.html`: the Snap card (§ Snap · Tile & Scenes), the `.snapov` overlay mockup CSS (dim, dashed selection, size label, loupe), the snap thumb mockup (`snap:{w:330,h:72,r:26}`), and the activity priority (`OSD > snap thumb > notif > timer done > …`).
4. `task/stage-6-report.md` and the Timer code (`app/src/timer.rs`, `dj-core/src/island.rs`) for how a new island state, its priority, its layout rects and its app timers are wired. Read `dj-render/src/gpu.rs` and `dj-win/src/window.rs` before designing the overlay window.

## Design guidance
- **Overlay window:** a second top-level window (`WS_POPUP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW`, no `NOACTIVATE`: it needs the keyboard for Esc and the arrows). It covers `SM_XVIRTUALSCREEN…`. Prefer giving the existing GPU/Skia path a second DComp target and swap chain sized to the virtual desktop, created per snap and dropped afterwards. If that is impractical, document why and use the simplest correct alternative (for example a CPU raster Skia surface + `UpdateLayeredWindow`); the plan allows that fallback. Render only on input.
- **Capture** (`dj-capture`): DXGI Desktop Duplication needs a D3D11 device. Create it per snap, `AcquireNextFrame` with a short timeout, and copy each output to a CPU-readable texture. If any output fails, use GDI `BitBlt` for that monitor. Keep frames as BGRA plus a rect and DPI. Exclude our own windows from the window-snap list.
- **Coordinates:** everything is in physical desktop pixels; the process is Per-Monitor-V2 aware. The overlay converts mouse positions with its own origin (`SM_XVIRTUALSCREEN`, which can be negative).
- **Island:** add a `SnapThumb` state with its own layer and the priority from the plan. It has a 6 s one-shot timer (`TIMER_*` ids 1, 2 and 4–6 are taken, so use 7+), and hover pauses it. The thumb's thumbnail is a downscaled Skia image of the crop, made once.
- **Hotkeys:** ids 1 and 2 are taken. Free `WM_APP` ids start at `WM_APP + 9`.
- **Config:** `[snap] hotkey`, `[snap] save_dir` (a leading `~` means the profile; relative paths fall back to the default).
- **Licences:** no new third-party crates unless they are MIT/Apache/BSD; WIC and DXGI come with Windows. Nothing from Lightshot, ShareX (GPL) or similar may be copied.

## Finish
Commit in small steps on your branch and don't push. Write `task/stage-7a-report.md`: an honest per-item checklist, how to run, known issues, what was not verified, and the **screen/pill-local rects of the snap thumb's copy/save buttons** so the coordinator can drive a live test. Then stop.
