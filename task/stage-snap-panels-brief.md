# Snap follow-up: Keep open panels in screenshots

Requested on 2026-10-28 after Stage T4.

- Hotkey capture freezes the current visible screen without collapsing the island or moving focus first. The Home Snap tile may still collapse its launcher before capture.
- Keep terminal and Spotlight state through the overlay's temporary focus loss. Ordinary click-away dismissal resumes after capture.
- Include the visible island panel as a window-selection target so clicking the terminal in the frozen image selects it.
- Capture completion, cancellation and failures preserve the existing panel; a fresh thumbnail waits behind open panels instead of replacing them. Keep preview-stack copy/save behavior.
- Restore the previous foreground window after completion/cancel when the overlay still owned the foreground; never reactivate an invalid/hidden window or take focus back after a deliberate switch elsewhere.
- No new dependency, polling or permanent change to hide-on-blur settings. Headless regressions must cover launch-source policy, panel/thumbnail priority, temporary and stale focus-loss events, and synthetic window selection. Run fmt, clippy, workspace tests and release build; report live limits honestly.
