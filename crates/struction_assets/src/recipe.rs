//! Asset recipes: the preparation settings a source compiles with, kept beside it as
//! `<source file name>.recipe.json` (`ogre.blend.recipe.json`) so they survive
//! recompiles and go into version control with the source. A source without a recipe
//! compiles with [`PrepareSettings::default`].
//!
//! Recipes are plain JSON written by the tools (the editor's utilities, the CLI,
//! AI clients through either); unknown fields are errors so typos do not silently
//! fall back to defaults. Presets are starting points, not stored names: a recipe
//! always holds the resulting numbers.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::collision::CollisionSettings;
use crate::compile::PrepareSettings;
use crate::error::AssetError;
use crate::lod::LodSettings;

pub const RECIPE_SUFFIX: &str = ".recipe.json";

/// Upper bound on generated LOD levels; more add nothing a renderer uses.
pub const MAX_LOD_LEVELS: u32 = 8;

/// Where the recipe of `source` lives.
pub fn recipe_path(source: &Path) -> PathBuf {
    let mut name = source.file_name().unwrap_or_default().to_os_string();
    name.push(RECIPE_SUFFIX);
    source.with_file_name(name)
}

/// The recipe of `source`, `None` when it has none.
pub fn read_recipe(source: &Path) -> Result<Option<PrepareSettings>, AssetError> {
    read_recipe_text(source)?
        .map(|text| parse_recipe(source, &text))
        .transpose()
}

pub(crate) fn read_recipe_text(source: &Path) -> Result<Option<String>, AssetError> {
    let path = recipe_path(source);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AssetError::io(path, error)),
    }
}

pub(crate) fn parse_recipe(source: &Path, text: &str) -> Result<PrepareSettings, AssetError> {
    let path = recipe_path(source);
    let settings: PrepareSettings =
        serde_json::from_str(text).map_err(|error| AssetError::Recipe {
            path: path.clone(),
            message: error.to_string(),
        })?;
    let problems = check_settings(&settings);
    if !problems.is_empty() {
        return Err(AssetError::Recipe {
            path,
            message: problems.join("; "),
        });
    }
    Ok(settings)
}

/// The text a recipe with `settings` is written as.
pub fn recipe_json(settings: &PrepareSettings) -> String {
    let mut text = serde_json::to_string_pretty(settings).expect("settings serialize");
    text.push('\n');
    text
}

/// Settings outside the ranges the generators handle, as messages naming the field.
pub fn check_settings(settings: &PrepareSettings) -> Vec<String> {
    let mut problems = Vec::new();
    let mut check = |ok: bool, message: &str| {
        if !ok {
            problems.push(message.to_owned());
        }
    };
    let lod = &settings.lod;
    check(
        lod.levels <= MAX_LOD_LEVELS,
        &format!("lod.levels must be at most {MAX_LOD_LEVELS}"),
    );
    check(
        lod.reduction > 0.0 && lod.reduction < 1.0,
        "lod.reduction must be between 0 and 1 (exclusive)",
    );
    check(
        lod.max_error >= 0.0 && lod.max_error.is_finite(),
        "lod.maxError must be zero or positive",
    );
    let collision = &settings.collision;
    check(
        collision.trimesh_ratio > 0.0 && collision.trimesh_ratio <= 1.0,
        "collision.trimeshRatio must be above 0 and at most 1",
    );
    check(
        collision.trimesh_max_error >= 0.0 && collision.trimesh_max_error.is_finite(),
        "collision.trimeshMaxError must be zero or positive",
    );
    check(
        (1..=256).contains(&collision.max_parts),
        "collision.maxParts must be between 1 and 256",
    );
    check(
        collision.concavity > 0.0 && collision.concavity <= 1.0,
        "collision.concavity must be above 0 and at most 1",
    );
    problems
}

/// Starting points for LOD generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LodPreset {
    /// Only the imported mesh.
    Off,
    Gentle,
    Balanced,
    Aggressive,
}

impl LodPreset {
    pub const ALL: [Self; 4] = [Self::Off, Self::Gentle, Self::Balanced, Self::Aggressive];

    pub fn settings(self) -> LodSettings {
        let (levels, reduction, max_error) = match self {
            Self::Off => (0, 0.5, 0.05),
            Self::Gentle => (2, 0.6, 0.02),
            Self::Balanced => return LodSettings::default(),
            Self::Aggressive => (4, 0.4, 0.1),
        };
        LodSettings {
            levels,
            reduction,
            max_error,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Gentle => "Gentle",
            Self::Balanced => "Balanced",
            Self::Aggressive => "Aggressive",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Off => "No LODs: small or always-close objects",
            Self::Gentle => "Two close-looking levels for hero props and characters",
            Self::Balanced => "Three levels, each about half the previous",
            Self::Aggressive => "Four coarse levels for scenery seen from afar",
        }
    }

