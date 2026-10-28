# Stage 8 brief: Clipboard history

Status: planned, after 7c-2 and 7d. Read the [coordination plan](stage-snap-clipboard-plan.md).
Unlock Clipboard only when the working monitor, storage, UI and paste flow are integrated.
No OCR, cloud sync, uploads or Win+V replacement hook.

## User experience

- Open from **Ctrl+Alt+V**, Home -> Clipboard or the tray. Register through the existing
  hotkey system; on conflict show the unavailable shortcut and keep Home/tray working.
  Do not take Ctrl+Shift+V away from the integrated Terminal.
- First opening offers **Enable history**; it starts recording future clipboard changes
  and persists that choice. Do not import/read the pre-existing clipboard on enable or
  launch. When enabled before, resume monitoring at startup.
- Use a native 400 by 360 logical-pixel island panel: header, search, filter chips, a
  scrollable list of five 44-pixel rows and footer actions. Coordinator updates maximum
  swap-chain sizing once; there is no per-frame swap-chain resize.
- Filters: All, Text, Images, Files, Pinned. URLs and colours appear as text subtypes,
  with domain/colour swatch; neither triggers a network request.
- Newest first; pinned state is visible but does not randomly reorder the current
  search result. Clicking selects, double-click/Enter pastes, Ctrl+C copies, Shift+Enter
  pastes plain text when available. Alt+1..9 addresses visible result slots only.
- Search has Unicode editing, Ctrl+A, normal clipboard paste into the search field and
  backspace/delete. Bare digits type into search. Arrow/Page keys navigate the list;
  Tab reaches action buttons; Ctrl+P toggles pin; Escape closes and restores the previous target.
- Provide Pin/Unpin, Delete, Copy and image/file Drag actions. Deleting affects history
  only, never the current system clipboard or original/exported files.
- A bounded in-panel preview shows the selected image or longer text without a separate
  webview. Hover may show that same preview; keyboard users can open it explicitly.
  Display only plain text in previews, even when HTML/RTF is retained for paste.
- Footer: Pause/Resume history and Clear history. Clear defaults to unpinned items; an
  explicit include-pinned choice and confirmation handles the destructive all-history
  action. Pausing stops capture but retains existing history/search/paste.
- Empty, disabled, paused, loading, no-results, full, corrupt-item and write/paste errors
  each have an honest state. A failed operation never removes the selected item.

## Supported content

Store one logical item per clipboard sequence, with compatible alternate representations.

| Primary type | Capture and replay |
|---|---|
| Text/link/colour | CF_UNICODETEXT; optional validated HTML Format and bounded Rich Text Format alternatives; plain paste publishes only Unicode |
| Image | Prefer registered PNG, otherwise CF_DIBV5/CF_DIB; normalize to checked BGRA + PNG and thumbnail; replay PNG + DIB |
| Files/folders | CF_HDROP path list, preserving order; replay as copy-only and never preserve a cut/move instruction |

Classify in priority order Files -> Image -> Text when several primary forms coexist;
retain only alternatives that describe the selected primary object. Unicode text is
required for a text history item; HTML/RTF-only objects without a safe text fallback are
reported unsupported in this stage. Link detection accepts HTTP(S); colour detection
accepts standalone #RGB, #RGBA, #RRGGBB or #RRGGBBAA. Classification never executes content.

Handle ordinary PNG and uncompressed/bitfield 24/32-bit DIB/DIBV5, checked stride/masks/
orientation, alpha and malformed dimensions. Unsupported bitmap encodings fail cleanly;
do not claim every proprietary Photoshop/Figma clipboard object is supported. Read only
known formats, never deserialize arbitrary application-private formats.

Per-item limits: 1 MiB Unicode text, 4 MiB each HTML/RTF, 32 MiB encoded image,
64 MiB decoded pixels, maximum image edge 16,384 pixels, 128 paths and 256 KiB serialized
path text. Aggregate raw snapshot limit is 64 MiB. Refuse oversize content without
truncating a saved/pasted payload or freezing the UI. A skipped item gets aggregate
status feedback without retaining its content.

File entries store paths, not file contents. Do not open files, probe UNC shares or
read thumbnails merely to build a history row. Missing files are checked on an explicit
transfer; a missing member fails that transfer instead of silently pasting a subset.

