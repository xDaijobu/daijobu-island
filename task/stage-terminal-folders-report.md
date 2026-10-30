# Stage Terminal Folders report

Implementation worktree: `C:/Users/UseR/orca/workspaces/daijobu-island/stage-terminal-folders`, base HEAD `9335f13`. Changes remain uncommitted; no Git mutation, main/release changes, desktop input or running-app lifecycle actions performed. The coordinator owns authorized Git integration, merge and worktree removal.

## Delivered

- [x] Initial empty Terminal open, new-shell shortcut and new Shell/Claude/Codex menu actions show pre-launch chooser; Spotlight folder launch prefills chooser. Existing-tab opens and switching do not launch chooser. Pinned restore and explicit restart remain existing behavior.
- [x] Reuse existing profiles, including their pre-existing permission-bypass flags. No CLI install, third-party process launch, guessed flags or conversation resume behavior.
- [x] Editable folder with selection, caret, Home/End/arrows, Backspace/Delete, Ctrl+A/C/V, Tab/Shift+Tab focus and Enter activation; mouse rows, Browse, Open, Cancel, recent paths, Clear history. Escape cancels idle chooser. Browse/validation temporarily lock chooser input.
- [x] Per-profile last successful folder, six-entry exact-string deduplicated MRU, TOML under `~/.daijobu/terminal-folders.toml`. Clear removes both last folders and MRU, not active field/session state. Never reads conversation or command history.
- [x] Absolute existing directory validation, invalid-path feedback, fake-spawn failure feedback, save failure feedback. Validation runs on bounded one-shot worker with wake notification, no idle polling. Hidden validation is discarded and cannot launch a stale request.
- [x] Native cwd passed unchanged to existing SpawnOptions; path never enters shell command arguments. Native spawn rejects vanished/inaccessible explicit cwd rather than silently falling back home. Existing agent command strings unchanged.
- [x] Explicit-only IFileOpenDialog on dedicated STA worker; COM init failure returned, all COM objects released before balanced CoUninitialize, returned string freed. Owner HWND supplied; focus loss while picker open does not collapse panel.
- [x] Existing themed Skia surface and shared GPU ownership unchanged. Four themes/scales raster regression checks clip full chooser including six recents inside existing panel.

## Validation

Final fmt check, workspace clippy/all-targets with `-D warnings`, full serial workspace tests and optimized `terminal-folders` build passed on 2026-10-30: **1,661 passed, 0 failed, 0 ignored, 0 filtered out**. The final optimized build finished successfully in 1m 08s; the executable was built but never launched. Test log: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/terminal-folders-workspace-tests.log`.

The 2026-10-30 completion run uses the full workspace suite with no test exclusions, superseding the historical result of 1,641 passed with 16 filtered. Fourteen stage tests: three history/path, five fake-controller launch/failure/stale-validation/re-entry/save-feedback, one raster all-theme/DPI, one bounded display, one bulk Unicode editor, one bounded storage-reader and two native cwd regressions. Native test shells are authorized by the current verification request; no real Claude/Codex agent or user app was launched.

An initial native assertion was corrected to omit wide-glyph spacer cells. A full default-concurrency suite then passed; a later default-concurrency rerun hit the existing `conpty_round_trip_echo_then_exited` timeout (output arrived, exit event missed). The coordinator confirmed this intermittent failure predates the stage and is recorded in the upstream v0.3.1 verification history. Final full tests run with `--test-threads=1`, retaining every native/process test; no ConPTY refactor or test exclusion was introduced. The previously filtered popup test creates only its own hidden window and asserts foreground preservation.

Commands (working directory above):

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4" -- -D warnings
cargo test --workspace --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4" -- --test-threads=1
cargo build --profile terminal-folders --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4"
```

Executable (built, not launched): `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/terminal-folders/daijobu.exe`.

## Exact files

- `Cargo.toml`: separate release-inheriting terminal-folders profile, no version edit.
- `app/src/app.rs`: history wiring, save feedback, picker blur handling and view.
- `app/src/storage.rs`: local folder-only TOML read/write.
- `app/src/terminal.rs`: chooser entry/input isolation, native cwd preservation and five regressions.
- `app/src/terminal/folders.rs`: chooser controller, validation/picker completion.
- `crates/dj-core/src/lib.rs`: folder model export.
- `crates/dj-core/src/line_edit.rs`: optional per-editor length ceiling; existing default unchanged.
- `crates/dj-core/src/terminal_folders.rs`: history, validation, layout/view and three regressions.
- `crates/dj-render/src/terminal.rs`: themed chooser and raster regression.
- `crates/dj-term/src/session.rs`: reject invalid explicit cwd, never fallback from explicit path.
- `crates/dj-term/src/tests.rs`: native explicit cwd preservation and invalid cwd rejection regressions.
- `crates/dj-win/src/lib.rs`: picker export.
- `crates/dj-win/src/folder_picker.rs`: native explicit-only STA picker.
- `task/progress.md`, `task/stage-terminal-folders-brief.md`, `task/stage-terminal-folders-report.md`.

## Independent review fixes (2026-10-29)

