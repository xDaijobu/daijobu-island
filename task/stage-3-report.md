# Stage 3 report: OSD volume

Branch `xDaijobu/stage-3-osd`, four commits on top of `master` (`acb83f0`). Nothing pushed.

## Verdict

Every scope item is implemented in code and the unit-level behaviour is covered by tests. **None of the live behaviour has been checked.** Per the headless brief, I did not run `daijobu.exe`, send input, or change the volume, so the acceptance test ("press volume keys, the pill shows, the flyout does not appear") is still open. That is for the user to test.

Verification run here (debug profile, `CARGO_BUILD_JOBS=2`):

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: all pass. dj-core 44 (38 before, plus the new OSD, island and config tests), dj-win 4 (2 existing, plus the hook and volume clamp tests). `app` has no tests.

Not run: `cargo build --release`. The contract lists it, but the brief limits verification to fmt, clippy and test. Please run it before the PR.

## Checklist

| Item | State | Notes |
|---|---|---|
| Unlock OSD in the feature registry | Done | `features.rs`: `osd` is `Available`, everything else stays `Locked`. Music is not in this tree (Stage 2 is elsewhere). |
| `WH_KEYBOARD_LL` hook for `VK_VOLUME_UP/DOWN/MUTE`, swallow, apply via `IAudioEndpointVolume` | Code done, **not verified live** | `dj-win/src/hook.rs` swallows down and up for the three keys and posts `WM_VOLUME_KEY` to the window. The app then calls `VolumeStepUp/Down` or toggles mute on the default render endpoint (`dj-win/src/volume.rs`). The hook callback only posts. |
| Changes from elsewhere via `IAudioEndpointVolumeCallback`, device changes via `IMMNotificationClient` | Code done, **not verified live** | Callbacks only `PostMessageW`. The UI re-reads the endpoint and shows the pill. Default-device changes re-acquire the endpoint silently. |
| OSD state in `dj-core`: temporary on top of Idle/Home, 220×40 pill, speaker icon, 4 px bar, mono value, 1.5 s hold reset on each change | Done (tests); visual **not verified** | `dj-core/src/osd.rs` (model), `island.rs` (pill morph and state), `dj-render/src/paint.rs` + `icons.rs` (drawing). The pill springs to the OSD size and back to the previous state. |
| Click or drag on the bar sets the volume | Done (tests); **not verified live** | Click uses the bar's hit area. Drag uses `SetCapture`, so it keeps tracking outside the pill. The OSD stays up while dragging. |
| Holding the key repeats; each step moves the bar with a spring | Code done, **not verified live** | Every auto-repeat key-down posts one step. The level spring is 0.25 s, zeta 1.0. |
| OSD tile in Home opens the OSD view (manual peek) | Done (tests); **not verified live** | Shows the OSD with the current volume for the normal 1.5 s, then returns to Home. See decision 3. |
| Config `[osd] replace_volume_flyout = true` turns the hook off when false | Done | Default `true`. Read at startup only. Editing the file needs a restart. When `false` the hook is not installed and the Windows flyout stays. The endpoint callbacks still run, so Settings-slider changes still show the pill. |
| Idle CPU ≈ 0 | Reasoned, **not measured** | When the OSD is hidden, `is_animating()` is false, so the loop blocks in `GetMessageW`. The hook is cheap and only posts on volume keys. |
| Tests: OSD timer and state transitions, icon thresholds, level clamping | Done | `osd.rs` tests (hold, reset, pin, clamp, percent, icon thresholds, settle). `island.rs` tests (morph, return to Home, tile blocking, bar click and drag, drag while pinned). `layout.rs` tests (bar mapping). `config.rs` test (`[osd]` default and parsing). |
| `build --release`, `clippy -D warnings`, `test` pass | clippy and test pass; **`build --release` not run** | See above. |
| Acceptance: volume keys show the pill, flyout does not appear, Settings slider shows the pill, 1.5 s back to clock, OSD unlocked in Home | **Not verified, user to test** | |

## How to test (user)

1. `cargo build --release` then run `target\release\daijobu.exe`. Use a machine where you can afford a volume change.
2. Press the volume up, down and mute keys. Expected: the island morphs to the OSD pill, the Windows flyout does not appear, the bar and number follow each press, and the pill returns to the clock 1.5 s after the last press.
3. Hold a volume key. The bar should move smoothly through repeated steps.
4. Move the volume with the Settings slider or another app. Expected: the pill appears.
5. Click the OSD bar, and drag along it, including past its ends. Expected: the volume follows the pointer, and the pill stays up during the drag.
6. Click the pill to expand it, then click the OSD tile. Expected: the OSD shows for 1.5 s, then the Home grid returns.
7. Set `[osd] replace_volume_flyout = false` in `~/.daijobu/config.toml`, restart, and press the volume keys. Expected: the Windows flyout appears as before. The pill should still appear for Settings-slider changes.
8. Check idle CPU in Task Manager after the pill has hidden. It should be about 0%.
9. **Restore your original volume and mute state.** I did not change anything on this machine.

## Decisions I made

