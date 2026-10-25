# Daijobu Island

**A lofi dynamic island for Windows 11. Built in Rust, rendered with Skia, native Win32.**

*Daijobu* (だいじょうぶ) means "it's okay" or "are you okay?" in Japanese. Daijobu Island is a small pill at the top of your screen. It plays your music, catches your screenshots, remembers your clipboard, tiles your windows, and quietly checks whether your PC is okay.

> **Status: pre-alpha / planning.** Nothing below ships yet. It is the design target.
> See the interactive design doc: [`docs/plan.html`](docs/plan.html). Open it in a browser.

---

## Why

- **Native, for real.** Raw Win32 with DirectComposition and Skia on D3D12. No webview, no Electron, no winit. It uses about 0% CPU when idle.
- **Modern motion, lofi texture.** Interruptible spring morphs, plus film grain, ordered dithering, segmented VU meters and dot-matrix digits.
- **Useful, not decorative.** Every state answers a question: what's playing, how loud, is my PC healthy, where did that screenshot go.

## Features (planned)

| | Feature | What it does |
|---|---|---|
| 🎵 | **Media** | Now playing via SMTC, with cover, controls, seek, synced lyrics (LRCLIB) and a loopback audio visualizer |
| 🔊 | **OSD** | Replaces the native volume and brightness flyouts |
| 📸 | **Snap** | Lightshot-style screenshots: region and window select, loupe, annotate, blur, OCR, pin to screen |
| 🫳 | **Snap stack** | Previews float under the island for a few seconds. You can multi-select them and drag them into any app as files |
| 📋 | **Clipboard history** | Text, links, colors, files and **images** from any app. Search, pin, quick paste (`Ctrl+Shift+V`, with an optional `Win+V` takeover) |
| 🪟 | **Tile** | Split windows into 2, 3 or 4 zones. You can drag a window onto the island to pick a layout |
| 🌅 | **Scenes & Boot Scene** | Save a window layout, then restore it with one click or automatically after a restart, including apps that need admin |
| 🩺 | **Vitals** | CPU, RAM, disk, network, battery, top processes, and a 0–100 health score |
| 🧹 | **Sweep** | Clean temp and cache files, purge dev artifacts (`node_modules`, `target`, …), and find big files. Each run previews first and sends files to the Recycle Bin |
| ⏱ | **Timer / Pomodoro** | Countdown in the island, with a focus mode |
| 🔔 | **Notifications** | Windows toasts shown in the island |
| 🌧 | **Ambient** | Rain, café and fire sounds, plus lofi radio |
| ⌨ | **`dj` CLI** | Lets any script push a live activity, for example `cargo build && dj done "build ok"` |
| 🎨 | **Themes** | 12+ built-in themes defined in TOML: colors, fonts, texture, shape, visualizer and motion. Themes hot-reload and can switch automatically by time, app or focus mode |

## Tech

- **Language:** Rust (edition 2024)
- **Rendering:** [`skia-safe`](https://github.com/rust-skia/rust-skia) (Ganesh on D3D12), DirectComposition premultiplied-alpha swap chain, SkSL shaders for grain and dither
- **Platform:** [`windows`](https://github.com/microsoft/windows-rs) crate (Win32, COM, WinRT): SMTC, WASAPI loopback, Windows.Graphics.Capture, Windows.Media.Ocr, OLE drag & drop, Task Scheduler
- **No** webview, winit or async runtime in the UI path

```
crates/
  dj-core      state machine, springs, config, themes (no unsafe, no platform)
  dj-render    Skia + D3D12 + DirectComposition, SkSL effects
  dj-win       window, message loop, hooks, tray, hotkeys, DPI
  dj-media     SMTC, lyrics
  dj-audio     loopback capture, FFT, endpoint volume
  dj-vitals    system metrics, health score
  dj-sweep     clean / purge / big files, path guard, recycle, op log
  dj-capture   screenshot capture, overlay, annotation, OCR
  dj-clip      clipboard history, OLE drag-out
  dj-tile      zones, scenes, boot scene
  dj-ipc       named-pipe API and the `dj` CLI
app/           the Daijobu Island binary
helper/        daijobu-elevated.exe (launches and places admin apps)
```

## Roadmap

- **F0. Foundation:** transparent topmost window, Skia/D3D12, spring morph, theme engine, tray, hotkeys
- **F1. Daily MVP:** media, visualizer, volume OSD, compact vitals, fullscreen auto-hide
- **F2. Live activities:** timer, lyrics, notifications, **Snap + Snap stack**, **clipboard history**, `dj` CLI, all themes
- **F3. Power tools:** full vitals, Sweep, **Tile, Scenes and Boot Scene**
- **F4. Extras:** file shelf, ambient mixer, mic mute, audio device switcher, glance indicators

## Requirements (target)

- Windows 11 22H2 or later
- A GPU with D3D12 support (a CPU fallback is planned)

## Building

Not buildable yet. Once F0 lands:

```sh
git clone https://github.com/<you>/daijobu-island
cd daijobu-island
cargo build --release
```

Skia may build from source on first compile. That needs LLVM, Ninja, Python 3 and the VS 2022 C++ Build Tools.

## Privacy

There is no telemetry. Clipboard history stays on your machine, encrypted with DPAPI, and items flagged by password managers are skipped. Screenshots are saved locally and nothing is uploaded unless you opt in.

## Acknowledgements

Inspired by Apple's Dynamic Island, by [WinIsland](https://github.com/WinIslandProject/WinIsland) for the island-on-Windows concept, and by [Mole](https://github.com/tw93/Mole) for the system cleanup ideas. Daijobu Island is an independent project with no affiliation to either. **No code is copied from either project.** Both are GPL-3.0, so this project re-implements the ideas from scratch.

## License

TBD. MIT / Apache-2.0 dual license is planned.
