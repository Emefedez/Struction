//! Controller-driven animation rigs: the rig root follows the interpolated body and the feet are
//! placed by physics ray casts that skip the character's own capsule and sensor volumes.

use avian3d::prelude::*;
use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*};
use struction_anim::{
    humanoid::{self, ANKLE_HEIGHT},
    locomotion::{FootTarget, LocomotionParams},
    moves::{MoveKind, MovePose},
    plugin::{AnimMotion, Locomotor, SolvedPose},
};
use struction_character::{CharacterAnimationPlugin, prelude::*, spawn_rig};
use struction_physics::{prelude::*, testing::*};

const FEET: f32 = 0.8;

fn app() -> App {
    let mut app = headless_app_with((
        CharacterPlugins,
        DodgePlugin,
        CombatPlugin,
        CharacterAnimationPlugin,
    ));
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(AccumulatedMouseMotion::default());
    app.world_mut()
        .spawn(GravityField::scene(Vec3::NEG_Y * 9.81));
    app
}

#[test]
fn roll_tumbles_the_pose_without_rotating_the_body_or_rig_root() {
    let mut app = app();
    floor(&mut app, Quat::IDENTITY);
    let (body, rig) = character(&mut app, Vec3::new(0.0, FEET, 0.0));
    app.world_mut().entity_mut(body).insert(Roll::default());
    step(&mut app, 30);
    let standing = app.world().get::<SolvedPose>(rig).unwrap().0.clone();
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .roll_requested = true;
    step(&mut app, 20);
    let roll = app.world().get::<MovePose>(rig).unwrap();
    assert_eq!(roll.kind, MoveKind::Roll);
    assert!(roll.phase > 0.4 && roll.phase < 0.55, "{}", roll.phase);
    let pose = &app.world().get::<SolvedPose>(rig).unwrap().0;
    let humanoid = humanoid::rig();
    assert!(
        pose.locals[humanoid.pelvis]
            .rotation
            .angle_between(standing.locals[humanoid.pelvis].rotation)
            > 2.0
    );
    assert!(pose.locals[humanoid.pelvis].translation.y < 0.7);
    assert!(root(&app, rig).rotation.abs_diff_eq(Quat::IDENTITY, 1e-4));
    assert!((app.world().get::<Rotation>(body).unwrap().0 * Vec3::Y).abs_diff_eq(Vec3::Y, 1e-4));
    assert_eq!(
        app.world()
            .get::<Locomotor>(rig)
            .unwrap()
            .state
            .output()
            .footfalls,
        0
    );
    step(&mut app, 70);
    assert_eq!(app.world().get::<MovePose>(rig).unwrap().weight, 0.0);
    for foot in feet(&app, rig) {
        assert!(foot.planted);
        assert!((foot.position.y - ANKLE_HEIGHT).abs() < 0.02);
        assert!((foot.position - root(&app, rig).translation).length() < 0.5);
    }
}

#[test]
fn cancelled_roll_blends_out_and_animation_never_moves_physics() {
    let mut app = app();
    floor(&mut app, Quat::IDENTITY);
    let (body, rig) = character(&mut app, Vec3::new(0.0, FEET, 0.0));
    app.world_mut().entity_mut(body).insert(Roll::default());
    step(&mut app, 30);
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .roll_requested = true;
    step(&mut app, 18);
    app.world_mut().entity_mut(body).remove::<Roll>();
    step(&mut app, 1);
    let weight = app.world().get::<MovePose>(rig).unwrap().weight;
    assert!(weight > 0.0 && weight < 1.0);
    let before = *app.world().get::<Position>(body).unwrap();
    app.world_mut().run_schedule(PostUpdate);
    assert_eq!(*app.world().get::<Position>(body).unwrap(), before);
    step(&mut app, 10);
    assert_eq!(app.world().get::<MovePose>(rig).unwrap().weight, 0.0);
}

#[test]
fn a_swing_raises_the_arm_while_the_legs_keep_walking() {
    let mut app = app();
    floor(&mut app, Quat::IDENTITY);
    let (body, rig) = character(&mut app, Vec3::new(0.0, FEET, 0.0));
    app.world_mut().entity_mut(body).insert(Attack::default());
    step(&mut app, 30);
    let standing = app.world().get::<SolvedPose>(rig).unwrap().0.clone();
    {
        let mut intent = app.world_mut().get_mut::<CharacterIntent>(body).unwrap();
        intent.attack_requested = true;
        intent.movement = Vec2::Y;
    }
    step(&mut app, 12);
    let swing = *app.world().get::<MovePose>(rig).unwrap();
    assert_eq!(swing.kind, MoveKind::Swing);
    assert!(swing.phase > 0.2 && swing.phase < 0.5, "{}", swing.phase);
    let humanoid = humanoid::rig();
    let arm = humanoid.skeleton.joint_id("upper_arm_r").unwrap();
    let pose = &app.world().get::<SolvedPose>(rig).unwrap().0;
    assert!(
        pose.locals[arm]
            .rotation
            .angle_between(standing.locals[arm].rotation)
            > 1.0
    );
    let mut footfalls = 0;
    for _ in 0..40 {
        step(&mut app, 1);
        footfalls += app
            .world()
            .get::<Locomotor>(rig)
            .unwrap()
            .state
            .output()
            .footfalls;
    }
    assert!(footfalls > 0, "walking continues through the swing");
    assert_eq!(app.world().get::<MovePose>(rig).unwrap().weight, 0.0);
}

