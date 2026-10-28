# Terminal pin restore follow-up

Requested on 2026-10-28. Branch `xDaijobu/fix-terminal-pin-restore`.

## Scope and acceptance

- Restore pinned shell/agent tabs at startup in their saved order without opening the panel or taking focus. Live shortcuts appear immediately.
- Keep pin metadata, shell arguments, agent type and the last reported working directory across restarts. Pin/unpin/close/reorder and metadata changes save promptly.
- Starting without pins creates no shell until requested. Opening the panel after startup does not duplicate restored tabs. Failed sessions remain retryable.
- Regression tests cover startup, idempotence, multiple pins and directory updates. Run fmt, workspace tests, clippy and release; verify the new executable starts.
- Running OS processes and terminal scrollback are not serialized by this change. Agent conversation restoration is pending the user's clarification.
