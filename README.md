<h1 align="center">Daijobu Island</h1>

<p align="center">
  A lofi dynamic island for Windows 11.<br>
  Rust, Skia and plain Win32. WebView2 only for the YouTube Music engine.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-c9a26b" alt="License: Apache-2.0"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2011-5b6b73" alt="Platform: Windows 11">
  <img src="https://img.shields.io/badge/status-early%20development-a39785" alt="Status: early development">
  <a href="https://github.com/xDaijobu/daijobu-island/stargazers"><img src="https://img.shields.io/github/stars/xDaijobu/daijobu-island?style=flat&color=e0a458" alt="GitHub stars"></a>
</p>

<p align="center">
  <img src="docs/images/app-pill.png" alt="Collapsed island" height="34"><br><br>
  <img src="docs/images/app-home.png" alt="Expanded island: feature grid" width="620">
</p>

<p align="center"><sub>Early app-shell screenshots. The current build also includes Music, Snap, Clipboard, Vitals, Timer, tiling, Terminal and Spotlight.</sub></p>

---

*Daijobu* (大丈夫) is Japanese for "it's fine" or "are you okay?". The island sits at the top of the screen and stays out of the way. When you need it, it opens up to show what's playing, catch a screenshot, bring back something you copied, put your windows in place, or tell you whether your PC is having a bad day.

## Status

Early development. Music (YouTube Music), the volume OSD, Vitals with Sweep, Timer, Snap capture and annotations, floating screenshot previews, screen pins, Clipboard history, window tiling, a terminal with tabs, Spotlight search and a standalone Settings window are implemented in the current source. Clipboard passed independent review, workspace checks and synthetic native smoke; these changes do not imply a new published release. Features get unlocked one at a time. Day-to-day progress and outstanding manual checks are tracked in [`task/progress.md`](task/progress.md).

