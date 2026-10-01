//! Drawing the scene vocabulary: shapes and looks become meshes and materials, humanoids become
//! rigs dressed with their models, and surfaces that hide the player fade along the camera's
//! sight line. Needs a rendering app (`DefaultPlugins`, preceded by [`EngineAssetsPlugin`]), the
//! physics, character animation and player camera plugins.

mod figures;
mod looks;
mod sight_fade;

pub use crate::Figure;
pub use figures::{Dressed, Models, Piece};
pub use looks::{WaterSurface, shape_mesh};
pub use sight_fade::{FadeMaterial, FadesWith, SightFade, SightUniform, fade_material};

use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;
use struction_camera::CameraSystems;

use crate::engine_assets;

/// Registers the engine's assets as the `engine://` asset source. Add it before `DefaultPlugins`:
/// Bevy only accepts sources registered before its `AssetPlugin`.
pub struct EngineAssetsPlugin;

impl Plugin for EngineAssetsPlugin {
    fn build(&self, app: &mut App) {
        let path = engine_assets().to_string_lossy().into_owned();
        app.register_asset_source("engine", AssetSourceBuilder::platform_default(&path, None));
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneRenderSystems {
    /// Meshes, materials and rigs for new or edited entities.
    Dress,
    /// Sight-line fading and first-person hiding, once the camera has moved this frame.
    View,
}

/// Geometry and model drawing without physics, input, camera behavior or animation stepping.
/// A viewer may supply transforms and solved joint poses from another world.
pub struct SceneVisualsPlugin;

impl Plugin for SceneVisualsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::SceneRigPlugin>() {
            app.add_plugins(crate::SceneRigPlugin);
        }
        app.add_plugins((figures::FiguresPlugin, sight_fade::SightFadePlugin))
            .configure_sets(
                Update,
                SceneRenderSystems::Dress.after(crate::SceneRigSystems),
            )
            .add_systems(
                Update,
                (
                    figures::rig_visibility,
                    looks::dress_looks,
                    figures::request_models,
                    figures::finish_compiles,
                    figures::dress_figures,
                )
                    .chain()
                    .in_set(SceneRenderSystems::Dress),
            );
    }
}

/// Gameplay presentation adds camera-dependent fading and first-person hiding to the visuals.
pub struct SceneRenderPlugin;
impl Plugin for SceneRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SceneVisualsPlugin)
            .configure_sets(
                Update,
                (
                    SceneRenderSystems::Dress.before(CameraSystems::Follow),
                    SceneRenderSystems::View.after(CameraSystems::Follow),
                ),
            )
            .add_systems(
                Update,
                (sight_fade::fade_sight_lines, figures::hide_in_first_person)
                    .in_set(SceneRenderSystems::View),
            );
    }
}
