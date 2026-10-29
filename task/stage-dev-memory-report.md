# Stage Dev Memory report

Base `f700575`, worktree `C:/Users/UseR/orca/workspaces/daijobu-island/stage-dev-memory`. Independent review found no blockers. User authorized commit, PR, merge and release on 2026-10-29. Integration revalidation passed fmt check, locked workspace clippy/all-targets with warnings denied, locked workspace tests and locked optimized memory-opt build using target/stage-t4. Existing app was not started, stopped or restarted. Native RAM, under-100MB, smoothness, DPI and device-loss acceptance remain unverified.

## Approved RAM optimization (code complete; independent review/runtime acceptance pending)

Scope recorded before implementation. Island and every PopupRenderer (tray, Spotlight, Settings, diagnostics) now share one UI-thread-owned D3D12 device/queue/Skia DirectContext. Rc/RefCell enforces same-thread ownership and exclusive access; thread-local slot contains only Weak, so it cannot pin final context. No new dependency or unsafe sharing. Separate per-window swapchains, fonts, icon/painter state and Present(1) remain warm; no per-click context churn, per-frame purge, working-set trimming or spring/timer change. Sharing removes redundant context ownership, but does not itself measure saved RAM.

Each renderer drops wrapped surfaces and flushes/submits/syncs before chain destruction; chain fields precede shared owner so D3D/composition objects drop before final context. Fonts/Painter initialize before chain creation so their errors cannot strand a successful chain. Partial chain-construction failures clear/sync already-wrapped buffers before releasing swapchain. Device health is checked before reuse/draw: main follows existing rebuild path; popup rebinds its chain to current valid pooled context on next render/resize. Same-size resize retries missing chains after failure. Older hidden owners can retain removed context until next render or destruction; live-context count includes them.

Diagnostics reports live unique Gpu instances on current UI thread (including independent capture/tile/preview/pin stacks), plus current pooled cache usage once and pooled renderer-owner count. Removed older context generations and independent stacks are included in context count but their cache byte totals remain uninstrumented; no cache sum claims process RAM. Window buffer estimates and font-source counters remain separate and overlapping. Independent capture/tile/preview/pin contexts and Fonts sharing are outside narrow optimization scope.

Three pure regression checks exercise real weak-pool selection logic: pointer reuse, exclusive RefCell borrow, remaining-owner survival, final-owner release/reopen, removed-generation replacement, and failed replacement preserving existing owner. Native D3D device-loss injection, swapchain lifetime validation and visual smoothness are not covered by these pure tests.

Under-100MB target remains aspirational. Read-only evidence supplied before implementation found three private committed resident PAGE_WRITECOMBINE blocks, 32 MiB each; graphics upload pools remain hypothesis, not proven per-context allocations. No claim of 64 MiB savings, measured new RAM or target attainment. No app execution/launch/stop/restart, desktop input or Git mutation performed; existing diagnostics edits preserved.

### Optimization validation (2026-10-28)

- `cargo fmt --all`, `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4" -- -D warnings`: passed. Initial run found test-module ordering lint; moved tests after production items, rerun clean.
- `cargo test --workspace --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4" --quiet`: 1,647 passed, zero failures/ignored/filtered (render 174; other crate counts unchanged below).
- `cargo build --profile memory-opt --config 'profile.memory-opt.inherits=\"release\"' --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4"`: passed, optimized, 1m 04s. Command uses PowerShell escaping as in earlier build.
- `git diff --check`: passed.

