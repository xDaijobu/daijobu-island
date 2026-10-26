# Stage 6 brief: Timer / Pomodoro (for the worktree agent)

Your worktree starts from `master`, which includes Stages 1–5 (shell, Music, OSD, Vitals, Sweep). You are unlocking the **Timer** tile: Pomodoro focus/break cycles, a compact countdown, a mini timer on Music compact, and a chime alert when a session ends.

## HEADLESS (overrides everything)
- Never run `daijobu.exe` and never play audio. No windows, hooks, input, volume changes or screenshots.
- Test `PlaySound` wiring through a trait with a fake. Only build the WAV bytes in tests.
- `CARGO_BUILD_JOBS=2`. Verify only with `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and `cargo build --release`.
- In the report, mark live checks as "not verified, user to test".

## Read first
1. `AGENTS.md`.
2. **Contract:** `task/progress.md`, section **"Stage 6: Timer / Pomodoro"**. Build exactly that. Don't edit progress.md.
3. `docs/plan.html`: the size table (Timer compact 172×36, r 18, ring 18 + Doto 18), the activity priority (`… notif > timer done > … > media > timer running > vitals alert > idle`) and the Doto timer digits sample.
4. `task/stage-4-report.md` and `task/stage-5-report.md`: how a panel is wired (island state, layout rects, painter, `ClickOutcome`, app timers).

## Design guidance
- **`crates/dj-core/src/timer.rs`** (pure): `Pomodoro` with the phase, the focus counter, the settings and a `State` enum. Every method takes `now: Instant`. Expose `remaining(now)`, `progress(now)` (0..=1), `label()`, `next_deadline()` (for the app's one-shot timer) and `tick(now) -> Option<Finished>`. Don't keep a ticking counter; derive everything from `ends_at - now`.
- **Island** (`dj-core/src/island.rs`): add `TimerPanel`, `TimerCompact` and `TimerAlert` states and give each a layer. `rest_state()` becomes:
  - Music compact when a track is loaded (with the timer arc drawn on it when the timer runs);
  - otherwise Timer compact while Running or Paused;
  - otherwise Idle.

  `Done` puts `TimerAlert` over rest states and compact views only. If a panel is open, play the chime at once and show the alert when the panel collapses. The Timer panel shows the done state inline.
- **Layout** (`dj-core/src/layout.rs`): the panel is 400×184 and the alert about 360×72. Keep the swap-chain rule: the Home grid is still the largest state, so `window_size()` must not change.
- **App** (`app/src/app.rs`): `ClickOutcome::Open` for `"timer"`. Use new `TIMER_*` ids that are **not already taken** (1, 2 and 4 are used here, and check `vitals.rs` for 3). Use a one-shot `SetTimer` at `next_deadline()` and a 1 Hz redraw timer only while digits are visible. Hiding the island keeps the deadline timer.
- **Chime** (`dj-win`, or a small `app/src/chime.rs`): synthesise the samples (for example 880 Hz + 1320 Hz partials, a 5 ms attack and an exponential decay to −60 dB over about 0.8 s, peak ≤ 0.6) into a RIFF/WAVE buffer once. Play it with `PlaySound` (`SND_MEMORY | SND_ASYNC | SND_NODEFAULT`) behind a `Chime` trait. Add only the `windows` features you need (`Win32_Media_Audio` already exists in dj-win). The `unsafe` goes in platform code with `// SAFETY:` comments.
- **Render** (`dj-render/src/timer.rs` + `paint.rs`): the panel, compact, alert and the Music-compact arc. Use the existing Doto font for the digits and theme tokens only (accent for focus, `warn` for the alert). Text must keep the theme contrast rules.
- No Focus scene, ambient grain, split bubble or notifications. Those are later stages.

## Finish
Commit in small steps on your branch and don't push. Write `task/stage-6-report.md`: an honest per-item checklist, how to run, known issues and what was not verified. Then stop.
