# Stage 6 report: Timer / Pomodoro

Built on `stage-6-timer`, starting from `0fd06f9` (contract and brief). The work was headless: the exe was never run, no audio was played, and nothing was clicked or captured. All live, visual and audio behaviour is **not verified, user to test**.

## Verdict

Every scope item is implemented and the pure parts have unit tests. Nothing in the contract was skipped. The deviations are small and listed below.

Verification on `stage-6-timer` (MSVC 14.51, Windows SDK 10.0.26100, `dj-build-env.sh`):

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: **234 passed**, 0 failed (dj-core 143, dj-sweep 35, dj-vitals 26, dj-render 14, app 12, dj-win 4). Stage 5 ended at 196, so this stage adds 38: dj-core +26, app +10, dj-render +2.
- `cargo build --release`: OK.

## Checklist (contract)

| Item | State | Notes |
|---|---|---|
| Unlock Timer (4 of 12 ready), Home → Timer opens the panel, `ClickOutcome::Open` handled | Done | `features.rs` marks Timer `Available`. `app.rs` handles `Open(i)` for `"timer"` with `island.open_timer`. The Home footer count comes from the registry. Live: **not verified, user to test**. |
| `dj-core::timer`: deadline-based `Stopped \| Running { ends_at } \| Paused { left } \| Done { finished, next }`, time as `Instant` | Done | `crates/dj-core/src/timer.rs`. Nothing ticks inside: the remaining time comes from `ends_at - now`. |
| Phases, long break every `every`-th focus, "focus 2/4" counter | Done | Tested over two full cycles of 4. |
| start, pause/resume, skip (no alert), reset (counter cleared), continue, stop | Done | See the semantics notes below. |
| Never negative, a backwards clock never adds time | Done | Remaining is `saturating_duration_since`, capped at the run's length when it was started or resumed. |
| Config `[timer]`, out-of-range values fall back to the defaults | Done | Each field falls back on its own, and the rest of the file still loads. |
| Presets 25/5, 50/10, 15/3, only while Stopped, kept until restart, not written to the config | Done | `Pomodoro::set_preset`. The config is never saved from the timer. |
| Timer panel 400×184 (r 32): Doto mm:ss, ring, phase label, chips, start/pause, skip, reset; empty click collapses | Done | Visual: **not verified, user to test**. |
| Timer compact 172×36 (r 18): ring 18, `focus`/`break` label, Doto 18; rest state while running without music; click opens the panel | Done | Visual: **not verified, user to test**. |
| With music: Music compact stays the rest state, with a thin edge arc and a small mono mm:ss; Music expanded unchanged | Done | The arc runs clockwise from the top centre along the pill's edge. While a timer is active, the title gives up 42 px for the digits. Visual: **not verified, user to test**. |
| Session end: alert about 360×72 (`focus done · break 5:00`, ▸ continue, × stop), chime once, waits for a click; OSD shows over it; over an open panel the chime plays at once and the alert waits for the collapse; the Timer panel shows done inline | Done | Island tests cover all five panels. Live and audio: **not verified, user to test**. |
| Chime synthesised in code (sine partials, exponential decay, about 0.8 s, 44.1 kHz mono 16-bit), in-memory WAV, `PlaySound(SND_MEMORY \| SND_ASYNC)` | Done | `app/src/chime.rs` builds the WAV and `crates/dj-win/src/sound.rs` plays it (`SND_MEMORY \| SND_ASYNC \| SND_NODEFAULT`). There is no audio asset. Audio: **not verified, user to test**. |
| Hidden island: the timer keeps running and the chime plays; the alert shows on the next show | Done (by design) | The deadline `WM_TIMER` stays armed while hidden. The island moves to TimerAlert while hidden, and the first frame after showing draws it. **Not verified, user to test**. |
| Idle CPU ≈ 0: one `WM_TIMER` at the deadline, the redraw only while digits are on screen, nothing armed while Stopped or Paused off screen | Done | See "Timers" below. **Not verified, user to test.** |
| Tests: state machine, config ranges, hit testing, rest state with and without music, chime WAV | Done | Listed below. |

### Tests added

