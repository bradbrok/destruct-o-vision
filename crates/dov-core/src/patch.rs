//! Patch files: which modules are in the rack, where, with what settings,
//! and how they are cabled. Ports and params are referenced by id so patches
//! survive changes to module layouts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModuleId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortAddr {
    pub module: ModuleId,
    pub port: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cable {
    /// An output port.
    pub from: PortAddr,
    /// An input port.
    pub to: PortAddr,
    /// Index into the UI's cable palette.
    #[serde(default)]
    pub color: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleState {
    pub id: ModuleId,
    /// Module type id, e.g. `dov.vco`.
    pub kind: String,
    /// Rack position: (row, column in HP).
    pub pos: (i32, i32),
    /// Param and attenuverter values by id (attenuverters as `<param>_cv`).
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    /// The text field, for modules that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    #[serde(default = "Patch::current_version")]
    pub version: u32,
    #[serde(default)]
    pub raster: Option<String>,
    /// Packages the patch's modules come from: package id to version.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, String>,
    pub modules: Vec<ModuleState>,
    pub cables: Vec<Cable>,
}

impl Patch {
    pub const VERSION: u32 = 1;

    fn current_version() -> u32 {
        Self::VERSION
    }

    pub fn to_ron(&self) -> Result<String, ron::Error> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
    }

    pub fn from_ron(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_ron() {
        let patch = Patch {
            version: Patch::VERSION,
            raster: Some("NTSC 480i".into()),
            requires: [("alice.vhs-pack".to_string(), "1.2.0".to_string())].into(),
            modules: vec![ModuleState {
                id: ModuleId(1),
                kind: "dov.vco".into(),
                pos: (0, 4),
                params: [("multiple".to_string(), 8.0)].into(),
                text: Some("HELLO".into()),
            }],
            cables: vec![Cable {
                from: PortAddr {
                    module: ModuleId(1),
                    port: "out".into(),
                },
                to: PortAddr {
                    module: ModuleId(2),
                    port: "sig".into(),
                },
                color: 3,
            }],
        };
        let text = patch.to_ron().unwrap();
        assert_eq!(Patch::from_ron(&text).unwrap(), patch);
    }
}
