# Stage 4 report: Vitals (compact)

Branch `xDaijobu/stage-4-vitals`. Nothing pushed. It contains:

- the previous (Haiku) agent's three commits: the dj-core model, the dj-vitals skeleton and the lockfile;
- that agent's uncommitted work, reviewed and committed in five steps (health score, samplers, `WM_VITALS`, panel painting, app wiring);
- a merge of `xDaijobu/stage-3-osd`, which brings Stage 3 (OSD) and Stage 2 (Music);
- the move to a coalescable timer.

## Verdict

Every scope item is implemented, and the pure parts are covered by tests. **None of the live behaviour has been checked.** Per the headless brief, I did not run `daijobu.exe` and showed no windows. The acceptance test (live rings and net, a sensible score, the lamp turning warn under load, idle CPU about 0) is for the user to test.

Verification run here (debug profile, `CARGO_BUILD_JOBS=2`), on the merged tree:

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: all pass.
  - dj-core: 85
  - dj-vitals: 23
  - dj-render: 8
  - dj-win: 4
  - app: 2
  - dj-music has no tests.

Not run: `cargo build --release`. The contract lists it, but the brief limits verification to fmt, clippy and test. Please run it before the PR.

## Checklist

| Item | State | Notes |
|---|---|---|
| Unlock Vitals in the feature registry | Done | `features.rs`: `music`, `osd` and `vitals` are `Available`, the other 10 stay `Locked`. The test asserts exactly that set. |
| `crates/dj-vitals`, Win32 only | Done (code); values **not verified live** | `win.rs` holds the unsafe calls, each with a `// SAFETY:` comment: `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetDiskFreeSpaceExW` on `%SystemDrive%\`, `GetIfTable2` (`IfOperStatusUp` and not `IF_TYPE_SOFTWARE_LOOPBACK`, freed with `FreeMibTable` on every path), `GetSystemPowerStatus`, `GetTickCount64`. `calc.rs` holds the pure maths: deltas, rates, CPU %, used %, battery mapping and drive root. No WMI, no PowerShell. If an API fails, that field falls back to 0, and battery falls back to `None`. |
| Sampling on a worker: 1 Hz with the panel open, 30 s otherwise, coalescable timer, none while hidden | Done (code + cadence test); **not verified live** | `app/src/vitals.rs`. The UI thread arms `SetCoalescableTimer`: 1000 ms with 100 ms tolerance, or 30 000 ms with 3 s tolerance. Each tick pokes the worker thread, which samples, queues the `Reading` and posts `WM_VITALS`. Hiding the island, or turning the lamp off with the panel closed, kills the timer. Opening the panel samples at once when the last sample is over 1 s old. |
| Health score in `dj-core` (weights, clamp, bands, reasons) | Done (tests) | `health.rs`. Penalties are linear between the contract breakpoints. Uptime gives 1 above 7 days and 3 above 14 days. The score is rounded and clamped. Bands are 85/65/45. Reasons are "High CPU", "High RAM", "Low disk" and "Restart soon", ordered by largest penalty. Ties keep CPU, RAM, disk, uptime order. NaN or infinite inputs count as 0, and battery does not affect the score. |
| Idle-pill lamp follows the band, `[vitals] health_lamp = true` | Done (config test); colour **not verified live** | The lamp uses the theme `warn` colour for `hmm` and `dame`, and `accent` otherwise. With `health_lamp = false` the lamp stays `accent`, and nothing is sampled while the panel is closed. |
| Vitals panel 400×150 | Done (code); visual **not verified** | `dj-render/src/paint.rs` (`vitals_panel`) and `vitals.rs` (pure helpers). Contents: a header reading "daijobu? · health" plus the score and band, in accent or warn; three 56 px rings with a 4 px stroke on a `surface2` track, which turn warn past CPU 85, RAM 88 and disk 93; the ↓/↑ rates; a 60-sample download sparkline; battery `BAT 82% +` (charging), shown only when present; and the top reason tag. Before the first reading, the values show "…". |
| Opens from the Home tile; an empty click goes back | Done (tests) | `Island::open_vitals`. The panel has no controls, so any click inside it collapses to the rest state: Idle, or Music compact when a track is loaded. Music's expanded view behaves the same way. |
| Idle CPU about 0 with the panel closed; at most one redraw per sample with it open | Reasoned, **not measured** | With the panel closed: one coalesced tick every 30 s, one sample, and a redraw only when the lamp colour changes. With it open: one `dirty` frame per reading, plus any springs. |
| Tests: bands, penalties, delta/rate maths with counter wrap, formatting | Done | `health.rs` (16), `calc.rs` (23), `dj-render/src/vitals.rs` (rate formatting, sparkline, ring sweep), `island.rs` (open, collapse, Music rest state, OSD over Vitals), `config.rs` (`[vitals]`), app (cadence). |
| `build --release`, clippy, test | clippy and test pass; **`build --release` not run** | See above. |
| Acceptance: live values at 1 Hz, sensible score/band, lamp changes under load, idle CPU ≈ 0 | **Not verified, user to test** | |

## Merge with `stage-3-osd` (OSD + Music)

- **Island:** Stage 2 replaced the single fade spring with per-state opacity layers. Vitals is now the fifth layer (`Layers::vitals`), and the earlier `vfade` and `IslandView::vitals` are gone. The OSD covers the panel the same way it covers the other states. The state stays `Vitals`, and the panel comes back when the OSD hides (tested).
- **WM_APP ids:** `WM_WAKE` stays at `WM_APP+6`, and `WM_VITALS` moved to `WM_APP+7`. `window::wake` now goes through the shared `post_to`.
- **Timer ids:** `TIMER_CLOCK` = 1, `TIMER_VU` = 2, the vitals timer = 3.
- **Paint:** the panel draws with `smoothstep(0.35, 1, layers.vitals) × (1 − osd.alpha)`, like the Music layers.
- **Merge commit extras:** besides the conflict resolution, the merge commit also contains two small panel tweaks (the ring track uses `surface2` as in plan.html, and the RAM ring turns warn at 88 to match its breakpoint) and a doc comment on `VitalsConfig`.

## How to test (user)

1. `cargo build --release`, then run `target\release\daijobu.exe`.
2. Click the pill, then the **Vitals** tile. Expected: the pill morphs to 400×150. Within about 1 s the rings, net rates and score fill in, and they then update once a second. Check that CPU, RAM and the system-drive % roughly match Task Manager.
3. Start a download. ↓ should rise and the sparkline should grow from the right. On a laptop, check that the battery shows and the `+` follows the charger. On a desktop, no battery text should appear.
4. Click anywhere in the panel. Expected: back to the clock pill, or the Music compact view when a track is loaded.
5. Press a volume key with the panel open. Expected: the OSD pill shows, and the panel comes back after 1.5 s.
6. Load the CPU (for example a build) for over a minute with the panel closed. Within about 30 s the idle lamp should turn the theme's warn colour once the score drops below 65. It should go back once the load stops.
7. Set `[vitals] health_lamp = false` in `~/.daijobu/config.toml` and restart. The lamp should stay the accent colour.
8. Check Task Manager with the panel closed. Idle CPU should be about 0%.

## Decisions I made

1. **Empty click collapses** to the rest state rather than to Home. That matches Music's expanded view. The earlier agent had it go to Home.
2. **Counter wrap policy:** a backwards counter step counts as 0, not as a wrap, and the new value becomes the baseline. 64-bit octet counters don't wrap in practice, and a backwards step usually means an adapter came or went. Without this, a disappearing adapter would show a huge spike.
3. **First sample waits 1 s** after the sampler's baseline, so the first CPU and net values cover a real interval.
4. **The lamp only uses `warn` for `hmm` and `dame`.** `ok` stays accent, because the theme has just those two colours.
5. **Sparkline scale** is the 60-sample window's maximum, with a floor of 64 KiB/s so an idle link draws flat.
6. **The history mixes cadences.** 30-second lamp samples also enter the 60-sample sparkline, so right after the panel opens the older part of the line spans a longer time.
7. **The ring label is "DISK"**, not the drive letter shown in the plan mockup.

## Known issues and risks

- Nothing has been seen on screen. Text positions in the panel (header baseline, ring numbers, bottom row) are computed, not eyeballed. Fonts without `↓ ↑ · …` are covered by a cmap test for Geist Mono only. Themes using VT323, Silkscreen or Doto as the label font may show boxes for the arrows.
- `GetSystemTimes` CPU % is system-wide and averaged over the sample interval: 30 s in lamp mode and 1 s in panel mode.
- The net rate includes every up, non-loopback interface, including virtual adapters (Hyper-V, VPN). Traffic routed through a virtual switch may count twice.
- `app` still has only the two cadence tests. Timer and worker wiring is untested beyond compilation.
