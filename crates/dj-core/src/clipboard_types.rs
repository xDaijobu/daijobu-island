//! Owned Clipboard data; platform-free and safe to encrypt before persistence.
use serde::{Deserialize, Serialize};

pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_RICH_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_PIXELS_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_IMAGE_EDGE: u32 = 16_384;
pub const MAX_PATHS: usize = 128;
pub const MAX_PATH_BYTES: usize = 256 * 1024;
pub const MAX_RAW_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RAM_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_STORED_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_UNPINNED: usize = 500;
pub const MAX_PINNED: usize = 100;
pub const TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct ItemId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Text,
    Link,
    Colour,
    Image,
    Files,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Filter {
    #[default]
    All,
    Text,
    Images,
    Files,
    Pinned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    /// Top-down tightly packed BGRA, straight alpha.
    pub bgra: Vec<u8>,
    pub png: Vec<u8>,
    /// At most 160x90, preserving aspect ratio.
    pub thumbnail: Thumbnail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Content {
    Text {
        text: String,
        html: Option<Vec<u8>>,
        rtf: Option<Vec<u8>>,
    },
    Image(ImageData),
    Files(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub content: Content,
    pub no_cloud: bool,
}

/// Bounded display metadata. Sensitive fields are encrypted at rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub id: ItemId,
    pub created_ms: u64,
    pub used_ms: u64,
    pub pinned: bool,
    pub kind: Kind,
    pub preview: String,
    pub source: Option<String>,
    pub thumbnail: Option<Thumbnail>,
    pub bytes: u64,
    pub corrupt: bool,
}
