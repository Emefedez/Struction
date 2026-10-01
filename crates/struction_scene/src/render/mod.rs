//! Drawing the scene vocabulary: shapes and looks become meshes and materials, humanoids become
//! rigs dressed with their models, and surfaces that hide the player fade along the camera's
//! sight line. Needs a rendering app (`DefaultPlugins`), the physics, character animation and
//! player camera plugins.

mod figures;
mod looks;
mod sight_fade;

pub use figures::{Dressed, Figure, Models, Piece};
pub use looks::{WaterSurface, shape_mesh};
pub use sight_fade::{FadeMaterial, FadesWith, SightFade, SightUniform, fade_material};

use bevy::prelude::*;
use struction_camera::CameraSystems;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneRenderSystems {
    /// Meshes, materials and rigs for new or edited entities.
    Dress,
    /// Sight-line fading and first-person hiding, once the camera has moved this frame.
    View,
}

pub struct SceneRenderPlugin;

impl Plugin for SceneRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((figures::FiguresPlugin, sight_fade::SightFadePlugin))
            .configure_sets(
                Update,
                (
                    SceneRenderSystems::Dress.before(CameraSystems::Follow),
                    SceneRenderSystems::View.after(CameraSystems::Follow),
                ),
            )
            .add_systems(
                Update,
                (
                    looks::dress_looks,
                    figures::attach_rigs,
                    figures::request_models,
                    figures::finish_compiles,
                    figures::dress_figures,
                )
                    .chain()
                    .in_set(SceneRenderSystems::Dress),
            )
            .add_systems(
                Update,
                (sight_fade::fade_sight_lines, figures::hide_in_first_person)
                    .in_set(SceneRenderSystems::View),
            );
    }
}
