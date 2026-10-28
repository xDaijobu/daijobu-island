# Stage V: Stay behind, Settings window, Spotlight — plan

> Written by the coordinator (Claude Opus 5.5) on 2026-10-28.
> - **Branch:** `xDaijobu/stage-v-notch`, worktree `workspaces/daijobu-island/stage-v-notch`, from `master` `06fe0fc`.
> - **Source of the ideas:** V-Notch (w0otdev/V-Notch, Apache-2.0), but only the ideas. No code or assets are copied.

## Decided with the user (2026-10-28)

- **Spotlight** finds **apps and files** (no calculator, no AI tab). It opens on **Alt+Space**, which the user accepted even though it means Windows' own Alt+Space window menu is lost.
- **Stay behind windows** keeps the notch on the desktop layer, under every app window. Anything that opens a panel (a hotkey, the OSD, music, a snap thumb) **brings it forward for a while**, and it goes back when the island rests again. With it on, "Reserve top space" doesn't apply.
- **Settings becomes a large window** with a sidebar, as in V-Notch:
  - It holds today's settings plus the new ones (stay behind, Spotlight, terminal) and **sliders for the notch size**.
  - It has Apply, Save and Reset.
  - The gear on Home opens it, replacing the small panel inside the island.

## Spike results

- **Behind windows works (checked live).** Take the island out of topmost, then `SetWindowPos(island, GetWindow(Progman, GW_HWNDPREV))`. That puts it directly above the desktop (`Progman`): it draws over the wallpaper, and every normal window covers it.
- `HWND_BOTTOM` does **not** work: the island goes below `Progman` and disappears.
- This machine now runs a 3440×1440 primary monitor.

---

## V1 — Stay behind windows

- **Config:** `[island] stay_behind = false` (default off).
  - `IslandConfig::reserves_top()` becomes `reserve_top && style == Notch && !stay_behind`.
  - A lenient field, like the others.
- **dj-core** `island.rs`: `pub fn wants_front(&self) -> bool`.
  - True when the island isn't resting (`!state.is_rest()`), the OSD is showing, or a spring is still collapsing back to rest.
  - Pure; tested per state.
- **dj-win** `window.rs`: `pub fn set_layer(hwnd, Layer)`, where `enum Layer { Top, Desktop }`.
  - `Top` = `HWND_TOPMOST`.
  - `Desktop` = `HWND_NOTOPMOST`, then insert after `GetWindow(FindWindowW("Progman"), GW_HWNDPREV)`, skipping if that is the island itself. If `Progman` can't be found, fall back to `HWND_NOTOPMOST` only.
  - Always `SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE`.
  - Also drop `WS_EX_TOPMOST` when creating the window if the config says behind.
- **dj-win** `system.rs`: the topmost hook (`foreground_changed`) re-asserts the island only while it should be on top (an atomic flag set by the app). Behind, it re-asserts the desktop layer instead, because a desktop click can reorder windows.
- **App:** after every state change and every frame that ends in rest, `layer = if !stay_behind || island.wants_front() { Top } else { Desktop }`. It calls `set_layer` only when the layer changes. A `sync_appbar` follows the setting.
- **Snap thumb and preview stack** stay topmost windows of their own and aren't touched.
- **Tests:**
  - `wants_front` for every state and the OSD.
  - `reserves_top` with `stay_behind`.
  - Config round trip.
