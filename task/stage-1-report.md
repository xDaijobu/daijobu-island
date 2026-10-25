# Stage 1 report: App shell

Branch `stage-1-app-shell`, committed locally, not pushed. Screenshot: `task/stage-1-screenshot.png` (Tobacco Tape, home grid).

## Verdict

Every checklist item is implemented. I ran the release exe on this machine (1 monitor, 150% DPI) and watched most of it work: the pill, expand/collapse, locked tiles, theme hotkey and tray menu, hot reload, click-through, single instance, autostart, and idle CPU. The gaps are listed under "Not verified".

`cargo build --release`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt` are clean. `cargo test --workspace` passes: 29 tests in `dj-core` and 2 in `dj-win`.

## Checklist

| Item | State | Evidence / notes |
|---|---|---|
| Cargo workspace: `dj-core`, `dj-render`, `dj-win`, `app` (bin `daijobu`) | Done | `dj-core` is `#![forbid(unsafe_code)]` and has no Win32. Every `unsafe` block has a `// SAFETY:` comment. |
| Raw Win32 window, styles, PMv2 manifest, top-center, not in Alt-Tab/taskbar | Done, partly verified | Ex-style `0x8200088` = NOREDIRECTIONBITMAP, TOOLWINDOW, TOPMOST, NOACTIVATE. The manifest is embedded via `build.rs` and `/MANIFESTINPUT`. The window reports dpi=144 and sits top-center. Alt-Tab was not looked at. TOOLWINDOW is the reason it should be absent. |
| DComp + D3D12 + Skia, premultiplied composition swap chain at max size | Done | 2 buffers, FLIP_SEQUENTIAL, PREMULTIPLIED, BGRA8. Sized to the home state x DPI. `ResizeBuffers` is never called. A DPI or size change rebuilds the whole renderer. Falls back to WARP if there is no hardware adapter. |
| Click-through outside the pill | Done, verified | `SetWindowRgn` tracks the pill (union of this and last frame's rect while animating). `WindowFromPoint` returns the pill inside it and the window underneath for points outside it, even inside the window rect. |
| Spring morph with content crossfade (blur + scale) | Done, verified visually | Spring settings: expand 0.45/0.75, collapse 0.35/0.95, content fade 0.25/1.0, 1/240 s substeps, retarget keeps velocity. The crossfade uses opacity, blur and scale 0.94 to 1. |
| Idle state: clock + status lamp | Done | The lamp is static: a breathing lamp would stop idle CPU from being 0. The clock re-renders from a timer at each minute boundary. |
| Home grid: 13 locked tiles, shake + "coming soon" on click | Done, verified | 5 columns, last row of 3 centred. Clicking a tile shakes it and shows "MEDIA - COMING SOON" in the footer for 1.8 s. Clicking any non-tile area of the pill (header, footer, gaps) collapses it. |
| Feature registry in `dj-core` | Done | `features.rs`: id, name, icon, `Locked | Available`. Clicking an Available tile returns `ClickOutcome::Open(i)`. Nothing handles it yet, because nothing is unlocked. |
| Theme engine: schema, `base`, contrast validation, interpolation, hot reload | Done, verified | The TOML schema matches the plan. `base` chains and cycles are handled. Text vs pill must be at least 4.5:1, otherwise the theme is rejected and its reason shown in the home footer in the warn colour. Colours interpolate in OKLab and numbers linearly over 450 ms. Hot reload uses `ReadDirectoryChangesW` with a 200 ms debounce. I edited, added and removed theme files live and the island followed each change. |
| Built-in themes | Done, verified | Tobacco Tape (default), Midnight Cassette, Paper Zine (light), Clean. Each was seen on screen. |
| SkSL grain (18 fps quantised) + ordered dither + optional scanlines | Done, scanlines only lightly checked | Effects compile once at startup. Grain uses `floor(t*grain_fps)` as its seed, so it only changes while something is animating. A Bayer 4x4 dithered glow runs at the theme's `dither_levels` and `dither_cell`. No built-in theme enables scanlines. I forced them with a temporary user theme and saw a faint effect, not a rigorous check. |
| Embedded OFL fonts: Geist, Geist Mono, Doto | Done | Geist Regular/Medium/SemiBold, Geist Mono Regular/Medium, Doto (variable). The OFL texts are in `assets/fonts/`. A theme naming another family falls back to a system font, then to Geist Mono. |
| Tray: Show/Hide, Theme ▸, Start with Windows ✓, Quit | Done, mostly verified | The icon is registered in the tray (found through UI Automation, seen in the overflow flyout). The menu opened, showed the theme submenu with a check on the current theme, and picking a theme changed it and saved the config. See "Not verified" for the right-click. |
| Hotkeys `Ctrl+Alt+H` and `Ctrl+Alt+T` | Done, verified | Synthetic key presses hid/showed the island and cycled themes. A hotkey that is already taken by another app is only written to the log. |
| Single instance, autostart, `~/.daijobu/config.toml` | Done, verified | A second launch exits. The autostart toggle wrote and removed `HKCU\...\Run\DaijobuIsland` and updated the config. I removed the test entry afterwards. The config (theme + `start_with_windows`) is saved on change. |
| Idle CPU ≈ 0% | Done, verified | Process CPU time did not change over 5 s idle, either collapsed or expanded. The loop blocks in `GetMessageW`. |
| Build, clippy, `dj-core` tests | Done | `dj-core` tests cover the spring (plan table constants, overshoot, critical damping, retarget keeps velocity), theme parse, `base` inheritance, cycles, validation and rejection, interpolation, layout, and the island state machine. |

## How to run

```
cargo build --release
target\release\daijobu.exe
```

- Click the pill to expand it. Click a tile to see it shake and show "coming soon". Click elsewhere on the pill to collapse it.
- `Ctrl+Alt+T` cycles themes, `Ctrl+Alt+H` hides or shows the island, and the tray menu does the same plus autostart and Quit.
- User themes go in `~/.daijobu/themes/*.toml`. The file stem is the theme id and `base = "tobacco-tape"` inherits from a built-in.
- Errors are logged to `~/.daijobu/daijobu.log`.

## Known issues and deviations

- **skia-safe features differ from the plan.** `default-features = false` makes skia-bindings look for a prebuilt that isn't published, then fall back to a source build. That source build failed here with "Filename too long" in the Skia submodule checkout. Using skia-safe's default features plus `d3d` and `textlayout` matches a published prebuilt (`d3d-ganesh-jpegd-jpege-pdf-textlayout`), so it downloads and builds in about 30 s. The cost is that pdf and jpeg are linked in, and `daijobu.exe` is about 8.6 MB. Pinned `=0.153.3`, `windows` 0.62.2.
- **The TOML example in the plan is not valid TOML.** It puts several keys on one line. The built-in themes use one key per line.
- **No custom exe icon.** The tray icon is drawn in code. The exe has no embedded `.ico`, so Explorer shows the default.
- **Grain freezes when nothing moves.** That follows from "render only while animating", and I believe it is the intended trade.
- **Tray left-click** shows or hides the island. It wasn't in the checklist but it is the usual expectation.
- **The home footer** shows the active theme name, and the rejected-theme message when there is one. The plan says rejected themes are "flagged in the UI" and there is no settings window yet, so the footer is where it goes.
- **`accent_from_cover`** is parsed and validated but unused until Media exists.

## Not verified

- **A real right-click on the tray icon.** My synthetic right-click on the icon in the overflow flyout did not open the menu. I opened the menu by posting the tray callback message to the window, and everything in the menu worked from there. The shell → window path is the standard `WM_RBUTTONUP` handling, but I did not see it fire from a real click.
- **Alt-Tab and taskbar absence**, by eye.
- **DPI change at runtime and multiple monitors.** The code path (`WM_DPICHANGED` / `WM_DISPLAYCHANGE` → rebuild renderer and re-place) exists. I only had one 150% monitor.
- **GPU device-lost recovery.** `Present` errors map to a renderer rebuild. I never triggered one.
- **Exclusive-fullscreen games and z-order fights with the taskbar.** The foreground hook re-asserts topmost; I did not test it against a game.
- My test clicks and key presses were synthetic and shared the machine with the user's real mouse, which showed up as extra mouse-move events in my traces.

## Suggested next steps

1. Stage 2 (Media): unlock a tile by flipping `FEATURES[0].availability` and handling `ClickOutcome::Open`. The activity scheduler from the plan is the next structural piece, since Idle and Home are currently the only states.
2. Click through the tray with a real mouse and check Alt-Tab and a second monitor once.
3. Decide whether the pdf and jpeg code in the Skia binary is worth a source build to drop later (needs a short repo path or git long paths).
4. Add an exe icon and a simple CI job (`cargo clippy -D warnings`, `cargo test`) as the plan's F0 describes.
