mod pack;
mod unpack;

pub use pack::{pack, PackOptions};
use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
pub use unpack::{unpack, UnpackOptions};

pub const DEFAULT_PIXI_PACK_VERSION: &str = "1";
pub const PKGS_DIR: &str = "pkgs";
pub const PIXI_PACK_METADATA_PATH: &str = "pixi-pack.json";

/// The metadata for a "pixi-pack".
#[derive(Serialize, Deserialize, Debug)]
pub struct PixiPackMetadata {
    /// The pack format version.
    pub version: String,
    /// The platform the pack was created for.
    pub platform: Platform,
}
