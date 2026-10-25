# Stage 5 report: Vitals + Sweep

Stage 5 was built on `xDaijobu/stage-5-vitals-sweep` (PR #6). It was merged into `master` before the safety review's findings were fixed. The fixes are on `stage-5-safety-fixes`, together with a live test run on the user's machine.

## Verdict

Every scope item is implemented. The review findings B1, M1, M2 and M3 and all the lows are fixed and have regression tests. The live test passed for everything except the big-file recycle, which is not live-tested (see below).

Verification on `stage-5-safety-fixes` (MSVC 14.51, Windows SDK 10.0.26100):

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 196 passed, 0 failed (dj-core 117, dj-sweep 35, dj-vitals 26, dj-render 12, dj-win 4, app 2). No junction or git test skipped.
- `cargo build --release`: OK.

## Checklist (contract)

| Item | State | Notes |
|---|---|---|
| Remove the Sweep tile (12 features) | Done | Sweep is a sub-view of Vitals. |
| Full vitals: disk I/O, top 3 CPU/RAM, disk-I/O penalty | Done, **verified live** | The panel showed rings, ↓/↑, `io r … w …`, top processes and the "Low disk" reason. |
| `dj-sweep`: temp & reports, browser, app & dev caches, dev purge, big files, quick actions | Done | The lists are our own. Big files is a partial list on large profiles (see known issues). |
| Safety: guard, no reparse following, final-path re-verify, preview before Clean, recycle for purge and big files, off-thread with a budget, operations log | Done, plus the fixes below | |
| Reclaimable estimate cache (6 h, panel open, never while hidden) | Done, **verified live** | `sweep-cache.toml` was written on the first Vitals open. |
| UI: "🧹 X reclaimable", Sweep view, preview, Clean, progress, result, back to Vitals | Done, **verified live** | |
| Idle CPU ≈ 0, no scanning unless needed | **Verified live** | About 1 core while scanning, then 0 ms of CPU over 5 s after Ctrl+Alt+H. |
| Tests in temp dirs only | Done | |

## Safety fixes (PR #6 review)

| Finding | Fix | Test |
|---|---|---|
| **B1** Temp trusted `%TEMP%` (falls back to the profile when TMP and TEMP are unset) | `rules::check_temp_dir`: `%TEMP%` is used only when its final path is `%LOCALAPPDATA%\Temp`, with a Temp/Tmp leaf at least 2 levels deep, `%LOCALAPPDATA%` inside the profile, and no key folder or project root inside it. Otherwise the temp part is skipped with a note ("temp files skipped: …"). WER and CrashDumps still work. `ScanEnv::from_system` returns `Err` when USERPROFILE is missing, relative or too shallow; the app then shows the reason and cleans nothing. | `temp_dir_is_trusted_only_in_local_appdata`, `temp_falling_back_to_the_profile_is_never_cleaned`, `scan_env_needs_an_absolute_profile` |
| **M1** TOCTOU in `remove_rec` (a subfolder swapped for a junction could be followed) | Each folder is held open (`FILE_LIST_DIRECTORY`, share read and write but not delete, `OPEN_REPARSE_POINT`) while its contents are deleted. While the handle is open the folder can't be renamed or removed, so it can't be swapped. A swap that happened before the open shows up as a reparse point on the handle, and only the link is removed. Note: a handle with only attribute access is ignored by the share check. The first attempt found this, and the test caught it. | `a_held_folder_cannot_be_swapped`, the existing junction tests |
| **M2** `FOF_WANTNUKEWARNING` could permanently delete an item that was then logged as RECYCLED | **I deviated from the handover here.** The flag is kept: without it, `FOF_NOCONFIRMATION` lets Windows delete an item that doesn't fit in the bin for good, silently. The logging is fixed instead. Recycling first needs a fixed drive (`GetDriveTypeW`) with a bin (`SHQueryRecycleBinW`), or the item is FAILED and left untouched. Afterwards the bin's item count is compared, and a recycle that really deleted the item is logged as `REMOVED (…, not recycled: deleted for good)` and counted as freed. `Ops::recycle` now returns `Recycled::{InBin, DeletedForGood}`. | `a_recycle_that_deleted_for_good_is_logged_as_removed` |
| **M3** 14-day activity check gave up at 5000 entries or depth 3 and called the project quiet | The walk has no depth limit. Past 20,000 entries or the budget, the project counts as unknown and is skipped. Only build output is skipped during the walk: `node_modules`, `__pycache__`, `.git` and venvs at any depth, plus direct children that pass the purge context rule. `bin`, `build` and `dist` count as project files unless the project's markers say they are output. Creation time counts as activity too. | `activity_ignores_build_output`, `dev_purge_skips_projects_too_big_to_check` |
| Low: temp age | `Tally` uses max(mtime, ctime). | covered by the temp tests |
| Low: process probe fails open | `running_processes()` returns `None` on failure, and the probe then treats every app as running. | (Win32 path, not unit-tested) |
| Low: guard names | Added `Cookies-journal`, `cookies.sqlite-wal`/`-shm`, `sessionstore*` and Local `Microsoft\Crypto`. | `guard_protects_system_and_secrets` |
| Low: partial-clean measure shared the used-up budget | A fresh budget is used, and an incomplete measure counts 0 freed. | |
| Low: `OpLog::record` swallowed errors | It returns `io::Result`. `CleanOutcome.log_failed` is set, and the UI shows "could not write operations.log". | `oplog_reports_write_errors` |
| Low: relative `project_roots` | Dropped in `SweepConfig::roots` and again in `ScanEnv`. | `sweep_roots_expand_the_profile` |
| Low: big-file confirm armed by index | `Confirm::Recycle(PathBuf)`. It is cleared in `forget` (for that path) and in `set_category`. | `recycle_confirm_follows_the_file_not_the_row` |
| Low: empty bin double click | The second click must come at least 400 ms after the first (`BIN_CONFIRM_DELAY`). A quicker click keeps it armed. | `recycle_and_empty_bin_need_two_clicks` |

Found during the live test: the **big files** list was always empty ("0 B, time limit reached"). The profile walk used up the budget, and the check loop then stopped on the same budget. The checks only read metadata, so they now run past the budget. After the fix the list fills.

The other WIP that the handover mentioned (`wip/stage-5-safety-fixes`, on the other machine) was not available here. Confirm-by-path was redone from scratch.

## Live test (2026-10-26, user's machine, 3440×1440, scale 1.0)

1. There was no `~/.daijobu` yet, so there was nothing to back up.
2. **Vitals:** score, rings, disk I/O, top processes and "Low disk" all showed. The button went from "scanning…" to "64 GB reclaimable".
3. **Sweep preview:** temp & reports 79 GB, browser 149 MB ("skipped while running"), app & dev caches 16 GB (also skipped while running), dev purge 298 MB (2 items, not touched), big files 0 B (the bug above). In the temp list, the items matched an independent read-only PowerShell check: about 87 GB of `%LOCALAPPDATA%\Temp` was older than 7 days, and the recent `claude` and `sm3fb0lh` folders were excluded.
4. **Clean temp only** (the other categories unticked): 2700 items, 2700 `REMOVED` lines, 0 `FAILED`, and every line inside `%LOCALAPPDATA%\Temp`. Free space on C: went from 29.4 GB to 108.6 GB. The view went back to Vitals, with the disk ring going from 96 to 86 and the score from 86 to 96.
5. **Dev purge** against a throwaway project (`~/dj-livetest`, set as the only root through `config.toml` and aged 30 days): the list held only `oldproj / target`. Clean wrote `RECYCLED … (dev_purge, 5.0 MB)`. The folder was in the Recycle Bin with its original location, and restoring it brought it back. The test folder and config were then removed.
6. **Ctrl+Alt+H during a scan:** the scan stopped (0 ms of CPU over 5 s while hidden). Unhiding re-ran the preview and kept the ticks.

## Not verified

- **Recycling a big file** live: on this profile the time-limited walk never reached the test file. The recycle path itself was verified through dev purge, and confirm-by-path has unit tests.
- **The nuke-warning path** (an item too big for the bin): this needs a file larger than the bin. The bin-count logic is tested with a fake.
- Flush DNS, Empty bin and Reveal: not clicked.
- Browser and app-cache cleaning: skipped, because the apps were running. That is correct behaviour, but no clean ran.
- The swap-protection held up in tests. Whether an ancestor folder can be renamed while a descendant is held open depends on Windows. The tested guarantee is for the held folder itself.

## Known issues

- **Big files is a partial list on large profiles.** The 15 s depth-first walk finishes inside the first heavy folders (for example Flutter or Codex caches), so files elsewhere in the profile can be missed. A breadth-first walk or a longer budget for this category would help.
- "Low disk" still shows at 86% used. That's the health threshold, not a Sweep bug.
- A new launch resets the ticks to the defaults (temp, browser and app ticked). The Clean button in a preview list cleans every ticked category, not only the open list.

## How to run (this machine)

The VS 18 Insiders install has no x64 CRT libraries. Until the "MSVC v14x x64/x86 build tools" component is added in the VS Installer, builds need `LIB`/`INCLUDE` pointing at the CRT from `xwin splat` (`~/.xwin-tool/splat/crt`) plus Windows SDK 10.0.26100. The Windows Kits `bin\…\x64` folder must be on PATH for `mt.exe`.
