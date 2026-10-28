# Terminal tab rename

Requested on 2026-10-28. Branch `xDaijobu/feat-terminal-rename`.

## Scope and acceptance

- Add Rename tab to a tab's context menu, including pinned/background/exited tabs.
- Show a native Skia text editor prefilled with the current name. Enter/Save applies, Escape/Cancel/outside click cancels; an empty name restores automatic application titles.
- Support Unicode text, existing line-edit navigation, Ctrl+A and paste. Consume rename input so it never reaches the shell or launches/restarts a process.
- Custom names survive application title changes and shell restart. Pinned tabs persist the override in the existing TOML file; older files retain automatic naming.
- Verify save/cancel/input isolation, targeting, overrides, pin round-trip and render geometry across themes/DPI. Run fmt, clippy, workspace tests and release; restart verified build and record live limits.
