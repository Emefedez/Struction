//! Headless Bevy App tests: the plugin writes solved poses to joint entities before transform
//! propagation, and gameplay intents (`hold`, `look_at`, `sit`) drive constraints without
//! naming bones.

use std::time::Duration;

use bevy::MinimalPlugins;
use bevy::app::App;
use bevy::ecs::entity::Entity;
use bevy::math::{Quat, Vec3};
use bevy::time::TimeUpdateStrategy;
use bevy::transform::TransformPlugin;
use bevy::transform::components::{GlobalTransform, Transform};
use struction_anim::AnimPlugin;
use struction_anim::affordance::{Grabbable, GripTarget, HandPreference, Sittable};
use struction_anim::humanoid;
use struction_anim::locomotion::{LocomotionParams, PlaneGround};
use struction_anim::plugin::{
    AnimConstraints, AnimIntent, AnimMotion, GazeTarget, GroundQuery, Locomotor,
    MotionFromTransform, RigJoints, spawn_character,
};
use struction_anim::rig::{Limb, Rig};

const DT: f32 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AnimPlugin))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
            DT,
        )))
        .insert_resource(GroundQuery::new(PlaneGround {
            point: Vec3::ZERO,
            normal: Vec3::Y,
        }));
    app
}

fn spawn(app: &mut App, transform: Transform) -> Entity {
    let world = app.world_mut();
    let entity = {
        let mut commands = world.commands();
        spawn_character(
            &mut commands,
            humanoid::rig(),
            transform,
            LocomotionParams::default(),
        )
        .unwrap()
    };
    world.flush();
    entity
}

fn limb_joint(app: &App, character: Entity, limb: Limb) -> Entity {
    let world = app.world();
    let rig = world.get::<Rig>(character).unwrap();
    let joint = *rig.binding(limb).unwrap().chain.last().unwrap();
    world.get::<RigJoints>(character).unwrap().0[joint]
}

fn global(app: &App, entity: Entity) -> Transform {
    app.world()
        .get::<GlobalTransform>(entity)
        .unwrap()
        .compute_transform()
}

fn intent(app: &mut App, character: Entity) -> bevy::ecs::world::Mut<'_, AnimIntent> {
    app.world_mut().get_mut::<AnimIntent>(character).unwrap()
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn walking_character_writes_planted_feet_to_joint_transforms() {
    let mut app = app();
    let character = spawn(&mut app, Transform::IDENTITY);
    app.world_mut()
        .entity_mut(character)
        .insert(MotionFromTransform::default());
    let feet = [
        limb_joint(&app, character, Limb::LeftFoot),
        limb_joint(&app, character, Limb::RightFoot),
    ];
    app.update();

    let velocity = Vec3::new(0.0, 0.0, -1.4);
    let mut previous: Option<[(Vec3, bool); 2]> = None;
    let mut steps = 0;
    for frame in 0..300 {
        app.world_mut()
            .get_mut::<Transform>(character)
            .unwrap()
            .translation += velocity * DT;
        app.update();
        let locomotor = app.world().get::<Locomotor>(character).unwrap();
        let targets = locomotor.state.output().feet.clone();
        let now = [0, 1].map(|i| (global(&app, feet[i]).translation, targets[i].planted));
        if frame > 30 {
            let measured = app.world().get::<AnimMotion>(character).unwrap().velocity;
            assert!(
                measured.distance(velocity) < 1e-3,
                "velocity estimated from transform"
            );
            for i in 0..2 {
                // Same frame: the solved pose reached GlobalTransform before rendering would.
                assert!(
                    now[i].0.distance(targets[i].position) < 1e-3,
                    "frame {frame}: foot {i} at {} but target {}",
                    now[i].0,
                    targets[i].position
                );
            }
        }
        if let Some(prev) = previous {
            for i in 0..2 {
                if prev[i].1 && now[i].1 {
                    assert!(
                        prev[i].0.distance(now[i].0) < 1e-3,
                        "frame {frame}: planted foot {i} slid"
                    );
                }
                steps += usize::from(prev[i].1 && !now[i].1);
            }
        }
        previous = Some(now);
    }
    assert!(steps >= 10, "only {steps} steps in 5 s");
}

