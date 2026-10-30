# Stage 9f — native Windows Music

Implementation worker report, 2026-10-30. Coordinator owns Git, final live UI review and delivery/restart. No commit, push, merge, package registration, trust installation, Windows setting change, provider configuration edit or app restart was performed by this worker. Existing user music profile files were not read, rewritten or deleted.

## Delivered scope

- [x] Replaced `dj-music` with `dj-media`. Removed the old WebView2 engine, JavaScript bridge, artwork downloader, embedded sign-in/browse window, engine tray commands and WebView2 dependencies/lockfile entries. Tray Music now opens the native panel.
- [x] Discovery starts independently of notification settings. Windows GSMTC current-session/session-list events and selected-session metadata/playback/timeline events drive one bounded native MTA worker. Idle waits block; there is no system-session polling timer.
- [x] Manager and session event tokens are revoked on replacement/drop, including partial subscription failures. Async operations use completion callbacks and bounded waits/cancellation, never UI-thread waits. Failed queries do not retry on every timeline event; explicit Retry or a new session/metadata event permits recovery.
- [x] Title, artist, album, source, artwork, playing state, timeline and per-control capabilities are carried by native snapshots. Unknown browser services retain their browser source name; the app does not claim a Chrome session is Spotify or YouTube Music.
- [x] Controls retain the exact native session and snapshot generation. Generated Windows class equality uses `windows-core`'s canonical `IUnknown` comparison. The app also retains the generation actually rendered, so a newly received session cannot receive an obsolete visible control click. Capabilities/current identity/generation are rechecked before issuing an operation, and async false/failure results produce visible feedback. No global media keys or autonomous playback.
- [x] Timeline math handles finite bounds, start/end offsets, pause/resume with repeated OS anchors, rate, external seek and unknown duration. Unsupported/unknown-duration seeking is disabled. Closed/Stopped sessions clear active now-playing. The 250 ms progress/lyric timer runs only while the expanded Music panel is visible and playing, and stops at known track end; compact playback status is static.
- [x] Artwork is capped at 4 MiB, checked against 4096 × 4096 source dimensions before rasterization, and reduced to at most 256 × 256. Shared bytes and content comparison avoid clearing/decoding unchanged art on timeline events. New renderer creation restores retained art. Stale artwork and lyric publications cannot replace newer generations.
- [x] No-session and error states, supported-control dimming, Retry and fixed external `https://music.youtube.com/` / `https://open.spotify.com/` links. Removed the audio-band/VU path; GSMTC does not provide an audio spectrum.
- [x] Optional LRCLIB synced lyrics, default off. Settings explains that artist/title/duration are sent. Disabled and obsolete queries cannot reach the transport. A separate single bounded lyric worker keeps HTTP away from native controls; pending work coalesces, responses are generation-gated, lookups are sequential, and off/no-match/offline/loading states are distinct. Current synchronized line appears in expanded Music.
- [x] LRCLIB uses the documented HTTPS `/api/get` endpoint, identified User-Agent, encoded parameters, no redirect following, a 5 s global timeout, 256 KiB response cap, bounded LRC parsing, 16-entry/15-minute memory cache, and request cooldown/Retry-After handling. No song lyrics were copied into tests or docs. No lyric lookup was enabled against real user media during verification.
- [x] Legacy `[music] signed_in` is tolerated/ignored; unrelated configuration continues to round-trip and does not gain new opt-ins. Old `~/.daijobu/ytmusic/` files are preserved.
- [x] Updated README, AGENTS, interactive design source and notification packaging documentation. Browser opening of `docs/plan.html` was attempted through Orca but failed first with `browser_no_tab`, then tab-creation timeout. Its inline JavaScript passes `node --check`; visual browser review remains unverified.

## Windows identity and notifications

The sparse manifest declares `globalMediaControl` as `uap7:Capability`, with namespace `http://schemas.microsoft.com/appx/manifest/uap/windows10/7`, alongside the existing notification capability. Package version advances from `0.3.1.0` to `0.3.1.1`; stable extraction advances coherently to `~/.daijobu/notification-identity/0.3.1.1/<external-folder-hash>/`. Workspace/application release version remains `0.3.1`. Family, publisher and application ID are unchanged. Existing extracted manifests are not overwritten in place. Signed distribution builds must supply a newly signed payload with this manifest/version.