Optimization executable: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/memory-opt/daijobu.exe`. Existing running dev-memory-list executable untouched.

Optimization changed only `app/src/app.rs` (start resource rows with shared counters), `crates/dj-render/src/gpu.rs`, `crates/dj-render/src/popup.rs`, `crates/dj-render/src/lib.rs`, and existing stage brief/progress/report. Earlier uncommitted diagnostics changes in these and other files remain.

Coordinator acceptance still needed: independent ownership/recovery review; matched baseline/optimized idle and warmed tray/Spotlight/Settings/diagnostics process private resident/private commit samples; quick reopen/spring frame pacing; DPI resize, close order and device-loss recovery. No runtime measurements were taken by this optimization task.

## User refinement: Task Manager-style list

User tested original build and requested KB/MB/GB, task-manager-like list, and descriptions explaining purpose. Replaced raw string dump with shared platform-free structured rows and themed table: fixed title, Refresh/Close controls, column header, alternating rows, section groups, scrollbar and range footer. Processes sort resident memory descending (unavailable last), keep main app clearly labelled, and show PID, resident memory, private commit and peak. On narrower layouts commit/peak move into row details. Resource rows show adaptive usage and concise purpose/measurement descriptions, not fake allocations for counts. Decimal units: 1 KB = 1,000 bytes; MB = 1,000,000; GB = 1,000,000,000. GPU/exact/estimate/count distinctions remain visible.

Text is measured with active font, clipped per column with ellipsis, purpose wraps into two measured lines. Long details remain bounded/truncated rather than extending outside window. Native sampler/ancestry unchanged. Independent reviewer wheel-direction finding fixed (positive wheel scrolls up); worker spawn/disconnect errors now persist across refresh, mark dirty and clear on successful sample. Tests cover units, sorting, direction and retained failure feedback.

## Delivered

- [x] Default-off persisted `developer_mode`; Settings > General toggle enables and opens diagnostics. Turning it off closes diagnostics. Tray > Developer memory explicitly enables and opens it; reopening never samples automatically at startup.
- [x] Dedicated active-theme Skia/native popup; scrollable bounded rows, wheel/arrows/Page/Home/End, R/Refresh-button refresh, top-right Close/Escape/Alt+F4. Popup destroyed on close to release its renderer. Theme crossfade follows app theme.
- [x] Worker-only native process snapshot and memory collection, at most once per second, at most one outstanding request. Timer killed and worker cancellation requested on close; no new sampling dispatch while closed. Already-entered native OS call cannot be interrupted; worker checks cancellation between records and discards cancelled results.
- [x] App PID/name and verified descendants only. Handles remain held during sampling; creation times must predate snapshot and child creation must be strictly later than current parent's creation, all the way to app. Denied/exited/unverifiable candidates excluded with omitted count. Generic descendant role intentionally avoids guessing shell/ConPTY/WebView ownership from basename. Missing intermediates mean descendants omitted, not guessed.
- [x] Native EX2 working set/private working set/private commit/peak working set; EX fallback reports private WS unavailable. Missing values remain Unavailable. Subtotals show readable/total counts; resident shared-page double-count caveat and peak/commit distinctions displayed.
- [x] Actual island and retained popup buffer counts/dimensions with BGRA estimates, Skia resource-cache byte/resource usage (GPU, not process RAM), font cache counts and embedded source lengths, island art/thumb pixel estimates, Spotlight icon pixel estimate/count/index count, exact Snap stack referenced pixel/PNG lengths, unique pin ledger bytes including operation holds, clipboard selected payload retained data length, terminal tab/backend count and actual history-line/grid-cell counts via nonblocking lock (busy terminals skipped).
- [x] Counters explicitly overlap, never summed into process allocation, never subtracted to invent unaccounted RAM. GPU cache usage not GPU budget. No content export or automatic diagnostics persistence. No clipboard/terminal text, titles or command lines collected by diagnostics.
- [x] Pure ancestry/reuse/denied/cycle, unavailable/zero, bounded scrolling, default-off Settings toggle and closed-worker tests; native bounded read-only sampler test exercises own PID and native memory fields. Existing tray navigation expectations updated for new command; popup-kind roundtrip extended.

## Validation

Final commands completed successfully on 2026-10-28:

- `cargo fmt --all` and `cargo fmt --all -- --check`.
- `cargo clippy --workspace --all-targets --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4" -- -D warnings`.
- `cargo test --workspace --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4" --quiet`: 1,644 passed, zero failures/ignored/filtered. Counts: app 297, capture 8, clip 82, core 763, render 171, sweep 35, term 71, vitals 26, win 191; music/doc tests zero.
- `cargo build --profile dev-memory-list --config 'profile.dev-memory-list.inherits=\"release\"' --target-dir "C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4"`: optimized success. PowerShell escaping preserved.
- `git diff --check`: success.

Executable: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/dev-memory-list/daijobu.exe`.

Initial tests found three tray tests with old numeric row indices; expectations corrected and entire suite passed. Initial compilation caught missing Config test field and wheel field mismatch; corrected before successful final gates.

## Limits / not verified

No desktop control, app launch/restart/stop, sounds or Git mutations performed. Existing app remains untouched. User/coordinator must manually validate popup appearance, font wrapping, controls, DPI/multi-monitor behavior, theme changes, close/reopen cadence and real app descendants. No live app RAM measurement or claimed explanation of user's 100MB total.

Internal counters are partial instrumentation, not heap profiler. Other capture/pin/preview GPU surfaces, decoded image/typeface heaps, terminal cell allocation capacity/extra graphemes, clipboard SQLite/cache/queues and native WebView/driver heaps remain unmeasured. Terminal counters read metadata only and skip busy locks. Snap stack reference lengths may overlap across consumers; pin ledger deduplicates only its own ledger. Retained popup font-source lengths repeat per renderer and must not be summed as unique process allocation.

Process list capped at 32,768 entries, 256 candidate PIDs and 64 ancestry generations; note displays these bounds. Exited intermediate ancestors cannot be reconstructed. Sampling is not atomic across processes. Cancellation cannot interrupt native calls already executing. Popup uses existing custom-drawn infrastructure, not native accessibility controls. Column labels use measured ellipsis and purposes wrap to two measured lines, with longer details truncated. Very small monitors retain one clipped row minimum; practical DPI/font/small-monitor readability still needs manual check. No export added.
