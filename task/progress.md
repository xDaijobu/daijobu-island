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
| 6 | Timer / Pomodoro | 🔒 | — | |
| 7 | Snap + Snap stack | 🔒 | — | |
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

## Changelog
- 2026-10-25: PR #4 (Vitals) merged; stage worktrees cleaned up. **Sweep is merged into Vitals** and moved up to Stage 5 (the old Stage 12 is folded in). Stages from Timer onwards shift by one.
- 2026-10-25: PRs #2 (Music) and #3 (OSD) merged at the user's request before live testing ("fixes can come later"). Their worktrees and branches were removed. Focus is on Stage 4.
- 2026-10-25: Stage 2 held (code done). Stage 3 (OSD volume) started.
- 2026-10-25: Stage 2 redefined: Music = YouTube Music (headless WebView2).
- 2026-10-25: Stage 1 (app shell) done.
- 2026-10-25: progress.md created. The plan, the README draft, and the name **Daijobu Island** were finalized.