- **dj-core `timer`** (14): stopped state, deadline countdown, pause/resume keeping the remainder, never negative, `tick` firing once, backwards clock, continue/stop, long break every 4 over two cycles, `every = 1`, skip (running and paused), reset, presets locked unless Stopped, clock format, view.
- **dj-core `config`** (2): `[timer]` defaults and reading, per-field fallback (0, 181, −5, `every = 0` and `13`, edges 1 and 180).
- **dj-core `layout`** (3): the window still fits Home (the size is unchanged) and holds the three Timer views; panel controls fit and don't overlap; compact, alert and Music-compact timer parts fit.
- **dj-core `island`** (8, plus the locked-tile test moved to Snap): the tile opens the panel and an empty click collapses it; the panel controls' actions, including continue/stop inline when done; Timer compact as the rest state; music outranking the timer and handing back; the alert over the rest states, with and without music; the alert waiting for continue or stop; done over each of the five panels waiting for the collapse; OSD over the alert.
- **dj-render `timer`** (2): ring sweep, and the edge path is one closed loop of the right length that starts at the top centre.
- **app `chime`** (4): RIFF/WAVE header fields, length (35,280 samples, 70,604 bytes), no clipping (peak ≤ 0.6 FS and at the set level), silent start and end with a decay.
- **app `timer`** (6, with a fake `Chime`): the chime plays once per session end; no chime when `chime = false` or when skipping; nothing armed unless running; the deadline is armed while hidden and the digits only on screen, aligned to the digit change; presets; timer ids not taken.

## Semantics decided here (the contract leaves them open)

- **Skip** ends the current phase as if it had finished, and a skipped focus counts toward the long break. It doesn't chime or show the alert. A running skip starts the next phase. A paused skip stays paused, with the full length of the next phase. Skip does nothing while Stopped or Done.
- **Stop** (from Done) goes back to Stopped at focus and keeps the counter, so the next start is "focus 3/4". If the long break was next, the cycle is over and the counter clears.
- **Reset** works from any state: back to Stopped, first focus, counter cleared.
- **In Done**, the panel's start button reads CONTINUE and reset reads STOP. Skip and the chips are dimmed and do nothing. Clicking a dimmed control does not collapse the panel.
- **Alert clicks:** only ▸ continue and × stop respond. A click elsewhere on the alert does nothing (`Ignored`), because the contract says the island waits for a click. Home can't be reached until the alert is answered.
- **Labels:** `focus N/every`, `break`, `long break`, `paused`, `focus done`. The alert line uses an unpadded minute (`break 5:00`), and the clocks use `mm:ss` rounded up, so a fresh phase shows `25:00` and `00:00` only at the end.

## Timers (idle CPU)

These are new ids in `app/src/timer.rs`. App ids 1, 2 and 4 and the vitals id 3 are taken, and a test checks this.

