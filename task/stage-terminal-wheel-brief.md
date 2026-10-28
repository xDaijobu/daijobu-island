# Terminal wheel follow-up

Requested on 2026-10-28. Branch `xDaijobu/fix-terminal-wheel`.

## Scope

- Route wheel input to the program when it requests xterm mouse tracking (SGR, legacy and UTF-8 coordinates), including pointer cell location.
- For alternate-screen applications without mouse tracking, honor alternate-scroll mode by sending cursor up/down keys; retain normal scrollback in ordinary shells.
- Preserve existing three-lines-per-notch and fractional-wheel accumulation. Ignore terminal header/menu wheel events rather than sending them as application input.
- No general mouse click/drag reporting, terminal discovery or agent-specific key hacks.

## Acceptance

- Feed real VT escape modes into the emulator and verify mouse bytes, direction, coordinates, alternate-screen fallback and normal scrollback; verify controller pointer mapping and fractional steps.
- Run fmt, workspace clippy/tests and release build. Restart the verified build and document live verification and limits.

Protocol reference: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html (mouse tracking / wheel buttons and alternate scroll).
