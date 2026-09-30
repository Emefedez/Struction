//! Bevy integration: components, explicit system sets and the plugin.
//!
//! Animation is presentation. Everything runs in `PostUpdate`, so after the fixed-timestep
//! physics loop and before transform propagation; other crates order themselves against
//! [`AnimSystems`]. Rig joints are child entities mirroring the skeleton, so attachments and
//! renderers use ordinary transform hierarchy.

use bevy::app::{App, Plugin, PostUpdate};
use bevy::ecs::change_detection::DetectChangesMut;
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::name::Name;
use bevy::ecs::query::{With, Without};
use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::{IntoScheduleConfigs, SystemSet};
use bevy::ecs::system::{Commands, Query, Res, SystemParam};
use bevy::log::error;
use bevy::math::Vec3;
use bevy::reflect::Reflect;
use bevy::time::Time;
use bevy::transform::TransformSystems;
use bevy::transform::components::{GlobalTransform, Transform};

use crate::affordance::{Grabbable, Sittable, assign_grips};
use crate::base_pose::BasePoseSet;
use crate::constraint::{
    AnimConstraint, AnimatedWeight, ConstraintOrigin, ConstraintProperty, ConstraintTarget,
    Falloff, ResolvedConstraint, arbitrate, resolve,
};
use crate::error::AnimError;
use crate::humanoid;
use crate::locomotion::{Ground, LocomotionInput, LocomotionParams, LocomotionState};
use crate::pose::Pose;
use crate::rig::{Limb, Rig};
use crate::solve::{PoseSolver, SolveFrame, SolverSettings, foot_goals};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnimSystems {
    /// Estimates velocity for characters that do not get it from the controller.
    Motion,
    /// Turns gameplay requests (`hold`, `sit`, `look_at`) into constraints.
    Intent,
    /// Animates constraint weights and resolves targets to world space.
    Constraints,
    /// Steps legs and secondary body motion.
    Locomotion,
    /// Runs the pose solvers.
    Solve,
    /// Writes the solved pose to the joint entities' `Transform`s.
    Write,
}

#[derive(Default)]
pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<BasePoseSet>() {
            app.insert_resource(humanoid::base_poses());
        }
        app.register_type::<Rig>()
            .register_type::<BasePoseSet>()
            .register_type::<LocalUp>()
            .register_type::<AnimMotion>()
            .register_type::<MotionFromTransform>()
            .register_type::<CustomGround>()
            .register_type::<AnimConstraints>()
            .register_type::<AnimIntent>()
            .register_type::<Locomotor>()
            .register_type::<SolvedPose>()
            .register_type::<RigJoints>()
            .register_type::<Grabbable>()
            .register_type::<Sittable>()
            .register_type::<crate::affordance::Climbable>()
            .configure_sets(
                PostUpdate,
                (
                    AnimSystems::Motion,
                    AnimSystems::Intent,
                    AnimSystems::Constraints,
                    AnimSystems::Locomotion,
                    AnimSystems::Solve,
                    AnimSystems::Write,
                )
                    .chain()
                    .before(TransformSystems::Propagate),
            )
            .add_systems(
                PostUpdate,
                (
                    estimate_motion.in_set(AnimSystems::Motion),
                    process_intents.in_set(AnimSystems::Intent),
                    update_constraints.in_set(AnimSystems::Constraints),
                    update_locomotion.in_set(AnimSystems::Locomotion),
                    solve_poses.in_set(AnimSystems::Solve),
                    write_poses.in_set(AnimSystems::Write),
                ),
            );
    }
}

/// Direction of local up (opposite of gravity). The character crate owns the real value; this
/// crate only reads it.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct LocalUp(pub Vec3);

impl Default for LocalUp {
    fn default() -> Self {
        Self(Vec3::Y)
    }
}

/// Motion state the animation needs from the controller.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct AnimMotion {
    pub velocity: Vec3,
    pub grounded: bool,
    /// Gravitational acceleration; `None` means 9.81 opposite to `LocalUp`.
    pub gravity: Option<Vec3>,
}

impl Default for AnimMotion {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            grounded: true,
            gravity: None,
        }
    }
}

impl AnimMotion {
    /// Locomotion input for a root at `root` with the given local up.
    pub fn locomotion_input(&self, root: Transform, up: Vec3) -> LocomotionInput {
        let up = up.normalize_or(Vec3::Y);
        LocomotionInput {
            root,
            velocity: self.velocity,
            up,
            grounded: self.grounded,
            gravity: self.gravity.unwrap_or(-up * 9.81),
        }
    }
}