Native media does not require notification opt-in, identity setup or listener consent. The coordinator's earlier read-only C# probe and this worker's Rust adapter probe both succeeded with the existing full-trust Stage 9e notification identity, which lacks the new capability. The Rust adapter also succeeded without package identity. Accordingly, there is no mandatory or automatic migration.

Only a real media access-denied error offers explicit Windows integration Update and then Restart recovery. This reuses package setup and the existing commit/parent-exit handshake without enabling notifications or requesting listener consent. The media-specific restart route reopens Music; the notification restart route still reopens Notifications. Recovery explains that restarting closes shells and preserves saved settings/pinned tabs. No recovery/registration/restart action was exercised live by this worker.

## Verification

Final checks against the completed implementation:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace -- --test-threads=1`: **1,736 passed**, zero failed/ignored; 20 result suites including doctests. The final full rerun passed the previously flaky pipe test as well.
- `cargo tree --workspace --locked --prefix none`: no `webview2` or `dj-music` dependency.
- `node --check` on extracted design-page inline JavaScript: passed.
- Optimized build and delivery hash are recorded in the delivery section below.

Durable verification artifacts live under `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/stage-9f-native-music/`: `fmt.log`, `clippy.log`, `workspace-tests.log`, `optimized-build.log`, `dependency-tree.txt`, `validation-summary.txt`, and both native probe logs. The final workspace counts by executable are app 327 plus 2 integration tests, capture 8, clip 82, core 789, media 7, render 177, sweep 35, term 73, vitals 26 and win 210; doctest suites add zero tests.

Regression coverage includes exact/stale target handling, unsupported controls/no-session routing, disabled hidden Retry, finite timeline/pause/seek/end/unknown duration, repeated-anchor pause/resume, artwork byte/dimension caps, stale artwork/lyrics publication, metadata-dirty preservation when an in-flight fetch is invalidated, worker-start error visibility, old-config migration, disabled/stale lyric transport gates, cached offline behavior, bounded synthetic LRC parsing and time selection.

One intermediate full workspace attempt encountered the existing `dj-win::agent_notifications::tests::native_receiver_rejects_bad_frames_and_bounds_its_queue` failure at `agent_notifications.rs:511`: one native send returned an error while the queue test expected success. It passed immediately in isolation (1 passed, 209 filtered out). No unrelated notification-pipe implementation was modified. The failed run is retained in `workspace-tests-transient-pipe-failure.log` under the artifact directory; final run status is recorded separately.

### Read-only native evidence

`crates/dj-media/examples/media_probe.rs` is a bounded read-only diagnostic. It starts `Media::start(false, ...)` and prints package-identity presence, source, metadata presence, artwork byte count, finite-progress/capability facts and generation; it does not print real titles, lyrics or artwork contents. It never invokes a control, network opt-in or setup action.

- Manifest-free probe: `package_identity=false`, query code `15700`; Chrome current session detected, playing, title/artist present, duration known, finite position, 22,057-byte artwork, play disabled/pause enabled/previous-next-seek enabled, lyrics Off. Four updates in the recorded 3 s run establish startup plus event-driven updates.
- Manifest-bearing probe in the already registered Stage 9e external directory: `package_identity=true`, size-query code `122`; equivalent native metadata/artwork/capability discovery succeeded, lyrics Off. Existing package/registration and running app were untouched.
- Logs: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/stage-9f-native-music/native-probe-unpackaged.txt` and `native-probe-packaged.txt`.
- A separate diagnostic helper was copied to `target/stage-t4/stage-9e-notification-setup/stage9f-media-probe.exe` and run; it is not the app executable and sends no controls.

## How to run / handoff

Use stable Rust/MSVC and the existing Windows toolchain. This dispatch had exclusive ownership of `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4` for the entire check/test/build sequence, including serial test execution.

```powershell
$env:CARGO_TARGET_DIR = 'C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
cargo build --profile terminal-folders
cargo run -p dj-media --example media_probe -- 10
```

