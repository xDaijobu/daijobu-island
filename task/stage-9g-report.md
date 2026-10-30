# Stage 9g - Compact Music playback glyph

- [x] Compact now shows Pause when playing and Play when paused, matching expanded Music. Click routing is unchanged.
- [x] fmt, strict workspace/all-target clippy and optimized terminal-folders build passed.
- [x] Workspace serial tests: 1,735 passed, one existing native notification-pipe queue test failed. Complete dj-win retry passed all 210 tests. No unrelated pipe code changed; logs preserved under target/stage-t4/stage-9g-music-icon.
- [x] Delivered hash C0F7473891D6B2C7B3D0C7F406C086F58579AECE446640D4D6A4AAAB1D1E12BB to the existing registered Stage9e executable path and launched PID41836 after graceful shutdown. Package identity remains valid.
- [x] Inspected playing expanded panel screenshot. Compact screenshot verification could not be completed because Orca subsequently returned windowNotFound; compact glyph branches were reviewed directly. No new implementation-mirroring tests added for this two-line visual fix.

Run the installed daijobu.exe, play media and collapse Music: the compact right-hand glyph is Pause. Paused media uses Play. The preserved build is target/stage-t4/stage-9g-music-icon/daijobu.exe.
