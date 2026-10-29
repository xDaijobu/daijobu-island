# Stage Dev Memory brief

Base: `f700575`; worktree: `stage-dev-memory`.

## Scope

Opt-in developer mode accessible from Settings and tray. Active-theme Skia/native diagnostics popup with bounded navigation and refresh. Read-only native process metrics from existing Windows bindings; app plus descendants only when parent identity/creation times prove ancestry. Show PID, executable basename, role, working set, private working set if available, private committed bytes and peak with precise labels. Sum only comparable available metrics and warn shared resident pages may count twice.

Sample on worker, no more than 1 Hz while visible, zero samples while closed. Bound process/table work and handle denied, exited and reused PIDs honestly. Internal accounting uses real retained lengths/counts and explicitly labelled estimates for existing graphics/cache/font/image/terminal/clipboard/snap data where feasible. Never imply these overlapping counters sum to process allocation; GPU budget is not allocation.

No clipboard/terminal contents, window titles or command lines. No automatic persistent export. No new dependencies, desktop control, app launch/restart/stop, sounds or Git mutation. Existing user app remains untouched.

## User refinement (2026-10-28)

Replace text dump with theme-consistent Task Manager-style process and resource lists. Sort process working sets descending; show clear app identity, PID, memory, private commit and peak where width permits. Adaptive decimal KB/MB/GB (1 KB = 1,000 bytes), honest unavailable states, exact/estimate/count labels, and concise purpose descriptions per resource. Keep keyboard/wheel navigation, bounded small-monitor/DPI layout and privacy/overlap caveats. Preserve sampler behavior; fix independent-review wheel direction and persistent worker-error feedback. Build separate optimized `dev-memory-list` profile because user's original diagnostics build is running.

## Approved RAM optimization (2026-10-28)

Target under 100 MB when feasible, never at cost of smoothness; target is not guaranteed. Preserve all diagnostics work. Share one UI-thread-owned D3D12 device/queue/Skia DirectContext across island and PopupRenderer windows using Rc/RefCell and a non-owning thread-local weak slot. Keep independent swapchains, warm popup/font state and vsync; no per-click context recreation, working-set trimming or per-frame cache purges. Bound scope to island and popups; capture, tile, preview and pin contexts remain independent. Handle device removal and surface-before-context destruction without unsafe sharing or cross-thread access. Add unique-context diagnostics and lifecycle regression checks. Font sharing deferred unless trivial.

Read-only scan found three private committed resident PAGE_WRITECOMBINE allocations of 32 MiB each. Graphics upload pools are plausible, not proven; do not claim these map one-to-one to contexts or promise 64 MiB savings. Runtime RAM and smoothness remain unverified without coordinator testing. Build new release-inheriting memory-opt profile against target/stage-t4; never run/stop/restart app or use desktop input.

## Validation

Pure attribution/accounting/navigation tests and bounded read-only native sampler checks that do not touch clipboard, focus, windows or input. Run cargo fmt, workspace clippy/all-targets with warnings denied, workspace tests, optimized custom dev-memory profile using shared stage-t4 cache. Record exact outcomes and executable path in stage report. Manual active-theme/DPI/input/visibility acceptance is not claimed without execution.
