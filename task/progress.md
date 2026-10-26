# Daijobu Island — Progress

How we work: **one stage at a time.** Each stage runs in its own Orca worktree, with a Claude Sonnet 5.5 agent spawned there. When a stage is done, its status and log are updated here.

Spec and design: [`docs/plan.html`](../docs/plan.html) · README: [`README.md`](../README.md) · Repo: https://github.com/xDaijobu/daijobu-island

Legend: ⬜ todo · 🟡 in progress · 🔵 PR open · ✅ done · 🔒 locked (coming soon in the app)

---

## Stages

| # | Stage | Status | Worktree / branch | Notes |
|---|---|---|---|---|
| 0 | Toolchain setup (Rust, Windows SDK, LLVM) | ✅ | (main checkout) | Rust 1.99.1, SDK 26100, LLVM 23.1.4, Ninja 1.13.2 |
| 1 | **App shell**: window, island, themes, tray, all features locked | ✅ | PR #1 (merged) | brief: [`stage-1-brief.md`](stage-1-brief.md) |
| 2 | **Music = YouTube Music**: one-time sign-in, headless WebView2, island = player | ✅ merged (PR #2), live test pending | `xDaijobu/stage-2-music` | Haiku 5.5 via Orca · brief: [`stage-2-brief.md`](stage-2-brief.md) |
| 2b | **Music Library**: search, library (liked/playlists/listen again), queue, all inside the island (native Skia; data via the hidden YT Music engine) | 🔒 | — | after Stage 2 sign-in is confirmed; mockup in plan.html first |
| 3 | **OSD volume** | ✅ merged (PR #3), live test pending | `stage-3-osd` | done ahead of Stage 2's PR · Haiku 5.5 + subagents · brief: [`stage-3-brief.md`](stage-3-brief.md) |
| 4 | **Vitals (compact)** | ✅ merged (PR #4), live test pending | — | Haiku started, Opus 5.5 finished · lock-icon fix by the coordinator after the first live run |
| 5 | **Vitals + Sweep**: Sweep merged into the Vitals panel (diagnose → clean), full vitals (disk I/O, top processes) | ✅ | PR #6 + `stage-5-safety-fixes` | Opus 5.5 (4 agents: sweep, wiring, render, safety review) · fixes and live test by the coordinator · [`stage-5-report.md`](stage-5-report.md) |
| 6 | **Timer / Pomodoro**: focus/break cycles, chime alert, mini timer on Music compact | ✅ | `stage-6-timer` | Opus 5.5 agent (headless) · live test by the coordinator · [`stage-6-report.md`](stage-6-report.md) |
| 7a | **Snap core**: PrtSc freeze, region/window select with loupe, auto-copy, save + open, snap thumb | ✅ | `stage-7a-snap` | Opus 5.5 agent (headless) · live test with the user · [`stage-7a-report.md`](stage-7a-report.md) |
| 7b | **Snap annotations**: Lightshot-style editor on release (8 tools, select/move, text re-edit, HSV picker, undo/redo) | ✅ merged, live test pending | `xDaijobu/stage-7b-work` | Opus 5.5 agent (headless) · merged at the user's request before the live test · [`stage-7b-report.md`](stage-7b-report.md) |
| 7c | Snap stack: floating previews, countdown, keep-in-stack pin, multi-select, copy/save | planned, not started | — | after 7b verification; [`stage-7c-brief.md`](stage-7c-brief.md); no OLE in this stage |
| 7c-2 | Snap stack: multi-file OLE drag-out | locked | — | after 7c; separate implementation and live target-app checks |
| 7d | Snap: standalone pin-to-screen + OCR | locked | — | moved out of 7b; distinct from keeping a stack card |
| S | **Settings (minimal)**: gear icon on Home, settings panel in the island: auto-startup toggle, Snap hotkey + alt hotkey chips | ✅ | `stage-s-work` | user-requested · Sonnet 5.5 agent · live test with the user · [`stage-s-report.md`](stage-s-report.md) · press-to-record later |
| N | **Notch style**: the island attached to the top edge like an iPhone 13 / MacBook notch (Settings choice, default notch) | ✅ | PR #14 (merged) | user-requested while 7b runs · coordinator skeleton + 4 parallel Opus subagents · live test passed · [`stage-n-report.md`](stage-n-report.md) |
| N2 | **Reserve top space**: the notch registers a top AppBar so maximized windows (Chrome tabs) start below it (Settings switch, default on) | ✅ | `xDaijobu/stage-n2-appbar` | user-requested · built by the coordinator · live-checked by the coordinator |
| 8 | Clipboard history | 🔒 | — | |
| 9 | Notifications | 🔒 | — | needs sparse MSIX |
| 10 | Lyrics | 🔒 | — | |
| 11 | dj CLI | 🔒 | — | |
| 12 | Tile + Scenes + Boot Scene | 🔒 | — | |
| 13 | Shelf, Ambient, glance extras | 🔒 | — | |

The order of stages 2+ can still change. Only one stage runs at a time.

---

## Stage 1: App shell

**Goal:** Daijobu Island runs as a real app. The island looks, moves and feels finished, and every feature is shown but **locked as "coming soon"**.

### Scope
- [x] Cargo workspace: `crates/dj-core`, `crates/dj-render`, `crates/dj-win`, `app` (binary `daijobu`)
- [x] Raw Win32 window: `WS_POPUP` + `WS_EX_NOREDIRECTIONBITMAP | TOOLWINDOW | TOPMOST | NOACTIVATE`, Per-Monitor-V2 DPI manifest, top-center of the primary monitor, not in Alt-Tab or the taskbar
- [x] DirectComposition + D3D12 + Skia (skia-safe `d3d`), premultiplied-alpha composition swap chain sized to the maximum expanded state; the pill animates inside it (no per-frame `ResizeBuffers`)
- [x] Click-through outside the pill (the window rect/region tracks the pill)
- [x] Spring morph (response/ζ from the plan), with a content crossfade (blur + scale)
- [x] **Idle** state: clock + status lamp
- [x] **Expanded "home"**: grid of feature tiles (Media, OSD, Vitals, Timer, Snap, Clipboard, Notifications, Lyrics, dj CLI, Tile & Scenes, Sweep, Shelf, Ambient). All 🔒 locked with "coming soon". Clicking a locked tile plays a small shake + "coming soon" hint
- [x] Feature registry in `dj-core` (id, name, icon, `Locked | Available`), so unlocking a feature later is a single registry change plus its view
- [x] Theme engine: TOML schema from the plan, `base` inheritance, contrast validation, token interpolation on switch, hot reload from `~/.daijobu/themes/`
- [x] Built-in themes: Tobacco Tape (default), Midnight Cassette, Paper Zine (light), Clean
- [x] SkSL grain (time quantized to 18 fps) + ordered dither + optional scanlines, driven by theme tokens
- [x] Embedded OFL fonts: Geist, Geist Mono, Doto (license files included)
- [x] Tray icon: Show/Hide, Theme ▸, Start with Windows ✓, Quit
- [x] Hotkeys: `Ctrl+Alt+H` hide/show, `Ctrl+Alt+T` cycle theme
- [x] Single instance (named mutex), autostart (HKCU Run), config `~/.daijobu/config.toml`
- [x] Idle CPU ≈ 0% (blocks in `GetMessage` when nothing is animating)
- [x] `cargo build --release` passes, `cargo clippy --workspace -- -D warnings` is clean, and `dj-core` has unit tests (spring, theme parse/inherit/validate)

### Acceptance
Run `daijobu.exe`. The pill appears at the top center. Clicking it expands smoothly to the home grid, where every feature shows 🔒 coming soon, and clicking again collapses it. `Ctrl+Alt+T` switches themes smoothly, and so does the tray menu. Idle CPU is about 0%.

### Log
- 2026-10-25: stage defined.
- 2026-10-25: toolchain installed. Worktree `stage-1-app-shell` created and the Sonnet 5.5 agent spawned.
- 2026-10-25: **done.** 4 commits on `stage-1-app-shell` (not merged to master yet). Agent report: `task/stage-1-report.md` in the worktree.
- 2026-10-25: re-checked from the main session. `cargo build --release` OK, `clippy -D warnings` clean, 31 tests pass. Ran `daijobu.exe`: the pill is top-center with the lamp and clock, idle CPU was 0 ms over 5 s, RAM about 110 MB.
- Deviations: skia-safe uses default features + `d3d` + `textlayout`, because the prebuilt for a minimal feature set isn't published and a source build hit "Filename too long". The exe is about 8.6 MB.
- Not verified yet (needs a human check): a real right-click on the tray icon, absence from Alt-Tab, runtime DPI change and multi-monitor, device-lost recovery, exclusive-fullscreen games.
- Follow-ups: exe icon, CI (clippy + test), fix the TOML example in plan.html (several keys on one line), activity scheduler (needed before Stage 2).
- 2026-10-25: public repo https://github.com/xDaijobu/daijobu-island created. PR #1 (`stage-1-app-shell` → `master`): https://github.com/xDaijobu/daijobu-island/pull/1

## Stage 2: Music (YouTube Music)

**Goal:** click **Music** on the island, sign in to YouTube Music once, and from then on YouTube Music plays in the background with **no window**. The island is the player. The "Media"/SMTC approach was dropped (see the log).

### Scope
- [ ] Rename the feature **Media → Music** (registry id, label, icon stays). Unlock **Music**. All other tiles stay 🔒.
- [ ] **`crates/dj-music`**: a hidden **WebView2** engine (`webview2-com`, MIT).
  - It lives in its own hidden top-level window (not the island window). Profile/user-data folder `~/.daijobu/ytmusic/`. Browser args include `--autoplay-policy=no-user-gesture-required`.
  - The engine only starts once the user has signed in or clicks Sign in. After sign-in it auto-starts with the app.
- [ ] **Sign-in flow**:
  - Music view when signed out shows a "Sign in to YouTube Music" button. It shows the engine window (normal size, centred, titled "Daijobu Island · YouTube Music") at `https://music.youtube.com`.
  - When the user is signed in (detect it from the page, e.g. the avatar/account button), the window hides itself. Store `signed_in = true` in config.
  - Never automate the login and never spoof the user agent. If Google blocks embedded sign-in, show a clear message plus "Open in browser".
- [ ] **Bridge script** (injected with `AddScriptToExecuteOnDocumentCreated`):
  - It posts JSON through `window.chrome.webview.postMessage`: title, artist, album, artwork URL (largest from `navigator.mediaSession.metadata`), playing/paused, position, duration and like state if cheap.
  - Post on change, and roughly every 1 s while playing. Don't poll faster.
- [ ] **Controls** via `ExecuteScript`: play/pause, next, previous and seek. Use the page's own player API/buttons, and keep the selectors in one file.
- [ ] **VU**: inside the page, use a Web Audio `AnalyserNode` on the media element (connect it back to the destination so the sound isn't lost). It posts 5 or 22 bands at ≤ 30 Hz while playing. If the analyser yields nothing, hide the VU. Never fake it.
- [ ] **Island states**:
  - Idle (clock), then Music compact (224×36: art 24, title, 5-col VU) when YT Music is playing or paused-with-track.
  - Click → Music expanded (400×184: art 88, title/artist, "youtube music" + lamp, scrubber with times, prev/play/next, VU strip, buttons **Open YouTube Music**, which shows the window for browse/search, and **Home**).
  - Click empty → back to compact. If nothing is loaded, return to Idle.
- [ ] Artwork: download the artwork URL on a worker (≤ 256 px) and decode with Skia. Accent from cover is clamped (S 45–65%, L 62–72%) and mixed via `accent_from_cover`.
- [ ] Tray: "YouTube Music ▸ Show window / Sign out / Stop engine". Sign out stops the engine and deletes `~/.daijobu/ytmusic/`.
- [ ] Idle CPU ≈ 0 with nothing playing (the island doesn't redraw). While playing, the island redraws the VU at ≤ 30 fps.
- [ ] AGENTS.md: add the single exception to non-negotiable #1 (WebView2 only as the hidden YouTube Music engine + its sign-in/browse window).
- [ ] Tests for pure logic (bridge message parsing, state transitions, accent clamp). `build --release`, `clippy -D warnings` and `test` pass.

### Acceptance
Run the app and click the pill → Home → Music → "Sign in to YouTube Music". Sign in with Google (done by the **user**). The window hides. Play something via "Open YouTube Music", then hide the window. The island shows compact music with art, title and a moving VU, and expanded controls work. Restart the app: still signed in, no window. Sign out from the tray works.

### Rules for the agent
- **No audible tests.** Don't play audio, don't open Media Player, and don't sign in. The user tests playback and sign-in themselves. The agent verifies the build, that the engine window opens at music.youtube.com, that the script injects, and the parsing/state logic with unit tests and fake messages.

### Log
- 2026-10-25: the first Stage 2 attempt ("Media", SMTC + WASAPI, Sonnet) was **stopped and thrown away** at the user's request. It had opened Windows Media Player and played test audio. Its worktree and branch were deleted without a commit.
- 2026-10-25: Stage 2 redefined as Music = YouTube Music (headless WebView2). The plan was updated (`docs/plan.html`). The agent runs on Haiku 5.5 to save quota.
- 2026-10-25: worktree `stage-2-music` created and the Haiku 5.5 agent spawned (handle `structured-agent-session-claude_bce718e6_1990_448b_ba16_f328ea83207e`).
- 2026-10-25: the agent finished. 4 commits; clippy is clean; 51 tests pass (dj-core 49, dj-win 2). Report: `task/stage-2-report.md` in the worktree.
  - Verified live: the hidden engine reaches music.youtube.com, the bridge posts `auth signedIn:false`, and idle CPU is about 0.
  - **Not verified** (needs the user): the sign-in click, the island visuals, the `bands`/`state` messages, the controls, and the Google-block detection.
  - The `#play-pause-button`/`.next-button` selectors are written from memory and haven't been checked against the live page.
- 2026-10-25: **held** at the user's request. No PR until the user tests sign-in and playback. The worktree is kept.

---

## Stage 3: OSD volume

**Goal:** the volume keys show Daijobu Island's own volume pill instead of the Windows flyout.

### Scope
- [ ] **Unlock OSD** in the feature registry. Everything else stays locked, except Music once Stage 2 merges.
- [ ] Low-level keyboard hook (`WH_KEYBOARD_LL`) for `VK_VOLUME_UP/DOWN/MUTE`. Swallow the key and apply the change through `IAudioEndpointVolume` (`VolumeStepUp/Down`, `SetMute`) on the default render endpoint. The hook callback must return fast, with the work posted to the UI thread. The native flyout then never appears.
- [ ] Volume changes from elsewhere (the Settings slider, other apps) are picked up with `IAudioEndpointVolumeCallback` (`RegisterControlChangeNotify`) and also show the pill. Track default-device changes with `IMMNotificationClient`.
- [ ] **OSD state** in `dj-core`: a temporary activity on top of whatever is showing (Idle/Home). The pill morphs to 220×40 with a speaker icon (muted, low, mid or high), a 4 px level bar and a mono number. It returns to the previous state 1.5 s after the last change, and each new change resets the timer. Click or drag on the bar to set the volume.
- [ ] Holding the key repeats the steps, and each one updates the bar smoothly (spring on the level).
- [ ] The OSD tile in Home opens the same OSD view (a manual peek). A config toggle `[osd] replace_volume_flyout = true` turns the hook off when false.
- [ ] Idle CPU ≈ 0. The hook only does cheap work, and nothing redraws when idle.
- [ ] Tests: OSD timer/state transitions, icon level thresholds, level clamping. `build --release`, `clippy -D warnings` and `test` pass.

### Acceptance
Press volume up/down/mute: the island shows the volume pill and the Windows flyout does not appear. Changing volume from the Windows Settings slider also shows the pill. After 1.5 s it returns to the clock. OSD is unlocked in Home.

### Rules for the agent
- **No audio.** Changing the volume level is OK, but **restore the user's original volume and mute state** after testing. Never play sound.
- Use **Haiku 5.5 subagents** for independent parts (see the brief). The main agent integrates.

### Log
- 2026-10-25: stage defined. Stage 2 is held, so Stage 3 runs first, from `master`.
- 2026-10-25: relaunched in **headless** mode while the user games. Done: 5 commits; clippy and test pass (headless). `build --release` and every live check (hook, flyout suppression, callbacks, visuals, idle CPU) are **user to test**. Report: `task/stage-3-report.md` in the worktree.

---

## Stage 4: Vitals (compact)

**Goal:** a quick "is my PC okay?" check. The idle pill's lamp reflects health, and the Vitals panel shows the numbers.

### Scope
- [ ] **Unlock Vitals** in the feature registry.
- [ ] **`crates/dj-vitals`** (Win32 only, no WMI and no PowerShell):
  - CPU % from `GetSystemTimes` deltas
  - RAM % from `GlobalMemoryStatusEx`
  - system-drive used % from `GetDiskFreeSpaceExW`
  - net ↓/↑ bytes/s from `GetIfTable2` deltas (up, non-loopback adapters)
  - battery %/charging from `GetSystemPowerStatus` (hidden when there's no battery)
  - uptime from `GetTickCount64`
- [ ] **Sampling** runs on a worker. It ticks at 1 Hz **only while the Vitals panel is open**, and once every 30 s otherwise (just for the lamp), using a coalescable timer. No sampling while the island is hidden.
- [ ] **Health score** (pure, in `dj-core`, ideas from the plan; our own weights, no copied lists): start at 100, then subtract penalties:
  - CPU: none up to 50%, up to 15 at 85%, up to 30 above that
  - RAM: none up to 70%, up to 12 at 88%, up to 25 above that
  - disk: none up to 80%, up to 10 at 93%, up to 20 above that
  - uptime: 1 above 7 days, 3 above 14 days

  Clamp to 0–100. Bands: 85+ "daijobu", 65+ "ok", 45+ "hmm", below that "dame". Penalties also produce reason tags ("High CPU", "Low disk", "Restart soon"). (Disk I/O and battery wear are left for the full Vitals stage.)
- [ ] The **idle-pill lamp colour** follows the band (theme accent / warn), behind config `[vitals] health_lamp = true`.
- [ ] **Vitals panel** (400×150, per plan.html):
  - header "daijobu? · health" + score + band
  - 3 rings (CPU, RAM, system drive) with values
  - net ↓/↑ text + sparkline of the last 60 samples
  - battery if present
  - top reason tag if any

  It opens from the Home tile. Clicking an empty area goes back to Home or Idle, matching how the other panels behave.
- [ ] Idle CPU ≈ 0 with the panel closed (one sample every 30 s). With the panel open, the island redraws at most once per sample (1 Hz) plus springs.
- [ ] Tests: score bands and penalties, delta/rate maths (including counter wrap), formatting. `build --release`, `clippy -D warnings` and `test` pass.

### Acceptance
Open Home → Vitals: the rings and net show live values once per second, and the score and band look sensible. The idle lamp colour changes when the load is high. Idle CPU is about 0 with the panel closed.

### Rules
**HEADLESS** (the user may be gaming): don't run the exe and don't show windows. Verify with fmt/clippy/test only, with `CARGO_BUILD_JOBS=2`. Live checks are marked "user to test". Use Haiku subagents.

### Log
- 2026-10-25: stage defined. Starts from the `stage-3-osd` branch (stacked) once Stage 3 is done.
- 2026-10-25: Haiku was stopped part-way and Opus 5.5 finished the stage. It merged Stage 3 (OSD + Music); Music, OSD and Vitals are all Available. 12 commits; clippy is clean; tests pass. The coordinator checked `build --release` (2m45s, exe 9.8 MB). Live checks (values, lamp, panel visuals, idle CPU) are user to test. Report: `task/stage-4-report.md`.

---

## Stage 5: Vitals + Sweep

**Goal:** Vitals diagnoses and Sweep cleans, in one panel. You see "Low disk", click "🧹 6.8 GB reclaimable", preview, clean, and watch the disk ring and score improve.

### Scope
- [x] **Remove the Sweep tile** from the registry (12 features). Vitals stays Available, and the tests are updated.
- [x] **Full vitals** (`dj-vitals`):
  - **disk I/O** (read + write bytes/s, through PDH `\PhysicalDisk(_Total)\Disk Bytes/sec` or an equivalent Win32 API)
  - **top processes**: top 3 by CPU and top 3 by RAM (Toolhelp snapshot + process times / memory counters), sampled only while the panel is open
  - the health score gains a disk-I/O penalty: none below 50 MB/s, linear up to 10 at 150 MB/s and above
- [x] **`crates/dj-sweep`**. Every category list is **our own**, written from Windows knowledge. Nothing is copied from Mole.
  - **Temp & reports:** user `%TEMP%` older than 7 days, WER ReportArchive/ReportQueue, and CrashDumps `*.dmp` older than 7 days.
  - **Browser caches:** Chrome, Edge, Brave and Vivaldi per profile (`Cache`, `Code Cache`, `GPUCache`), plus Firefox `cache2`. Skip a browser while its process is running.
  - **App & dev caches:** path-based only, with no spawning of tools. VS Code/Discord/Slack Electron caches, npm `_cacache`, pip cache, Cargo `registry/cache`, Yarn cache, and the Go build cache.
  - **Dev purge:** roots from config `[sweep] project_roots` (default `~/Documents`, `~/Projects`, `~/Code`, `~/source/repos`, `~/dev`), depth ≤ 4.
    - Candidates: `node_modules`, `target`, `dist`, `build`, `.next`, `.venv`/`venv`, `__pycache__`, `bin`/`obj`. Each needs a **context marker**:
      - `target` needs `Cargo.toml` or `pom.xml`
      - `bin`/`obj` need a `*.csproj`
      - `build`/`dist` need a build signal (`package.json` with a build script, CMake, Gradle, pyproject `[build-system]`)
    - The candidate must be a direct child of the project, contain no nested `.git`, and have **no git-tracked files** (`git ls-files`; skip on error).
    - Skip any project touched in the last 14 days.
  - **Big files:** the top 20 under the user profile (skipping protected paths), with *Reveal* (`explorer /select`) and *Recycle*.
  - **Quick actions:** flush DNS (`DnsFlushResolverCache`) and empty the Recycle Bin (`SHEmptyRecycleBinW`, with a confirm step).
- [x] **Safety (mandatory):**
  - A protected-path guard (Windows, Program Files, ProgramData, the profile root itself, `.ssh`, `.gnupg`, `.aws`, `.azure`, `.kube`, `.docker`, browser `Login Data`/`Cookies`/`Local Storage`/`IndexedDB`/`Sessions`, Credentials/Vault/Crypto, `AppData\Local\Packages`, and `~/.daijobu` itself).
  - Never follow reparse points. Resolve the final path and re-verify immediately before deleting.
  - **Preview is required** before the Clean button enables.
  - Purge and big files go to the **Recycle Bin** (`IFileOperation`). Only temp/cache entries are deleted permanently.
  - Scans run off the UI thread, with a time budget. Admin-only locations are out of scope for this stage.
  - Every action is logged to `~/.daijobu/operations.log` (`[time] [sweep run=<id>] REMOVED|RECYCLED|FAILED <path> (detail)`).
- [x] **Reclaimable estimate:** a cached background scan stored in `~/.daijobu/sweep-cache.toml`. It refreshes when older than 6 h and the panel opens, never on a hot loop, and never while the island is hidden.
- [x] **UI:**
  - The Vitals panel gets a "🧹 X reclaimable" button, made prominent when the reason is "Low disk", plus disk I/O and top-3 processes.
  - The button morphs into the **Sweep view**: category checklist with sizes, a per-category preview list, a "Clean X" button, progress, then a result ("Freed 4.1 GB").
  - Afterwards it goes back to Vitals with the disk ring and score refreshed. The OSD still overlays it.
- [x] Idle CPU ≈ 0. No scanning unless the panel is open, the cache is stale, or the user asks.
- [x] Tests (**temp dirs only**): protected-path guard, reparse skip, age filter, purge context rules, git-tracked skip, size aggregation, oplog format, disk-I/O penalty, top-N selection. `build --release`, `clippy -D warnings` and `test` pass.

### Acceptance
Home → Vitals shows the score, rings, disk I/O, top processes and "🧹 X reclaimable". Click it → Sweep → preview the categories → Clean. The files go to the Recycle Bin or are removed (temp/cache), the log has entries, and Vitals shows more free disk. Protected paths are never touched.

### Rules for the agent
**HEADLESS** (don't run the exe; fmt/clippy/test only; `CARGO_BUILD_JOBS=2`). **Never delete or scan the user's real files.** Every scan and delete in tests runs inside temp directories. The user does the real clean themselves.

### Log
- 2026-10-25: stage defined (the user asked for Sweep inside Vitals).
- 2026-10-26: built by 4 parallel Opus agents. fmt, clippy -D warnings, 188 tests and the release build pass (verified by the coordinator). PR opened; live test pending.
- 2026-10-26: merged at the user's request before the safety fixes. **Open BLOCKER:** the Temp category trusts `%TEMP%` (if unset, Windows falls back to the profile, so profile folders could be deleted permanently). Do not clean "temp & reports" until it is fixed. Also open: junction TOCTOU in `remove_rec`, `FOF_WANTNUKEWARNING` on recycle, the 14-day activity cap, the big-file confirm armed by index, and a few lows (see the PR #6 comments). Unfinished fix: local branch `wip/stage-5-safety-fixes`. `task/stage-5-report.md` is not written yet.
- 2026-10-26: safety fixes on `stage-5-safety-fixes` (B1, M1, M2, M3 and the lows, plus a big-files budget bug found live). Live test on the user's machine passed: temp clean (2700 removed, 0 failed, 29 → 109 GB free), dev purge to the bin and restore, hide stops the scan. Report: [`stage-5-report.md`](stage-5-report.md). Known issue: big files is a partial list on large profiles.

---

## Stage 6: Timer / Pomodoro

**Goal:** a Pomodoro you can run from the island without looking away. Start a 25/5 cycle from the Timer tile, watch it count down in the pill (or on the edge of Music compact), and get a soft chime and an alert when a session ends.

### Scope
- [x] **Unlock Timer** in the feature registry (4 of 12 ready). Home → Timer opens the **Timer panel**; `ClickOutcome::Open` is handled.
- [x] **`dj-core::timer`** (pure, `forbid(unsafe)`): deadline-based state machine `Stopped | Running { ends_at } | Paused { left } | Done { finished, next }`. Time comes in as `Instant`, so it never drifts and is fully testable.
  - Phases: focus, break, long break. Every `every`-th focus (default 4) is followed by the long break. A focus counter shows "focus 2/4".
  - Actions: start, pause/resume, skip (ends the phase now, without the alert), reset (back to Stopped, counter cleared), continue (from Done, starts `next`), stop (from Done, back to Stopped).
  - A remaining time is never negative. A clock that seems to go backwards (an `Instant` before the start) never adds time.
- [x] **Config** `[timer]` in `config.toml`: `focus_min = 25`, `break_min = 5`, `long_break_min = 15`, `every = 4`, `chime = true`. Out-of-range values (0, over 180 min) fall back to the defaults.
- [x] **Presets** in the panel: 25/5, 50/10, 15/3 (the long break and `every` come from the config). They can only be changed while Stopped. The chosen preset is in effect until the app restarts (not written back to config).
- [x] **Timer panel** 400×184 (r 32): big Doto mm:ss, a progress ring, the phase label (`focus 2/4` · `break` · `long break` · `paused`), preset chips, and start/pause, skip and reset buttons. An empty click collapses it, like Vitals.
- [x] **Timer compact** 172×36 (r 18), per the plan: ring 18, a `focus`/`break` label, Doto 18 mm:ss. It is the rest state while the timer runs and no music is loaded. A click opens the panel.
- [x] **With music:** Music compact stays the rest state (media > timer running). It gains a thin progress arc on its edge plus a small mono mm:ss. Music expanded is unchanged.
- [x] **Session end** ("timer done" is above media): the pill expands to a **Timer alert** (about 360×72: `focus done · break 5:00` with ▸ continue and × stop). The chime plays once (if `chime = true`), and the island waits for a click. Continue starts the next phase; × stops. The OSD still shows over it. If a panel (Home, Music expanded, Vitals, Sweep, Timer) is open, the chime plays at once, but the alert waits until the panel collapses. The Timer panel itself shows the done state inline.
- [x] **Chime:** a short, soft bell **synthesised in code** (sine partials with an exponential decay, about 0.8 s, 44.1 kHz mono 16-bit), built once into an in-memory WAV and played with `PlaySound(SND_MEMORY | SND_ASYNC)`. No third-party audio asset.
- [x] **Hidden island** (Ctrl+Alt+H): the timer keeps running and the chime still plays. The alert shows when the island is next visible.
- [x] **Idle CPU ≈ 0:** one `WM_TIMER` at the deadline, plus a 1 Hz redraw only while timer digits are on screen. Nothing is armed while the timer is Stopped, or while it is Paused and off screen.
- [x] Tests: state machine (start, pause/resume keeps the remainder, skip, reset, continue/stop, long break every 4, counter, clock going backwards), config ranges, panel/alert hit testing, the island rest state with and without music, and the chime WAV (header, length, no clipping). `build --release`, `clippy -D warnings` and `test` pass.

### Acceptance
Home → Timer → 25/5 preset → start. The pill shows the countdown (or the arc on Music compact while a song plays). Pause, resume, skip and reset all behave. At 0 the alert appears with a soft chime. ▸ starts the break, and after 4 focus sessions the long break comes up. Idle CPU stays about 0 while the timer is Stopped.

### Rules for the agent
Headless: fmt, clippy and test only, `CARGO_BUILD_JOBS=2`. Don't run the exe and don't play audio; the coordinator does the live test. No new third-party assets.

### Log
- 2026-10-26: stage defined with the user (Pomodoro, chime alert that waits for a click, mini timer on Music compact, panel presets + config).
- 2026-10-26: built by one Opus 5.5 agent (234 tests). Coordinator re-verified fmt, clippy, test and release, then live-tested the panel, compact, pause, done inline, the alert, continue, the long break, reset, presets and idle CPU 0. Not verified live: chime audibility, the Music compact arc, sleep.

---

## Stage 7a: Snap core

**Goal:** a Lightshot-style screenshot that never leaves the island's look. Press PrtSc, drag a region (or click a window), release, and paste. The image is already on the clipboard, and the island shows what you took.

Snap is split into **7a** core (this one), **7b** annotations, **7c** the preview stack, **7c-2** OLE drag-out, and **7d** standalone pin-to-screen + OCR. Later-stage features stay out of this stage.

### Scope
- [x] **Unlock Snap** in the feature registry (5 of 12 ready). Clicking the tile starts a snap (the island collapses first, so it isn't in the shot).
- [x] **Hotkey:** PrtSc (`VK_SNAPSHOT`, no modifiers) through `RegisterHotKey`. If registration fails (for example, Windows' "Use the Print screen key to open screen capture" setting holds it), fall back to **Ctrl+Shift+S**, and write the reason to the app log. `[snap] hotkey = "PrtSc"` in config accepts `"PrtSc"`, `"Ctrl+Shift+S"` or `"Ctrl+Alt+S"`.
- [x] **`crates/dj-capture`** (Win32 + D3D11/DXGI; the only new crate):
  - **Freeze:** capture every monitor once, before the overlay shows. DXGI Desktop Duplication per output, with GDI `BitBlt` from the screen DC as the fallback. The result is one BGRA frame per monitor, with its desktop rect and DPI.
  - **Window rects** for snapping: `EnumWindows` → visible, not minimised, not cloaked (`DWMWA_CLOAKED`), not tool windows, not our own windows, with the size from `DWMWA_EXTENDED_FRAME_BOUNDS`. Z-ordered top to bottom.
  - **PNG encode** through WIC (`IWICImagingFactory`, built into Windows; no new crates).
  - **Clipboard:** `CF_DIB` (top-down converted to bottom-up, 32-bit) plus the registered `"PNG"` format, in one `OpenClipboard`/`EmptyClipboard` session.
  - All `unsafe` stays in this crate (and existing platform crates), each block with a `// SAFETY:` comment.
- [x] **`dj-core::snap`** (pure, tested): selection geometry in physical desktop px, which can be negative on left or top monitors.
  - drag in any direction, normalised to a rect
  - a click with no drag selects the window under the cursor (topmost hit from the rect list)
  - arrow keys nudge the selection by 1 px (Shift: 10 px)
  - clamping to the virtual desktop
  - a minimum size of 2×2 (anything smaller cancels)
  - loupe sampling: a 15×15 px grid around the cursor, clamped at the edges
  - the size label and hex colour text
  - the file name `snap-YYYY-MM-DD_HHMMSS.png`, with `-2`, `-3`… when the name is taken
- [x] **Overlay window:** a separate topmost `WS_POPUP` covering the whole virtual desktop, drawn with Skia on the existing DComp/D3D12 path. It shows the frozen frame, dims (about 55%) outside the selection, a dashed selection outline, the size label, and the loupe (8× magnified, pixelated, with a crosshair and `#E0A458 · 812,344`). Hovering highlights the window under the cursor before any drag. Esc or a right click cancels. The cursor is a crosshair. It renders only on input (no idle loop). It's created when the snap starts and destroyed after it, so the swap-chain rule for the island window is unaffected.
- [x] **On release:** crop from the frozen frame (across monitors if the rect spans several). Copy it to the clipboard (PNG + CF_DIB), close the overlay, and show the **snap thumb** in the island.
- [x] **Snap thumb** 330×72 (r 26, per the plan's size): a thumbnail (76×48, r 8, aspect-fit), `snap · 612×388`, `✓ copied`, and **copy** and **save** buttons. Activity priority sits right below the OSD (above timer done and media). It disappears after 6 s; hovering pauses the countdown. A click elsewhere on the thumb dismisses it. After it goes, the island returns to its rest state (Music/Timer compact or Idle).
- [x] **Save:** PNG to `[snap] save_dir` (default `%USERPROFILE%\Pictures\Daijobu`, created if missing), and the thumb then shows `saved`. The save never overwrites (see the naming rule). Failures show `save failed` and are written to the app log.
- [x] **Multi-monitor and DPI:** the overlay spans every monitor, the selection may cross them, and the output is in physical pixels (no scaling).
- [x] **Idle CPU ≈ 0:** nothing is captured or kept in memory outside a snap. The frozen frames are freed when the overlay closes; only the last crop is kept, for the thumb's copy/save, until the thumb goes.
- [x] Tests:
  - selection geometry (directions, click-to-window with z-order, nudge, clamps, negative coordinates, the minimum size)
  - loupe sampling at the edges
  - hex and size text
  - file naming and collisions
  - config parsing
  - the snap-thumb priority and timeout in the island
  - DIB conversion (row order, stride)
  - a WIC PNG encode→decode round trip on a synthetic bitmap (no screen capture in tests)

  `build --release`, `clippy -D warnings` and `test` pass.

### Acceptance
Press PrtSc. The screen freezes under a dimmed overlay, the loupe follows the cursor with coordinates and hex, and hovering a window outlines it. Drag a region and release: paste into Paint or a chat and the exact pixels appear. The island shows the snap thumb with the size and `✓ copied`; **save** writes a PNG to `Pictures\Daijobu`. Esc cancels without touching the clipboard. Idle CPU is about 0 between snaps.

### Rules for the agent
**Headless:** fmt, clippy and test only, `CARGO_BUILD_JOBS` from the env script. Don't run the exe, don't capture the real screen, and don't touch the real clipboard in tests (put the clipboard behind a trait, or test only the byte conversion). The coordinator does the live test.

### Log
- 2026-10-26: Snap split into 7a/7b/7c with the user. 7a defined: PrtSc with a Ctrl+Shift+S fallback, auto-copy on release plus the island thumb, save to `Pictures\Daijobu` without a dialog.
- 2026-10-26: built by one Opus 5.5 agent (284 tests, new crate `dj-capture`). The coordinator re-verified fmt, clippy, test and release. Live test with the user passed (freeze, loupe, window snap, auto-copy PNG + DIB, save, window click). Added at the user's request: after save, the button reads **open** and reveals the file in Explorer.
- 2026-10-26: hotkey fix after the user reported that PrtSc opened Snipping Tool. Windows' "Print screen opens screen capture" doesn't use RegisterHotKey, so the keyboard hook now takes plain PrtSc (Alt/Win+PrtSc stay Windows'). The always-on `[snap] alt_hotkey` defaults to Ctrl+Space (the user's choice; it takes autocomplete away from IDEs, and `"none"` turns it off). It replaces the Ctrl+Shift+S fallback, which would have taken Save As from every app. Both verified live by the user.

---

## Stage 7b: Snap annotations

**Goal:** Lightshot-style markup before the shot leaves the overlay. Press PrtSc, drag a region and release: the selection stays and a toolbar appears. Draw an arrow, box a button, type a note, pixelate a password, number the steps, undo a mistake, then press Ctrl+C. The pasted image has the markup, and the island shows the thumb as before.

Pin and OCR move to a new Stage 7d. The Snap stack stays 7c.

### Scope
- [x] **Flow (replaces 7a's copy on release).** The overlay has two phases:
  - **Selecting:** unchanged from 7a (hover outline, drag, window click, arrow nudge, loupe). Esc or a right click cancels.
  - **Editing:** starts on release (a drag of at least 2×2, or a window/monitor click). The selection stays with 8 handles and its size label, the toolbar appears, and the loupe hides. Nothing touches the clipboard yet. A right click does nothing here.
  - **Finish:**
    - `Ctrl+C`, `Enter` or the copy button: render the shot with its markup, copy it (PNG + `CF_DIB`), close the overlay and show the snap thumb (as in 7a).
    - `Ctrl+S` or the save button: the same, plus a save to `[snap] save_dir` (the 7a naming rule). The thumb opens as `saved`, and its button reads `open`.
    - `Esc` or ✕: cancel. Nothing is copied or saved, and there is no confirm.
  - The thumb's copy/save buttons work as in 7a, on the annotated image.
- [x] **Toolbar.** It uses the pill colour, r 12, 26 px buttons with r 8, and the active button in accent (plan.html `.snapov .tools`). Contents in order:
  - `select`
  - the 8 tools: `pen`, `line`, `arrow`, `rect`, `marker`, `text`, `pixelate`, `step`
  - 8 colour swatches plus a custom swatch
  - sizes `S`, `M`, `L`
  - `undo`, `redo`
  - `copy`, `save`, `✕`

  Placement: under the selection, left-aligned with it. If there's no room it goes above, and if neither fits it sits inside the selection's bottom edge. It is clamped to the monitor that holds most of the selection, and wraps to two rows if it is wider than that monitor.
- [x] **Keys.** Tool shortcuts work except while typing: `V` select, `P` pen, `L` line, `A` arrow, `R` rect, `H` marker, `T` text, `X` pixelate, `N` step. The mouse wheel steps S/M/L. `Ctrl+Z` undoes, and `Ctrl+Y` or `Ctrl+Shift+Z` redoes.
  - **While typing a text:** Enter is a new line (it doesn't copy), and Backspace/Delete edit the text (they don't delete a shape). `Ctrl+C` and `Ctrl+S` commit the text first, then finish. Esc commits the text and stays in editing; a second Esc cancels.
- [x] **Tools.** Geometry is in physical desktop px. Sizes are logical px × the DPI scale of the monitor that holds most of the selection. Drawing starts only inside the selection, and markup is clipped to the selection on screen and in the output.
  - **pen:** freehand with round caps and joins, smoothed. A click without movement leaves a dot. Width S/M/L = 2/4/8.
  - **line, arrow:** straight from press to release. The arrow has a filled head scaled with the width. A drag shorter than 3 px is dropped.
  - **rect:** outline only, same widths.
  - **marker:** like pen but 4× the width, in the colour at 40% alpha. One stroke never darkens where it crosses itself.
  - **text:** a click puts a caret there. Typing, Backspace, Delete, ←/→, Home/End and Enter (new line) work. A click elsewhere, Esc or a tool change commits the text, and an empty text is dropped. Font Geist Medium at 14/20/28. The caret doesn't blink (no timer). No IME composition: input comes through `WM_CHAR`, surrogate pairs included.
  - **pixelate:** drag a box. The frozen screen under it shows as square blocks of max(12 px, 10/14/20 × scale), on a grid anchored at the box's top-left. It covers any earlier markup under it. This is pixelate, not blur, because blur can be partly reversed.
  - **step:** a click places a filled circle (diameter 20/28/38) with the next number (the largest existing + 1, so an undo gives the number back). The number is black or white, whichever contrasts more with the colour.
- [x] **Colour.**
  - 8 swatches: red `#E5484D`, yellow `#F5C451`, green `#46A758`, blue `#3E7BFA`, purple `#8E4EC6`, white `#FFFFFF`, black `#111111`, and the theme accent.
  - A custom swatch shows the last custom colour. Clicking it opens an **HSV picker** popover next to the toolbar: a saturation/value square, a hue strip, a preview and the hex text. Dragging in it sets the colour live, and a click outside closes it.
- [x] **Select tool** (`V`, or a click on the active tool):
  - **Selection:** drag a handle to resize the selection, or drag on an empty spot inside it to move it. It is clamped to the desktop with a 2×2 minimum, and dragging past the opposite edge flips it. This is not undoable (as in Lightshot).
  - **Shapes:** a click selects a shape, topmost first, with about 6 px of hit tolerance on strokes. The selected shape gets a dashed box. Drag to move it, press Delete or Backspace to remove it, or click a swatch or a size to restyle it. Shapes can't be resized or rotated.
  - **Text:** double-click a text to edit it again. The text tool also edits an existing text when you click on it.
  - Markup stays put on screen when the selection moves, and what falls outside is clipped.
- [x] **Undo/redo.** The history keeps snapshots of the shape list (cap 100). Each of these is one step: add, move (press → release), delete, restyle, one text edit session, and one picker drag on a selected shape. A new step clears redo. `Ctrl+Z` while typing commits the text, then undoes it. The selection geometry and the current tool, colour and size are not in the history.
- [x] **Prefs** are remembered across snaps and restarts in `[snap.annotate]`:
  - keys: `tool`, `color`, `custom`, `size`
  - defaults: arrow, red, `#FF7AC6`, `M`; invalid values fall back to them
  - written when the overlay closes, only if they changed, keeping the other sections (the Stage S writer)
- [x] **The output is what you see.** The copied/saved image is drawn by the same markup code as the overlay, onto a CPU surface the size of the crop, in physical px. It never contains the handles, outline, toolbar, dim or loupe. With no markup it is byte-identical to the 7a crop.
- [x] **Cursors:**
  - crosshair while selecting and with a drawing tool
  - I-beam for text
  - move/size cursors over shapes and handles with select
  - arrow over the toolbar and the picker
- [x] **Idle CPU ≈ 0.** The overlay still draws only on input, with no caret blink and no animation. The frozen frames live until the overlay closes (as in 7a).
- [x] **Tests.** `fmt`, `clippy -D warnings`, `test` and `build --release` pass.
  - **dj-core** (pure):
    - each tool from press/drag/release, including dots and dropped short drags
    - history: undo/redo, the cap, redo cleared by a new step, one step per move/edit/picker drag
    - step numbering after undo
    - hit testing: topmost first, tolerance, text bounds through a fake measurer
    - moving, deleting and restyling a shape
    - text editing: insert, Backspace at 0, caret moves, new line, empty text dropped, re-edit is one step, Ctrl+Z while typing
    - selection handles: resize, flip, clamp, minimum size, move
    - toolbar placement (below / above / inside / wrap) and toolbar hit testing
    - HSV ↔ RGB round trip and picker hits
    - key mapping: letters off while typing
    - `[snap.annotate]` parsing, defaults and invalid values
  - **dj-render** (CPU surface):
    - an output with no markup equals the crop byte for byte
    - a rect shape puts its colour on the border and leaves the inside alone
    - pixelate gives uniform blocks
    - markup is clipped to the selection
  - **app** (fake screen and clipboard):
    - release enters editing and the clipboard is untouched
    - Ctrl+C copies the annotated PNG + DIB and shows the thumb
    - Ctrl+S saves and copies
    - Esc while editing copies nothing
    - prefs are written once on close, only when they changed

### Acceptance
Press PrtSc, drag and release: the toolbar appears, and the clipboard still holds what it held before. Use each tool once. Recolour with a swatch and with the picker. Move a shape, delete one, re-edit a text, undo and redo through all of it, and resize and move the selection. Press Ctrl+C and paste into Paint: the image matches the overlay inside the selection, markup included. Ctrl+S saves the same image. Esc while editing cancels and leaves the clipboard untouched. The next snap starts with the same tool, colour and size.

### Rules for the agent
Headless, as in 7a: fmt, clippy, test and release only. Don't run the exe, don't capture the real screen or touch the real clipboard, and don't create windows, hooks or input in tests. The coordinator and the user do the live test.

### Log
- 2026-10-26: defined with the user. The Lightshot flow was chosen: release opens the editor, and Ctrl+C or Enter copies. This replaces the handover's earlier idea ("edit from the thumb, keep auto-copy on release"). The 4 extra tools (arrow, marker, pixelate, step numbers), select/move/delete, text re-edit and an HSV picker are all in. Pin + OCR move to 7d.
- 2026-10-26: built by one Opus 5.5 agent (Orca worker, headless) in 5 commits on `xDaijobu/stage-7b-work`.
  - The coordinator re-verified fmt, clippy `-D warnings` and 360 tests, after cleaning the workspace crates, because the shared target dir can mix crates between worktrees.
  - Merged `master` (with Stage N) in; the only conflict was one `use` line in `app.rs`.
- 2026-10-26: merged at the user's request before the live test ("klo ud kelar dihajar aja"). **Live test pending:** the user runs the report's live test script.
- 2026-10-26: follow-up headless verification on `master` (`f7f9215`) found and fixed pixelate raster failure silently omitting redaction, and double-clicks entering text behind toolbar/picker UI. Five regression tests failed before fixes and passed after; fmt/check, clippy `--all-targets -D warnings`, 393 workspace tests and release build passed using isolated `target/verify-7b` (no shared-artifact reuse). Changes are uncommitted for review. No desktop control was available, so live checks remain pending; possible lost-mouse-capture recovery is still unverified. Details: [`stage-7b-report.md`](stage-7b-report.md#follow-up-verification-on-master-2026-10-26).

---

## Stage 7c: Snap stack (preview UI only)

**Status:** contract prepared, implementation not started. Stage 7b live verification remains pending. Resolve confirmed 7b regressions before starting a 7c worktree.

**Goal:** keep several finished, annotated snaps within reach below the island without implementing OLE in the same stage. Defaults follow the stack mockup in `docs/plan.html`: 10 seconds, up to 5 cards, hover/selection pause. The mockup describes the eventual product; the scope here is the implementation gate.

### Scope
- [ ] **Keep the 7b finish flow.** Release opens the annotation editor; Ctrl+C/Enter copies, Ctrl+S copies and saves, Esc cancels. Add one card only after finishing, never on selection release or cancel. Keep the existing latest-shot thumb and its 6 s feedback; the stack owns its own lifetime. Neither expiry nor dismissal clears the clipboard or deletes saved files.
- [ ] **Floating previews:** a separate native Win32/DComp surface below the island, not another island activity and not another AppBar. Cards are 124×80 logical px with r 12, aspect-fit annotated thumbnails, a 16 px countdown ring, a selected mark and visible action labels. Newest first; gap 10 px. Four/five cards may fan/overlap at rest; hovering or selecting spreads them into individually reachable cards. Follow the theme and spring tokens; do not copy the mockup's fake browser drag behavior.
- [ ] **Placement/input:** follow the island's current lower edge with a 12 logical px gap, clamp to its monitor's usable bounds, handle DPI/negative desktop coordinates and reflow on display changes. Never cover island controls. Size the stack swap chain once for its maximum layout at the current DPI, not per animation frame. Only actual cards/action bar receive input; transparent gaps and shadows are click-through. Appearance does not steal keyboard focus or add an Alt-Tab/taskbar entry. Provide deliberate keyboard access to card actions without new global shortcuts; do not rely on color alone for selection/status.
- [ ] **Countdown:** `[snap] preview_seconds = 10`; valid integer range 3–60, otherwise default 10. No new Settings row. Store remaining duration using injected monotonic time in `dj-core`; resume the remainder, never reset it. Hover pauses the relevant card; any selected card pauses the whole stack while batch actions are available. Pin pauses that card indefinitely for this app session. Unpin/deselect resumes only when all other pause reasons end. No time accrues while the stack is hidden or a capture overlay is active.
- [ ] **Pin means keep in stack**, not a draggable full-size reference window. It does not persist across restart. Standalone pin-to-screen and OCR stay in 7d.
- [ ] **Selection/actions:** clicking card body toggles its selection without clearing other cards. Per-card buttons: `Copy`, `Save` (then `Open`), `Pin`/`Unpin`, `Dismiss`. Buttons do not also toggle selection. With a selection show count, `Select all`, `Clear selection`, `Save selected`, `Dismiss selected`; single-selection `Copy` is allowed, but no misleading multi-image clipboard copy. Batch save keeps cards and selection, records per-item paths/errors and never silently discards failures. Dismiss only releases previews, including pinned ones explicitly chosen by the user.
- [ ] **Payload/storage:** reuse the final flattened BGRA/PNG from 7b and existing no-overwrite save/reveal paths. Do not recapture, retain full frozen desktops, or introduce a clipboard database. No automatic disk writes merely to display/pin a card. Re-saving an already saved card uses its existing path; if the file disappeared, report it and permit saving again. File/clipboard failures stay visible on the affected card and are logged; they never report success or dismiss the card.
- [ ] **Bounded retention:** at most 5 cards and 256 MiB of retained full-resolution BGRA + PNG payloads for the stack, counted once if shared with the thumb. Evict oldest unpinned, unselected, unhovered, non-busy cards only when needed. If protected cards or an oversized incoming shot prevent admission, keep existing cards, preserve the new shot's normal clipboard/save/6 s thumb flow, and show `Preview stack full`. Never evict a pinned/selected card silently. Identify items by stable IDs, not changing display indices.
- [ ] **Lifecycle/performance:** hide stack with Ctrl+Alt+H and before every screen freeze, wait for composition to hide it before capture, then restore after finish/cancel. Snapshot window lists exclude stack windows. The stack must not appear in captured pixels. Theme, notch/pill and reserve-top switches still work. Empty stack frees images/windows/timers. Render only on input, springs and a bounded countdown tick (at most 10 Hz while a visible countdown runs); paused/static stacks return to blocking `GetMessageW` after springs settle. Save/encode work must not stall countdown/input; stale async completions cannot update a different card.
- [ ] **Tests:** pure clock/pause combinations, remaining-time resume, expiry, selection/batch actions, capacity/byte budget (all protected, oversized shot, pinned retention), stale IDs, DPI/layout/hit regions and config defaults; synthetic-image rendering; fake clipboard and temp-dir saves including collisions, missing files and partial failures. `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo build --release` pass.

### Out of scope
OLE/COM drag sources, CF_HDROP, drag ghosts, clipboard history, uploads, edit-from-card, persistent pins, full-size pin windows, OCR, recording, new crates, and new Settings UI. No feature-registry unlock: Snap is already available; unrelated tiles stay locked.

### Acceptance
1. Finish three different annotated captures quickly. Clipboard/PNG outputs remain correct; newest-first cards appear below the island without stealing focus or showing up in the next capture.
2. Let one count down, hover another, select several, pin one, then unpin/deselect. Rings pause/resume from the correct remainder; unprotected cards expire at 10 active seconds, not while interacting or hidden.
3. Copy an older card, save one, select all and save the rest. Verify original pixels, collision-safe paths, `Open`, accurate partial-failure status, and that expiry/dismissal never deletes those files or alters the clipboard.
4. Fill all 5 slots with protected cards and finish another capture. Existing cards survive; the new result still copies/saves and has its thumb, with explicit stack-full feedback. Also test the byte budget with synthetic images.
5. Verify keyboard access, transparent-gap click-through, all four themes, notch/pill, expanded panels, OSD/timer coexistence, hide/show, 100%/150% DPI, another monitor and display changes. Empty or fully paused stack has no recurring rendering after animation settles.

### Rules for the agent
Use [`stage-7c-brief.md`](stage-7c-brief.md). Implementation runs in its own worktree only when dispatched. Headless verification only: no real screenshots, clipboard writes, GUI input, windows or hooks in tests. Record every live check as unverified until the coordinator/user performs it.

### Log
- 2026-10-26: continuation chose planning + 7b verification, not 7c-lite implementation. Defaults captured from the design; OLE split into 7c-2 to keep implementation and target-app testing separate. No 7c code shipped in this session.

## Stage 7c-2: Snap stack OLE drag-out (deferred)

**Gate:** only after 7c passes its own acceptance; write a separate brief before dispatch. Not part of the current implementation contract.

- Native `DoDragDrop` with `IDataObject` + `IDropSource`; use Windows' `IDragSourceHelper` for the thumbnail/count ghost rather than implementing that helper interface ourselves.
- Multiple real PNG files via `CF_HDROP`; single-image drags also offer registered `PNG` and `CF_DIB`. Those image formats cannot represent a bundle: never advertise several images as one or silently pick the first for multi-select.
- Materialize files through the existing collision-safe save path only when explicitly dragging/saving. Keep files after cancellation, drop or preview expiry; a target may read them later. Never delete user-saved PNGs as drag cleanup.
- Copy-only effects; cancellation/rejected drops retain cards and selection. Handle COM apartment, reference counting, `FORMATETC`/`STGMEDIUM` ownership, reentrancy and all error paths in platform code. No UI-thread state borrows held across `DoDragDrop`'s nested message loop.
- Require headless format/ownership tests plus actual single/multi-file and cancel/reject checks in Explorer, Discord and a browser file-upload target. Never upload/post/send to a service just to test a drop. Record target versions/results; unsupported formats must be reported honestly.

---

## Stage S: Settings (minimal)

**Goal:** change the basics without editing `config.toml`: a gear icon on Home opens a small Settings panel in the island.

### Scope
- [x] **Gear icon** in the Home grid header (right side, next to the clock), with hover and press states like the other controls. A click opens the **Settings panel**. It is not a feature tile, so the registry count stays 5 of 12.
- [x] **Settings panel** inside the island (about 400×184, r 32, same style as the Vitals/Timer panels). A back control (or an empty click) returns to Home. Rows:
  - **Start with Windows**: an on/off toggle. It uses the existing autostart code (the same one as the tray's *Start with Windows*), the tray's check mark stays in sync, and it is saved to config.
  - **Snap hotkey**: a chip showing the current key. Each click cycles `PrtSc → Ctrl+Space → Ctrl+Alt+S → Ctrl+Shift+S → PrtSc`.
  - **Snap alt hotkey**: a chip; each click cycles `Ctrl+Space → Ctrl+Alt+S → Ctrl+Shift+S → off → Ctrl+Space`. It can't be set to the same key as the main hotkey; that value is skipped.
- [x] **Applied live:** a hotkey change unregisters the old key and registers the new one at once (PrtSc goes through the keyboard hook, the others through `RegisterHotKey`), and saves `[snap] hotkey` / `alt_hotkey`. If registration fails (another app holds the key), the chip shows the key in the warn colour with a short status line `taken by another app`, and the reason goes to the log.
- [x] **Persisted:** `config.toml` is rewritten with the new values, and the other sections are kept as they were.
- [x] Idle CPU ≈ 0 (no timers for this panel).
- [x] Tests: the hotkey cycle order (including skipping the main key for alt, and "off"), panel hit testing (gear, toggle, chips, back), config round trip with the new values, and the island transitions Home ↔ Settings. `fmt`, `clippy -D warnings`, `test` and `build --release` pass.

### Acceptance
Home → gear → Settings. Toggling *Start with Windows* updates the tray check and the HKCU Run entry. Clicking the hotkey chip changes the key at once: the new key opens a snap and the old one doesn't. After a restart the choices are still there.

### Rules for the agent
Headless: fmt, clippy, test and release only. Don't run the exe, and don't touch the real Run key or hotkeys in tests (use fakes or traits).

### Log
- 2026-10-26: defined with the user (minimal: auto-start + Snap hotkeys; press-to-record later).
- 2026-10-26: built by one Sonnet 5.5 agent (303 tests). Live test passed: gear → panel, the autostart toggle (Run key + config), hotkey chips applied live (the user pressed the keys), then restored to PrtSc + Ctrl+Space.

---

## Stage N: Notch style

**Goal:** the island looks like a hardware notch (iPhone 13 / MacBook) attached to the top edge of the screen, not a pill floating under it. It is a choice in Settings, and notch is the default.

### Scope
- [x] **Config** `[island] style = "notch" | "pill"`, default `notch`. An unknown value falls back to notch.
- [x] **Shape (notch).**
  - The body reaches the top edge (y = 0) with a flat top.
  - Where it meets the edge there are concave shoulders of 8 px (× scale) on each side.
  - The sides are straight and the bottom corners use the view's animated radius.
  - It grows downward and sideways with the spring morph and stays attached to the edge.
  - The colour is the theme's pill colour (Paper Zine gives a light notch).
  - The hairline outlines the sides, shoulders and bottom, never the top edge.
- [x] **Nothing else moves.** The pill rect, every view's content and dj-core's hit testing are unchanged. Only the drawn body grows into the 6 px gap above the pill and out by the shoulders (`layout::notch_bounds`). Pill style renders exactly as before.
- [x] **Click-through.** In notch style the window region is the pill's column from the top edge down, plus a strip as high as a shoulder, so the area beside the pill below the shoulders stays click-through. A click, hover or drag on the strip above the pill or on a shoulder acts on the nearest point of the pill.
- [x] **Settings.** A 4th row, "ISLAND STYLE", with a chip (`notch` / `pill`). A click toggles it live, with no restart, and saves the config. The panel grows from 400×184 to 400×220.
- [x] **No reserved screen space.** It stays an overlay; Windows can only reserve a full-width strip (AppBar), so that was rejected.
- [x] Idle CPU ≈ 0 (no new timers).
- [x] **Tests:**
  - style parsing and defaults
  - `notch_bounds`
  - the Settings rects and the hit test for the new chip
  - the notch path: filled at the top edge and the shoulders, empty inside the shoulder curve, degenerate morph frames
  - the neck/shoulder clamp in the app

  `fmt`, `clippy -D warnings`, `test` and `build --release` pass.

### Acceptance
With no `[island]` in the config, the island starts as a notch flush with the top of the screen, with smooth concave shoulders. Expanding it (Home, Music, Vitals, Timer, Settings, the snap thumb, the OSD) grows the notch downward while it stays attached. Clicking the strip above the content works like clicking the island. Settings → ISLAND STYLE toggles between notch and pill at once, and the choice survives a restart. Every theme looks right, including Paper Zine.

### Log
- 2026-10-26: requested by the user while Stage 7b runs, and built in parallel with it ("langsung aj diterapin", "paksa spawn subagent 4 biji").
  - Decided: a Settings choice rather than a replacement, default notch, theme pill colour (forced black was rejected), no AppBar.
  - Approach: the pill rect stays put and only the body shape grows to the edge, so the layout and hit testing are untouched.
  - The coordinator wrote the shared skeleton (`d74d2bc`). Four Opus subagents filled in:
    - dj-core tests
    - dj-render shape
    - the dj-render Settings row and docs
    - the app region and neck clamp
- 2026-10-26: integrated by the coordinator.
  - The dj-core agent widened the window by 16 px (520 → 536) so Home's shoulders fit. Accepted: the swap chain is still sized once.
  - The coordinator narrowed the click-through region from the whole `notch_bounds` box to the column plus the shoulder strip (`dj_win::window::set_hit_rects`, `CombineRgn`).
  - fmt, clippy `-D warnings`, 328 tests (was 303) and `build --release` pass. Report: [`stage-n-report.md`](stage-n-report.md).
- 2026-10-26: live test passed.
  - Coordinator: idle notch flush with the edge, with concave shoulders and grain over the whole body. Home → gear → Settings stays attached. The ISLAND STYLE chip toggles notch ↔ pill live and saves `[island] style`.
  - User, with the real mouse ("notch oke"): clicks on the top edge, click-through beside the panel, themes.

---

## Stage N2: Reserve top space (AppBar)

**Goal:** the notch no longer covers the middle of Chrome's tab strip (or any maximized window's top). macOS-style "split around the notch" is impossible on Windows: there is no safe-area API, and Chrome draws its own tab strip. So the island reserves a strip along the top of the screen, and maximized and snapped windows start below it.

### Scope
- [x] **Config** `[island] reserve_top = true` (default on). It reserves only in notch style, and only while the island is visible (`IslandConfig::reserves_top`).
- [x] **AppBar** (`dj_win::appbar`, `SHAppBarMessage`).
  - The strip is the primary monitor's full width (Windows only reserves whole edges) and `layout::RESERVE_H` = 42 logical px × scale tall. That holds the notch in every rest state (idle, Music and Timer compact); expanded panels still hang over windows.
  - Nothing is drawn in the strip, so the wallpaper shows beside the notch (the user's choice).
- [x] **Lifecycle.**
  - The strip is reserved at start.
  - It is given back when the island is hidden, in pill style, or when the switch is off.
  - It is set again on `ABN_POSCHANGED`, a DPI or display change, and an Explorer restart (`TaskbarCreated` re-registers).
  - It is always given back on quit and on `WM_DESTROY` (e.g. `WM_CLOSE` from outside), so the work area never stays short.
- [x] **Settings.** A 5th row, "RESERVE TOP SPACE", with a switch like Start with Windows. It is dimmed in pill style (the value is kept but has no effect). The panel is now 400×256.
- [x] **Tests:**
  - config default, `reserves_top`, and the round trip
  - the strip holds every rest state
  - the strip's rect at different scales and on a monitor that doesn't start at the origin
  - the new row's rects and hit test
  - island click → `ToggleReserve`

  fmt, clippy `-D warnings`, 388 tests and `build --release` pass.

### Known limits
- A fullscreen app (F11 video, a game) still covers the whole screen.
- A window that isn't maximized can still be dragged under the strip.
- Desktop icons move down by the strip's height.
- If the process is killed (not closed), the strip stays until Explorer notices the window is gone.

### Log
- 2026-10-26: requested by the user ("tab chrome / aplikasi lain otomatis kebela"). Options were AppBar, dodge on hover, and a smaller idle notch; the user chose AppBar, an empty strip, and a Settings switch that defaults to on.
- 2026-10-26: built by the coordinator on `xDaijobu/stage-n2-appbar` from `master` (`608b179`).
  - Live check on the user's machine (3440×1440, scale 1.0): the work area top goes 0 → 42 at start, and maximized Chrome's tabs start below the notch.
  - The switch toggles 42 ↔ 0 live and saves `reserve_top`. Pill style gives the strip back (0) and notch takes it again (42). `WM_CLOSE` gives it back (0).

---

## Changelog
- 2026-10-25: PR #4 (Vitals) merged; stage worktrees cleaned up. **Sweep is merged into Vitals** and moved up to Stage 5 (the old Stage 12 is folded in). Stages from Timer onwards shift by one.
- 2026-10-25: PRs #2 (Music) and #3 (OSD) merged at the user's request before live testing ("fixes can come later"). Their worktrees and branches were removed. Focus is on Stage 4.
- 2026-10-25: Stage 2 held (code done). Stage 3 (OSD volume) started.
- 2026-10-25: Stage 2 redefined: Music = YouTube Music (headless WebView2).
- 2026-10-25: Stage 1 (app shell) done.
- 2026-10-25: progress.md created. The plan, the README draft, and the name **Daijobu Island** were finalized.
