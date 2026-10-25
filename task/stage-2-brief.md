# Stage 2 brief: Music = YouTube Music (for the worktree agent)

Your worktree starts from `master` (Stage 1 app shell). You are building **Music**. Click it, sign in to YouTube Music once, and from then on YouTube Music plays in a **hidden WebView2** while the island acts as the player.

## Read first
1. `AGENTS.md` (your worktree): rules. Note that this stage adds **one** exception to rule #1, WebView2 as the hidden YouTube Music engine only.
2. **Contract:** `C:/Users/chris/orca/projects/daijobu-island/task/progress.md`, section **"Stage 2: Music (YouTube Music)"**. Read it from the main checkout and don't edit it. Every checkbox there is your job, and nothing more.
3. `docs/plan.html`: Music compact/expanded sizes and visualizer styles.
4. `task/stage-1-report.md`: how the stage 1 code is organised (`dj-core` island state machine, feature registry, `app/src/app.rs` main loop).

## Hard rules
- **No audio.** Never play anything and never open Windows Media Player or any player. Don't sign in to Google. The user does the sign-in and playback tests.
- No user-agent spoofing and no login automation.
- Build only the contract. No SMTC, no WASAPI, no other features. Write ideas in your report instead.
- Dependencies must be Apache-2.0 compatible (`webview2-com` is MIT). Pin versions. A second `windows` crate version pulled in by `webview2-com` is acceptable.
- `unsafe` needs `// SAFETY:`. Pure logic goes in `dj-core` or pure modules, with unit tests.

## Step by step
1. **Rename Media → Music** in the `dj-core` feature registry and mark it `Available`. Keep the other 12 locked. Fix the tests.
2. **New crate `crates/dj-music`:**
   - `Engine`: creates a hidden top-level window (its own class, normal styles, ~1100×760, centred, title "Daijobu Island · YouTube Music", `ShowWindow(SW_HIDE)` initially).
   - `CreateCoreWebView2EnvironmentWithOptions(None, user_data = ~/.daijobu/ytmusic, options with AdditionalBrowserArguments "--autoplay-policy=no-user-gesture-required")`, then `CreateCoreWebView2Controller(hwnd)`.
   - These are async COM callbacks that complete on the **UI thread's message loop**, which is fine because the app already pumps `GetMessageW`.
   - Resize the controller bounds on `WM_SIZE`. Closing the window (X) **hides** it instead of destroying it.
   - `add_WebMessageReceived` → parse JSON → send to the app as an event (`PostMessageW(WM_APP + n)` or a channel plus a wake message).
   - `AddScriptToExecuteOnDocumentCreated(BRIDGE_JS)`, then `Navigate("https://music.youtube.com")`.
3. **`bridge.js`** (embed it with `include_str!`). It runs only on `music.youtube.com`:
   - Every 1 s, and on `play`/`pause`/`loadedmetadata`/`seeked` of the `<video>`/`<audio>` element, post `{t:"state", title, artist, album, art, playing, pos, dur}` where something changed. `art` is the largest `navigator.mediaSession.metadata.artwork` src.
   - Post `{t:"auth", signedIn}`. Detect signed-in by the account avatar button and signed-out by a "Sign in" link. Keep all selectors in one object at the top of the file.
   - **VU:** on the first `play`, create an `AudioContext`, run `createMediaElementSource(media)` → `AnalyserNode(fftSize 1024)` → `destination`, which keeps the sound. While playing, post `{t:"bands", v:[22 floats 0..1]}` at ≤ 30 Hz using log-spaced bins. If the analyser data stays all-zero, post `{t:"bands", v:null}` once and stop.
   - Control functions: `window.__dj = {toggle(), next(), prev(), seek(sec)}`, implemented with the page's player buttons or the media element.
4. **Rust message model** (pure, tested): `enum MusicMsg { State{..}, Auth{signed_in}, Bands(Option<Vec<f32>>) }`, parsed with `serde_json`. Add tests with sample JSON, including malformed input.
5. **Controls:** `engine.exec("window.__dj.toggle()")` and so on via `ExecuteScript`.
6. **Island states** (`dj-core`): add `MusicCompact` and `MusicExpanded` next to Idle/Home. Transitions:
   - Idle → MusicCompact when there is a track (playing or paused).
   - Click compact → expanded. Click an empty area in expanded → compact. Expanded has a "Home" button → Home.
   - Home Music tile → expanded, or the sign-in view when signed out.
   - Track gone → Idle.
   - Unit test the transitions.
7. **Views** (`dj-render` painter, same style as stage 1):
   - Signed-out view: the "Sign in to YouTube Music" button.
   - Compact 224×36: art 24 r6, title, 5-col VU (theme visualizer style).
   - Expanded 400×184: art 88, title 15 / artist 12, "youtube music" + lamp, scrubber with times (clickable → seek), prev/play/next (34 px targets), VU strip, "Open YouTube Music" + "Home" buttons.
   - Redraw only on state changes, and while playing for the VU at ≤ 30 fps.
8. **Artwork:** fetch the art URL on a worker thread (`ureq`, MIT/Apache, blocking is fine there), cap at 2 MB, decode with Skia, downscale to ≤ 256. Accent from cover is clamped to S 45–65% and L 62–72%, then `mix(theme accent, cover, accent_from_cover)`. Unit-test the clamp.
9. **Sign-in flow:**
   - Clicking Sign in starts the engine if needed and shows the window.
   - On `Auth{signed_in:true}`, hide the window and save `[music] signed_in = true` in config.
   - When `signed_in = true`, auto-start the engine hidden at app start.
   - If Google shows an "unsafe browser" block page, detect it by URL/title, show a message on the island, and offer to open https://music.youtube.com in the default browser (`ShellExecuteW`).
10. **Tray:** add "YouTube Music ▸ Show window / Sign out / Stop engine". Sign out: stop the engine, delete `~/.daijobu/ytmusic/`, set `signed_in = false`.
11. **AGENTS.md:** under rule #1, add "Exception: WebView2 is allowed only as the hidden YouTube Music engine and its sign-in/browse window (`dj-music`)." Add `crates/dj-music/` to the layout.

## Verify (no audio, no sign-in)
- Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
- Run the release exe:
  - Home shows Music unlocked.
  - Clicking it shows the sign-in view, and Sign in opens the engine window at music.youtube.com. Check the window title/URL, then hide it. **Don't log in.**
  - The bridge posts `auth signedIn:false`. Log it to `~/.daijobu/daijobu.log`.
  - Idle CPU ≈ 0.
- Screenshot only the island (sign-in view) to `task/stage-2-signin.png`. Don't capture other windows.

## Finish
Commit in small steps on your branch and don't push. Write `task/stage-2-report.md`: a per-item checklist against the contract (honest), how the user should test sign-in and playback, known issues, and what was not verified. Then stop.

Toolchain: `%USERPROFILE%\.cargo\bin`, `C:\Program Files\LLVM\bin`, `%LOCALAPPDATA%\Microsoft\WinGet\Links`. Prepend them to `PATH` if needed.
