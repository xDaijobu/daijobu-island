# Stage N report: Notch style

Built on `xDaijobu/stage-n-notch` from `master` (`9d022aa`), in parallel with Stage 7b. The coordinator wrote the shared skeleton (`d74d2bc`), and four Opus subagents filled it in, each owning separate files. The coordinator then integrated, verified and live-tested it with the user.

## Verdict
Every scope item in `task/progress.md` → "Stage N: Notch style" is done and live-tested. The notch is the default, and pill style renders as before.

Verification:
- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: **328 passed** (Stage S ended at 303).
- `cargo build --release`: OK.

## How it works
- **Config:** `dj_core::config::IslandStyle { Pill, Notch }` and `[island] style`, read through `IslandConfig::style()`. An unknown value falls back to notch.
- **Shape:** `dj_render::notch`.
  - `body_path` gives the pill (`Path::rrect`, the same rounded rect as before) or the notch: one closed path along y = 0, with conic quarter-circle shoulders (`NOTCH_SHOULDER` = 8 px × scale), straight sides and the view's rounded bottom corners. It is clamped so every morph frame is valid.
  - `body_outline` is the hairline: open at the top, 0.5 px inside the body.
  - `Painter::paint` fills and clips with the path. Glow and grain cover the path's bounds, and the content layers still use the pill rect.
- **Layout:** the pill rect, content and hit testing are unchanged. `layout::notch_bounds(pill)` is the box the notch covers.
  - The window is 536 × 348 (was 520 × 348), so Home's shoulders fit. Everything is placed relative to `window_size()`, so nothing moves on screen.
  - The Settings panel is 400 × 220 (was 184), with the new `ST_ROW_STYLE` / `ST_STYLE` row.
- **App:** `app/src/notch.rs`.
  - The window region is the pill's column from the top edge plus a shoulder strip (`dj_win::window::set_hit_rects`, via `CombineRgn`), so the space beside the pill below the shoulders stays click-through.
  - `to_pill` maps a click, hover or drag on the neck or a shoulder to the nearest point of the pill.
  - `SettingsAction::ToggleStyle` saves the config and redraws at once.

## Tests added (25)
- **dj-core:**
  - style names and defaults, and reading `[island]`
  - `notch_bounds`, and that every view's notch fits the window
  - the style row lines up with the rows above
  - the window still holds the taller Settings panel
  - the Settings hit test for `ST_STYLE`
  - island click → `ToggleStyle`
  - the config round trip toggles the style
- **dj-render:**
  - the notch's flat top and concave shoulders at scale 1 and 2
  - the rounded bottom corners
  - the open, inset outline
  - pill style is the same rounded rect, and draws byte-identical CPU pixels to the old code
  - the notch contains the pill
  - the fill reaches the top row
  - degenerate morph frames
- **app:**
  - the body column and the shoulder strip
  - device edges
  - the neck and shoulders map into the pill
  - points beside the pill below the shoulders pass through

## Live test (2026-10-26, user's machine, 3440×1440, scale 1.0)
| Check | Result |
|---|---|
| Idle notch flush with the top edge, concave shoulders, rounded bottom, grain over the whole body | Pass (coordinator, small crops, deleted after) |
| Home → gear → Settings grows down and stays attached; the ISLAND STYLE row shows `notch` | Pass (posted clicks) |
| The chip toggles notch → pill → notch live; `config.toml` gets `[island] style` | Pass |
| Real mouse: a click on the top edge opens the island; a click beside the panel goes through; themes | Pass (user: "notch oke") |

## Not verified
- Mixed DPI and multi-monitor (the island lives on the primary monitor only, as before).
- The GPU drawing of the pill path matching the old rrect exactly. It is checked on the CPU only; Skia should treat an rrect path the same way.

## Known issues
- The region's shoulder strip is a rectangle, so the few pixels just outside each concave curve are clickable and act on the pill's top corner.
