//! Skia (Ganesh, D3D12) rendering onto a DirectComposition swap chain, plus the
//! painter that draws the island from `dj-core` view state, and the Snap overlay.

mod fonts;
mod fx;
mod gpu;
mod icons;
mod music;
mod notch;
mod paint;
mod settings;
mod snap;
mod sweep;
mod timer;
mod vitals;

pub use gpu::{RenderError, Renderer};
pub use paint::{Frame, MusicFrame};
pub use snap::{SnapFrame, SnapOverlay, SnapScene};