/// Locomotion of this character is stepped by another system in [`AnimSystems::Locomotion`]
/// with its own [`Ground`], typically physics queries that need system parameters and so cannot
/// live in [`GroundQuery`]. The built-in step skips it.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct CustomGround;

/// Derives `AnimMotion::velocity` from the root's `Transform` each frame, for characters that
/// are moved directly (tests, scripted movers) rather than by a physics controller.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct MotionFromTransform {
    previous: Option<Vec3>,
}

#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct Locomotor {
    pub state: LocomotionState,
    /// Blend of the leg system and body motion into the pose; 0 while seated, for example.
    pub weight: AnimatedWeight,
}

impl Locomotor {
    pub fn new(rig: &Rig, params: LocomotionParams) -> Result<Self, AnimError> {
        let mut weight = AnimatedWeight::new(1.0, 0.4);
        weight.advance(f32::INFINITY);
        Ok(Self {
            state: LocomotionState::from_rig(rig, params)?,
            weight,
        })
    }
}

#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component)]
pub struct AnimConstraints(pub Vec<AnimConstraint>);

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum GazeTarget {
    Point(Vec3),
    Entity(Entity),
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum IntentRequest {
    Hold(Entity),
    Release,
    Sit(Entity),
    Stand,
    LookAt(GazeTarget),
    StopLooking,
}

/// What gameplay wants the character to do, without naming a bone. Requests are consumed by
/// the intent system in the same frame.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component)]
pub struct AnimIntent {
    queue: Vec<IntentRequest>,
}

impl AnimIntent {
    /// Take hold of an object's grips with the hands that fit best.
    pub fn hold(&mut self, object: Entity) {
        self.queue.push(IntentRequest::Hold(object));
    }

    pub fn release(&mut self) {
        self.queue.push(IntentRequest::Release);
    }

    pub fn sit(&mut self, seat: Entity) {
        self.queue.push(IntentRequest::Sit(seat));
    }

    pub fn stand(&mut self) {
        self.queue.push(IntentRequest::Stand);
    }

    pub fn look_at(&mut self, target: GazeTarget) {
        self.queue.push(IntentRequest::LookAt(target));
    }

    pub fn stop_looking(&mut self) {
        self.queue.push(IntentRequest::StopLooking);
    }
}

#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component)]
pub struct SolvedPose(pub Pose);

/// Joint entities of a rig, indexed like the skeleton.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct RigJoints(pub Vec<Entity>);

/// Solver state and this frame's resolved goals.
#[derive(Component)]
pub struct AnimSolver {
    pub solver: PoseSolver,
    goals: Vec<ResolvedConstraint>,
}

impl AnimSolver {
    pub fn new(rig: Rig, settings: SolverSettings) -> Self {
        Self {
            solver: PoseSolver::new(rig, settings),
            goals: Vec::new(),
        }
    }
}

/// Ground query used by locomotion; without it feet stay at their rest height.
#[derive(Resource)]
pub struct GroundQuery(Box<dyn Ground + Send + Sync>);

impl GroundQuery {
    pub fn new(ground: impl Ground + Send + Sync + 'static) -> Self {
        Self(Box::new(ground))
    }
}

struct NoGround;

impl Ground for NoGround {
    fn cast(&self, _: Vec3, _: Vec3, _: f32) -> Option<crate::locomotion::GroundHit> {
        None
    }
}

/// Spawns a character: the root with all animation components and the joint hierarchy below
/// it. Gameplay and physics components are added to the returned entity by the caller.
pub fn spawn_character(
    commands: &mut Commands,
    rig: Rig,
    transform: Transform,
    params: LocomotionParams,
) -> Result<Entity, AnimError> {
    let locomotor = Locomotor::new(&rig, params)?;
    let rest = rig.skeleton.rest_pose();
    let root = commands.spawn_empty().id();
    let mut joints: Vec<Entity> = Vec::with_capacity(rig.skeleton.len());
    for (def, local) in rig.skeleton.joints().iter().zip(&rest.locals) {
        let parent = def.parent.map_or(root, |p| joints[p]);
        joints.push(
            commands
                .spawn((Name::new(def.name.clone()), *local, ChildOf(parent)))
                .id(),
        );
    }
    commands.entity(root).insert((
        transform,
        AnimSolver::new(rig.clone(), SolverSettings::default()),
        rig,
        locomotor,
        RigJoints(joints),
        SolvedPose(rest),
        LocalUp::default(),
        AnimMotion::default(),
        AnimConstraints::default(),
        AnimIntent::default(),
    ));
    Ok(root)
}

/// World transforms for roots and targets. Entities without a parent use their `Transform`,
/// which is current when animation runs before propagation; parented ones use last frame's
/// `GlobalTransform`.
#[derive(SystemParam)]
struct WorldTransforms<'w, 's> {
    query: Query<
        'w,
        's,
        (
            &'static Transform,
            &'static GlobalTransform,
            Option<&'static ChildOf>,
        ),
    >,
}

impl WorldTransforms<'_, '_> {
    fn get(&self, entity: Entity) -> Option<Transform> {
        let (local, global, parent) = self.query.get(entity).ok()?;
        Some(if parent.is_some() {
            global.compute_transform()
        } else {
            *local
        })
    }
}

fn estimate_motion(
    time: Res<Time>,
    mut query: Query<(&Transform, &mut AnimMotion, &mut MotionFromTransform)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (transform, mut motion, mut from) in &mut query {
        if let Some(previous) = from.previous {
            motion.velocity = (transform.translation - previous) / dt;
        }
        from.previous = Some(transform.translation);
    }
}

fn release_all(constraints: &mut AnimConstraints, origin: ConstraintOrigin) {
    for c in constraints.0.iter_mut().filter(|c| c.origin == origin) {
        c.weight.goal = 0.0;
    }
}

fn process_intents(
    mut characters: Query<(
        Entity,
        &mut AnimIntent,
        &mut AnimConstraints,
        &AnimSolver,
        Option<&mut Locomotor>,
    )>,
    grabbables: Query<&Grabbable>,
    sittables: Query<&Sittable>,
    transforms: WorldTransforms,
) {
    for (character, mut intent, mut constraints, solver, mut locomotor) in &mut characters {
        for request in core::mem::take(&mut intent.queue) {
            match request {
                IntentRequest::Hold(object) => {
                    release_all(&mut constraints, ConstraintOrigin::Hold);
                    let (Ok(grabbable), Some(object_world)) =
                        (grabbables.get(object), transforms.get(object))
                    else {
                        continue;
                    };
                    let grips: Vec<_> = grabbable
                        .grips
                        .iter()
                        .map(|g| (g.hand, object_world.mul_transform(g.local).translation))
                        .collect();
                    let root = transforms.get(character).unwrap_or_default();
                    let hands: Vec<(Limb, Vec3)> = [Limb::LeftHand, Limb::RightHand]
                        .into_iter()
                        .map(|limb| {
                            let at = solver
                                .solver
                                .report
                                .ends
                                .iter()
                                .find(|(l, _)| *l == limb)
                                .map_or(root.translation, |(_, p)| *p);
                            (limb, at)
                        })
                        .collect();
                    for (grip, limb) in assign_grips(&grips, &hands) {
                        let g = &grabbable.grips[grip];
                        constraints.0.push(AnimConstraint {
                            origin: ConstraintOrigin::Hold,
                            source: limb,
                            target: ConstraintTarget::Entity {
                                entity: object,
                                offset: g.local,
                            },
                            property: ConstraintProperty::Pose,
                            weight: AnimatedWeight::new(1.0, 0.35),
                            priority: 10,
                            falloff: Falloff::Smooth {
                                start: 1.5,
                                end: 2.5,
                            },
                            pose: Some(g.pose.clone()),
                        });
                    }
                }
                IntentRequest::Release => release_all(&mut constraints, ConstraintOrigin::Hold),
                IntentRequest::Sit(seat) => {
                    release_all(&mut constraints, ConstraintOrigin::Sit);
                    let Ok(sittable) = sittables.get(seat) else {
                        continue;
                    };
                    constraints.0.push(AnimConstraint {
                        origin: ConstraintOrigin::Sit,
                        source: Limb::Pelvis,
                        target: ConstraintTarget::Entity {
                            entity: seat,
                            offset: sittable.seat,
                        },
                        property: ConstraintProperty::Pose,
                        weight: AnimatedWeight::new(1.0, 0.6),
                        priority: 5,
                        falloff: Falloff::None,
                        pose: Some(sittable.pose.clone()),
                    });
                    if let Some(l) = locomotor.as_mut() {
                        l.weight.goal = 0.0;
                    }
                }
                IntentRequest::Stand => {
                    release_all(&mut constraints, ConstraintOrigin::Sit);
                    if let Some(l) = locomotor.as_mut() {
                        l.weight.goal = 1.0;
                    }
                }
                IntentRequest::LookAt(target) => {
                    release_all(&mut constraints, ConstraintOrigin::Gaze);
                    constraints.0.push(AnimConstraint {
                        origin: ConstraintOrigin::Gaze,
                        source: Limb::Head,
                        target: match target {
                            GazeTarget::Point(p) => ConstraintTarget::Point(p),
                            GazeTarget::Entity(entity) => ConstraintTarget::Entity {
                                entity,
                                offset: Transform::IDENTITY,
                            },
                        },
                        property: ConstraintProperty::Direction,
                        weight: AnimatedWeight::new(1.0, 0.4),
                        priority: 0,
                        falloff: Falloff::Smooth {
                            start: 8.0,
                            end: 15.0,
                        },
                        pose: None,
                    });
                }
                IntentRequest::StopLooking => release_all(&mut constraints, ConstraintOrigin::Gaze),
            }
        }
    }
}

fn update_constraints(
    time: Res<Time>,
    mut characters: Query<(
        &mut AnimConstraints,
        &mut AnimSolver,
        Option<&mut Locomotor>,
    )>,
    transforms: WorldTransforms,
) {
    let dt = time.delta_secs();
    for (mut constraints, mut solver, locomotor) in &mut characters {
        if let Some(mut l) = locomotor {
            l.weight.advance(dt);
        }
        for c in &mut constraints.0 {
            c.weight.advance(dt);
        }
        // Released constraints leave once fully faded out.
        constraints.0.retain(|c| !c.weight.is_finished_releasing());

        let ends = &solver.solver.report.ends;
        let goals = resolve(
            &constraints.0,
            &|target| match target {
                ConstraintTarget::Point(p) => Some(Transform::from_translation(*p)),
                ConstraintTarget::Entity { entity, offset } => {
                    transforms.get(*entity).map(|w| w.mul_transform(*offset))
                }
            },
            &|limb| ends.iter().find(|(l, _)| *l == limb).map(|(_, p)| *p),
        );
        solver.goals = goals;
    }
}

fn update_locomotion(
    time: Res<Time>,
    ground: Option<Res<GroundQuery>>,
    mut characters: Query<
        (Entity, &mut Locomotor, &AnimMotion, Option<&LocalUp>),
        Without<CustomGround>,
    >,
    transforms: WorldTransforms,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let ground: &dyn Ground = ground.as_deref().map_or(&NoGround, |g| g.0.as_ref());
    for (entity, mut locomotor, motion, up) in &mut characters {
        let Some(root) = transforms.get(entity) else {
            continue;
        };
        let input = motion.locomotion_input(root, up.map_or(Vec3::Y, |u| u.0));
        locomotor.state.update(&input, ground, dt);
    }
}

type SolveData = (
    Entity,
    &'static mut AnimSolver,
    &'static mut SolvedPose,
    &'static AnimConstraints,
    Option<&'static Locomotor>,
    Option<&'static LocalUp>,
);

fn solve_poses(
    time: Res<Time>,
    poses: Res<BasePoseSet>,
    mut characters: Query<SolveData>,
    transforms: WorldTransforms,
) {
    let dt = time.delta_secs();
    for (entity, mut solver, mut solved, constraints, locomotor, up) in &mut characters {
        let Some(root) = transforms.get(entity) else {
            continue;
        };
        let up = up.map_or(Vec3::Y, |u| u.0).normalize_or(Vec3::Y);
        let mut goals = core::mem::take(&mut solver.goals);
        let (output, body_weight) = match locomotor {
            Some(l) => {
                let w = l.weight.value();
                goals.extend(foot_goals(l.state.output(), up, root.rotation, w));
                (Some(l.state.output()), w)
            }
            None => (None, 0.0),
        };
        arbitrate(&mut goals);
        let frame = SolveFrame {
            root,
            up,
            base_poses: &poses,
            constraints: &constraints.0,
            goals: &goals,
            locomotion: output,
            body_weight,
            dt,
        };
        match solver.solver.solve(&frame) {
            Ok(pose) => solved.0 = pose,
            Err(e) => error!("animation solve failed: {e}"),
        }
    }
}

fn write_poses(
    characters: Query<(&RigJoints, &SolvedPose), With<AnimSolver>>,
    mut transforms: Query<&mut Transform>,
) {
    for (joints, pose) in &characters {
        for (&joint, local) in joints.0.iter().zip(&pose.0.locals) {
            if let Ok(mut transform) = transforms.get_mut(joint) {
                transform.set_if_neq(*local);
            }
        }
    }
}
