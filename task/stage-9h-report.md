# Stage 9h - Music Windows volume

- Added labelled Windows master-volume slider, percentage, muted/unavailable states, click/drag and scroll over its row (two percentage points per notch, proportional high-resolution deltas).
- Uses existing default-output Core Audio mixer and endpoint notifications. Music remains open instead of showing its volume OSD; other panel OSD behavior is preserved.
- Music layout is 400x216, with the volume row between lyrics and footer links. Seek controls keep separate hit areas. Drag ends on release, panel change or device loss.
- Fmt and strict workspace clippy passed. Full serial workspace run passed 1,738 tests before wheel follow-up; targeted core-volume and native-window tests verify the follow-up.
- Initial optimized build delivered and launched PID9332 at the existing registered executable path. Identity query passed. Live screenshot confirmed Windows volume row and percentage, and the corrected compact Pause glyph during playback. User activity prevented attributing the observed volume change (16% to 26%) to the test click, so no live interaction success is claimed and no volume restoration was forced over the user's current setting.
- Final scroll build/delivery evidence recorded below. Live scroll and output-device removal remain unverified; pure tests cover direction/clamping, hit-area separation and drag cancellation.

Artifacts and test/build logs: target/stage-t4/stage-9h-music-volume. No new dependencies or network behavior. This controls all Windows audio, not one player.

Final wheel follow-up: 2 core-volume tests and all 30 native-window tests passed; strict workspace clippy and optimized build passed. Final SHA-256: 8135C1C0FB32E794B978FC78BD4A5B49E2867056F8476432C2A9E47DA80F94C0. Delivered this build to the existing registered location and restarted with Music open. Initial targeted window tests exposed the keyboard-mode wheel gate; fixed both translation gates while preserving keyboard message consumption, then reran all native-window tests successfully.
