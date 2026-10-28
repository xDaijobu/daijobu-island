# Snap panel-preservation follow-up

Brief: [`stage-snap-panels-brief.md`](stage-snap-panels-brief.md).

## Delivered

- [x] Hotkey Snap freezes the visible screen without collapsing open island panels or moving focus first. Only the Home Snap tile collapses its launcher before capture.
- [x] Overlay-induced focus loss preserves the terminal and Spotlight. Stale blur events after focus restoration are ignored; normal click-away dismissal remains enabled.
- [x] The visible top-layer island is an explicit window-selection target. The frozen terminal can be selected with a click instead of requiring a manual region drag.
- [x] New screenshot thumbnails wait behind expanded panels. Copy/save/cancel retain the panel; the preview stack still receives completed captures, and thumbnails expire normally under a panel.
- [x] Return the foreground after overlay teardown when appropriate, before encoding/copying can delay restoration. Do not pull focus back after a deliberate switch away. First-frame renderer failure also returns focus; platform code already rejects invalid or hidden return windows.
- [x] Main-terminal key input is suspended while the overlay owns capture. No settings or permanent hide-on-blur behavior changed.
- [x] No new dependency or polling. README updated.

## Verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace --locked`: **1211 passed**, 0 failed. Four new app regressions cover all open panel types, Home-vs-hotkey preparation, temporary/stale/ordinary focus loss and synthetic own-window selection/cancellation. Three older thumbnail-priority tests now require the panel to stay open.
- `cargo build --release --locked --target-dir C:/Users/UseR/orca/projects/daijobu-island/target/stage-t4`: pass. The verified executable was copied into the main checkout's `target/release/daijobu.exe` and restarted.

## Live check (2026-10-28)

- [x] Opened the terminal, triggered Snap through a posted hotkey, waited for the real overlay to become visible, and cancelled through its close message. No cursor movement, agent prompt, clipboard write or screenshot save was used.
- [x] Terminal region width before/after: **776 / 776 device px** (including notch shoulders). Keyboard mode remained enabled and the island became visible again.
- [x] Foreground restored to its previous window; clipboard sequence number unchanged; updated app remained responsive.
- [ ] Manual visual comparison of the frozen image and a completed real copy/save, plus Spotlight and other panels, multiple monitors and DPI changes. Their domain behavior is covered headlessly; those desktop checks remain for the user.

## Run

The updated build is already running from `target/release/daijobu.exe`. Open a terminal tab, press PrtSc or the configured Snap hotkey, then select the terminal window or drag a region. Complete or cancel the capture: the panel should remain open and focused. Normal click-away still hides it when `hide_on_blur` is enabled.
