# Daijobu Island — Progress

How we work: **one stage at a time.** Each stage runs in its own Orca worktree, with a Claude Sonnet 5.5 agent spawned there. When a stage is done, its status and log are updated here.

Spec and design: [`docs/plan.html`](../docs/plan.html) · README: [`README.md`](../README.md) · Repo: https://github.com/xDaijobu/daijobu-island

Legend: ⬜ todo · 🟡 in progress · ✅ done · 🔒 locked (coming soon in the app)

---

## Stages

| # | Stage | Status | Worktree / branch | Notes |
|---|---|---|---|---|
| 0 | Toolchain setup (Rust, Windows SDK, LLVM) | ✅ | (main checkout) | Rust 1.99.1, SDK 26100, LLVM 23.1.4, Ninja 1.13.2 |
| 1 | **App shell**: window, island, themes, tray, all features locked | ✅ | `stage-1-app-shell` → `C:/Users/chris/orca/workspaces/daijobu-island/stage-1-app-shell` | Claude Sonnet 5.5 via Orca · brief: [`stage-1-brief.md`](stage-1-brief.md) |
| 2 | Media (SMTC + visualizer) | 🔒 | — | |
| 3 | OSD volume | 🔒 | — | |
| 4 | Vitals (compact) | 🔒 | — | |
| 5 | Timer / Pomodoro | 🔒 | — | |
| 6 | Snap + Snap stack | 🔒 | — | |
| 7 | Clipboard history | 🔒 | — | |
| 8 | Notifications | 🔒 | — | needs sparse MSIX |
| 9 | Lyrics | 🔒 | — | |
| 10 | dj CLI | 🔒 | — | |
| 11 | Tile + Scenes + Boot Scene | 🔒 | — | |
| 12 | Vitals (full) + Sweep | 🔒 | — | |
| 13 | Shelf, Ambient, glance extras | 🔒 | — | |

The order of stages 2+ can still change. Only one stage runs at a time.

---

## Stage 1: App shell

**Goal:** Daijobu Island runs as a real app. The island looks, moves and feels finished, and every feature is shown but **locked as "coming soon"**.

### Scope
- [x] Cargo workspace: `crates/dj-core`, `crates/dj-render`, `crates/dj-win`, `app` (binary `daijobu`)
- [x] Raw Win32 window: `WS_POPUP` + `WS_EX_NOREDIRECTIONBITMAP | TOOLWINDOW | TOPMOST | NOACTIVATE`, Per-Monitor-V2 DPI manifest, top-center of the primary monitor, not in Alt-Tab or the taskbar
- [x] DirectComposition + D3D12 + Skia (skia-safe `d3d`), premultiplied-alpha composition swap chain sized to the maximum expanded state; the pill animates inside it (no per-frame `ResizeBuffers`)
- [x] Click-through outside the pill (the window rect/region tracks the pill)
- [x] Spring morph (response/ζ from the plan), with a content crossfade (blur + scale)
- [x] **Idle** state: clock + status lamp
- [x] **Expanded "home"**: grid of feature tiles (Media, OSD, Vitals, Timer, Snap, Clipboard, Notifications, Lyrics, dj CLI, Tile & Scenes, Sweep, Shelf, Ambient). All 🔒 locked with "coming soon". Clicking a locked tile plays a small shake + "coming soon" hint
- [x] Feature registry in `dj-core` (id, name, icon, `Locked | Available`), so unlocking a feature later is a single registry change plus its view
- [x] Theme engine: TOML schema from the plan, `base` inheritance, contrast validation, token interpolation on switch, hot reload from `~/.daijobu/themes/`
- [x] Built-in themes: Tobacco Tape (default), Midnight Cassette, Paper Zine (light), Clean
- [x] SkSL grain (time quantized to 18 fps) + ordered dither + optional scanlines, driven by theme tokens
- [x] Embedded OFL fonts: Geist, Geist Mono, Doto (license files included)
- [x] Tray icon: Show/Hide, Theme ▸, Start with Windows ✓, Quit
- [x] Hotkeys: `Ctrl+Alt+H` hide/show, `Ctrl+Alt+T` cycle theme
- [x] Single instance (named mutex), autostart (HKCU Run), config `~/.daijobu/config.toml`
- [x] Idle CPU ≈ 0% (blocks in `GetMessage` when nothing is animating)
- [x] `cargo build --release` passes, `cargo clippy --workspace -- -D warnings` is clean, and `dj-core` has unit tests (spring, theme parse/inherit/validate)

### Acceptance
Run `daijobu.exe`. The pill appears at the top center. Clicking it expands smoothly to the home grid, where every feature shows 🔒 coming soon, and clicking again collapses it. `Ctrl+Alt+T` switches themes smoothly, and so does the tray menu. Idle CPU is about 0%.

### Log
- 2026-10-25: stage defined.
- 2026-10-25: toolchain installed. Worktree `stage-1-app-shell` created and the Sonnet 5.5 agent spawned.
- 2026-10-25: **done.** 4 commits on `stage-1-app-shell` (not merged to master yet). Agent report: `task/stage-1-report.md` in the worktree.
- 2026-10-25: re-checked from the main session. `cargo build --release` OK, `clippy -D warnings` clean, 31 tests pass. Ran `daijobu.exe`: the pill is top-center with the lamp and clock, idle CPU was 0 ms over 5 s, RAM about 110 MB.
- Deviations: skia-safe uses default features + `d3d` + `textlayout`, because the prebuilt for a minimal feature set isn't published and a source build hit "Filename too long". The exe is about 8.6 MB.
- Not verified yet (needs a human check): a real right-click on the tray icon, absence from Alt-Tab, runtime DPI change and multi-monitor, device-lost recovery, exclusive-fullscreen games.
- Follow-ups: exe icon, CI (clippy + test), fix the TOML example in plan.html (several keys on one line), activity scheduler (needed before Stage 2).
- 2026-10-25: public repo https://github.com/xDaijobu/daijobu-island created. PR #1 (`stage-1-app-shell` → `master`): https://github.com/xDaijobu/daijobu-island/pull/1

---

## Changelog
- 2026-10-25: Stage 1 (app shell) done.
- 2026-10-25: progress.md created. The plan, the README draft, and the name **Daijobu Island** were finalized.
