# Stage 1 brief: App shell (for the worktree agent)

You are implementing **Stage 1** of Daijobu Island: a lofi-modern "dynamic island" overlay for Windows 11.

## Read first (absolute paths in the main checkout. Read them, don't edit them)
- Spec and interactive design: `C:/Users/chris/orca/projects/daijobu-island/docs/plan.html`. Read the design tokens, the themes TOML schema, the springs, the per-state sizes, the architecture and the risks.
- Stage scope and acceptance criteria: `C:/Users/chris/orca/projects/daijobu-island/task/progress.md`, section "Stage 1: App shell". **That checklist is your contract.**
- README: `C:/Users/chris/orca/projects/daijobu-island/README.md`. Copy it into your worktree root.

## Non-negotiables
- **Rust + Skia (`skia-safe`, `d3d` feature) + native Win32 via the `windows` crate.** No winit, webview, Electron, egui or tokio.
- The name is **Daijobu Island** (never "Daijōbu"). The binary is `daijobu.exe`. Config lives in `~/.daijobu/`.
- WinIsland and Mole are GPL-3.0. **Do not copy their code.** You may read WinIsland for API patterns only.
- Stage 1 is the **shell only**. Every feature tile is locked ("coming soon"). Do not implement Media, Snap or any other feature, and **do not add features that are not in the checklist.**
- Every `unsafe` block gets a `// SAFETY:` comment. Keep `dj-core` free of unsafe and platform code.

## Technical guidance (from research; verify against current docs)
- Pin `skia-safe` (≈0.153, features `d3d`, `textlayout`, `binary-cache`) and `windows` 0.62 (match what skia-safe depends on).
- D3D12 device + DIRECT queue → `gpu::direct_contexts::make_d3d`. Create the swap chain with `CreateSwapChainForComposition`: 2 buffers, FLIP_SEQUENTIAL, `DXGI_ALPHA_MODE_PREMULTIPLIED`, sized to the max expanded state × DPI.
  - Wrap each back buffer through `backend_render_targets::make_d3d` → `wrap_backend_render_target`.
  - Each frame: `clear(TRANSPARENT)` → draw → `flush_surface_with_access(Present)` → `submit` → `Present(1)`.
- DComp: `DCompositionCreateDevice2` → `CreateTargetForHwnd(hwnd, true)` → visual → `SetContent(swapchain)` → `Commit`.
- Window: `WS_POPUP`, ex-styles `NOREDIRECTIONBITMAP | TOOLWINDOW | TOPMOST | NOACTIVATE`, `WM_MOUSEACTIVATE` → `MA_NOACTIVATE`.
  - Use a Per-Monitor-V2 manifest (embed it via `build.rs`) and handle `WM_DPICHANGED`.
  - `HTTRANSPARENT` doesn't work across processes, so make the window rect (or `SetWindowRgn`) track the pill.
  - Re-assert topmost on `EVENT_SYSTEM_FOREGROUND`.
- Idle: block in `GetMessageW`. Render only while a spring is moving, plus one final frame.
- Springs: mass 1, `ω=2π/response`, `k=ω²`, `c=2ζω`, semi-implicit Euler, 1/240 s substeps, retarget keeps velocity. Expand 0.45 / 0.75, collapse 0.35 / 0.95.
- SkSL grain/dither snippets are in plan.html. Compile `RuntimeEffect`s once at startup.
- If the skia-safe prebuilt binary download fails and it falls back to a source build, LLVM + Ninja + Python are needed. LLVM is being installed at `C:\Program Files\LLVM` (set `LIBCLANG_PATH` / `LLVM_HOME` if needed). Report it if a build blocks on tooling.

## Toolchain on this machine (installed 2026-10-25)
- Rust 1.99.1 stable-msvc (+ clippy, rustfmt): `%USERPROFILE%\.cargo\bin`
- MSVC Build Tools 2026 (14.50) and Windows SDK 10.0.26100
- LLVM 23.1.4: `C:\Program Files\LLVM\bin` (set `LIBCLANG_PATH` to this if bindgen needs it)
- Ninja 1.13.2: `%LOCALAPPDATA%\Microsoft\WinGet\Links`
- Your terminal may predate these installs. If `cargo`, `ninja` or `clang` aren't found, prepend those folders to `PATH` in your shell session.

## Workflow
1. Scaffold the workspace. Get a transparent topmost pill on screen early, then iterate.
2. Commit in small, meaningful steps on your branch. Don't push and don't open a PR.
3. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings` and `cargo test` before declaring done.
4. Launch `target/release/daijobu.exe` and sanity-check: the pill shows, clicking expands and collapses it, the theme hotkey works, and idle CPU is about 0. If you can, take a screenshot into `task/stage-1-screenshot.png` in your worktree.
5. Finish by writing `task/stage-1-report.md` in your worktree. Include: a checklist with each item marked done or not (be honest), how to run, known issues, and suggested next steps. Then stop.
