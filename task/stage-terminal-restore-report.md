# Terminal pin restore report

Brief: [stage-terminal-restore-brief.md](stage-terminal-restore-brief.md).

## Delivered

- [x] Startup restores saved pinned tabs in the background after renderer/DPI initialization, then synchronizes live idle shortcuts. It never opens Terminal or takes keyboard focus.
- [x] First panel open reuses restored sessions; repeat restore calls do not duplicate tabs. No pins means no startup shell. Failed starts stay pinned and retryable.
- [x] Persist last reported PowerShell folder and title changes immediately for pinned tabs, alongside existing shell arguments, agent type and display order. Drain pending terminal events before shutdown so queued metadata gets saved before sessions are dropped.
- [x] README explains startup restoration and its limits. No new dependencies, timers or polling.

## Verification

- fmt and workspace clippy, all targets with warnings denied: pass.
- Workspace tests: **1215 passed**, 0 failed. Four new controller regressions cover empty startup, latest folder/name persistence, unchanged and unpinned metadata avoiding writes, and multiple ordered pins including failed sessions. Existing restart test now restores before opening the panel and checks no duplication afterward.
- Release build with `--locked`: pass, using main checkout `target/stage-t4` cache.
- Installed the verified release executable into `target/release/daijobu.exe` and restarted the app.
- Live Win32 check: both existing pinned shells started automatically before any Terminal hotkey; opening the panel retained those exact process IDs and created no extra shell. Startup kept the panel closed, keyboard mode disabled and foreground unchanged. Clipboard sequence unchanged; app responsive (PID 14976 at verification). No cursor movement, clipboard writes or agent prompts. Panel collapsed again afterward.

## Run and limitations

Pin a tab from its tab menu, then quit/restart Daijobu Island. Pinned tabs and their clock shortcuts return automatically. Unpinning or explicitly closing a pinned tab removes it from restoration.

This starts new shell/agent processes. Running commands, screen scrollback and agent conversations are not restored. Conversation continuation was offered as a clarification, with no answer received during implementation; this change covers tabs and folders. Shells that do not report cwd keep their original configured/saved folder. Full manual visual review and restart after changing folders via the real keyboard were not performed; folder persistence is covered by the controller regression.