## Monitoring and privacy

- Add dj-clip as a platform/storage crate only when Stage 8 starts. Use a dedicated
  message-window thread with AddClipboardFormatListener / WM_CLIPBOARDUPDATE; no polling
  the clipboard on a timer. Slow/delayed native reads cannot block the island UI thread.
- Copy bounded bytes while the clipboard is open, close promptly on every exit, then
  normalize/encrypt/store off the UI thread. Never keep a clipboard-owned handle.
- Use a sequence token and capture generation before/after acquisition; reject an
  incoherent snapshot. Retry transient OpenClipboard contention finitely using events/
  timers on the worker, not sleeps in the UI path. Do not record old data as a new copy.
- Inspect exclusion formats before reading payloads. Skip when
  ExcludeClipboardContentFromMonitorProcessing is present, or a valid
  CanIncludeInClipboardHistory DWORD is zero. Treat malformed explicit privacy flags
  conservatively. Preserve CanUploadToCloudClipboard=0 when republishing retained items;
  that flag alone is not an instruction to omit local history.
- Support an excluded-source executable list in config. Resolve source identity from
  clipboard ownership when possible; do not collect window titles or guess a source
  from whatever app currently has focus.
- Pause/disable/clear increments a generation and invalidates pending work before it
  can become visible or commit to storage. Re-enabling starts from future events.
- Use a private operation token/sequence to suppress Clipboard's own replay duplicates.
  Do not ignore every Daijobu-owned write: new Snap clipboard copies and Terminal copies are
  valid history input once, subject to the same privacy policy.
- Bounded queues and in-flight byte budgets; when overloaded, coalesce pending updates
  to the latest sequence and report skipped captures. Never grow an unbounded raw queue.
- Do not log payloads, text previews, paths, HTML/RTF, URLs, image bytes or encrypted
  blobs. Diagnostics carry operation IDs, sizes and error categories only.

## Persistence and limits

- SQLite through a locked compatible rusqlite dependency, under
  ~/.daijobu/clipboard/history.sqlite3. Use a versioned schema and one store worker.
- Encrypt payloads, preview/search text, source metadata, paths and thumbnail blobs with
  current-user DPAPI before they reach SQLite; no plaintext FTS table, sidecar thumbnail,
  temp PNG, journal or WAL content. IDs/timestamps/type/pin flags/size may remain metadata.
  Search uses a bounded decrypted metadata index in RAM.
- Non-interactive DPAPI with user scope; never fall back to plaintext after encryption/
  decryption errors. Pin/unpin/delete commands receive an acknowledged transactional
  result before UI claims they are persisted.
- Deduplicate exact logical payloads including rich alternatives; keep stable IDs and
  pin state when promoting a repeat. Hashes used for dedupe stay in RAM, with a byte
  equality check for a candidate; no unsalted content hashes persisted in plaintext.
- Keep at most 500 unpinned records and 30 days of unpinned history. Pins bypass those
  two eviction rules but are capped at 100. Total encrypted payload budget is 512 MiB;
  evict oldest unpinned first, otherwise refuse the new item with History full.
  Budget is for stored payloads; SQLite/WAL overhead is measured/reported separately.
- Enforce limits in the insertion transaction. Prune on load/write and a coarse expiry
  deadline; never redraw or poll continuously while idle. Selected items use stable
  IDs and owned payload leases so pruning cannot paste a different row.
- Bounded decrypted preview/image cache (128 MiB); evict unused cache entries, never
  history records, merely to render a new preview.
- Crash-safe insert/replace/delete, rollback on failure, orderly pending-write shutdown
  with a bound, corrupt row handling and unsupported schema detection. Do not silently
  reset/delete a damaged or newer database.
- The listener and store must shut down without hanging the UI. A delayed third-party
  clipboard read may outlive a shutdown deadline; do not join it indefinitely or use
  unsafe thread termination. Its generation can no longer commit or post to a dead HWND.

## Copy, paste and drag

- Capture the target before opening the panel: external HWND plus process identity, or
  an exact live island Terminal TabId. Opening from Home/tray reuses the last valid
  non-Clipboard target; if none exists, Copy works and Paste reports no target.
- Publish prepared formats in one clipboard session. Allocate/validate before clearing
  existing content. Close on every path; accurately report partial publication errors.
  Never auto-paste after a clipboard write failure.
