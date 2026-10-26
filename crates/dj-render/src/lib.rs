//! Skia (Ganesh, D3D12) rendering onto a DirectComposition swap chain, plus the
//! painter that draws the island from `dj-core` view state, the Snap overlay, and the Snap
//! preview stack (its own transparent window, `previews::PreviewSurface`).

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
mod timer;
mod vitals;

pub use gpu::{RenderError, Renderer};
pub use markup::{render_output, TextMetrics};
pub use paint::{Frame, MusicFrame};
pub use previews::{PreviewBar, PreviewButton, PreviewCard, PreviewScene, PreviewSurface};
pub use snap::{EditScene, SnapFrame, SnapOverlay, SnapScene};
