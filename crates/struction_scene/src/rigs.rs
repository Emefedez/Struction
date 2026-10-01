//! Rig construction shared by simulation and rendering; no assets or GPU required.
use crate::{Rigs, Shape};
use bevy::prelude::*;
use struction_anim::locomotion::LocomotionParams;
use struction_character::{RigOf, spawn_rig};

/// On a rigged body: the rig drawn for it, and the skeleton it was built from.
#[derive(Component)]
pub struct Figure {
    pub rig: Entity,
    skeleton: String,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneRigSystems;

pub struct SceneRigPlugin;
impl Plugin for SceneRigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Rigs>().add_systems(
            Update,
            (attach_rigs, remove_orphans, sync_targets)
                .chain()
                .in_set(SceneRigSystems),
        );
    }
}

fn remove_orphans(
    mut commands: Commands,
    rigs: Query<(Entity, &RigOf)>,
    bodies: Query<(&Figure, &Shape)>,
) {
    for (entity, owner) in &rigs {
        if bodies.get(owner.0).is_err() {
            commands.entity(entity).despawn();
            if let Ok(mut body) = commands.get_entity(owner.0) {
                body.remove::<Figure>();
            }
        }
    }
}

/// Gives rigged bodies their skeleton, rebuilt when live reload changes it and removed when the
/// shape stops being rigged.
fn attach_rigs(
    mut commands: Commands,
    rigs: Res<Rigs>,
    bodies: Query<(Entity, &Shape, Option<&Figure>), Changed<Shape>>,
) {
    for (body, shape, figure) in &bodies {
        let skeleton = match shape {
            Shape::Rigged { rig, .. } => Some(rig),
            _ => None,
        };
        if let Some(figure) = figure
            && skeleton != Some(&figure.skeleton)
        {
            commands.entity(figure.rig).despawn();
            commands.entity(body).remove::<Figure>();
        } else if figure.is_some() {
            continue;
        }
        let Some(skeleton) = skeleton else {
            continue;
        };
        let Some(built) = rigs.build(skeleton) else {
            let known: Vec<_> = rigs.names().collect();
            warn!(
                "{body}: unknown rig {skeleton:?}, registered: {}",
                known.join(", ")
            );
            continue;
        };
        let rig = match spawn_rig(&mut commands, body, built, LocomotionParams::default()) {
            Ok(rig) => rig,
            Err(error) => {
                warn!("{body}: rig {skeleton:?} cannot animate: {error}");
                continue;
            }
        };
        commands
            .entity(rig)
            .insert(Name::new(format!("{skeleton} rig")));
        commands.entity(body).insert(Figure {
            rig,
            skeleton: skeleton.clone(),
        });
    }
}

fn sync_targets(
    mut commands: Commands,
    defaults: Option<Res<struction_anim::base_pose::BasePoseSet>>,
    bodies: Query<(
        Ref<Figure>,
        Option<Ref<struction_anim::base_pose::PoseTargets>>,
    )>,
    rigs: Query<&struction_anim::rig::Rig>,
    mut removed: RemovedComponents<struction_anim::base_pose::PoseTargets>,
) {
    use struction_anim::base_pose::RigPoseSet;
    for body in removed.read() {
        if let Ok((figure, None)) = bodies.get(body) {
            commands.entity(figure.rig).remove::<RigPoseSet>();
        }
    }
    let Some(defaults) = defaults else {
        return;
    };
    for (figure, targets) in &bodies {
        let Some(targets) = targets else {
            continue;
        };
        if !figure.is_added() && !targets.is_changed() && !defaults.is_changed() {
            continue;
        }
        let Ok(rig) = rigs.get(figure.rig) else {
            continue;
        };
        match targets.resolve(&defaults, &rig.skeleton) {
            Ok(poses) => {
                commands.entity(figure.rig).insert(RigPoseSet(poses));
            }
            Err(error) => {
                warn!("Invalid pose targets: {error}");
            }
        }
    }
}
