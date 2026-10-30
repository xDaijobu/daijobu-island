# Stage 9h - Music volume slider

User requests volume adjustment inside expanded Music. Optional scope question remains unanswered after time to reply; use default Windows master volume, explicitly labelled Windows volume. Reuse the existing Core Audio endpoint mixer and callbacks. No per-app/session attribution or new dependency.

Add a click/drag slider and percentage, live external-change synchronization and unavailable/muted states. Keep Music open while its volume changes; do not replace the panel with the OSD. Clamp values and cancel dragging on release/state change/device loss. Preserve track seek and existing standalone OSD behavior. Expand Music by one row, preserving lyrics and footer links.

User follow-up: scroll up/down over the volume row adjusts volume by two percentage points per wheel notch, including without keyboard focus. Finish, merge and release promptly; use targeted checks after the already-passing workspace run.

Acceptance: meaningful interaction regression tests, fmt/clippy/workspace checks, optimized build, live appearance and conservative volume interaction if possible; restore original volume after live test. Report limitations honestly. Deliver to registered location, PR/merge and remove worktree.
