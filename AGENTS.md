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
4. **Build only what the stage asks for.** Features outside the current stage stay locked ("coming soon"). Don't add features nobody asked for, and suggest them instead. Screen recording and OCR were explicitly rejected; OCR was removed from the roadmap on 2026-10-28.
5. **License is Apache-2.0.** New dependencies and assets must be compatible (MIT, Apache, BSD, OFL, …). No GPL/LGPL/AGPL. Third-party assets are listed in `NOTICE`.
6. **`unsafe` only in platform or GPU code**, each block with a `// SAFETY:` comment. `dj-core` is `#![forbid(unsafe_code)]` and has no Win32 or Skia dependency.

## Workflow

Development happens **one stage at a time**:

1. The coordinator defines the stage in `task/progress.md` (scope checklist + acceptance) and writes `task/stage-N-brief.md`.
2. An Orca worktree is created per stage (`stage-N-<slug>`) and agents implement there. Keep the plan, scaffold and implementation uncommitted until the implementation is complete and review/tests pass.
3. The agent finishes by writing `task/stage-N-report.md`: an honest per-item checklist, how to run, known issues, and what was not verified.
4. The coordinator re-checks the result (build, clippy, tests, run the exe), resolves review findings, updates `task/progress.md`, then commits and opens a PR into `master`. Merge only after this verification; remove the stage worktree after the merge is confirmed.

When the user requests parallel subagents, keep them within the current stage. Freeze
shared interfaces before dispatch, assign non-overlapping file ownership, and let the
coordinator integrate shared files and manage Git. The Snap/Clipboard dispatch plan is
`task/stage-snap-clipboard-plan.md`; the current stage dispatch file records exact
ownership and interfaces. This conversation remains the coordinator.

When a stage unlocks a feature, flip its entry in `dj-core`'s feature registry from `Locked` to `Available` and handle `ClickOutcome::Open`.

For the user-requested Stage 9 integration, feature tracks use separate Orca
worktrees under one coordinator; see `task/stage-9-integration-plan.md` for
dependencies and the frozen notification interface. Integrate shared wiring
sequentially. When worktrees share a Cargo target directory, grant exclusive
ownership for the entire check/test/build sequence, including test execution:
Cargo's compilation lock alone does not prevent cross-worktree test-binary replacement.

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
crates/dj-clip/      Clipboard: event-driven listener, format normalization, current-user DPAPI and SQLite history
crates/dj-term/      Terminal panel: shell sessions over `alacritty_terminal` (Apache-2.0) and ConPTY, screen snapshots
crates/dj-core/themes/   built-in themes (TOML)
assets/fonts/        embedded OFL fonts + licenses
docs/                plan.html (design) and images/ (README images)
task/                progress.md, stage briefs and reports
```

Crates planned for later stages (`dj-media`, `dj-audio`) are added only when their stage starts. `dj-clip` was added in Stage 8. Tile lives in the existing crates.

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

Windows notification distribution builds embed a pre-signed sparse MSIX using
`DAIJOBU_NOTIFICATION_MSIX` and set `DAIJOBU_REQUIRE_SIGNED_NOTIFICATION_PAYLOAD=1`.
Signing/SDK tools stay on the build machine; never embed private signing keys.
Default builds embed a loose development identity payload usable only with existing
Windows Developer Mode. Setup is an explicit in-app action, followed by graceful
restart and separate Windows consent; it never imports certificates or enables
Developer Mode. See `packaging/notifications/README.md` for build and delivery details.

## Conventions

- Keep idle CPU at about 0%: block in `GetMessageW`, and render only while a spring moves (plus one final frame).
- The swap chain is sized to the largest state and never resized per frame.
- Themes are data (TOML). Text must contrast with the pill at 4.5:1 or the theme is rejected.
- User-facing text is English. Planning docs may be in Indonesian.
- Commit messages are short and imperative. Don't push to `master` directly; use a PR.

## Updating this file

If you change the workflow, layout or a non-negotiable, update this file in the same PR.
