# Stage V report: Stay behind, Settings window, Spotlight

Plan: `task/stage-v-plan.md`. Branch `xDaijobu/stage-v-notch` (not pushed, no PR yet).

## Built

- [x] **V1 Stay behind windows** (`[island] stay_behind`, default off).
  - `Island::wants_front()` decides when the island comes to the top.
  - `window::set_layer` uses the desktop layer: just above `Progman` (spike-checked), never `HWND_BOTTOM`.
  - The foreground hook re-applies whichever layer is current.
  - `reserves_top()` is off while staying behind.
- [x] **V2 Settings window.**
  - An activatable popup of 868×608 logical px (the 820×560 panel plus its shadow margin).
  - Sidebar with search and five pages: General, Notch, Hotkeys, Terminal, Spotlight.
  - Controls: toggle, segmented, chip, slider, fixed value and read-only lines.
  - **Every change applies and saves at once** (asked for on 2026-10-28). Apply and Save are gone; only Reset is left, and it also applies at once.
  - Sliders preview live while dragged and save on release. ✕ and Esc just close.
  - Opened from the Home gear (the island collapses) and from the new tray item "Settings…". The title bar drags the window.
  - Apply applies live:
    - Start with Windows
    - theme
    - Snap and Spotlight hotkeys
    - notch style, size, reserve and stay behind
    - terminal settings: new tabs use the new shell; the font size applies at once
- [x] **Runtime notch size.** `[island] notch_width` (96–240) and `notch_height` (24–44). The reserved strip follows the height.
- [x] **V3 Spotlight** on Alt+Space (or Ctrl+Alt+Space, or off).
  - Apps come from `shell:AppsFolder`. Files come from a name scan of Desktop, Documents, Downloads, Pictures, Videos and Music (depth 6, at most 60k entries). The index is built on a background thread at startup and refreshed when Spotlight opens if older than 5 minutes.
  - Ranking tiers: exact > prefix > word start or acronym > substring > subsequence. Apps rank before folders and files.
  - Icons load on a background thread for the visible rows.
  - Keys: Enter opens, Ctrl+Enter reveals the file in Explorer, Esc closes, and clicking away closes.
  - **Size and file count, loaded lazily** (asked for on 2026-10-28). Only the rows on screen ask for them, through one background worker whose queue is replaced on every redraw, so rows that scroll or get typed away are dropped. A row shows "…" until its size arrives. Search itself never waits.
    - Files: the size from metadata (cached 1 minute).
    - Folders: the file count and total size, summed recursively without following junctions. The count stops at 200,000 files or 2 seconds and then reads "≥ …" (cached 5 minutes).
    - Apps: `EstimatedSize` from the Uninstall registry keys, matched by name (exact, or a prefix at a word boundary). Nothing is shown when there is no good match.
- [x] **The old in-island Settings panel** is no longer reachable from the gear. Its code is still there (`IslandState::Settings`), and removing it is a follow-up.

## Verification

- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- `cargo test --workspace`: **1177 passed**, 0 failed. Worker tests cover the Settings model, ranking, line edit, layer bookkeeping, the file scan, live app enumeration and icons, and the painters on a CPU raster.
- `cargo build --release` passes.

## Live (coordinator, 2026-10-28, 3440×1440 monitor)

- [x] **Spike:** the desktop layer shows the island over the wallpaper, and app windows cover it.
- [x] **Spotlight window:** it is created at the right place under the notch.
- [x] **Alt+Space:** opens Spotlight with the focus in the field, the placeholder "Search apps and files" and the "esc" chip, styled like the island.
- [ ] **First Alt+Space after a start:** the window was created but not shown. It worked on the next run. This may be the foreground lock; not reproduced and not explained yet.
- [ ] **Typing a query:** the test was stopped. The user was using the PC (a snap card appeared, the page scrolled), and synthetic input would have collided with them. Spotlight closed without a focus-lost event; the cause is unknown.
- [ ] **Not checked live:**
  - search results and icons
  - launch and reveal
  - the Settings window as a whole
  - stay behind with a maximized window
  - notch size sliders
  - theme follow in the popups

## Known issues and notes

- Some `shell:AppsFolder` entries are web links or document paths. They open directly instead of through `shell:AppsFolder`.
- The Spotlight window is kept hidden between uses, so only the first open pays for D3D12 device creation.
- The popups follow a theme crossfade only while the island is drawing frames.
- `Event` lost `Eq`, because `PopupInput` carries `f32`.
- New `windows` feature: `Win32_UI_Shell_Common` in dj-win. No new crates.
