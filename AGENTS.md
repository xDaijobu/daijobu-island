# AGENTS.md

Guidance for coding agents (and humans) working on Daijobu Island.

## What this is

Daijobu Island is a dynamic-island overlay for Windows 11, written in Rust and drawn with Skia. It is a small pill at the top of the screen that expands into panels: media, screenshots, clipboard, window tiling, system vitals, and so on. The look is "modern but lofi": spring motion, film grain, ordered dither and mono/dot-matrix type.

- Spec and interactive design: `docs/plan.html`. Open it in a browser. This is the source of truth for tokens, sizes, springs, themes and feature behaviour.
- Progress and stage contracts: `task/progress.md`.
- Per-stage briefs and reports: `task/stage-N-brief.md`, `task/stage-N-report.md`.

## Non-negotiables

1. **Rust + Skia (`skia-safe`, D3D12) + native Win32 (`windows` crate).** No winit, webview, Electron, egui or tokio in the UI path.
   Exception: WebView2 is allowed only as the hidden YouTube Music engine and its sign-in/browse window (`dj-music`).
2. **The name is "Daijobu Island".** Never write "Daijōbu". The binary is `daijobu.exe`, the config dir is `~/.daijobu/` and the CLI is `dj`.
3. **No GPL code.** WinIsland and Mole are GPL-3.0. They can be used as references for ideas and API choices only. Never copy their code or lists verbatim, and never use the name "Mole".
4. **Build only what the stage asks for.** Features outside the current stage stay locked ("coming soon"). Don't add features nobody asked for, and suggest them instead. Screen recording was explicitly rejected.
5. **License is Apache-2.0.** New dependencies and assets must be compatible (MIT, Apache, BSD, OFL, …). No GPL/LGPL/AGPL. Third-party assets are listed in `NOTICE`.
6. **`unsafe` only in platform or GPU code**, each block with a `// SAFETY:` comment. `dj-core` is `#![forbid(unsafe_code)]` and has no Win32 or Skia dependency.

## Workflow

Development happens **one stage at a time**:

1. The coordinator defines the stage in `task/progress.md` (scope checklist + acceptance) and writes `task/stage-N-brief.md`.
2. An Orca worktree is created per stage (`stage-N-<slug>`) and an agent works there. Commit in small steps on that branch and don't push.
3. The agent finishes by writing `task/stage-N-report.md`: an honest per-item checklist, how to run, known issues, and what was not verified.
4. The coordinator re-checks the result (build, clippy, tests, run the exe), updates `task/progress.md`, and opens a PR into `master`.

When a stage unlocks a feature, flip its entry in `dj-core`'s feature registry from `Locked` to `Available` and handle `ClickOutcome::Open`.

## Layout

```
app/                 binary crate `daijobu` (window wiring, tray, storage, main loop)
crates/dj-core/      domain: springs, themes, feature registry, layout, island state machine
crates/dj-render/    Skia on D3D12 + DirectComposition, fonts, SkSL effects, icons, painting
crates/dj-win/       Win32: window, message loop, tray, hotkeys, autostart, single instance, top AppBar strip
crates/dj-music/     YouTube Music engine: hidden WebView2, page bridge (`bridge.js`), artwork fetch
crates/dj-vitals/    Win32 samplers for the Vitals panel: CPU, RAM, system drive, net, disk I/O, battery, uptime, top processes
crates/dj-sweep/     Sweep (inside Vitals): category scanners, protected-path guard, clean/recycle ops, operations log
crates/dj-capture/   Snap capture: monitor freeze (DXGI Duplication, GDI fallback), window rects, WIC PNG, clipboard (CF_DIB + PNG)
crates/dj-core/themes/   built-in themes (TOML)
assets/fonts/        embedded OFL fonts + licenses
docs/                plan.html (design) and images/ (README images)
task/                progress.md, stage briefs and reports
```

Crates planned for later stages (`dj-media`, `dj-audio`, `dj-clip`, `dj-tile`, `dj-ipc`) are added only when their stage starts.

## Commands

```sh
cargo build --release                               # target/release/daijobu.exe
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo fmt --all
```

Toolchain: stable Rust (MSVC), VS Build Tools with the C++ workload, and Windows SDK 10.0.26100+. LLVM is only needed if Skia falls back to a source build.

- `skia-safe` is pinned and uses default features plus `d3d` and `textlayout`, because that combination has a published prebuilt binary.
- A source build on Windows can fail with "Filename too long" unless the repo path is short or git long paths are enabled.

## Conventions

- Keep idle CPU at about 0%: block in `GetMessageW`, and render only while a spring moves (plus one final frame).
- The swap chain is sized to the largest state and never resized per frame.
- Themes are data (TOML). Text must contrast with the pill at 4.5:1 or the theme is rejected.
- User-facing text is English. Planning docs may be in Indonesian.
- Commit messages are short and imperative. Don't push to `master` directly; use a PR.

## Updating this file

If you change the workflow, layout or a non-negotiable, update this file in the same PR.
