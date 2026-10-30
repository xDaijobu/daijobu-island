# Stage 9c report: Explorer starting folders

Implemented on the Terminal Folders prerequisite, then integrated by the coordinator
onto master `5b5d07f` (Windows notifications PR #37 and edge reveal PR #38).

## Scope

- [x] New Shell/Claude/Codex chooser defaults to Explorer first. Settings → Terminal
  → Start in active Explorer folder disables that preference, retaining per-profile
  last folder, configured cwd and home fallback. Config is `terminal.follow_explorer`.
- [x] Explicit Spotlight paths, user edits and explicit folder selections win over
  delayed Explorer results. Empty/invalid explicit paths are validated rather than
  silently replaced. Untouched refreshes with no usable active view restore fallback.
- [x] Up to six unique Explorer folders, source switching with F6/source-row click,
  F5 refresh, loading/unavailable/partial feedback, Recent and native Browse retained.
- [x] A folder click selects chooser data only; Open still validates and confirms
  launch. No existing-session `cd` or other input is injected. Native cwd, provider
  arguments/permissions, pinned restore and restart behavior remain unchanged.
- [x] Existing foreground event hook records Explorer HWND without activation.
  Each bounded enumeration uses a dedicated STA thread, sends data over a channel
  and wakes the owner. Requests coalesce; no permanent polling or directory scan.
- [x] Explorer automation reads at most 128 registrations, resolves active shell
  views through SID_STopLevelBrowser/IShellBrowser/IFolderView/IPersistFolder2,
  requires a visible view belonging to the reported top-level Explorer window,
  checks again after filesystem validation, and rejects conflicting registrations.
- [x] Filesystem folder attributes, filesystem display names and existing native
  absolute-directory validation reject virtual namespaces, invalid paths and files.
- [x] Generation checks reject stale Explorer completions after chooser reentry,
  hide, edits, Browse, explicit selection and Open; snapshots never launch a session.
- [x] README and stage brief document the interaction and native limits.

## Validation

The coordinator explicitly granted this worker exclusive ownership of the shared
`C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4` Cargo target. No Cargo ran
before that grant. All commands below use that target and run sequentially.

- [x] `cargo fmt --all`, final `cargo fmt --all -- --check`, and `git diff --check`.
- [x] `cargo clippy --locked --workspace --all-targets -- -D warnings`.
  Initial closure/unwrap lint findings were corrected before the passing run.
- [x] `cargo test --locked --workspace -- --test-threads=1`: 1,689 passed,
  zero failed/ignored. Per test binary: app 308, capture 8, clip 82, core 782,
  music 0, render 177, sweep 35, term 73, vitals 26, win 198; doc tests passed.
- [x] `cargo build --locked --profile terminal-folders`: optimized build passed
  in 2m 38s; this profile inherits release optimization/LTO settings.
  Artifact: 27,659,776 bytes; SHA-256
  `38B36576B580BF064E66634EFD3B9A02A6478E4A94C186AD515FE0835AD2E789`.
- [x] `cargo test --locked -p dj-win explorer::tests::read_only_explorer_smoke --
  --test-threads=1 --nocapture`: passed. The command-output condenser omitted its
  diagnostic, so the freshly built `dj_win-7ef5f2017bc39c04.exe` was run directly
  with that exact test filter while the exclusive slot was still held: two real
  filesystem folders, `active=false`, `incomplete=false`, passed in 0.04 seconds.
  This enumerates existing views only; the standalone test does not install the
  application's foreground hook, so it does not verify foreground tracking.

All Cargo processes exited before the worker explicitly reported the shared slot
free to the coordinator. No source changed after the passing Clippy/test/build
sequence; only this report was finalized. Git's final whitespace check emitted
normal LF-to-CRLF working-copy conversion notices for the two terminal Rust files,
with no whitespace errors.

Regression coverage includes explicit-path precedence, profile/config/home fallback,
latest Explorer selection, ambiguity rejection, stale async results, edited/selected
field preservation, empty/unavailable refresh fallback, invalid/virtual/file paths,
six-folder bounds and the full chooser layout across themes/scales. Session tests
use fake backends and spawners; they never launch real providers.

## Run and native acceptance limits

After validation, run the optimized artifact at
`target/stage-t4/terminal-folders/daijobu.exe` in the main checkout's target directory
when the coordinator/user chooses to replace their running app. Open a new terminal
chooser while a filesystem Explorer window is active, inspect the suggested path,
switch sources with F6, refresh with F5, choose a folder and confirm Open.

This worker does not terminate/restart the running application or start real
Claude/Codex sessions. Interactive chooser input, provider startup, Settings clicks
and pinned restart were not manually exercised in a running build. Existing tests
cover their preserved controller behavior.

Windows 11 inactive/hidden tabs are intentionally omitted. The active-view and
visible-HWND checks are conservative native snapshots, not an exhaustive tab API or
stable tab identity contract across Windows builds. Conflicting registrations are
skipped instead of guessing; Home/This PC/search/Recycle Bin/non-filesystem shell
namespaces are unavailable. Foreground history begins with this process; closed,
unavailable or non-filesystem last-active windows use last/config/home. A COM or
network filesystem call may be slow; the UI keeps Browse/recent/fallback available
and never waits on that worker. These checks do not navigate, focus or inject input
into Explorer. Microsoft's active-view API is documented at
https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ishellbrowser-queryactiveshellview.

No dependencies or third-party assets were added. Shared progress/integration-plan
changes restored by the coordinator were left untouched. Git and final integration
remain coordinator-owned; this worker does not commit, push or open a PR.
