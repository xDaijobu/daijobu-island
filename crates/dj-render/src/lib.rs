//! Skia (Ganesh, D3D12) rendering onto a DirectComposition swap chain, plus the
//! painter that draws the island from `dj-core` view state, the Snap overlay, the Snap
//! preview stack (its own transparent window, `previews::PreviewSurface`), and the Tile
//! feature: the island's Tile panel (`tile::paint_tile_panel`) and the zone overlay (its
//! own transparent window, `tile::ZoneOverlay`).

mod fonts;
mod fx;
mod gpu;
mod icons;
mod markup;
mod music;
mod notch;
mod paint;
mod previews;
mod settings;
mod snap;
mod sweep;
mod tile;
mod timer;
mod vitals;

pub use gpu::{RenderError, Renderer};
pub use markup::{render_output, TextMetrics};
pub use paint::{Frame, MusicFrame};
pub use previews::{PreviewBar, PreviewButton, PreviewCard, PreviewScene, PreviewSurface};
pub use snap::{EditScene, SnapFrame, SnapOverlay, SnapScene};
pub use tile::{paint_tile_panel, TilePanel, TileThumb, ZoneOverlay, ZoneScene};