- External paste hides the panel, restores the intended window and checks foreground
  identity again immediately before sending Ctrl+V. Wait boundedly for physical hotkey/
  selection modifiers to be released; if focus/keys remain unsuitable, leave the item
  copied and show manual-paste feedback. Never send Enter.
- If the target closes, changes process identity, cannot activate or is elevated beyond
  our input rights, keep the item copied and report that automatic paste was not sent.
  No privilege escalation or foreground-attachment workaround.
- An internal Terminal target uses its existing paste path and bracketed-paste handling,
  not global SendInput to the island. Restore the exact tab/panel first; a closed tab
  fails safely. Image-only clipboard items offer Copy, not a claimed terminal image paste.
  File paths follow the terminal's existing file-paste semantics.
- After publishing Clipboard content, retain it as the system clipboard value; do not
  restore the old value on a guessed delay that may race the target's read.
- Image drag explicitly exports a collision-safe PNG into Pictures/Daijobu/Clipboard
  and reuses the 7c-2 OLE transport. Explain the destination in its action tooltip.
  Original file-list drags use their paths with copy-only effects. Text drag, multi-row
  bundles and virtual files are outside this stage.
- Retain exported PNGs after drop/cancel/history deletion so delayed consumers work.
  No plain PNG is written merely to capture, store, pin or preview a history item.

## Automated acceptance

- Model/search/filter/selection tests with Unicode, repeated entries, URL/colour
  classification, exact rich alternatives, stale IDs and all keyboard shortcuts.
- Synthetic clipboard adapters: flags before payload reads, busy retries, sequence races,
  self-replay versus Snap capture, pause/clear while processing, queue/byte overload and
  shutdown during blocked reads.
- Format fixtures: PNG/DIB orientation and alpha, malformed headers/offsets/masks/overflow,
  byte limits, double-NUL CF_HDROP, invalid paths, HTML UTF-8 offsets and text fallback.
- Temporary DB/fake crypto: encrypted bytes only, no plaintext WAL/index/thumbnail,
  restart pins, transactional failure/rollback, dedupe, TTL/count/byte limits, all-pinned
  refusal, corrupt row/newer schema and invalidated-generation writes.
- Fake focus/input/clipboard: valid paste, focus stolen, destroyed/reused HWND, held
  modifiers, partial publication, partial SendInput, internal Terminal target removed,
  bracketed text paste and no injected Enter. Reuse 7c-2 file-drag tests.
- Raster panel tests all themes and 100/150/200% DPI: long text, emoji fallback, image
  aspect ratio, list scrolling, focus/selection, empty/error and in-panel previews.
- The normal test suite does not inspect/write the user's clipboard, start a listener,
  register hooks, capture a screen, send OS input or open visible windows.

## Desktop acceptance for the coordinator

Use labelled synthetic text, rich text, images and temporary files, with the user aware
that copy/paste tests change the clipboard. Never paste into a live Codex/Claude session.

1. Enable, copy each supported type, search/filter, navigate/scroll, preview and copy.
   Verify one logical item per copy and usable empty/paused states.
2. Paste into a disposable Notepad document, a local rich-text browser fixture and Paint;
   Shift+Enter removes rich formats. Test a throwaway plain Terminal tab separately.
3. Pin entries, quit/restart and confirm exact restored payloads/pins. Exercise unpin,
   delete, clear-unpinned and explicitly clear-all without changing original files.
4. Synthetic exclusion flags and app exclusion produce no row; inspect the disposable
   DB/side files for absence of known plaintext markers. Verify native DPAPI round-trip
   with synthetic data and reject undecryptable/corrupt fixture content.
5. Clipboard contention, modifier hold, stolen focus, closed/elevated target and disk
   failure produce honest feedback without unwanted typing or lost history.
6. Drag a history image/file list to a local target, cancel and retry; verify stable
  files and copy-only effects. New Snap clipboard copies enter history once; pinning
  directly from the editor leaves the clipboard and history unchanged.
7. Snap while Clipboard/Terminal is open preserves the established panel behaviour;
   screen pins remain excluded. Check themes, DPI/monitor changes and idle redraw/CPU.

Finish with stage-8-report.md, standard workspace checks, the feature unlock and
README/progress updates in its PR. List unperformed target-app checks honestly.
