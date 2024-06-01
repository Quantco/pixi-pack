mod pack;
mod unpack;

pub use pack::{pack, PackError, PackOptions};
use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
pub use unpack::{unpack, UnpackError, UnpackOptions};

const CHANNEL_DIRECTORY_NAME: &str = "channel";
pub const DEFAULT_PIXI_PACK_VERSION: &str = "1";

/// The metadata for a "pixi-pack".
#[derive(Serialize, Deserialize, Debug)]
pub struct PixiPackMetadata {
    /// The pack format version.
    pub version: String,
    /// The platform the pack was created for.
    pub platform: Platform,
}