#[test]
fn hold_blends_the_hand_onto_the_grip_and_release_lets_go() {
    let mut app = app();
    let character = spawn(&mut app, Transform::IDENTITY);
    let grip_rotation = Quat::from_rotation_x(-1.2);
    let object = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.35, 1.0, -0.35),
            GlobalTransform::default(),
            Grabbable {
                grips: vec![GripTarget {
                    local: Transform::from_xyz(0.0, 0.05, 0.0).with_rotation(grip_rotation),
                    hand: HandPreference::Any,
                    pose: "grip".into(),
                }],
            },
        ))
        .id();
    run(&mut app, 5);
    let right = limb_joint(&app, character, Limb::RightHand);
    let left = limb_joint(&app, character, Limb::LeftHand);
    let grip = Vec3::new(0.35, 1.05, -0.35);
    let rest = global(&app, right).translation;
    let left_rest = global(&app, left).translation;

    intent(&mut app, character).hold(object);
    let mut distances = Vec::new();
    for _ in 0..40 {
        app.update();
        distances.push(global(&app, right).translation.distance(grip));
    }
    // The weight animates: the hand approaches over several frames instead of snapping.
    assert!(
        distances[0] > rest.distance(grip) * 0.9,
        "no snap on the first frame"
    );
    assert!(
        distances.windows(2).all(|w| w[1] <= w[0] + 1e-5),
        "monotonic approach"
    );
    assert!(
        distances[39] < 1e-3,
        "hand reached the grip: {}",
        distances[39]
    );
    let hand = global(&app, right);
    assert!(
        hand.rotation.angle_between(grip_rotation) < 1e-2,
        "hand aligned with the grip"
    );
    assert!(
        global(&app, left).translation.distance(left_rest) < 0.02,
        "other hand stays"
    );
    // The grip base pose curled the fingers of the holding hand.
    let fingers = {
        let rig = app.world().get::<Rig>(character).unwrap();
        let joint = rig.skeleton.joint_id("fingers_1_r").unwrap();
        app.world().get::<RigJoints>(character).unwrap().0[joint]
    };
    let curl = app.world().get::<Transform>(fingers).unwrap().rotation;
    assert!(curl.angle_between(Quat::IDENTITY) > 0.9, "fingers curled");

    // The hand follows the object while held.
    app.world_mut()
        .get_mut::<Transform>(object)
        .unwrap()
        .translation
        .y += 0.1;
    app.update();
    assert!(
        global(&app, right)
            .translation
            .distance(grip + Vec3::Y * 0.1)
            < 1e-3
    );

    intent(&mut app, character).release();
    run(&mut app, 60);
    assert!(
        global(&app, right).translation.distance(rest) < 0.02,
        "hand returned to rest"
    );
    assert!(
        app.world()
            .get::<AnimConstraints>(character)
            .unwrap()
            .0
            .is_empty(),
        "constraint removed after fading out"
    );
}

#[test]
fn look_at_turns_the_head_within_limits() {
    let mut app = app();
    let character = spawn(&mut app, Transform::IDENTITY);
    run(&mut app, 5);
    let head = limb_joint(&app, character, Limb::Head);
    let target = Vec3::new(-2.0, 2.2, -2.0);
    intent(&mut app, character).look_at(GazeTarget::Point(target));
    run(&mut app, 40);
    let h = global(&app, head);
    let forward = h.rotation * Vec3::NEG_Z;
    assert!(forward.angle_between(target - h.translation) < 0.02);

    // Behind the character: clamped to the yaw limit instead of twisting the neck around.
    intent(&mut app, character).look_at(GazeTarget::Point(Vec3::new(0.0, 1.7, 5.0)));
    run(&mut app, 40);
    let forward = global(&app, head).rotation * Vec3::NEG_Z;
    let yaw = forward.x.atan2(-forward.z).abs();
    assert!((yaw - 80f32.to_radians()).abs() < 0.05, "yaw {yaw}");
}

