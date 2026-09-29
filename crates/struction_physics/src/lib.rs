//! Physics package: Avian wrapped with fixed-timestep simulation, render interpolation, and
//! gravity that comes only from summed [`struction_gravity`] fields.
//!
//! Everything is composition on ordinary components and Avian colliders:
//! - [`Surface`] tunes friction, bounce, and drag of what bodies touch.
//! - [`Volume`] is a sensor region; [`Buoyancy`], [`VolumeDrag`], and [`DamageField`] make it
//!   water or lava (see [`water`] and [`lava`]).
//! - [`CameraZone`] is a volume carrying camera constraint data; [`CameraTarget`] entities learn
//!   which zone they are in.
//!
//! The simulation runs in `FixedPostUpdate` (60 Hz by default), reads only physics state, and
//! interpolates `Transform` for rendering. It needs no window, GPU, or asset plugins: add
//! `MinimalPlugins` and `TransformPlugin` (see [`testing`]).

use avian3d::{physics_transform::PhysicsTransformSystems, prelude::*};
use bevy::{ecs::schedule::IntoScheduleConfigs, prelude::*};
use struction_gravity::{GravityPlugin, GravitySystems};

mod camera_zone;
mod gravity;
mod surface;
pub mod testing;
mod volume;

pub use avian3d;
pub use camera_zone::{CameraConstraint, CameraMode, CameraTarget, CameraZone, InCameraZones};
pub use surface::Surface;
pub use volume::{
    Buoyancy, DamageField, Submersion, Volume, VolumeDamage, VolumeDrag, VolumeShape, lava, water,
};

pub mod prelude {
    pub use crate::{
        Buoyancy, CameraConstraint, CameraMode, CameraTarget, CameraZone, DamageField,
        EnvironmentSystems, InCameraZones, PhysicsPlugin, Submersion, Surface, Volume,
        VolumeDamage, VolumeDrag, VolumeShape, lava, water,
    };
    pub use struction_gravity::prelude::*;
}

/// Simulation frequency unless configured otherwise.
pub const DEFAULT_TICK_HZ: f64 = 60.0;

/// Ordering of the environment systems that feed forces into a physics step. They run in
/// `FixedPostUpdate` inside [`PhysicsSystems::Prepare`], so anything that reads their results
/// (character control) should run `.after` the last one.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnvironmentSystems {
    /// Applies [`LocalGravity`](struction_gravity::LocalGravity) to bodies as acceleration.
    ApplyGravity,
    /// Applies surface drag, buoyancy, and volume drag; emits [`VolumeDamage`].
    Effects,
}

/// Adds Avian, gravity fields, and the packages in this crate.
pub struct PhysicsPlugin {
    /// Fixed simulation frequency.
    pub tick_hz: f64,
}

impl Default for PhysicsPlugin {
    fn default() -> Self {
        Self {
            tick_hz: DEFAULT_TICK_HZ,
        }
    }
}

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(self.tick_hz))
            // Gravity comes only from fields, applied per body below.
            .insert_resource(Gravity::ZERO)
            .add_plugins((
                PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
                GravityPlugin::default(),
            ));

        // Every body senses gravity and fluids; only dynamic ones are affected by them.
        app.register_required_components::<RigidBody, struction_gravity::LocalGravity>();
        app.register_required_components::<RigidBody, Submersion>();

        app.configure_sets(
            FixedPostUpdate,
            (
                GravitySystems::Update,
                EnvironmentSystems::ApplyGravity,
                EnvironmentSystems::Effects,
            )
                .chain()
                .in_set(PhysicsSystems::Prepare)
                .after(PhysicsTransformSystems::TransformToPosition),
        );

        app.add_plugins((
            surface::plugin,
            volume::plugin,
            camera_zone::plugin,
            gravity::plugin,
        ));
    }
}