| Stage | | |
|---|---|---|
| 1 | App shell: window, Skia renderer, spring motion, themes, tray | ✅ done |
| 2 | Music: YouTube Music playing headless, the island as the player | ✅ merged, testing |
| 2b | Music library: search, playlists, queue inside the island | locked |
| 3 | Volume OSD replacing the Windows flyout | ✅ merged, testing |
| 4 | Vitals: health score, CPU/RAM/disk/net rings | ✅ merged, testing |
| 5 | Vitals + Sweep: disk I/O, top processes, safe disk cleanup | ✅ done |
| 6 | Timer / pomodoro | ✅ done |
| 7a–7c | Screenshots: capture, annotate, floating preview stack | ✅ done |
| 7c-2 | Screenshot drag-out into compatible apps | implemented; desktop drag checks pending |
| 7d | Screenshot pin-to-screen | ✅ merged (PR #30); native smoke checks passed |
| T | Terminal: shell tabs, pinned tabs, Claude and Codex tabs | ✅ merged |
| N–N2 | Notch style and reserved top space | ✅ done |
| V | Spotlight, standalone Settings, notch size and stay behind windows | ✅ merged, live testing pending |
| 8 | Clipboard history: search, pin, persistent history and paste | complete; native target-app/hardware gaps documented |
| 9–10 | Notifications, lyrics | locked |
| 12a | Window tiling: zones, a layout picker in the island, a window picker | ✅ done |
| 12b–12d | Scenes, restore-after-reboot, admin apps | locked |
| 13 | File shelf, ambient sounds | locked |

Clipboard history passed review and testing, after merged screenshot drag-out
and pin-to-screen. The
[implementation plan](task/stage-snap-clipboard-plan.md) includes stage briefs, worker
ownership, dependencies and acceptance checks. OCR has been removed from the roadmap.
Stage 7c-2 passed source review, workspace checks, release build and restart/Snap-cancel
smoke checks; its [report](task/stage-7c2-report.md) lists remaining desktop drag checks.
The [Stage 8 report](task/stage-8-report.md) records 1,633 passing tests, native
paste/privacy/restart evidence and remaining target-app/hardware checks. Later-stage features remain locked.

## Download and run

Get the Windows x64 build from the [latest GitHub release](https://github.com/xDaijobu/daijobu-island/releases/latest). Extract the portable ZIP and run `daijobu.exe`; there is no installer. Quit the previous instance from the tray before replacing it with a newer build. Settings and user themes stay in `~/.daijobu/`.

Requires Windows 11 with Direct3D 12. Music also needs the Microsoft Edge WebView2 Runtime. The other panels use native Win32 windows drawn with Skia.

## Where it's going

These are design mockups from [`docs/plan.html`](docs/plan.html), not screenshots. Open that file in a browser for the interactive version.

<table>
  <tr>
    <td width="50%"><img src="docs/images/concept-media.png" alt="Media"><br><b>Media.</b> Now playing from any app that reports to Windows, with a segmented VU visualizer.</td>
    <td width="50%"><img src="docs/images/concept-snap.png" alt="Snap concept"><br><b>Snap.</b> Region screenshots with a loupe, annotations and pixelation, plus separate pin-to-screen reference windows. The early concept image predates the current scope.</td>
  </tr>
  <tr>
    <td><img src="docs/images/concept-snap-stack.png" alt="Snap stack"><br><b>Snap stack.</b> New screenshots float under the island for a few seconds. Select several and drag them straight into Discord, Explorer or a browser.</td>
    <td><img src="docs/images/concept-clipboard.png" alt="Clipboard"><br><b>Clipboard.</b> History for text, links, colours, files and images, in the spirit of <code>Win+V</code>.</td>
  </tr>
  <tr>
    <td><img src="docs/images/concept-tile.png" alt="Tile"><br><b>Tile & Scenes.</b> Split windows into zones. Save a layout and have it restored after a reboot, admin apps included, through a one-time Task Scheduler consent.</td>
    <td><img src="docs/images/concept-vitals.png" alt="Vitals"><br><b>Vitals.</b> CPU, memory, disk and network, plus a health score that answers the question in the name.</td>
  </tr>
</table>

### Themes

Themes are small TOML files. Each one sets more than colours: fonts, grain, dither, corner shape, visualizer style and how bouncy the motion is. Drop a file into `~/.daijobu/themes/` and the island picks it up live. `Ctrl+Alt+T` cycles through them.

<img src="docs/images/concept-themes.png" alt="Theme gallery">

Four of these ship with the current build (Tobacco Tape, Midnight Cassette, Paper Zine, Clean). The rest come later.

## Building

Requirements: Windows 11, stable Rust (MSVC toolchain), Visual Studio Build Tools with the C++ workload, and Windows SDK 10.0.26100 or newer.

```sh
git clone https://github.com/xDaijobu/daijobu-island
cd daijobu-island
cargo build --release
target\release\daijobu.exe
```

`skia-safe` uses a prebuilt Skia when available. The first build downloads dependencies and takes longer than subsequent builds.

| Shortcut | Action |
|---|---|
| Click the island | Expand / collapse |
| `Ctrl+Alt+H` | Hide / show |
| `Ctrl+Alt+T` | Next theme |
| `PrtSc` / `Ctrl+Space` | Start a screenshot (default primary / alternate shortcut) |
| `Alt+Space` | Open / close Spotlight |
| `Ctrl+Alt+V` | Open Clipboard history |
| ``Ctrl+Alt+` `` | Open / close the terminal |

The tray also offers Settings, themes, *Start with Windows* and Quit. Open Settings from the Home gear or the tray. Its General, Notch, Hotkeys, Terminal and Spotlight pages apply and save changes immediately; sliders preview while dragging and save on release. Config is stored in `~/.daijobu/config.toml`.

Snap hotkeys capture open panels as shown, including the island terminal. Finishing or cancelling returns to the open panel; a new screenshot thumbnail waits behind it. Starting Snap from the Home tile still hides its launcher before capture.

Drag a screenshot preview by its image into an app that accepts file drops. Click
several previews first to drag them together; dragging an unselected preview selects
that image alone. Buttons remain separate from the drag gesture. Escape or releasing
before preparation finishes cancels the operation.

Drag-out exports the final annotated PNGs to the configured Snap save folder
(`Pictures/Daijobu` by default). These files remain after cancellation, a drop, preview
expiry or app exit. The drag copies files and leaves the clipboard unchanged. After a
successful copy drop, unpinned previews disappear; previews kept in the stack remain.

**Pin to screen** opens a separate reference window from the annotation editor, the
latest screenshot, or a preview's **More** menu. **Keep in stack / Release from stack**
in that menu controls only the preview countdown. A screen pin keeps its original
pixels after the preview expires and has its own Copy, Save/Open and Close actions.

Pinning does not copy or save automatically. References last for the current app
session, with a limit of eight pins and 256 MiB of unique retained image payloads.
Drag the image to move a reference; `Ctrl+Alt+H` hides/restores it with the island.
The tray's **Show pinned screenshots** offers keyboard access: Tab/Shift+Tab, Enter,
Ctrl+C, Ctrl+S and Escape to close. Pins hide before a new screenshot freezes the
desktop and return afterward; open island panels keep their existing Snap behaviour.

By default the island is a notch attached to the top edge of the screen. Choose *Island style* in Settings to switch to the floating pill, or set `[island] style = "pill"` in the config. The Notch page also adjusts its width and height.

The notch also reserves a strip as tall as itself along the top of the screen, so maximized windows (and browser tabs) start below it instead of under it. Turn it off with *Reserve top space* in Settings, or `[island] reserve_top = false`.

*Stay behind windows* keeps the resting island over the wallpaper and under application windows. Opening a panel or showing the volume OSD brings it forward temporarily. Reserved top space is inactive while this setting is on.

### Clipboard

Open **Clipboard** from Home, `Ctrl+Alt+V` or the tray. If another app owns the
shortcut, Home and the tray remain available. History is off by default: choose
**Enable history** to capture future copies. Enabling or restarting does not import
the clipboard's existing content. Pause stops capture while keeping saved history
available for search, copy and paste.

Search text and use the All, Text, Images, Files or Pinned filters. Links and colours
are text subtypes. Select a row and use **Preview** for longer text or an image;
previews display plain text even when rich formats are retained. History survives
restarts; pins bypass normal history expiry. Delete affects history only; Clear
normally removes unpinned items, with a separate confirmed choice to include pins.

Enter or double-click pastes to the previous target; `Shift+Enter` pastes plain text
when available. `Ctrl+C` copies and `Ctrl+P` toggles a pin. Arrow/Page keys navigate,
Tab reaches actions and `Alt+1`–`Alt+9` selects and pastes an available visible row.
Paste restores the intended external window or exact island Terminal tab. If the
target cannot accept automatic paste, the item stays copied with manual-paste
feedback. Image-only items can be copied but cannot be pasted into Terminal.

Supported content includes Unicode text with validated HTML/RTF alternatives,
ordinary PNG/DIB images, and file/folder lists. File entries retain paths rather
than file contents; missing files fail an explicit transfer. File copy and drag
use copy-only effects. Drag supports one selected image or its file list; text,
multi-row bundles and virtual files are outside this stage.

An explicit image drag exports a PNG to `~/Pictures/Daijobu/Clipboard`. Exported
files remain after a drop, cancellation, history deletion or app exit. Capturing,
storing, pinning and previewing history do not export plaintext image files.

History keeps up to 500 unpinned items for 30 days, plus up to 100 pins, within a
512 MiB encrypted-payload budget. Per item: 1 MiB text, 4 MiB each HTML/RTF,
32 MiB encoded images, 64 MiB decoded pixels, a 16,384-pixel maximum edge, and
128 file paths within 256 KiB path text. Oversize content is skipped rather than
truncated. Database overhead is separate from the payload budget.

Clipboard configuration lives in `~/.daijobu/config.toml`:

```toml
[clipboard]
enabled = false        # explicit opt-in; captures future changes only
paused = false
excluded_sources = []  # source executable names, e.g. ["private-app.exe"]
```

History is stored locally in `~/.daijobu/clipboard/history.sqlite3`. Payloads,
preview/search text, source metadata, paths and thumbnails are encrypted with
current-user Windows DPAPI before database writes; IDs, timestamps, types, pin
flags and sizes remain metadata. Search uses decrypted data in bounded memory.
Clipboard exclusion flags and configured source exclusions are respected. Native
acceptance evidence and remaining target-app/hardware checks are tracked in the Stage 8 report.

### Spotlight

Press `Alt+Space` to search apps, files and folders. File-name search covers Desktop, Documents, Downloads, Pictures, Videos and Music, with limits on scan depth and entry count. File results start after two query characters. Icons, file sizes, folder file counts and available app install sizes load in the background.

Enter opens a result; Ctrl+Enter selects a file or folder in Explorer. Esc closes an open menu, then clears a nonempty query, then closes Spotlight. Clicking another window also dismisses it. Settings can change the hotkey to `Ctrl+Alt+Space` or disable it; the default replaces Windows' Alt+Space window menu.

Right-click a result, or use Shift+F10 / the Menu key:

| Entry | Menu actions |
|---|---|
| File | Open, Open with…, Run as administrator for runnable files, Show in Explorer, Open in Terminal, Claude here, Codex here, Copy path, Copy file, Properties, Delete |
| Folder | Open, Show in Explorer, Open in Terminal, Claude here, Codex here, Copy path, Copy file, Properties, Delete |
| App | Open, Run as administrator, Copy name |

Terminal and agent actions start a new island terminal tab in the folder itself or the file's parent. Delete requires a second confirmation and moves the item to the Recycle Bin. Some Spotlight interactions and menu actions still await manual acceptance; see the [Stage V report](task/stage-v-report.md).

### Terminal

``Ctrl+Alt+` `` (or the Terminal tile on Home) drops a real shell out of the notch, like VS Code's built-in terminal. It keeps running when you close the panel, so a long build carries on while you work. Clicking another window hides it. Copy and paste work as in Windows Terminal: select and press Ctrl+C (without a selection Ctrl+C stops the command), Ctrl+V to paste, right-click to copy or paste.

Use the **+** menu for a new Terminal, Claude or Codex tab. Tabs can be reordered, closed and pinned; pinned tabs start in the background automatically after a restart, with their saved order, names, shell arguments and agent type. PowerShell tabs also restore their last reported folder. Their live shortcuts appear beside the clock without opening Terminal first. Unpinned tabs are not restored. This starts new processes; terminal scrollback, running commands and agent conversations are not restored.

Claude and Codex must be installed separately. Their tabs run in **yolo mode**, bypassing approval prompts (and Codex's sandbox); pinned agent tabs restart in that mode too.

The mouse wheel scrolls shell history. Programs that enable terminal mouse tracking receive wheel events at the pointer's cell instead, so they can scroll their own content. Full-screen programs without mouse tracking receive up/down keys when alternate-scroll mode is enabled.

Right-click a tab and choose **Rename tab...** to set its name. Press Enter or Save to apply, or Esc or Cancel to discard. Clear the name to follow the application's title again. Custom names stay through shell title changes and restarts; pin the tab to keep its name across app restarts.

When the island shows its idle clock, running terminal tabs appear beside it as shortcuts. Click an icon to reopen that exact tab. Claude and Codex use the same agent logos as the **+** menu; plain shells use the terminal icon. The notch widens to fit the shortcuts and returns to its configured width when the sessions close. These shortcuts track the island's own terminal sessions, including shells at a prompt after an agent exits.

```toml
[terminal]
shell = ""              # "" = PowerShell 7 if installed, else Windows PowerShell; or "cmd", "wsl", a path
args = []
cwd = ""                # "" = your home folder
font = "Cascadia Mono"  # a Nerd Font (e.g. "MesloLGL Nerd Font") shows prompt icons natively
font_size = 12
hide_on_blur = true     # false keeps the panel open while you work in another window
scrollback = 5000
```

Prompt icons from oh-my-posh or starship come from "Symbols Nerd Font" when it is installed, whatever `font` is.

## How it's built

- **Windows:** native Win32 windows with DirectComposition visuals. The island stays topmost by default, with an optional desktop layer while resting. Terminal input mode, Settings and Spotlight take keyboard focus when opened. Clicks outside the island's shape fall through to whatever is underneath.
- **Rendering:** Skia's GPU backend on Direct3D 12, presenting through a premultiplied-alpha composition swap chain. Grain and dither are SkSL shaders.
- **Motion:** damped springs (stiffness and damping derived from response and damping ratio) that can be interrupted mid-flight.
- **Idle cost:** none. When nothing moves, the message loop blocks and nothing is drawn.
- **Music:** the only place a webview is used. YouTube Music runs in a hidden WebView2 (its window only shows for sign-in and browsing), and the island is its player. The engine starts only when you use Music; the island itself is still drawn with Skia.

```
app/              the daijobu binary
crates/dj-core    springs, themes, feature registry, island state (no unsafe, no Win32)
crates/dj-render  Skia + D3D12 + DirectComposition
crates/dj-win     window, tray, hotkeys, autostart, sound
crates/dj-music   YouTube Music engine (hidden WebView2)
crates/dj-vitals  CPU, RAM, disk, net, disk I/O, battery, top processes
crates/dj-sweep   disk cleanup: scanners, protected-path guard, recycle, operations log
crates/dj-capture screen capture for Snap
crates/dj-clip    Clipboard listener, format normalization, DPAPI and SQLite history
crates/dj-term    the terminal: alacritty_terminal's emulator over a Windows pseudo console
```

Tile and Spotlight live in the existing crates. More crates are added as their stages start. [`AGENTS.md`](AGENTS.md) describes the conventions and the stage-by-stage workflow.

## Privacy

No telemetry. Screenshot capture, Spotlight file indexing and opt-in Clipboard history run locally. Clipboard content is encrypted at rest with current-user Windows DPAPI; capture honours supported privacy flags and configured source exclusions. There is no Clipboard cloud sync or upload. Music connects to YouTube Music through its WebView2 engine.

## Acknowledgements

The island-on-Windows idea owes a lot to [WinIsland](https://github.com/WinIslandProject/WinIsland), used as a reference for ideas only. Daijobu Island shares no code with that GPL-3.0 project. And of course Apple's Dynamic Island, which started it all.

## Contributing

Issues and ideas are welcome. The project is built in stages (see [`task/progress.md`](task/progress.md)), so it helps to check which stage a feature belongs to before opening a PR. Conventions are in [`AGENTS.md`](AGENTS.md).

## Thanks

To everyone who tries an early build, files an issue, or just reads this far: thank you. If Daijobu Island looks useful, a ⭐ on the repo helps other Windows users find it.

<a href="https://star-history.com/#xDaijobu/daijobu-island&Date">
  <img src="https://api.star-history.com/svg?repos=xDaijobu/daijobu-island&type=Date" alt="Star history" width="520">
</a>

## License

Licensed under the [Apache License, Version 2.0](LICENSE). See [`NOTICE`](NOTICE) for bundled fonts (SIL Open Font License 1.1).

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in this project is licensed under Apache-2.0 as above, without any additional terms or conditions.