1. **Keys are swallowed only when the endpoint is available.** If the endpoint fails to open, the hook is not installed, so the keys keep working through the Windows flyout. Without this guard the keys would do nothing.
2. **Pill morph spring.** Growing uses the theme's `expand` spring and shrinking uses `collapse`, decided by area.
3. **OSD tile is a timed peek.** It does not pin the OSD open. Clicking the tile while Home is open shows the OSD and then returns to Home.
4. **While the OSD is up, clicks off the bar are ignored.** They don't expand or collapse the island.
5. **Muted state.** The icon becomes an X, the bar and number are dimmed, and the number still shows the level.
6. **Default-device change is silent.** It re-acquires the endpoint but does not show the pill.
7. **Mouse capture on every left click.** `SetCapture` is set on press and released on release. This is what makes the bar drag work outside the pill.
8. **Hook passes through every other key.** Only the three volume keys are swallowed. Injected key events are not filtered, so synthetic `SendInput` tests will also be swallowed.

## Known issues and risks

- **COM threading is the biggest risk.** The endpoint callbacks come from audio threads and only post messages to the UI thread. The Mixer and its callbacks are created on the UI thread after `CoInitializeEx(STA)`. Whether Windows delivers the notifications as expected needs a live check.
- **Hook and message pump.** The hook depends on the main thread's `GetMessageW` loop. `frame()` only runs while something animates, but the message wait is always there, so this should hold.
- **Mixer created before hook.** If `Mixer::new` fails, the keys stay with Windows and the pill still reacts to endpoint notifications only if they arrive, which they can't without an endpoint. So on that failure there is no pill and no key handling. This is logged to `~/.daijobu/daijobu.log`.
- **Config is not hot-reloaded.** Changing `replace_volume_flyout` needs a restart.
- **Subagent code.** The four parts were written by Haiku subagents. At integration I fixed three clippy findings: an unnecessary closure in `hook.rs`, a constant assertion in `layout.rs`, and a collapsible `if` in `app.rs`. I also added the `windows-core` dependency that `#[implement]` needs. Review the commits with that in mind.
- **Not handled.** Multiple monitors and DPI changes are unchanged from Stage 1 and only the OSD path is new. Device-removed events are ignored; the endpoint is re-acquired only on a default-device change.

## Not verified

- Everything in the Verify section of the brief (live key presses, flyout suppression, Settings slider, the 1.5 s return, idle CPU).
- Screenshot `task/stage-3-osd.png`: not taken. Capturing the island would need the app running, and the brief says to skip it if other windows would be captured.
- `cargo build --release`.
- Pixel-level look of the OSD pill: the drawing code is unchecked against the design in `docs/plan.html`.
- Hook behaviour with real volume keys on the keyboard, and with media keys in other apps.

## Integration with Stage 2

The Music branch (`xDaijobu/stage-2-music`) was merged into this branch with a merge commit, not a rebase. Music and OSD are both Available in the feature registry, and every other feature stays locked.

**What conflicted (7 files)**

- `dj-core/src/features.rs`: stage 2 has `music` at index 0 (it replaces the stage-3 `media` tile). Stage 3 has `osd` at index 1. Both are now Available, and the list is still 13 tiles.
- `dj-core/src/island.rs`: both branches rewrote the state machine. The merged `IslandState` has Idle, Home, MusicCompact and MusicExpanded. The OSD is an overlay: it takes the pill at its own size while visible and never changes the state, so the island returns to whatever it was showing. Content opacity uses stage 2's four layer springs (`layers.home` replaces stage 3's `fade`). Stage 2's `grows()` is gone because the OSD size does not follow the state's direction. The area-based `expanding` flag from stage 3 now picks the expand or collapse spring for every change.
- `dj-core/src/config.rs`: both `[music]` and `[osd]` are kept.
- `dj-core/src/lib.rs`: both `music` and `osd` modules are exported.
- `dj-render/src/paint.rs`: all four content layers and the OSD are drawn. Every content layer is multiplied by `1 - osd.alpha`, so Music hides while the OSD is up.
- `dj-win/src/window.rs`: stage 2's `WM_WAKE` was `WM_APP+3`, which collides with stage 3's `WM_VOLUME_CHANGED`. `WM_WAKE` moved to `WM_APP+6`. The `Event` enum keeps both sets of variants.
- `app/src/app.rs`: both field sets and both message sets are wired. There is one `quit()`, which stops the engine, uninstalls the volume hook, releases the mixer and then does the stage-2 cleanup. Click handling runs the Music match first, then the OSD match. The YouTube Music tray submenu is kept.

**Planning docs.** `task/progress.md` and `docs/plan.html` were taken from the stage-2 branch. `diff` against that branch is empty.

**Behaviour added by the merge.** The OSD over Music is covered by one new test, `osd_over_music_returns_to_music`. It shows the OSD over the expanded view, checks that the state stays `MusicExpanded`, and checks that the view and its controls come back once the OSD hides.

**Tests and checks**

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 69 passed, 0 failed. That is 65 in `dj-core` and 4 in `dj-win`. `dj-music`, `dj-render` and `app` have no tests. Across the trees there are 51 `#[test]` attributes on the stage-2 side, 48 on the stage-3 side, and 69 in the merged tree (including the new one).
- The workspace test run includes `system::tests::autostart_roundtrip_uses_a_scratch_value`, which writes a scratch value under the Run key and removes it again.

**Not verified.** The app was not run, as the brief requires headless builds. Nothing here has been checked on screen: OSD over Music, the morph timing, and the paint crossfade all need a live check. The stage-2 report still says `WM_APP+3` for the wake message. That is the old id, and the code now uses `WM_APP+6`.