The default example build is manifest-free. For packaged probe evidence, it was separately linked with the existing optional sparse identity manifest and executed as a new helper in the registered external directory; this does not install or update a package.

Coordinator should preserve the current registered stable installation location when delivering the new app so working Windows notifications continue to use that identity. Optimized delivery artifact is intended for `target/stage-t4/stage-9f-native-music/daijobu.exe`; this worker must not launch it.

## Remaining live limits

- Actual Spotify and YouTube Music service identity, multiple browser tabs sharing one AUMID, live session removal/switch while commands/artwork are pending, and real play/pause/previous/next/seek commands were not exercised. Chrome is the observed source; service identity is not inferred.
- No live LRCLIB lookup, provider outage/rate-limit simulation or lyrics visual synchronization with a real track. Network gating/offline cache/staleness/parsing are synthetic tests.
- No live setup/package upgrade, media-only restart, new notification consent, notification delivery or source activation after replacing the app. Existing running Stage 9e app was left in place.
- No live music-panel screenshot, DPI/multi-monitor/fullscreen or GPU-loss recovery review. Artwork restoration and source-size guards are implemented; final UI review remains with the coordinator.
- Browser design preview was unavailable in Orca; only source/JavaScript validation was performed.

## Sources and licensing

Original Rust implementation, based on official APIs; no Hartono or GPL source/assets were copied. Hartono's MIT media/lyrics behavior reference is identified in the frozen brief, and no third-party assets were added.

- [Windows GSMTC session manager](https://learn.microsoft.com/en-us/uwp/api/windows.media.control.globalsystemmediatransportcontrolssessionmanager?view=winrt-26100)
- [Windows GSMTC session](https://learn.microsoft.com/en-us/uwp/api/windows.media.control.globalsystemmediatransportcontrolssession)
- [Capability declarations](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations)
- [uap7 capability schema](https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-uap7-capability)
- [Current official LRCLIB documentation](https://lrclib.net/docs.txt), consulted 2026-10-30

## Optimized delivery

- Build: `cargo build --profile terminal-folders` passed, optimized, 2m 01s.
- Preserved executable: `C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4/stage-9f-native-music/daijobu.exe`.
- Size: 28,189,184 bytes.
- SHA-256: `6459B72F615DE927A54F30BCD1E29C93355938EA113C4448F9675F044866E230`.
- Checksum file: `target/stage-t4/stage-9f-native-music/daijobu.exe.sha256`.
- The executable was copied only and was never launched by this worker; current Stage 9e app and registration remain untouched.

All authorized implementation and automated verification are complete. Coordinator retains live review, final deployment/restart, Git/PR/merge, and the manual limits listed above.

## Coordinator verification and live delivery

Coordinator reviewed session/generation races, metadata refresh retention, artwork reuse, control capability handling, UTF-8 preservation, identity integration and final diffs. Independently reran fmt and workspace/all-target clippy with warnings denied; both passed. Reviewed the final 1,736-test passing workspace log and preserved intermediate pipe-test failure evidence.

Gracefully closed the old app (PID 25836), retained its executable as `target/stage-t4/stage-9f-native-music/previous-stage9e.exe`, and copied the hash-verified optimized build into the existing registered installation at `target/stage-t4/stage-9e-notification-setup/daijobu.exe`. Launched with `--music` as PID 18112. GetPackageFullName succeeded with the existing `DaijobuIsland.Notifications_0.3.1.0_neutral__x1xqmdx6tfxf6` identity; no package update or new consent was necessary.

Live screenshot inspection confirmed the Music panel shows Chrome source, title/artist, cover, finite changing progress, supported controls and Lyrics off. Actual pause and resume through the panel succeeded; playback was left playing. No next/previous/seek commands or real lyrics requests were sent. Spotify and the specific browser service were not independently identified. Notifications retain their existing package identity; notification history/arrival was not rechecked after this replacement. No WebView engine is present in the new source/dependency graph.

Orca worker release returned `retained: user_takeover`; coordinator leaves that terminal/worktree intact rather than closing a user-owned session.