All three P2 performance findings addressed, no launch/cwd or GPU changes. Follow-up exact files: `app/src/storage.rs`, `crates/dj-core/src/terminal_folders.rs`, `crates/dj-core/src/line_edit.rs`, `crates/dj-render/src/terminal.rs`, this report and `task/progress.md`.

- Storage uses stdlib `Read::take(2 MiB + 1)` before parsing. Oversize, invalid UTF-8 and I/O errors produce empty history and logged read error (missing file is quiet). No truncated TOML is parsed. Pure parser also rejects over-cap strings before TOML parsing. 2 MiB accommodates nine maximum-length valid UTF-8 path strings plus normal TOML serialization; unknown/corrupt material is bounded by same byte cap.
- Recent paths are moved, not cloned into unbounded dedup state; at most six retained strings compared, stop once six unique valid entries found. Last-profile storage remains fixed three strings, validated against existing UTF-16 path ceiling; invalid strings cleared without truncating valid cwd. Parser still materializes TOML within fixed byte cap.
- Chooser labels and errors are limited to 160 Unicode scalar values plus ellipsis before existing fit routine. Full editor/history/native cwd remains unchanged. Rendering cost no longer scales quadratically with full path length.
- Bulk insertion computes remaining capacity once and byte offset once, filters/normalizes control characters, inserts one string and advances char-index caret once. Existing default length, caller selection handling and Unicode semantics unchanged.
- Four added regressions exercise exact reader consumption cap/invalid UTF-8/oversized Unicode; huge corrupt and duplicate-heavy history; 32K Unicode insertion at middle caret and equivalence to old per-char semantics; huge Unicode display/source preservation. No timing-threshold claims or live smoothness measurements.

Historical gates used the same 16 exclusions. One initial clippy module-order diagnostic and one incorrect test expectation about TOML fixed-array surplus entries were corrected before that clean rerun. The full verification requested on 2026-10-30 supersedes those exclusions (see Validation).

## Completion review fixes (2026-10-30)

- Replacing a chooser through `open_in` could leave the previous validation receiver attached, allowing its completion to launch the previous profile/folder without Open in the replacement chooser. `choose_folder` now discards pending validation before replacing the chooser; a fake-spawner regression verifies re-entry starts nothing and only a new Open launches the replacement profile.
- A folder-history save error persisted indefinitely after later saves succeeded. Save results now replace/clear the shared persistence warning, trigger redraw when that state changes, and show the warning inside or outside the chooser. Current validation/spawn errors retain priority; a controller regression verifies recovery and validation-error visibility.
- Removed two redundant chooser checks after earlier unconditional chooser returns; input isolation remains intact.
- Picker completion now refreshes terminal focus state from the actual owner HWND on its wake. Previously Browse blur left the terminal marked unfocused even if the native dialog returned focus to the owner; this refresh never requests foreground activation. Native dialog focus transfer itself still needs manual acceptance.
- Added native regressions for missing/file cwd rejection without home fallback, and a hidden ConPTY `cmd.exe` session reporting its actual cwd with spaces, Japanese characters and shell metacharacters. The command is constant and the path enters only `SpawnOptions.cwd`. The initial native assertion mistakenly included double-width glyph spacer cells; corrected the test helper to omit those layout cells before the final clean run.

## Honest manual limits / review items

No visual inspection, desktop/picker interaction, actual agent startup, UNC share, long-path OS behavior or smoothness measurement. Local native ConPTY cwd with spaces/Unicode/metacharacters is verified, alongside filesystem fixtures and fake native launch options. UNC syntax is accepted as path data but requires a reachable share and host support; no blanket UNC-support claim (cmd.exe has its own UNC cwd limitation).

Directory validation is off UI thread, but existing synchronous ConPTY spawn plus final existence check remain UI-thread work; slow network directories/process startup may still stall at final launch. Native picker owner/reentrancy/focus behavior needs live acceptance. Validation native filesystem calls are not cancellable once entered; hiding discards result, no stale launch. Picker thread exits on native dialog completion; app process exit ends it.

Folder field scrolls logical character window around caret; clicking field selects editing focus, not precise caret position. No text shaping/IME changes. History uses exact path spelling (case/aliases may appear separately). Persisted history is plaintext local config, not encrypted content storage. Writes reuse existing direct-write convention, not crash-atomic; save failure reported. Existing session restart/pinned restore deliberately bypass chooser. Chooser is shown for explicit new-session entry, not a separate Settings toggle. Code review is complete; manual/product acceptance remains pending.

Native smoke options inspected: `app/src/main.rs` has no smoke/headless argument or isolated test mode; normal startup acquires the global single-instance mutex and initializes the actual app. Launching the built executable would either exit against the running instance or start another desktop overlay, so it was not launched. The full workspace suite safely exercises hidden ConPTY sessions, shell I/O/exit/shutdown/wheel handling, actual local cwd and hidden popup geometry/foreground preservation. Raster tests cover all built-in themes/scales without touching the live D3D renderer. Native Browse ownership/STA focus/reentrancy, live D3D appearance/device recovery and manual smoothness still require an interactive session when desktop disruption is acceptable; no claim of those checks passing.