fn floor(app: &mut App, rotation: Quat) {
    // Top face through the origin whatever the tilt.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 40.0),
        Transform::from_translation(rotation * Vec3::NEG_Y * 0.5).with_rotation(rotation),
    ));
}

/// A character body and the rig that follows it.
fn character(app: &mut App, at: Vec3) -> (Entity, Entity) {
    let world = app.world_mut();
    let body = world
        .spawn((
            CharacterController::default(),
            Transform::from_translation(at),
        ))
        .id();
    let rig = spawn_rig(
        &mut world.commands(),
        body,
        humanoid::rig(),
        LocomotionParams::default(),
    )
    .unwrap();
    world.flush();
    (body, rig)
}

fn feet(app: &App, rig: Entity) -> Vec<FootTarget> {
    let locomotor = app.world().get::<Locomotor>(rig).unwrap();
    locomotor.state.output().feet.clone()
}

fn root(app: &App, rig: Entity) -> Transform {
    *app.world().get::<Transform>(rig).unwrap()
}

#[test]
fn rig_root_follows_the_body_and_feet_stand_on_the_floor() {
    let mut app = app();
    floor(&mut app, Quat::IDENTITY);
    let (body, rig) = character(&mut app, Vec3::new(0.0, FEET + 0.3, 0.0));
    step(&mut app, 90);

    let body_transform = *app.world().get::<Transform>(body).unwrap();
    let root = root(&app, rig);
    assert!(
        root.translation
            .abs_diff_eq(body_transform.translation - Vec3::Y * FEET, 1e-4),
        "root {} below body {}",
        root.translation,
        body_transform.translation
    );
    assert!(root.rotation.abs_diff_eq(body_transform.rotation, 1e-5));
    assert!(root.translation.y.abs() < 0.02, "settled on the floor");
    assert!(app.world().get::<AnimMotion>(rig).unwrap().grounded);

    let feet = feet(&app, rig);
    assert_eq!(feet.len(), 2);
    for foot in &feet {
        // Probes start inside the capsule; hitting it would lift the feet by half a meter.
        assert!(
            (foot.position.y - ANKLE_HEIGHT).abs() < 0.01,
            "foot at {}",
            foot.position
        );
        assert!(foot.planted);
    }
}

#[test]
fn walking_alternates_feet_planted_on_the_floor() {
    let mut app = app();
    floor(&mut app, Quat::IDENTITY);
    let (body, rig) = character(&mut app, Vec3::new(0.0, FEET, 0.0));
    step(&mut app, 30);
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .movement = Vec2::Y;

    let mut lifted = [false; 2];
    let mut footfalls = 0;
    for _ in 0..180 {
        step(&mut app, 1);
        let root = root(&app, rig);
        footfalls += app
            .world()
            .get::<Locomotor>(rig)
            .unwrap()
            .state
            .output()
            .footfalls;
        for (i, foot) in feet(&app, rig).iter().enumerate() {
            lifted[i] |= !foot.planted;
            if foot.planted {
                assert!((foot.position.y - ANKLE_HEIGHT).abs() < 0.01);
            }
            let reach = (foot.position - root.translation).with_y(0.0).length();
            assert!(reach < 1.0, "foot {i} trails {reach} m behind the root");
        }
    }
    assert!(root(&app, rig).translation.z < -8.0, "walked forward");
    assert_eq!(lifted, [true, true], "both legs stepped");
    assert!(footfalls >= 8, "{footfalls} footfalls in three seconds");
}

#[test]
fn feet_follow_a_slope() {
    let mut app = app();
    let tilt = Quat::from_rotation_z(20f32.to_radians());
    floor(&mut app, tilt);
    let (_, rig) = character(&mut app, Vec3::new(0.0, FEET + 0.2, 0.0));
    step(&mut app, 90);

    let normal = tilt * Vec3::Y;
    let feet = feet(&app, rig);
    for foot in &feet {
        assert!(foot.planted, "standing still settles both feet");
        let height = foot.position.dot(normal);
        assert!(
            (height - ANKLE_HEIGHT).abs() < 0.02,
            "foot {} is {height} m off the slope",
            foot.position
        );
        assert!(foot.normal.abs_diff_eq(normal, 1e-3));
    }
    // The slope rises toward +X, the character's right while it faces -Z.
    let [left, right] = [0, 1].map(|i| feet[i].position);
    assert!(left.x < right.x);
    assert!(right.y - left.y > 0.03, "left {left}, right {right}");
}

#[test]
fn feet_wade_through_water_to_the_pool_floor() {
    let mut app = app();
    floor(&mut app, Quat::IDENTITY);
    app.world_mut().spawn((
        water(VolumeShape::Box {
            half_extents: Vec3::new(3.0, 0.5, 3.0),
        }),
        Transform::default(),
    ));
    let (body, rig) = character(&mut app, Vec3::new(0.0, FEET, 0.0));
    step(&mut app, 90);

    assert!(!app.world().get::<CharacterState>(body).unwrap().swimming);
    for foot in feet(&app, rig) {
        assert!(
            (foot.position.y - ANKLE_HEIGHT).abs() < 0.01,
            "foot stands on the water surface at {}",
            foot.position
        );
    }
}
