//! Skia (Ganesh, D3D12) rendering onto a DirectComposition swap chain, plus the
//! painter that draws the island from `dj-core` view state.

mod fonts;
mod fx;
mod gpu;
mod icons;
mod paint;
mod vitals;

pub use gpu::{RenderError, Renderer};
pub use paint::Frame;