- `TIMER_DEADLINE = 5` is a one-shot at `next_deadline()` + 5 ms. It is armed only while Running, and it stays armed while hidden. `SetTimer` repeats, so the handler kills it, and `sync_timer` re-arms it only if the phase is still running (for example after an early fire).
- `TIMER_DIGITS = 6` is a one-shot at the next change of the rounded-up digits, so at most once a second. It is armed only while Running and the digits are on screen: the island is visible and shows Timer compact, the Timer panel, or Music compact with an active timer.
- Nothing is armed while Stopped, Paused (on screen or off: the digits don't move) or Done. This is stricter than the contract.

## Deviations

1. **Config minutes are `i64`, not unsigned.** With `u32`, a stray `focus_min = -5` would fail the whole TOML and `Config::parse` would reset every setting (theme, Music sign-in, …). With `i64` only that field falls back. A float like `25.5` still fails the whole file, the same as any other type error today.
2. **`every` accepts 1..=12.** The contract only names the minute ranges, so the cap of 12 is my choice.
3. **`Running { ends_at }` is kept exactly**, but `Pomodoro` has an extra private `span` (the run's length at start or resume). This is what caps the remainder when the clock seems to go backwards.
4. **The redraw is a one-shot aligned to the digit change rather than a free-running 1 Hz `SetTimer`.** It still runs at most once a second, and the digits don't lag by up to a second.
5. **The WAV is built on the first chime** (`OnceLock`), not at startup. It is still built only once, and the buffer is `'static`, so the async `PlaySound` can keep reading it.
6. **The panel and alert layouts are my own design.** plan.html only draws Timer compact. The alert uses the notification row of the size table (360×72, r 28).
7. **Build parallelism:** I used the coordinator's `dj-build-env.sh` (`CARGO_BUILD_JOBS=8` and the shared target dir) instead of the brief's `CARGO_BUILD_JOBS=2`. The release build therefore replaced `target/release/daijobu.exe` in the main checkout's target dir with this branch's build.

## Pill-local rects (logical px at 100%, from `layout.rs`)

The window is 520 logical px wide, at the top centre of the primary monitor (`top_center`). Each view is centred in the window at y = 6 (`TOP_MARGIN`):

screen x = monitor left + (monitor width − 520·s)/2 + ((520 − view w)/2 + local x)·s, and screen y = monitor top + (6 + local y)·s.

| View | Size | Element | Rect (x, y, w, h) | Centre at 3440 px wide, scale 1 |
|---|---|---|---|---|
| Home | 520×340 | Timer tile (index 3) | (312, 52, 88, 76) | (1816, 96) |
| Timer panel | 400×184, r 32 | Start/Pause/Resume (CONTINUE when done) | `TP_TOGGLE` (20, 140, 172, 30) | (1626, 161) |
| | | Skip | `TP_SKIP` (200, 140, 86, 30) | (1763, 161) |
| | | Reset (STOP when done) | `TP_RESET` (294, 140, 86, 30) | (1857, 161) |
| | | Chip 25/5 | `tp_chip(0)` (128, 102, 64, 24) | (1680, 120) |
| | | Chip 50/10 | `tp_chip(1)` (200, 102, 64, 24) | (1752, 120) |
| | | Chip 15/3 | `tp_chip(2)` (272, 102, 64, 24) | (1824, 120) |
| | | Empty spot (ring), collapses | `TP_RING` (20, 36, 92, 92) | (1586, 88) |
| Timer compact | 172×36, r 18 | Anywhere opens the panel | pill (0, 0, 172, 36) | (1720, 24) |
| Timer alert | 360×72, r 28 | ▸ continue | `AL_CONTINUE` (212, 20, 88, 32) | (1796, 42) |
| | | × stop | `AL_STOP` (308, 20, 32, 32) | (1864, 42) |
| Idle | 128×32 | Click opens Home | pill | (1720, 22) |

## How to run (this machine)

```sh
source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh
cd C:/Users/UseR/orca/workspaces/daijobu-island/stage-6-timer
cargo build --release   # target/release/daijobu.exe (in the shared target dir)
```

Optional `~/.daijobu/config.toml`:

```toml
[timer]
focus_min = 25
break_min = 5
long_break_min = 15
every = 4
chime = true
```

For a quick live test, set `focus_min = 1` and `break_min = 1`. This config value isn't one of the presets, so no chip is lit until you pick one.

## Suggested live test (not run)

1. Home → Timer → 25/5 → START. The pill shows the countdown once the panel collapses.
2. Pause, resume, skip and reset from the panel. Open Timer compact again by clicking it.
3. With a song loaded: Music compact shows the edge arc and the mm:ss, and Music expanded is unchanged.
4. At 0: the chime plays and the alert appears. ▸ starts the break. After 4 focus sessions the long break comes up.
5. Finish a session with Vitals open: the chime plays at once, and the alert appears after an empty click.
6. Ctrl+Alt+H during a run: the chime still plays, and the alert is there when the island is shown again.
7. Idle CPU with the timer Stopped and with it Paused, and the timer redraw rate while it runs.

## Not verified (user to test)

- All visuals: the panel, compact, alert and the Music-compact arc, Doto digit sizing, spacing and fades.
- The chime: whether it sounds soft, its level, and whether `PlaySound` with `SND_MEMORY | SND_ASYNC` plays through the default device.
- Every click flow and the live timing of `WM_TIMER` (deadline accuracy, the redraw rate, behaviour while hidden).
- Idle CPU.

## Known issues and risks

- **Sleep and hibernate:** `Instant` on Windows is QPC-based. Whether a phase "counts" time spent asleep depends on how QPC behaves across sleep on this machine. `WM_TIMER` fires after wake, and the remaining time is still never negative.
- **Contrast:** the panel's START, the alert's CONTINUE (pill ink on accent) and the warn ink (STOP, the finished digits) aren't checked by the theme validator, which only checks text on pill. The Music play button and the Vitals low-disk button already work the same way.
- **`PlaySound` is process-wide:** a new chime cuts off any other `PlaySound` sound from the app. There is none today.
- With music and a timer active, the Music compact title is truncated sooner (74 px instead of 118 px).
- A preset chosen in the panel lasts until restart (per the contract). A config value that isn't a preset shows no lit chip.