- **Live:**
  - With a maximized Chrome, the notch is hidden. The volume keys bring it forward and it sinks back after the OSD.
  - Ctrl+Alt+` brings the terminal over Chrome, and closing it sinks back.
  - With the setting off, everything behaves as before.

## V2 — Settings window

A separate, activatable popup window (not topmost, centred on the island's monitor) that **replaces the in-island Settings panel**.
- The gear on Home and a new tray item "Settings…" open it. If it's already open, they focus it.
- `IslandState::Settings` and the `ST_*` layout go away.
- Size: about **820×560 logical**, radius 20, with the theme's pill/surface colours, grain, and a soft shadow drawn inside the window (transparent margins as the island does).
- **Layout:** title bar (title, ✕, draggable), sidebar (search field + 5 pages), content (rows, scrollable with the wheel), and a footer with **Reset · Apply · Save**.

**Pages and rows:**

| Page | Rows |
|---|---|
| General | Start with Windows (toggle) · Theme (chip cycle over the loaded themes) |
| Notch | Island style (notch / pill segmented) · Notch width (slider 96–240, default 128) · Notch height (slider 24–44, default 32) · Reserve top space (toggle, dimmed in pill or behind) · Stay behind windows (toggle) |
| Hotkeys | Snap (chip cycle, as today) · Snap alt (chip cycle, as today) · Spotlight (Alt+Space / Ctrl+Alt+Space / off) · Terminal (shown, fixed Ctrl+Alt+`) |
| Terminal | Shell (PowerShell / pwsh / cmd / WSL segmented) · Font size (slider 8–24) · Hide on blur (toggle) · Scrollback (slider 0–50000, step 1000) |
| Spotlight | Enabled (toggle) · Search files (toggle) · Folders searched (read-only list: Desktop, Documents, Downloads, Pictures, Videos, Music) |

**Behaviour:**
- Every edit changes a **pending** `Config` copy.
- **Apply** writes it, saves `config.toml` and applies it live (autostart, hotkeys, AppBar, layer, notch size, terminal font, …).
- **Save** = Apply + close.
- **Reset** puts the current page's rows back to their defaults (pending until Apply/Save).
- **✕** or Esc closes; with unsaved changes, the footer shows "Unsaved changes" and a second ✕/Esc discards them.
- **Search** filters rows by label across every page (matches highlighted, empty pages hidden).
- **The notch size becomes runtime.** `layout::IDLE_W/IDLE_H` become the defaults. `Island` takes the idle size from config (`Island::set_idle_size`), and the notch strip, shoulders, hit region and reserve height follow.
  - The swap chain doesn't change: the window is already sized for the largest panel.
- **dj-core** `settings_ui.rs` (pure):
  - `SettingsDoc` with pages/rows built from a `Config`, plus the pending copy, the dirty flag, the search text, scroll, hover and the slider being dragged.
  - Hit testing in window-local logical px.
  - `press / drag / release / wheel / key / char` return `Outcome { redraw, apply: Option<Config>, close: bool }`.
  - The old `settings.rs` hotkey cycles are reused.
- **dj-win** `popup.rs`: a generic activatable `WS_POPUP` (`NOREDIRECTIONBITMAP`, not topmost, `WS_EX_APPWINDOW` so it is in Alt-Tab) with the same input events as the snap overlay (press, motion, release with capture, wheel, key, char, close, focus lost) and a cursor setter. Spotlight uses it too.
- **dj-render** `settings_window.rs`: `SettingsWindow { gpu, chain, fonts }` like `SnapOverlay`, with `render(&SettingsDoc, tokens)`.
  - Widgets: toggle, segmented, chip, slider with value, text field with caret, sidebar item, buttons.
- **App:** opens and closes the window, routes its events, and applies a returned `Config` via the same paths `settings_action` uses today.
- **Tests:**
  - The doc built from config.
  - Every widget's hit, toggle, slider drag and clamp, and keyboard step.
  - Reset per page, search filter, dirty and discard, and Apply producing exactly the edited config.
  - The CPU-raster painter for each page.

## V3 — Spotlight (Alt+Space)

A popup bar about **680 logical wide**, centred under the notch (top ≈ notch bottom + 24). It has a search field (52 high) and up to 8 result rows (44 high) below it, and it shrinks to the field when there are no results.
- **Keys:**
  - Typing filters as you type.
  - Up/Down move.
  - Enter opens.
  - Ctrl+Enter opens a file's folder with the file selected (`explorer /select,`).
  - Esc, losing focus or Alt+Space again close it.
