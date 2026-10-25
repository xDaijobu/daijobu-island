<h1 align="center">Daijobu Island</h1>

<p align="center">
  A lofi dynamic island for Windows 11.<br>
  Rust, Skia and plain Win32. No webview.
</p>

<p align="center">
  <img src="docs/images/app-pill.png" alt="Collapsed island" height="34"><br><br>
  <img src="docs/images/app-home.png" alt="Expanded island: feature grid" width="620">
</p>

<p align="center"><sub>The current build running on Windows 11. Every feature is still locked while it gets built.</sub></p>

---

*Daijobu* (大丈夫) is Japanese for "it's fine" or "are you okay?". The island sits at the top of the screen and stays out of the way. When you need it, it opens up to show what's playing, catch a screenshot, bring back something you copied, put your windows in place, or tell you whether your PC is having a bad day.

## Status

Early development. The app itself runs: the window, rendering, animation, themes, tray and hotkeys work. Features get unlocked one at a time, each in its own stage. Day-to-day progress is tracked in [`task/progress.md`](task/progress.md).

| Stage | | |
|---|---|---|
| 1 | App shell: window, Skia renderer, spring motion, themes, tray | ✅ done |
| 2 | Media: now playing, controls, audio visualizer | locked |
| 3–5 | Volume OSD, vitals, timer | locked |
| 6–7 | Screenshots with a floating preview stack, clipboard history | locked |
| 8–10 | Notifications, lyrics, `dj` CLI | locked |
| 11 | Window tiling, scenes, restore-after-reboot | locked |
| 12–13 | Disk cleanup, file shelf, ambient sounds | locked |

## Where it's going

These are design mockups from [`docs/plan.html`](docs/plan.html), not screenshots. Open that file in a browser for the interactive version.

<table>
  <tr>
    <td width="50%"><img src="docs/images/concept-media.png" alt="Media"><br><b>Media.</b> Now playing from any app that reports to Windows, with a segmented VU visualizer.</td>
    <td width="50%"><img src="docs/images/concept-snap.png" alt="Snap"><br><b>Snap.</b> Region screenshots with a loupe, annotations, blur, OCR and pin-to-screen.</td>
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

`skia-safe` downloads a prebuilt Skia, so the first build takes well under a minute on top of the Rust compile.

| Shortcut | Action |
|---|---|
| Click the island | Expand / collapse |
| `Ctrl+Alt+H` | Hide / show |
| `Ctrl+Alt+T` | Next theme |

The tray icon has the same controls, plus *Start with Windows*. Settings live in `~/.daijobu/config.toml`.

## How it's built

- **Window:** a raw Win32 popup (`WS_EX_NOREDIRECTIONBITMAP`, tool window, topmost, never activates) with a DirectComposition visual. Clicks outside the pill fall through to whatever is underneath.
- **Rendering:** Skia's GPU backend on Direct3D 12, presenting through a premultiplied-alpha composition swap chain. Grain and dither are SkSL shaders.
- **Motion:** damped springs (stiffness and damping derived from response and damping ratio) that can be interrupted mid-flight.
- **Idle cost:** none. When nothing moves, the message loop blocks and nothing is drawn.

```
app/              the daijobu binary
crates/dj-core    springs, themes, feature registry, island state (no unsafe, no Win32)
crates/dj-render  Skia + D3D12 + DirectComposition
crates/dj-win     window, tray, hotkeys, autostart
```

More crates (media, capture, clipboard, tiling, …) are added as their stages start. [`AGENTS.md`](AGENTS.md) describes the conventions and the stage-by-stage workflow.

## Privacy

No telemetry. When the clipboard and screenshot features land, everything stays on your machine. Clipboard history will be encrypted with Windows DPAPI and will skip anything a password manager marks as private.

## Acknowledgements

The island-on-Windows idea owes a lot to [WinIsland](https://github.com/WinIslandProject/WinIsland), and the cleanup features take ideas from [Mole](https://github.com/tw93/Mole). Both are GPL-3.0. Daijobu Island is an independent project and shares no code with either. And of course Apple's Dynamic Island, which started it all.

## License

Not decided yet (likely MIT / Apache-2.0).
