# Handover (2026-10-26, ~08:50)

## State
- `master` is clean and up to date. Merged today:
  - #7 Stage 5 safety fixes
  - #8 Stage 6 Timer
  - #9 README (WebView2 note)
  - #10 Stage 7a Snap core
  - #11 Snap hotkey fix
  - #12 handover
  - Stage S **Settings (minimal)**: gear on Home → autostart toggle + Snap hotkey chips (applied live)
- Tests: 303 passing. fmt, clippy `-D warnings` and `build --release` are clean.
- `daijobu.exe` (release, current master) is running on the user's machine.

## Next: Stage 7b (Snap annotations, pin, OCR), brainstorming in progress
Decided with the user so far:
- **Edit flow:** "edit" on the snap thumb. Release still auto-copies (the 7a fast path is unchanged). The thumb gets an **edit** button that opens the shot in an editor window with the toolbar; on done, the edited image replaces the clipboard copy. This matches the plan's thumb mockup (copy/save/edit/pin/ocr).

Still to ask, one question at a time, before writing the spec:
- the tool set and defaults (plan: arrow, box, pen, highlighter, text, blur/pixelate, step numbers, undo), colours and stroke width
- pin: a small floating window. Click-through? Opacity? How is it closed?
- OCR: `Windows.Media.Ocr` (offline, WinRT). Where does the text go (clipboard and/or a panel)? Which languages?
- whether the editor is a separate window or reuses the overlay

Then write the Stage 7b contract into `task/progress.md` plus `task/stage-7b-brief.md`, get the user's OK, create a worktree `C:/Users/UseR/orca/workspaces/daijobu-island/stage-7b-...` and spawn one background agent (same prompt shape as 7a). Re-verify, live-test with the user, PR, merge. 7c (Snap stack) comes after 7b. **Settings window** is a later stage (row `S` in progress.md).

## Build on this machine (important)
- Rust 1.99.1 (rustup). Windows SDK 10.0.26100 is installed.
- VS 18 Insiders has **no x64 CRT**, so the CRT comes from xwin (`~/.xwin-tool/splat`). For every cargo command run `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh` first. It sets LIB/INCLUDE/PATH, `CARGO_BUILD_JOBS=8` and the shared `CARGO_TARGET_DIR` (the main checkout's `target`).
- The permanent fix is to add "MSVC x64/x86 build tools" in the VS Installer.
- After deleting a worktree, the shared target keeps that worktree's manifest path. Run `cargo clean -p daijobu` and `cargo clean -p daijobu --release` if linking fails with `c1010070 … daijobu.manifest`.
- The empty folder `workspaces/daijobu-island/stage-7a-snap` is still locked by some process. It is harmless; delete it later.

## Live testing notes
- Drive the island with **posted messages** (`PostMessageW` of `WM_LBUTTONDOWN/UP` to `DaijobuIslandWindow`, and `WM_HOTKEY` ids 1=toggle, 3=snap, 4=snap Esc, 5=snap alt). Never move the user's real cursor. The scratchpad scripts `drv.ps1` / `post.ps1` were session-only, so recreate them as needed.
- A GDI `CopyFromScreen` can't see the fullscreen Snap overlay, and the overlay grabs all input, so the user tests overlay flows by hand.
- Screenshots may contain private content (chats). Use small crops or downscaled shots only, and delete them after.
- Don't clean or delete real user files without the user's OK (Sweep live test: temp only, a throwaway project for dev purge).

## User preferences seen today
- Replies in casual Indonesian. Wants autonomy: "lanjut", "merge aja" after live tests pass.
- Wants PR + merge after each verified stage, and short progress answers ("cek progress").
- The snap alt hotkey Ctrl+Space was the user's explicit choice (warned about IDE autocomplete).
