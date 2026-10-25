# Stage 2 report: Music (YouTube Music)

Branch `stage-2-music`, 3 commits on top of `master` (not pushed). Built with `cargo build --release`, `cargo clippy --workspace --all-targets -- -D warnings` (clean) and `cargo test --workspace` (dj-core 49, dj-win 2, all pass). `cargo fmt --all` applied.

## Verdict

The code for every item in the contract is written and compiles. Two things were verified live on this machine: the hidden engine starts, navigates to music.youtube.com and reports the signed-out state, and idle CPU is about 0. Everything that needs a click, a sign-in, or audio is **not verified live**. I stopped driving the mouse when you asked me not to use the GUI, so the sign-in click, the island's visuals and playback were never seen on screen.

## Checklist (against the contract)

| Item | State | Evidence / notes |
|---|---|---|
| Rename Media → Music (registry id `music`, label, icon kept), unlocked; the other 12 stay locked | Done | `features.rs`. Unit test: only `music` is `Available`. |
| `crates/dj-music`: hidden WebView2 engine (`webview2-com` 0.39.1, MIT) | Done, partly verified | Own top-level window, normal styles, 1100×760 centred, title `Daijobu Island · YouTube Music`, hidden at creation. Profile `~/.daijobu/ytmusic`. `--autoplay-policy=no-user-gesture-required`. **Verified:** the window exists with the right title and stays hidden; the page navigated to `https://music.youtube.com/` (log: `music: page https://music.youtube.com/ \| YouTube Music`). |
| Engine starts after sign-in or on the Sign in click, auto-starts at launch when `signed_in = true` | Done, partly verified | Launch auto-start verified with `signed_in = true`. The Sign in click path is written but **not verified** (no click). |
| Sign-in view (signed out): "Sign in to YouTube Music" button, shows the window | Written, not seen | The island view and its hit-testing are unit-tested. No screenshot (see below). |
| Window hides itself on `Auth{signed_in:true}`, saves `[music] signed_in = true` | Written, not verified | Needs a real sign-in, which you do. |
| Google "unsafe browser" block: message on the island + "Open in browser" | Written, not verified | Detection: URL host `accounts.google.com` and a title containing "couldn". The real block-page title is **not verified**. |
| Never automate login, no UA spoofing | Done | No login or UA code anywhere. |
| Bridge script (`bridge.js`, via `AddScriptToExecuteOnDocumentCreated`) | Done, partly verified | Posts `state` (title, artist, album, art, playing, pos, dur) on change, with a 1 s tick. Posts `auth` on change. **Verified live:** the signed-out page matched the sign-in-link selector, so `auth signedIn:false` was posted and logged. **Not verified:** the `state`, `bands`, and the `#avatar-btn` path. "Like state" was not implemented (not cheap). |
| Selectors in one object at the top of the file | Done | `SEL` in `bridge.js`. `#play-pause-button`, `.next-button`, `.previous-button` are from memory and **not checked** against the live DOM. |
| Controls via `ExecuteScript`: play/pause, next, previous, seek | Written, not verified | `window.__dj.toggle/next/prev/seek`. Uses the page buttons, falls back to the media element. Not run (no playback allowed). |
| VU: Web Audio analyser on the media element, feeds the destination, 22 log-spaced bands ≤ 30 Hz, null when silent | Written, not verified | `AudioContext` → `createMediaElementSource` → analyser (fftSize 1024) → destination. Interval 34 ms. Gives up after about 8 s of all-zero output and posts `bands: null`. **Not verified:** no audio was allowed. |
| Island states: Idle → MusicCompact (track), click → MusicExpanded, empty click → compact (or Idle with no track), Home → Music tile → expanded/sign-in, Home button | Done, unit-tested | `island.rs`: nine new tests cover transitions, hit-testing, scrubber seek, sign-in and browser buttons. Visual result not seen. |
| Compact 224×36 (art 24 r6, title, 5-col VU) | Written, not seen | `dj-render/src/music.rs`. |
| Expanded 400×184 (art 88, title 15 / artist 12, "YOUTUBE MUSIC" + lamp, scrubber with times, prev/play/next 34 px targets, VU strip, Open YouTube Music + Home) | Written, not seen | Positions are my own layout from the contract, not compared pixel-wise to `plan.html`. |
| VU styles from the theme (`bar`, `block`, `segment`) | Done | `draw_vu`. All three themes' values are covered. |
| Redraw only on change; VU at ≤ 30 fps while playing | Done | VU timer (33 ms) is armed only while visible, in a Music view, with `bands` and a playing track. Band messages do not force a redraw. |
| Artwork: download on a worker (≤ 256 px decode), accent clamp S 45–65 %, L 62–72 %, mixed by `accent_from_cover` | Done, clamp unit-tested | `dj-music/src/art.rs` (ureq, 2 MB cap, https only). Decode and downscale in `dj-render` (see deviations). Clamp test in `dj-core/src/music.rs`. Live art not verified (no track). |
| Tray: `YouTube Music ▸ Show window / Sign out / Stop engine` | Written, not clicked | `tray.rs`. Sign out stops the engine, deletes `~/.daijobu/ytmusic`, sets `signed_in = false`. |
| Idle CPU ≈ 0 (nothing playing) | Done, verified | Engine running hidden, signed-out page: 15.6 ms CPU over 5 s (~0.3 %). RAM about 120 MB for the app. |
| AGENTS.md exception for rule #1 and the `dj-music` layout line | Done | |
| Tests for pure logic (message parsing, state transitions, accent clamp) | Done | `dj-core`: `music.rs` (parse, malformed input, session, VU groups, time format, HSL, accent clamp) and `island.rs`. Config `[music]` round-trip. |
| `build --release`, `clippy -D warnings`, `test` | Done | See top of this report. |

