# Stage 9d — stay behind with top edge reveal

User scope: keep the island visible on the desktop layer when resting behind
windows; reaching the physical monitor top edge within its horizontal bounds
temporarily raises it. This is not desktop idle disappearance.

## Implementation contract

- Extend the existing `island.stay_behind` behavior and explain it in Settings.
- Use a native, nonactivating, capture-excluded one-device-pixel edge window;
  no global mouse/click hooks, desktop input capture, or idle mouse polling.
- Keep a revealed island usable while the pointer is within it, or its existing
  panel/OSD/activity requires the front. Return behind after a bounded leave grace.
- Suppress edge reveal for fullscreen games, borderless fullscreen windows and
  presentation mode using native shell state and monitor/window geometry.
- Preserve deliberate hide, capture visibility, keyboard/panel/terminal hotkeys,
  AppBar semantics and monitor/DPI repositioning. Keep unsafe in platform code.
- Add pure controller and native tests. Do not launch/restart the user's app or
  automate their desktop. No notifications, Explorer changes, progress/AGENTS edits,
  Git commits, pushes, PRs, merges or subagents.

## Acceptance and delivery

Run focused controller/native tests, format check, workspace clippy with warnings
denied, workspace tests and an optimized build using the assigned shared cache
`C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4` (a unique optimized
profile may avoid a running executable). Record exact results, executable path,
remaining manual fullscreen/DPI/monitor checks and honest limitations in
`task/stage-9d-report.md`. Coordinator owns integration and Git.
