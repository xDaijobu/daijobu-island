# Stage 11 report: dj CLI

Built on 2026-10-27 while the user was away. The coordinator (Claude Opus 5.5) wrote the contract and the plan. Four Claude Sonnet 5.5 workers built parts A–D in parallel worktrees, and the coordinator reviewed, merged and wired them (part F). Branch `xDaijobu/stage-11-dj-cli`, not merged; it waits for the user's live test and their review of the *(assumed)* decisions in `task/progress.md`.

## Checklist

- [x] **Crate `dj-ipc`**: `proto` (wire format, text cleaning, command line), `pipe` (server thread, client), `src/bin/dj.rs`. `target/release/dj.exe` is built next to `daijobu.exe`.
- [x] **Pipe and security**:
  - The name is `\\.\pipe\daijobu-island-<SID>-<session>`.
  - The DACL is `D:P(A;;GA;;;<user>)(A;;GA;;;SY)`, with `PIPE_REJECT_REMOTE_CLIENTS` and `FILE_FLAG_FIRST_PIPE_INSTANCE`. There is one instance, and the client uses SQOS identification so the island can't impersonate it.
  - Each connection carries one request and one reply, a JSON line of at most 4096 bytes with `"v":1`.
  - A silent client is dropped after 2 s. The UI has 2.5 s to answer before the client gets "busy" (the client waits 3 s in total).
- [x] **Commands**: `progress <n|n%> [label]`, `progress [label]` (open-ended), `done [message]`, `notify <message>`, `theme [id]`, `snap`, `scene` (coming soon), `--help`, `--version`. Exit codes are 0, 1 and 2.
- [x] **Island**:
  - Progress pill 300×36 and notice pill 360×36, as rest states with their own layers.
  - Priority: snap thumb > notice > timer alert > progress > Music compact > Timer compact > idle.
  - A click on the notice dismisses it; a click on the progress pill opens Home.
  - Done shows for 4 s and the notice for 5 s, counted from when the pill is first on screen (not behind a panel, the snap thumb, the other pill or a hidden island). One that never shows, and a progress with no update, goes after 30 min.
  - While a pill fades out, it keeps what it showed.
  - The tile picker also opens over a dj pill.
- [x] **Idle**: the pipe thread blocks in an overlapped `ConnectNamedPipe` and its stop event. The island wakes for a request and for one one-shot `TIMER_DJ` (13) at the next timeout, and nothing polls.
- [x] **Docs**: README "dj CLI" section (examples, PATH hint, exit codes) and the crate list; AGENTS.md layout line for `crates/dj-ipc/`.
- [x] **Tests**: 884 in the workspace. New ones:
  - `dj-core::dj` and the island's dj states, priorities, sizes and clicks.
  - `dj-ipc`: 41 lib tests (proto; pipe name, SDDL and the DACL read back from a real pipe; in-process end-to-end over a pipe with a test-only name; busy, silent client, oversized frame, second server, stop) and 10 bin tests (exit-code mapping).
  - `dj-render::dj`: 21 CPU-raster tests of the pills.
  - `app::dj`: 11 tests with plain requests: progress → island state, done → timer arming (including a timer that fires early), notice + click, theme ok/unknown/list, a long theme list within a frame, snap and snap-while-open, and the fade-out hold.

## Verification (coordinator)

- `cargo fmt --all`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 884 passed, 0 failed.
- `cargo build --release`: `daijobu.exe` and `dj.exe` built.

### Live, on this machine (3440×1440, scale 1.0, notch style)
The 12a build that was running was closed with WM_CLOSE, and the stage 11 build was started. It is still running for the user's test. Screenshots were small crops of the pill only, and they were deleted afterwards.

