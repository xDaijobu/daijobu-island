# Terminal tab rename report

Brief: [stage-terminal-rename-brief.md](stage-terminal-rename-brief.md).

## Delivered

- [x] Rename tab... appended to each tab's context menu. Applies to the chosen tab, including background, pinned and exited tabs, without changing the active session.
- [x] Skia modal editor with current name selected initially; Enter/Save commits, Escape/Cancel/outside click cancels. Empty/whitespace resets automatic naming. Hidden panels discard drafts.
- [x] Unicode-safe editing with existing 256-character LineEdit, arrows/Home/End, Backspace/Delete, Ctrl+A, Ctrl+C/X/V. Modal keys, clicks, selection, wheel and tab shortcuts never reach the shell; Enter's following WM_CHAR is consumed after the dialog closes.
- [x] Separate custom title from the shell's automatic title. Shell title/reset events and process restart retain the override; clearing it returns to the latest automatic title.
- [x] Pin metadata includes optional custom_title; older TOML files remain automatic and serialize without the extra field. Pinning later also saves an existing override. No dependencies, new unsafe code or polling.
- [x] README documents the controls.

## Verification

- fmt and workspace clippy, all targets with warnings denied: pass.
- New controller regressions cover background targeting/no process or keyboard side effects, automatic-title reset, pinned override round-trip/process restart, all cancel paths/input isolation, Unicode paste and exited-tab editing.
- One new core test checks custom-title serialization and old files; one raster test checks long-name clipping, caret/selection/placeholder differences and panel containment across all four themes and existing DPI scales.
- Full workspace tests were attempted. The existing tests::conpty_delivers_wheel_to_a_mouse_tracking_program is inconsistent on this host: its isolated run passed, while later full runs and a serial full run timed out after WHEEL-READY. This PR changes no production wheel/ConPTY code. An experimental test handshake was reverted; the existing test is unchanged.
- Workspace tests excluding only that exact existing host test: **1230 passed**, 0 failed, 1 filtered out. The unfiltered suite is not green; the known failure is recorded above.
- Locked release build: pass. The hash-verified executable was copied to the main checkout target/release/daijobu.exe and restarted (PID 6536).
- Desktop: opened Rename tab... through posted Win32 clicks and visually inspected the real editor. The user confirmed it works ("sip ud bisa"). A later synthetic save/pin assertion did not complete during interactive use, so that check is not claimed; the pins file remained empty. Persistence is covered by the controller/core regressions. No clipboard access or real cursor movement in the desktop check.

## Run and limits

Right-click a tab, choose Rename tab..., type a name and press Enter. Clear the name to follow the application again; pin the tab to persist the override across app restarts.

The field supports keyboard editing and select-all, not arbitrary mouse selection or IME composition. Rendered glyph coverage follows existing UI fonts. No actual agent prompt is needed for verification.
