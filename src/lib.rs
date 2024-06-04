mod pack;
mod unpack;

pub use pack::{pack, PackOptions};
use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
pub use unpack::{unarchive, unpack, UnpackOptions};

pub const CHANNEL_DIRECTORY_NAME: &str = "channel";
pub const PIXI_PACK_METADATA_PATH: &str = "pixi-pack.json";
pub const DEFAULT_PIXI_PACK_VERSION: &str = "1";

/// The metadata for a "pixi-pack".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PixiPackMetadata {
    /// The pack format version.
    pub version: String,
    /// The platform the pack was created for.
    pub platform: Platform,
}

impl Default for PixiPackMetadata {
    fn default() -> Self {
        Self {
            version: DEFAULT_PIXI_PACK_VERSION.to_string(),
            platform: Platform::current(),
        }
    }
}

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;
    use serde_json::{json, Value};

    #[rstest]
    fn test_metadata_serialization() {
        let metadata = PixiPackMetadata {
            version: DEFAULT_PIXI_PACK_VERSION.to_string(),
            platform: Platform::Linux64,
        };
        let result = json!(metadata).to_string();
        assert_eq!(result, "{\"version\":\"1\",\"platform\":\"linux-64\"}");
        assert_eq!(
            serde_json::from_str::<PixiPackMetadata>(&result).unwrap(),
            metadata
        );
    }

    #[rstest]
    #[case(json!({"version": "1", "platform": "linux64"}))]
    #[case(json!({"version": 1.0, "platform": "linux-64"}))]
    #[case(json!({"version": 1, "platform": "linux-64"}))]
    fn test_metadata_serialization_failure(#[case] invalid: Value) {
        assert!(serde_json::from_str::<PixiPackMetadata>(&invalid.to_string()).is_err());
    }
}
