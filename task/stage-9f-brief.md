# Stage 9f - Native system media replaces WebView music

## User request and scope
Replace the current music implementation with the Hartono/Vale approach: automatically detect Spotify and YouTube Music/browser playback without an embedded WebView. Remove the old engine, bridge, embedded sign-in/browse UI, engine tray commands and WebView2 dependencies. Preserve Daijobu native Rust/Skia appearance and existing island state/priority behavior. Do not delete existing user music profile files.

Target behavior: session discovery at startup and via Windows events; title/artist/album/source/art; play/pause, previous/next and seek only where supported; correct pause/seek/end timeline; meaningful no-session/error UI and fixed safe external links for YouTube Music and Spotify. Follow the Windows current media session; never start playback automatically or send blind global media keys. Commands must refer to the exact displayed session/generation, not just a browser AUMID (multiple tabs can share it). Stale artwork and results must not change a newer track/session. Media discovery must work without enabling Windows notification ingestion. No fake spectrum: remove WebAudio-dependent VU rather than claiming GSMTC provides audio bands.

Include optional LRCLIB synced lyrics, default-off. Coordinator asked the user, then stated this default assumption after allowing time for a reply; any later user correction will be forwarded. Enabling must explain that artist/title/duration go to LRCLIB; never make network requests while disabled. Bound response sizes, timeouts/cache/queues and reject stale responses. Use synthetic lyrics in tests, not copied song lyrics. Graceful unavailable/no-match/offline states, no secret APIs or account scraping. Consult current official LRCLIB docs before implementation.

## Architecture and ownership
One implementation worker owns new dj-media native adapter, pure dj-core music model, app/render/settings/tray wiring, removal of dj-music, manifests/docs/tests. Coordinator owns scope, review, Git/PR/merge, live app restart. No commit/push/merge or restart of the running app by worker. Do not touch unrelated notifications or terminal behavior except required media package capability/version integration. Shared target builds remain exclusive to this worker while dispatched.

Use one bounded event-driven worker and WinRT GSMTC APIs. No webview/Electron/tokio/UI-thread blocking. Subscribe/unsubscribe manager and session handlers cleanly; block idle, don't poll system sessions. Read metadata/artwork asynchronously off UI; cap thumbnail bytes and decode dimensions. Honor playback capabilities and async false/failure results. Retain exact session identity for commands; obsolete clicks must fail safely. Timer only while visible playing UI needs progress/lyrics. Finite/clamped seek/timeline values and unknown duration support. Configure meaningful errors and retry without busy loops.

Replace old signed_in/blocked UI facts with native availability/session capabilities; migrate/tolerate legacy [music] config without forcing new logins or dropping unrelated settings. Update docs/plan.html, AGENTS.md, README and stage report to reflect removal of WebView engine. No source copying required: original implementation based on official APIs; Hartono repository MIT source is reference for behavior only. No GPL code/assets or references from its unrelated cleaner.

Packaged/unpackaged compatibility: official API requires globalMediaControl. Declare it as uap7:Capability in sparse manifest (namespace http://schemas.microsoft.com/appx/manifest/uap/windows10/7), not rescap. Verify native operation with notification identity enabled and without it. If package payload/version changes, update stable extraction version and package version coherently; don't silently require notification opt-in just to play music. Handle existing Stage9e registration upgrade explicitly through setup/restart UI if needed.

## Sources examined
Hartono reference commit c2618b15c82e9290bae2d08c47c9426936cfcb1a, src/sources/media.rs and src/lyrics.rs, MIT. Local reference-only copies: C:/Users/UseR/orca/projects/daijobu-island/target/hartono-reference/ (do not ship these).
- https://github.com/hartono0297/dynamic-island/blob/main/src/sources/media.rs
- https://learn.microsoft.com/en-us/uwp/api/windows.media.control.globalsystemmediatransportcontrolssession
- https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations
- https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-uap7-capability
- https://lrclib.net/docs

## Acceptance
- No runtime WebView process launched by Daijobu; webview2-com removed from dependency graph/lockfile and old engine source removed.
- Live native read-only discovery and reasonable error evidence; coordinator tests actual player controls separately. Don't claim real Spotify/YTM tested if not installed/open.
- Regression tests for session lifecycle/stale target, unsupported controls, timeline pause/seek, thumbnail caps/stale updates, no-session UI and old-config migration. Lyrics tests if included cover disabled network gate, LRC parsing/bounds/time selection, stale query and offline behavior.
- cargo fmt --all -- --check, cargo clippy --workspace --all-targets -- -D warnings, cargo test --workspace -- --test-threads=1, optimized terminal-folders build; report honest test counts and manual limitations.
- Preserve delivery exe outside disposable worktree: root target/stage-t4/stage-9f-native-music/daijobu.exe. Do not launch it until coordinator takes over.