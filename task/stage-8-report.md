# Stage 8 report: Clipboard history

Date: 2026-10-28. Worktree: `stage-8-clipboard`, base `282899d` (Stage 7d, PR #30).
Status: **Implementation complete; verification passed with native gaps below.**
Clipboard is integrated and unlocked; post-fix independent review, workspace checks
and native smoke passed. No new release version is required.
Contract: [stage-8-brief.md](stage-8-brief.md); ownership:
[stage-8-dispatch.md](stage-8-dispatch.md).

## Implemented

Checked items describe code present in the worktree, not native acceptance.

- [x] Home, Ctrl+Alt+V and tray entry points, with shortcut-conflict feedback and
  Home/tray fallback. Explicit Enable history, saved opt-in, future-events-only
  capture, Pause/Resume and Clear with a confirmed include-pinned choice.
- [x] Native 400x360 Clipboard panel, Unicode search, All/Text/Images/Files/Pinned
  filters, stable-ID selection, keyboard navigation and actions, pin/unpin,
  history-only deletion, bounded in-panel text/image previews and honest status states.
- [x] Event-driven listener and off-UI acquisition/normalization/storage, bounded
  queues and byte budgets, contention retries, sequence/generation validation,
  pause/clear invalidation and bounded shutdown without indefinite UI joins.
- [x] Supported Unicode text with validated HTML/RTF alternatives, HTTP(S) links,
  standalone hex colours, PNG and supported DIB images, and ordered CF_HDROP
  file/folder paths. Files take precedence over images, then text. Malformed or
  oversized content fails without truncating a stored/pasted payload.
- [x] Privacy flags checked before payload acquisition, source-executable exclusions,
  no-cloud preservation and private replay suppression; new Snap/Terminal copies
  remain eligible for capture. No payload-bearing diagnostics.
- [x] `dj-clip` SQLite worker at `~/.daijobu/clipboard/history.sqlite3`, versioned
  schema and current-user DPAPI encryption before writes. Sensitive payload,
  preview/search, source, path and thumbnail data is encrypted; only allowed
  IDs/times/type/pin/size metadata remains plaintext. Search uses bounded RAM.
- [x] Transactional persistence acknowledgments, exact logical deduplication with
  stable IDs/pins, 500 unpinned / 30-day retention, 100 pins, 512 MiB encrypted
  payload budget, bounded decrypted cache and explicit corrupt/newer-schema errors.
- [x] Off-UI transfer preparation, prepared clipboard publication, rich/plain copy
  and paste, exact external-window identity and modifier/focus checks, manual-paste
  feedback on unsuitable targets, and exact Terminal-tab routing through its paste
  path. Image-only Terminal paste is refused; no Enter is injected.
- [x] Copy-only image/file drag reusing the Snap OLE transport. Explicit image drag
  creates collision-safe PNGs in `~/Pictures/Daijobu/Clipboard`, retained after
  cancel/drop/history deletion/exit. Ordinary capture/store/pin/preview does not
  write plaintext PNGs. Text, multi-row and virtual-file drag are outside scope.
- [x] App state/render sizing, hotkey/tray integration, feature registry unlock,
  `[clipboard] enabled`, `paused`, `excluded_sources`, and README/AGENTS/progress
  documentation. No OCR, screen recording, cloud sync or release version bump.
- [x] Additional user request: taskbar/tray latency diagnostic phases, including
  menu construction and activation before the modal menu loop. Numeric timings
  exclude deliberate time spent interacting with the open menu. Investigation
  remains unproven; this is not evidence of a measured cause or improvement.

Per-item limits are 1 MiB Unicode text, 4 MiB each HTML/RTF, 32 MiB encoded image,
64 MiB decoded pixels, maximum image edge 16,384, 128 paths / 256 KiB path text,
and 64 MiB aggregate raw snapshot bytes. The decrypted metadata/preview budget is
128 MiB. SQLite/WAL overhead is separate from the encrypted payload budget.

## Verification

Kilo reran these checks on 2026-10-28 after finishing the interrupted publication fix.
The build shell sources the existing `dj-build-env.sh` outside the repository.

| Check | Evidence / limit |
|---|---|
| `cargo fmt --all --check` | Passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |
| `cargo test --workspace --locked` | 1,633 passed; zero failed, ignored or filtered |
| Per-crate tests | app 293, capture 8, clip 82, core 758, render 171, sweep 35, term 71, vitals 26, win 189; music/doc tests zero |
| `cargo build --release --locked --target-dir C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4` | Passed |
| Clipboard raster tests | All built-in themes at 100/150/200%, included in renderer suite |
| Native synthetic smoke, without `DeferPasteCheck` | Passed, including exact external paste and current-user DPAPI restart replay |
| Taskbar/tray callback timing | Prior measurements: Stage7d 23/13/15ms; Stage8 42/14/15ms. Callback bypasses Explorer click; not proof of fixing user's report |

Native smoke used an isolated profile and disposable WinForms RichTextBox, synthetic
text/link/colour/HTML/RTF/PNG/file fixtures, and 100% DPI (asserted DPI96). It verified
future-only Enable, exclusion/history flags, local no-cloud capture, Pause/Resume,
eight restored rows and one pin, exact external paste, encrypted DB/WAL/SHM marker
absence, clear-unpinned preserving the pin/original file, explicit clear-all, and
Snap cancel preserving clipboard sequence. The harness installed the rebuilt binary
and restarted the real-profile app in cleanup; real history/config were not replaced.
Clipboard contents intentionally remain synthetic rather than restoring an old value.

The final native sequence probe measured 1287 before `CloseClipboard`, 1290 afterward.
Publication now reopens on the transfer worker, verifies exact owner HWND and private
16-byte operation token, then samples sequence while locked. Reopen contention is
bounded to five attempts. Unverified/zero sequence never authorizes paste; partial
writes report failure. A published marker retains its bounded exact-operation replay
reservation if finalization fails, preventing own recapture without suppressing Snap
or Terminal copies. Tests cover close-time increment, foreign owner/token, contention,
verification/close errors, allocations/ownership cleanup, partial writes and no paste
on failed/unverified publication.

Independent read-only Sol review found an allocator-padding bug in token verification;
fixed by accepting bounded 16..=64-byte allocations while comparing exactly 16 bytes.
Follow-up review found no remaining concrete defects. Failed-finalization tests cover
both fresh reservations and listener observations before completion. A sequence change
on the read-only verification session's second close remains a theoretical untested
case; no native failure was established.

Two final-build smoke attempts stopped before paste because Windows denied fixture
activation. Harness setup now falls back to a real click only after verifying the
point belongs to the disposable target. Product focus/input guards are unchanged.
The following complete run passed without deferring any paste assertion.

Evidence outside Git: `stage-8-kilo-tests.log`, `stage-8-kilo-transfer-tests.log`,
`stage-8-native-result.json`, `stage-8-native-panel.png`, and isolated profile
`stage-8-acceptance-14ba90fe4cb647fba3747779ce54c137`, under
`C:/Users/UseR/orca/workspaces/daijobu-island`. Native JSON records `Passed=true` and
`ExactExternalPaste=true`. Disposable history was intentionally cleared by acceptance.

## How to try

After the coordinator builds and starts the app, open Home → Clipboard, use
Ctrl+Alt+V or choose Clipboard in the tray. Choose Enable history, then copy
labelled synthetic content; the old clipboard value is not imported. Search or
filter, select and Preview; Ctrl+C copies, Enter/double-click pastes, Shift+Enter
pastes plain text where available, and Ctrl+P toggles a pin. Arrow/Page keys move
through results, Tab reaches actions and Alt+1..9 pastes available visible slots.

Use disposable local Notepad, rich-text browser and Paint targets, plus a throwaway
plain island Terminal tab. Never test paste into a live Claude/Codex session.
File history stores paths, not file contents; missing files fail explicit transfer.
Image exports from deliberate drag remain on disk for delayed consumers.

## Remaining limitations

- Native browser/Paint and exact Terminal paste, plain/rich application rendering,
  physical IME/AltGr, held-modifier/focus stress, closed/reused/elevated targets,
  delayed foreign clipboard owners and disk-failure injection were not exercised.
  Synthetic tests cover many of these paths, not their full target-app behaviour.
- Native OLE file/image drop targets, cancellation/retry, actual multi-monitor/DPI
  hardware, complete keyboard/preview walkthrough, source executable exclusion,
  Snap/Terminal copy-once and editor pin preservation remain unverified in this run.
- Raster scaling is not native DPI evidence. No new idle CPU measurement claimed.
- Taskbar versus tray icon clarification remains unanswered. Numeric callback
  timings exclude physical Explorer routing and do not establish cause or fix.

Unsupported bitmap encodings and proprietary app-private clipboard objects are not
promised. HTML/RTF-only objects without Unicode fallback are unsupported. Pins
bypass age/count eviction but remain subject to pin and payload limits; exported
drag PNGs are retained separately. Remaining target-app and hardware checks must
stay explicitly unverified when they cannot be performed.
