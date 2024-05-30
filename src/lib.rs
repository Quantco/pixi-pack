mod pack;
mod unpack;

pub use pack::{pack, PackOptions};
use serde::{Deserialize, Serialize};
pub use unpack::{unpack, UnpackOptions};

/// The metadata for a "pixi-pack".
#[derive(Serialize, Deserialize)]
pub struct PixiPackMetadata {
    /// The pack format version.
    pub version: String,
}