#[test]
fn sit_moves_the_pelvis_to_the_seat_and_fades_out_the_legs() {
    let mut app = app();
    let character = spawn(&mut app, Transform::IDENTITY);
    let chair = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.1),
            GlobalTransform::default(),
            Sittable {
                seat: Transform::from_xyz(0.0, 0.5, 0.0),
                pose: "seated".into(),
            },
        ))
        .id();
    run(&mut app, 5);
    intent(&mut app, character).sit(chair);
    run(&mut app, 60);
    let pelvis = {
        let rig = app.world().get::<Rig>(character).unwrap();
        app.world().get::<RigJoints>(character).unwrap().0[rig.pelvis]
    };
    assert!(
        global(&app, pelvis)
            .translation
            .distance(Vec3::new(0.0, 0.5, 0.1))
            < 1e-3
    );
    assert_eq!(
        app.world()
            .get::<Locomotor>(character)
            .unwrap()
            .weight
            .value(),
        0.0
    );
    let knee = {
        let rig = app.world().get::<Rig>(character).unwrap();
        app.world().get::<RigJoints>(character).unwrap().0
            [rig.binding(Limb::LeftFoot).unwrap().chain[1]]
    };
    // Thighs horizontal: the knee is in front of the pelvis at about seat height.
    let k = global(&app, knee).translation;
    assert!(k.z < -0.2 && (k.y - 0.5).abs() < 0.1, "knee at {k}");

    intent(&mut app, character).stand();
    run(&mut app, 60);
    assert_eq!(
        app.world()
            .get::<Locomotor>(character)
            .unwrap()
            .weight
            .value(),
        1.0
    );
    assert!(global(&app, pelvis).translation.y > 0.85);
}

#[test]
fn jump_predicts_the_landing_then_squashes_and_the_spine_springs_back() {
    let mut app = app();
    let character = spawn(&mut app, Transform::IDENTITY);
    run(&mut app, 30);
    let (root_joint, chest) = {
        let world = app.world();
        let rig = world.get::<Rig>(character).unwrap();
        let joints = &world.get::<RigJoints>(character).unwrap().0;
        (
            joints[rig.root],
            joints[rig.skeleton.joint_id("chest").unwrap()],
        )
    };
    let chest_rest = global(&app, chest).rotation;

    // Ballistic jump driven from outside, as a controller would.
    let gravity = Vec3::new(0.0, -9.81, 0.0);
    let mut velocity = Vec3::new(0.0, 4.0, -1.0);
    let mut position = Vec3::ZERO;
    let mut predicted = None;
    let mut airborne_frames = 0;
    let mut impact = 0.0;
    loop {
        velocity += gravity * DT;
        position += velocity * DT;
        let grounded = position.y <= 0.0;
        if grounded {
            impact = -velocity.y;
            position.y = 0.0;
            velocity = Vec3::ZERO;
        }
        app.world_mut()
            .get_mut::<Transform>(character)
            .unwrap()
            .translation = position;
        *app.world_mut().get_mut::<AnimMotion>(character).unwrap() = AnimMotion {
            velocity,
            grounded,
            gravity: Some(gravity),
        };
        app.update();
        if grounded {
            break;
        }
        airborne_frames += 1;
        let output = app
            .world()
            .get::<Locomotor>(character)
            .unwrap()
            .state
            .output()
            .clone();
        let landing = output.landing.expect("landing predicted while airborne");
        assert!(output.feet.iter().all(|f| !f.planted));
        if airborne_frames == 5 {
            predicted = Some(landing.point);
        }
    }
    let predicted = predicted.unwrap();
    assert!(
        predicted.distance(position) < 0.05,
        "predicted {predicted}, landed at {position}"
    );
    assert!(airborne_frames > 30);

    let mut min_scale = 1.0_f32;
    let mut max_pitch = 0.0_f32;
    for _ in 0..90 {
        app.update();
        min_scale = min_scale.min(app.world().get::<Transform>(root_joint).unwrap().scale.y);
        let forward = global(&app, chest).rotation * Vec3::NEG_Z;
        max_pitch = max_pitch.max(-forward.y);
    }
    assert!(impact > 3.5);
    assert!(min_scale < 0.97, "landing squashes the body ({min_scale})");
    assert!(
        max_pitch > 0.1,
        "impact pitches the chest forward ({max_pitch})"
    );
    assert!(
        global(&app, chest).rotation.angle_between(chest_rest) < 0.02,
        "chest springs back to the base pose"
    );
    let feet = app
        .world()
        .get::<Locomotor>(character)
        .unwrap()
        .state
        .output()
        .feet
        .clone();
    assert!(
        feet.iter()
            .all(|f| f.planted && (f.position.y - humanoid::ANKLE_HEIGHT).abs() < 1e-4)
    );
}