- **Hotkey:** `RegisterHotKey(MOD_ALT | MOD_NOREPEAT, VK_SPACE)`, id 20. If it is taken, Settings shows "taken by another app", as the snap chip does.
- **Index** (`dj-win` `launcher.rs`, built on a background thread, with no work while idle):
  - **Apps:** enumerate `shell:AppsFolder` (`SHGetKnownFolderItem(FOLDERID_AppsFolder)` → `IEnumShellItems`). This covers Start Menu desktop apps and Store apps.
    - Each app gives a name and a parsing name (an AUMID or path).
    - Launch: `ShellExecuteW("shell:AppsFolder\\" + parsing_name)`.
  - **Files:** a name scan of Desktop, Documents, Downloads, Pictures, Videos and Music (known folders).
    - Depth 6, at most 60k entries.
    - It skips hidden/system entries, `node_modules`, `.git`, `target`, `$Recycle.Bin` and `AppData`.
    - Launch: `ShellExecuteW("open", path)`.
  - The index is built at startup (low priority) and refreshed when Spotlight opens if it is older than 5 minutes. The previous list is kept until the new one is ready.
  - **Icons:** `IShellItemImageFactory::GetImage(32px)` → BGRA → a Skia image, cached by parsing name (apps) or extension (files), and loaded off the UI thread for the visible rows only.
- **Ranking** (`dj-core` `spotlight.rs`, pure):
  - Case-insensitive. Exact > prefix > word-start (including camel and `-_. ` boundaries) > substring > subsequence.
  - Apps come before files at equal score, and shorter names win ties.
  - Files only once the query has 2 or more characters.
- **dj-core** `spotlight.rs`:
  - `Spotlight` with query, caret, results, selection and scroll.
  - `key / char / paste / hover / click` return `Action { Launch(Entry), Reveal(Entry), Close, Redraw, None }`.
  - Layout rects.
- **dj-render** `spotlight.rs`: the bar and rows (icon, name with the matched characters in bold, dim path or "App"), the selection plate, and the theme colours, all in the same window kind as Settings.
- **App:**
  - The hotkey toggles Spotlight.
  - Opening it takes the keyboard (the window is activatable) and focus returns to the previous window after a launch or close.
  - `[spotlight] enabled = true, hotkey = "alt_space", files = true`.
- **Tests:**
  - Ranking order and the scan filters (on a temp dir).
  - Key navigation, wrapping, the 8-row window with scroll, and launch/reveal/close actions.
  - The painter on CPU raster.
  - The AppsFolder enumeration as one live-machine test that only checks that more than 10 apps come back.
- **Live:**
  - Alt+Space → "chr" → Chrome first → Enter opens it.
  - "budget" finds a file in Documents; Ctrl+Enter reveals it.
  - Esc and clicking away close it.
  - Idle CPU stays ≈ 0 with Spotlight closed.

---

## Parts and workers (Sonnet 5.5, own worktrees; the coordinator writes the shared types first and does the wiring)

| Part | Crate/files | Content |
|---|---|---|
| S (skeleton, coordinator) | `dj-core` | config fields, `Layer`, shared view types for Settings and Spotlight, the `popup.rs` event enum |
| A | `dj-core` | `settings_ui.rs`, `spotlight.rs` ranking and model, `wants_front`, runtime notch size |
| B | `dj-win` | `set_layer`, the hook flag, `popup.rs`, `launcher.rs` (AppsFolder, file scan, launch, icons) |
| C | `dj-render` | `settings_window.rs`, `spotlight.rs` painters, runtime notch size in the island painter |
| D (coordinator) | `app` | wiring, the tray item, removing the in-island Settings, live test, docs (README, AGENTS layout, progress) |

## Review focus

1. The **layer** never gets stuck on top or behind after fast state changes, an Explorer restart or a monitor change.
2. **Settings Apply** applies exactly the edited values. Hotkey re-registration doesn't leak or collide with Spotlight's Alt+Space.
3. **The Spotlight index** never blocks the UI thread. A rescan that is in flight when Spotlight closes is harmless. Unreadable folders are skipped.
4. **Keyboard focus** returns to the previous window after Spotlight or Settings closes, as the terminal does today.
5. **The runtime notch size** keeps the shoulders, hit region and AppBar strip consistent at every size and DPI.

## Out of scope

Calculator, the AI tab, web search, recent-launch ranking, a Windows Search (indexer) backend, editing folder lists, custom hotkey recording, settings import/export, and V-Notch's other features (lyrics, privacy indicators, MyDockFinder).
