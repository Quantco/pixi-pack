mod build_context;
mod pack;
mod unpack;
mod util;

pub use pack::{PackOptions, pack};
use rattler_conda_types::Platform;
use serde::{Deserialize, Serialize};
pub use unpack::{UnpackOptions, unarchive, unpack};
pub use util::{ProgressReporter, get_size};

pub const CHANNEL_DIRECTORY_NAME: &str = "channel";
pub const PYPI_DIRECTORY_NAME: &str = "pypi";
pub const PIXI_PACK_METADATA_PATH: &str = "pixi-pack.json";
pub const DEFAULT_PIXI_PACK_VERSION: &str = "1";
pub const PIXI_PACK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The metadata for a "pixi-pack".
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PixiPackMetadata {
    /// The pack format version.
    pub version: String,
    /// The version of pixi-pack that created the pack.
    pub pixi_pack_version: Option<String>,
    /// The platform the pack was created for.
    pub platform: Platform,
}

impl Default for PixiPackMetadata {
    fn default() -> Self {
        Self {
            version: DEFAULT_PIXI_PACK_VERSION.to_string(),
            pixi_pack_version: Some(PIXI_PACK_VERSION.to_string()),
            platform: Platform::current(),
        }
    }
}

/// The configuration type for pixi-pack - just extends rattler config and can load the same TOML files as pixi.
pub type Config = rattler_config::config::ConfigBase<()>;

/* --------------------------------------------------------------------------------------------- */
/*                                             TESTS                                             */
/* --------------------------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;
    use serde_json::{Value, json};

    #[rstest]
    fn test_metadata_serialization() {
        let metadata = PixiPackMetadata {
            version: DEFAULT_PIXI_PACK_VERSION.to_string(),
            pixi_pack_version: Some(PIXI_PACK_VERSION.to_string()),
            platform: Platform::Linux64,
        };
        let result = json!(metadata).to_string();
        assert_eq!(
            result,
            format!(
                "{{\"version\":\"1\",\"pixi-pack-version\":\"{}\",\"platform\":\"linux-64\"}}",
                PIXI_PACK_VERSION
            )
        );
        assert_eq!(
            serde_json::from_str::<PixiPackMetadata>(&result).unwrap(),
            metadata
        );
    }

    #[test]
    fn test_metadata_serialization_no_pixi_pack_version() {
        let metadata = serde_json::from_str::<PixiPackMetadata>(
            &json!({"version": "1", "platform": "linux-64"}).to_string(),
        );
        assert!(metadata.is_ok());
        let metadata = metadata.unwrap();
        assert_eq!(metadata.version, "1");
        assert!(metadata.pixi_pack_version.is_none());
        assert_eq!(metadata.platform, Platform::Linux64);
    }

    #[rstest]
    #[case(json!({"version": "1", "platform": "linux64"}))]
    #[case(json!({"version": 1.0, "platform": "linux-64"}))]
    #[case(json!({"version": 1, "platform": "linux-64"}))]
    fn test_metadata_serialization_failure(#[case] invalid: Value) {
        assert!(serde_json::from_str::<PixiPackMetadata>(&invalid.to_string()).is_err());
    }
}