| Acceptance | Result |
|---|---|
| 1. `dj progress 40 deploy` → 40 %; `dj progress 80` → 80 % with the label kept; `dj done "build ok"` → ✓ for 4 s, then the idle clock | ✅ |
| 2. `dj notify` shows the notice; a click dismissed it in under 1 s | ✅ |
| 3. `dj theme` lists with `*`; `dj theme midnight-cassette` switches; `dj theme nope` exits 1 with the list | ✅ (the theme was set back to Paper Zine) |
| 4. `dj snap` opens the overlay; a second `dj snap` exits 1 "a snap is already open"; Esc closed it (nothing saved) | ✅ |
| 5. With only the old 12a island running (no pipe): `dj progress`/`dj theme` exit 2 "Daijobu Island is not running" | ✅ |
| 6. The live pipe's ACL is SYSTEM + this user only; idle CPU was 16 ms over 15 s | ✅ (the DACL is checked, not attacked from another account) |

Also seen: an open-ended progress (track and "…"), a long label cut with "..", and a plain `dj done` ("done") in Midnight Cassette.

## How to run

```powershell
cargo build --release
target\release\daijobu.exe
$env:PATH += ";$PWD\target\release"
dj progress 40 deploy; dj progress 80; dj done "build ok"
dj notify "hello"            # click the pill to dismiss
dj theme; dj theme clean
dj snap
```

## Decisions made without the user (also marked *(assumed)* in progress.md)

- One pipe per user **and** session.
- One progress at a time; a new label replaces the old one, and an empty label keeps it.
- A new notify replaces the current one (no queue).
- A progress with no update for 30 minutes goes away by itself.
- The `dj theme` switch is saved to the config, like `Ctrl+Alt+T`.
- `dj snap` while a snap is open is an error (exit 1), not a no-op.
- A done message or a notice counts its 4 s / 5 s from when it is first on screen, so one sent while a panel is open (or the island is hidden) is not lost; it never waits longer than 30 min.
- A number-looking first word is always a percent: `dj progress 2024 release` is an error, while `dj progress v2024 release` is a label.

## Known issues and what was not verified

- **Other users, remote clients and elevation**: not tested with a second account, a remote client or an elevated island against a normal terminal (integrity levels). Only the DACL and the flags were checked.
- **Name squatting**: `FIRST_PIPE_INSTANCE` protects once the island is up. A process of the same user that creates the name first would get the requests, and `dj` doesn't check who serves the pipe.
- **Glyphs**: text in characters the embedded fonts lack (CJK, emoji) draws as boxes, as Music titles do.
- **Scale**: the pills are tested at 1, 1.5 and 2 on CPU raster; on screen only at 1.0.
- **`dj done` without a message** shows "done", not the progress label (per the spec).
- The workers' parts were reviewed by the coordinator; the wiring was reviewed by a read-only Sonnet reviewer (findings below).

## Review of the wiring

A read-only Sonnet 5.5 reviewer read the wiring commit. It found no crash, hang, stuck pill or busy loop. All four findings were fixed:

1. **A request already answered "busy" ran later** (the UI thread held up, e.g. by the tray menu): `Incoming::expired()` (`dj-ipc`), and `drain_dj` drops expired requests instead of carrying them out late.
2. **Done and notice time ran while the pill wasn't visible** (under a panel, the snap thumb, the other pill or a hidden island), so an agent's "needs input" could vanish unseen: the clocks now start when the pill is first on screen (`Dj::on_screen`, called from `sync_dj`), with a 30 min cap. Checked live: a notice sent while Home was open waited 7 s, then showed for its full 5 s after the collapse.
3. **The pills were pushed only after the whole batch**, so a `snap` in the same batch collapsed to the old rest state: `apply_dj` now runs after each request, before its effect.
4. **A due timeout re-armed the timer on every event** (mouse moves could postpone it): `Dj::next_at` gives a fixed instant, so the one-shot is armed once.

Not changed: if the pipe thread gives up after 50 connect failures in a row, nothing restarts it (it would take a broken system; `dj` then reports "busy" or a timeout).

After the fixes: fmt, clippy `-D warnings` and 884 tests are clean, the release build is OK, and progress, done, theme and the notice-behind-Home case were rechecked live. The stage 11 build (`stage-11-dj-cli/target/release/daijobu.exe`) is left running for the user's test.
