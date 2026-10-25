# Stage 5 handover (2026-10-26, ~01:00)

PR #6 (Vitals + Sweep) is **merged into master** at the user's request, *before* the safety fixes landed. It is green: fmt, clippy -D warnings, 188 tests and the release build all pass. Nothing has been run live yet.

## ⚠️ Don't use these until they're fixed
- **Clean "temp & reports"** (BLOCKER B1): the Temp category trusts `%TEMP%`. If TMP/TEMP are unset, Windows falls back to USERPROFILE, so Desktop, Downloads and other profile folders older than 7 days become **permanent-delete** candidates.
- Avoid big recycles of `node_modules` until M2 is fixed.

## Next steps, in order
1. **Fix the safety findings** on a new branch `stage-5-safety-fixes` from master. The full review was posted to the PR #6 comments; the list is below too.
   - **B1** (`dj-sweep` lib.rs:81, scan.rs:75/243-252): allow Temp only if its final path equals `%LOCALAPPDATA%\Temp` (leaf Temp/Tmp, at least 2 levels deep, not the profile, AppData, a project root or an ancestor of one). Otherwise skip the category with a note. Refuse to build a ScanEnv when USERPROFILE is missing or not absolute.
   - **M1** (fs.rs:280-322 `remove_rec`): close the TOCTOU gap where a subfolder swapped for a junction gets followed. Delete through handles (`FILE_FLAG_OPEN_REPARSE_POINT`) or call `std::fs::remove_dir_all` on each verified child.
   - **M2** (ops.rs:67-75): remove `FOF_WANTNUKEWARNING`, because it can permanently delete something that then gets logged as RECYCLED. Check that a bin exists (`SHQueryRecycleBinW`) before recycling, otherwise mark the item FAILED.
   - **M3** (fs.rs:219/231 `project_activity`): when the 5000-entry or depth-3 cap is hit, treat the project as "unknown" and skip it. Don't skip folders named bin/build/dist when looking for recent activity.
   - **Lows:**
     - Temp age should use max(mtime, ctime).
     - The process probe should fail closed.
     - Guard: add `Cookies-journal`, `cookies.sqlite-wal`, `sessionstore*` and Local `Microsoft\Crypto`.
     - Measure partial-clean failures with a fresh budget.
     - `OpLog::record` should return a result.
     - Reject relative `project_roots`.
     - Big-file recycle confirm: arm it by **path, not index**, and clear it in `forget`/`set_category`.
     - Empty bin: require at least 400 ms between the two clicks.
2. **Unfinished WIP:** the local branch `wip/stage-5-safety-fixes` (in the worktree `C:/Users/chris/orca/workspaces/daijobu-island/stage-5-vitals-sweep`) has a half-done version of the big-file "confirm by path" change in `crates/dj-core/src/sweep.rs`. dj-core tests pass, but **dj-render fails to compile** (E0308). Either finish it or cherry-pick it into the new branch.
3. **Write `task/stage-5-report.md`.** The agent reports from this session are summarised in the PR #6 description and the safety-review comment.
4. **Live test** once the fixes are in (step by step):
   - Back up `~/.daijobu`.
   - Vitals: check the score, rings, disk I/O, top processes and the 🧹 button.
   - Sweep: **preview only**, and page through every category.
   - Clean temp (only after B1 is fixed), then check `~/.daijobu/operations.log`.
   - Dev purge: read the list first, then confirm the folders land in the Recycle Bin and can be restored.
   - Recycle one big file and restore it. Hide the island (Ctrl+Alt+H) during a scan and confirm the scan stops.
5. Update `task/progress.md` → Stage 5 ✅ once the live test passes.

## Where things are
- Crates: `dj-sweep` (rules/fs/scan/ops/tests), `dj-core/src/sweep.rs` (view model), `app/src/sweep.rs` (worker, `WM_SWEEP = WM_APP+8`), `dj-render/src/sweep.rs` + `paint.rs`.
- Remote branch `xDaijobu/stage-5-vitals-sweep` is merged and can be deleted once the WIP has been taken over.
- Agent rules: headless, temp dirs only, `CARGO_BUILD_JOBS` 2 while gaming and up to 12 otherwise.