## Verify (as the brief lists it)

- fmt, clippy, test: **pass**.
- Home shows Music unlocked: covered by a unit test. **Not seen on screen.**
- Clicking Music shows the sign-in view; Sign in opens the window: **not verified by click**. The window opening and its title were verified via the launch path (see above).
- The bridge posts `auth signedIn:false` and it is logged: **verified** (`~/.daijobu/daijobu.log`).
- Idle CPU ≈ 0: **verified** (above).
- `task/stage-2-signin.png`: **not produced.** My earlier attempt captured other windows (a game and a screenshot overlay), so I deleted it. Without clicks, the island is in the Idle state, so a clean island-only capture needs a click first.

## How to test (you)

1. `cargo build --release`, then run `target\release\daijobu.exe`.
2. Click the pill → **Music** → **Sign in to YouTube Music**. The window opens at music.youtube.com. Sign in yourself. The window should hide, and `config.toml` should gain `[music] signed_in = true`.
3. In the tray, or from the island, **Open YouTube Music** to browse and play something, then close the window with X (it hides). The island should show the compact view with art, title and a moving VU. Click it to expand and try prev/play/next, the scrubber and Home.
4. Restart the app. It should still be signed in, with no window.
5. Tray → **YouTube Music ▸ Sign out**. The profile folder should be deleted and `signed_in` set to false.
6. If Google blocks the sign-in, the island should show a message with **Open in browser**.

## Deviations from the contract

- `MusicMsg` has an extra variant, `Blocked(bool)`, set from Rust navigation checks rather than from the page.
- The Music sign-in view is the expanded state with signed-out content, not a separate `IslandState`. The contract asked for Idle/Home/Compact/Expanded, and the sign-in view is part of Expanded.
- Art download runs on a worker, but the decode runs on the UI thread in `dj-render`, not on the worker. This avoids Skia use across threads.
- `IslandView.fade` became per-layer opacities (`Layers`), so Idle, Home and the two Music states can cross-fade.
- Logging moved from the app's `storage` to `dj_win::system::{data_dir, log}`, so both crates share it. `storage::log` and `data_dir` still exist and delegate to it.
- `ureq` uses `native-tls` (schannel on Windows), so no C toolchain is pulled in. `serde_json` was added to `dj-core`.
- A `Wake` event and `window::wake` (`WM_APP+3`) wake the UI loop from the art worker. `Event::Music` is raised from the WebView2 callbacks.

## Known issues and risks

- **Unverified selectors and block-page title.** Only the signed-out `auth` path has been seen live. Everything about the playing page (`state`, `bands`, controls, `#avatar-btn`) is from memory and can break when YouTube Music changes its DOM.
- **Sign-out deletes the profile while WebView2 may still hold files.** The deletion is logged on failure, not retried. If it fails, delete `~/.daijobu/ytmusic` by hand after quitting.
- **The VU gives up for the session** after about 8 s of all-zero analyser output (for example, muted or silent playback). That shows no VU until the page reloads. A silent track could trigger this.
- **The scrubber** moves between the page's 1 s messages using local interpolation, so it can drift a little.
- **Engine process lifetime:** killing the app process also removes the WebView2 helper processes (checked: none left after the kill).
- **`unsafe` blocks** in `dj-music/src/engine.rs` each have a `// SAFETY:` comment, but the WebView2 COM callbacks are only exercised for navigation, not for messages or controls.
- **Light-touch CPU measurements:** only the app process was measured, not the WebView2 renderer processes.

## Not verified

- The sign-in click, the window hiding after sign-in, and the config write on `signed_in`.
- Any visual result (island views, VU, art, accent colour), because no screenshot was taken.
- Playback, controls, seek, VU data and the give-up path (no audio was allowed).
- Tray submenu items, including sign-out and stop engine.
- The Google block-page detection and its message.
- Multi-monitor, DPI changes and the scrubber's accuracy over long tracks.

## Incidents during testing

- **Stray clicks on another app.** While driving the island I used the wrong DPI scale (150 % instead of 100 %). Two of the clicks at logical position (672,144) landed on the window **"Pokebonk TD (DEBUG)"** (process `Godot_v4.5.2-stable_win64`), not on the island. That game window was no longer running at my next check. I cannot tell whether my clicks closed it or whether it closed for another reason, so please check whether the game lost anything.
- **Config touched briefly.** For the launch-path test I set `[music] signed_in = true` in `~/.daijobu/config.toml`, then restored the original file byte-for-byte (checked). The test's `~/.daijobu/ytmusic` profile was deleted afterwards. `~/.daijobu/daijobu.log` was created by these runs and remains.
- **Stopped all GUI driving** as soon as you asked. The last run was started from config, with no clicks or screen captures.