    /// The preset these settings equal, if any.
    pub fn matching(settings: &LodSettings) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| {
            let preset = preset.settings();
            // With no levels the other numbers do nothing.
            preset == *settings || (preset.levels == 0 && settings.levels == 0)
        })
    }
}

/// Starting points for collision shapes. Every mesh always gets a convex hull
/// and a simplified trimesh; presets choose their detail and the decomposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollisionPreset {
    SimpleProp,
    Balanced,
    Scenery,
    ConcaveProp,
}

impl CollisionPreset {
    pub const ALL: [Self; 4] = [
        Self::SimpleProp,
        Self::Balanced,
        Self::Scenery,
        Self::ConcaveProp,
    ];

    pub fn settings(self) -> CollisionSettings {
        let base = CollisionSettings::default();
        match self {
            Self::SimpleProp => CollisionSettings {
                trimesh_ratio: 0.1,
                trimesh_max_error: 0.05,
                ..base
            },
            Self::Balanced => base,
            Self::Scenery => CollisionSettings {
                trimesh_ratio: 0.5,
                trimesh_max_error: 0.005,
                ..base
            },
            Self::ConcaveProp => CollisionSettings {
                convex_decomposition: true,
                ..base
            },
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SimpleProp => "Simple prop",
            Self::Balanced => "Balanced",
            Self::Scenery => "Static scenery",
            Self::ConcaveProp => "Concave prop",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::SimpleProp => "Convex hull for movable props, coarse trimesh",
            Self::Balanced => "Hull and a trimesh at a quarter of the triangles",
            Self::Scenery => "Close trimesh for static ground, walls and stairs",
            Self::ConcaveProp => "Convex parts so concave props can move and tumble",
        }
    }

    pub fn matching(settings: &CollisionSettings) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.settings() == *settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_sit_beside_their_source() {
        assert_eq!(
            recipe_path(Path::new("models/ogre.blend")),
            Path::new("models/ogre.blend.recipe.json")
        );
    }

    #[test]
    fn recipes_round_trip_and_fill_defaults() {
        let settings = PrepareSettings {
            lod: LodPreset::Aggressive.settings(),
            collision: CollisionPreset::ConcaveProp.settings(),
            ..PrepareSettings::default()
        };
        let text = recipe_json(&settings);
        assert!(text.contains("\"maxError\""), "{text}");
        assert_eq!(parse_recipe(Path::new("a.glb"), &text).unwrap(), settings);

        let partial = r#"{ "lod": { "levels": 1 } }"#;
        let parsed = parse_recipe(Path::new("a.glb"), partial).unwrap();
        assert_eq!(parsed.lod.levels, 1);
        assert_eq!(parsed.lod.reduction, LodSettings::default().reduction);
        assert_eq!(parsed.collision, CollisionSettings::default());
    }

    #[test]
    fn bad_recipes_name_the_file_and_the_problem() {
        let typo = parse_recipe(Path::new("m/a.glb"), r#"{ "lod": { "level": 1 } }"#)
            .unwrap_err()
            .to_string();
        assert!(typo.starts_with("m/a.glb.recipe.json: "), "{typo}");
        assert!(typo.contains("level") && typo.contains("line 1"), "{typo}");

        let range = parse_recipe(Path::new("a.glb"), r#"{ "collision": { "maxParts": 0 } }"#)
            .unwrap_err()
            .to_string();
        assert!(range.contains("collision.maxParts"), "{range}");
    }

    #[test]
    fn presets_are_valid_and_recognized() {
        for preset in LodPreset::ALL {
            let settings = PrepareSettings {
                lod: preset.settings(),
                ..Default::default()
            };
            assert!(check_settings(&settings).is_empty(), "{preset:?}");
            assert_eq!(LodPreset::matching(&preset.settings()), Some(preset));
        }
        for preset in CollisionPreset::ALL {
            let settings = PrepareSettings {
                collision: preset.settings(),
                ..Default::default()
            };
            assert!(check_settings(&settings).is_empty(), "{preset:?}");
            assert_eq!(CollisionPreset::matching(&preset.settings()), Some(preset));
        }
        let custom = CollisionSettings {
            trimesh_ratio: 0.33,
            ..Default::default()
        };
        assert_eq!(CollisionPreset::matching(&custom), None);
        assert_eq!(
            LodPreset::matching(&LodSettings {
                levels: 0,
                reduction: 0.3,
                max_error: 0.0
            }),
            Some(LodPreset::Off)
        );
    }
}
