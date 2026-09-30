//! Headless camera tests: the target is a bare entity with the components the camera reads, so
//! the views are checked without a controller moving it.

use avian3d::prelude::*;
use bevy::prelude::*;
use struction_camera::{
    PlayerCamera, PlayerCameraPlugin, PlayerCameraState, ViewMode, ground_forward,
};
use struction_character::{
    CharacterIntent, CharacterLook, InputActions, InputActionsPlugin, InputSystems,
    PlayerControlled,
};
use struction_gravity::LocalUp;
use struction_physics::{
    CameraConstraint, CameraMode, CameraZone, InCameraZones, VolumeShape, testing::*, water,
};

/// Device input for the next frame, applied after mapping as a device would be.
#[derive(Resource, Default)]
struct Script {
    look: Vec2,
    zoom: f32,
    toggle: bool,
}

fn app() -> App {
    let mut app = headless_app_with((InputActionsPlugin, PlayerCameraPlugin));
    app.init_resource::<Script>().add_systems(
        PreUpdate,
        (|mut script: ResMut<Script>, mut actions: ResMut<InputActions>| {
            actions.look += core::mem::take(&mut script.look);
            actions.zoom += core::mem::take(&mut script.zoom);
            actions.toggle_view.pressed |= core::mem::take(&mut script.toggle);
        })
        .after(InputSystems::Map)
        .before(struction_camera::CameraSystems::Input),
    );
    app
}

fn spawn(app: &mut App, at: Vec3, up: Dir3) -> (Entity, Entity) {
    // Facing -Z where that is level, otherwise +X.
    let forward = (Vec3::NEG_Z - *up * up.dot(Vec3::NEG_Z))
        .try_normalize()
        .unwrap_or(Vec3::X);
    let target = app
        .world_mut()
        .spawn((
            PlayerControlled,
            Transform::from_translation(at),
            LocalUp(up),
            CharacterLook {
                forward,
                up: *up,
                pitch: 0.0,
            },
        ))
        .id();
    let camera = app.world_mut().spawn(PlayerCamera::new(target)).id();
    (target, camera)
}

fn camera(app: &App, camera: Entity) -> (PlayerCamera, PlayerCameraState, Transform) {
    let world = app.world();
    (
        *world.get::<PlayerCamera>(camera).unwrap(),
        *world.get::<PlayerCameraState>(camera).unwrap(),
        *world.get::<Transform>(camera).unwrap(),
    )
}

fn intent(app: &App, target: Entity) -> CharacterIntent {
    *app.world().get::<CharacterIntent>(target).unwrap()
}

#[test]
fn third_person_look_orbits_the_camera_without_turning_the_body() {
    let mut app = app();
    let (target, entity) = spawn(&mut app, Vec3::ZERO, Dir3::Y);
    step(&mut app, 2);
    let (settings, state, transform) = camera(&app, entity);
    // Behind (+Z of a -Z heading) and above the focus, looking at it.
    let focus = Vec3::Y * settings.focus_height;
    assert!(transform.translation.z > 4.0 && transform.translation.y > focus.y + 1.0);
    assert!((transform.translation.distance(focus) - settings.distance).abs() < 1e-3);
    assert!(
        transform
            .forward()
            .dot((focus - transform.translation).normalize())
            > 0.999
    );
    assert!(state.forward.abs_diff_eq(Vec3::NEG_Z, 1e-5));

    let quarter = core::f32::consts::FRAC_PI_2;
    app.world_mut().resource_mut::<Script>().look = Vec2::new(quarter, 0.0);
    step(&mut app, 2);
    let (_, state, transform) = camera(&app, entity);
    assert!(
        state.forward.abs_diff_eq(Vec3::X, 1e-4),
        "{}",
        state.forward
    );
    assert!(transform.translation.x < -4.0, "{}", transform.translation);
    let intent = intent(&app, target);
    assert_eq!(intent.look, Vec2::ZERO, "the orbit keeps look deltas");
    assert!(intent.face_movement);
    assert!(intent.movement_forward.unwrap().abs_diff_eq(Vec3::X, 1e-4));

    // Pitch is limited: the camera never flips over the top.
    app.world_mut().resource_mut::<Script>().look = Vec2::new(0.0, 10.0);
    step(&mut app, 2);
    let (settings, state, _) = camera(&app, entity);
    assert_eq!(state.pitch, settings.max_pitch);
}

#[test]
fn first_person_keeps_the_orbit_direction_and_hands_look_to_the_character() {
    let mut app = app();
    let (target, entity) = spawn(&mut app, Vec3::ZERO, Dir3::Y);
    step(&mut app, 2);
    let quarter = core::f32::consts::FRAC_PI_2;
    app.world_mut().resource_mut::<Script>().look = Vec2::new(quarter, 0.0);
    step(&mut app, 1);
    app.world_mut().resource_mut::<Script>().toggle = true;
    step(&mut app, 1);
    let (settings, _, transform) = camera(&app, entity);
    assert_eq!(settings.view, ViewMode::FirstPerson);
    // The character is asked to turn a quarter right to face +X, and the view already does.
    let intent = intent(&app, target);
    assert!((intent.look.x - quarter).abs() < 1e-4, "{}", intent.look);
    assert!(!intent.face_movement);
    assert_eq!(intent.movement_forward, None);
    assert!(
        transform
            .translation
            .abs_diff_eq(Vec3::Y * settings.eye_height, 1e-4)
    );
    assert!(
        transform.forward().dot(Vec3::X) > 0.99,
        "{}",
        transform.forward()
    );

    app.world_mut().resource_mut::<Script>().toggle = true;
    step(&mut app, 2);
    assert_eq!(camera(&app, entity).0.view, ViewMode::ThirdPerson);
}

#[test]
fn zooming_in_past_the_closest_distance_enters_first_person_and_back_out() {
    let mut app = app();
    let (_, entity) = spawn(&mut app, Vec3::ZERO, Dir3::Y);
    step(&mut app, 2);
    for _ in 0..20 {
        app.world_mut().resource_mut::<Script>().zoom = 1.0;
        step(&mut app, 1);
        if camera(&app, entity).0.view == ViewMode::FirstPerson {
            break;
        }
    }
    let (settings, _, _) = camera(&app, entity);
    assert_eq!(settings.view, ViewMode::FirstPerson);
    assert_eq!(settings.distance, settings.min_distance);

    app.world_mut().resource_mut::<Script>().zoom = -3.0;
    step(&mut app, 1);
    let (settings, _, _) = camera(&app, entity);
    assert_eq!(settings.view, ViewMode::ThirdPerson);
    assert_eq!(settings.distance, settings.min_distance);
    app.world_mut().resource_mut::<Script>().zoom = -100.0;
    step(&mut app, 1);
    let (settings, _, _) = camera(&app, entity);
    assert_eq!(settings.distance, settings.max_distance);
}

#[test]
fn walls_pull_the_camera_in_but_water_and_zones_do_not() {
    let mut app = app();
    let (_, entity) = spawn(&mut app, Vec3::ZERO, Dir3::Y);
    // Sensors surrounding the camera's path.
    app.world_mut().spawn((
        water(VolumeShape::Box {
            half_extents: Vec3::splat(20.0),
        }),
        Transform::default(),
    ));
    step(&mut app, 3);
    let (settings, state, _) = camera(&app, entity);
    assert!(
        (state.distance - settings.distance).abs() < 1e-3,
        "{}",
        state.distance
    );

    let wall = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(20.0, 20.0, 0.2),
            Transform::from_xyz(0.0, 0.0, 2.5),
        ))
        .id();
    step(&mut app, 2);
    let (_, state, transform) = camera(&app, entity);
    assert!(state.distance < 2.5, "{}", state.distance);
    assert!(transform.translation.z < 2.4 - settings.collision_radius * 0.5);

    // Once the wall is gone the camera eases back out instead of jumping.
    app.world_mut().despawn(wall);
    step(&mut app, 1);
    let eased = camera(&app, entity).1.distance;
    assert!(eased < settings.distance - 0.5, "{eased}");
    step(&mut app, 240);
    assert!((camera(&app, entity).1.distance - settings.distance).abs() < 0.01);
}

#[test]
fn views_follow_the_targets_up_around_a_planet() {
    let mut app = app();
    let (target, entity) = spawn(&mut app, Vec3::Z * 5.0, Dir3::Z);
    step(&mut app, 2);
    let (_, state, transform) = camera(&app, entity);
    assert!(state.up.abs_diff_eq(Vec3::Z, 1e-5));
    assert!(transform.up().dot(Vec3::Z) > 0.9, "{}", transform.up());
    assert!(
        transform.translation.z > 6.0,
        "above the surface: {}",
        transform.translation
    );

    // Up changes discretely between fields; the view turns over several frames, stays finite
    // and keeps its heading tangent.
    app.world_mut().get_mut::<LocalUp>(target).unwrap().0 = Dir3::NEG_Y;
    step(&mut app, 1);
    let (_, state, transform) = camera(&app, entity);
    assert!(transform.rotation.is_finite());
    assert!(
        state.up.dot(Vec3::Z) > 0.5,
        "eases instead of snapping: {}",
        state.up
    );
    step(&mut app, 120);
    let (_, state, transform) = camera(&app, entity);
    assert!(state.up.abs_diff_eq(Vec3::NEG_Y, 1e-3), "{}", state.up);
    assert!(state.forward.dot(state.up).abs() < 1e-4);
    assert!(transform.up().dot(Vec3::NEG_Y) > 0.9);
}

#[test]
fn passing_under_an_overhead_zone_does_not_reverse_screen_right() {
    let mut app = app();
    let (target, entity) = spawn(&mut app, Vec3::new(0.0, 0.9, -6.0), Dir3::Y);
    app.world_mut()
        .get_mut::<CharacterLook>(target)
        .unwrap()
        .forward = Vec3::NEG_Z;
    let position = Vec3::new(0.0, 14.0, -8.0);
    let zone = app
        .world_mut()
        .spawn(CameraZone {
            constraint: CameraConstraint {
                mode: CameraMode::Fixed { position },
                weight: 1.0,
                priority: 1,
            },
        })
        .id();
    app.world_mut()
        .entity_mut(target)
        .insert(InCameraZones(vec![zone]));
    step(&mut app, 240);
    assert!(
        camera(&app, entity)
            .2
            .translation
            .abs_diff_eq(position, 0.01)
    );
    for frame in 0..120 {
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation
            .z = -6.0 - frame as f32 / 30.0;
        let previous = camera(&app, entity).2.rotation;
        step(&mut app, 1);
        let transform = camera(&app, entity).2;
        assert!(transform.rotation.is_finite());
        assert!(transform.rotation.angle_between(previous) < 0.1);
        assert!(transform.right().dot(Vec3::X) > 0.99);
        assert!(
            ground_forward(&transform, Vec3::Y)
                .unwrap()
                .dot(Vec3::NEG_Z)
                > 0.99
        );
    }

    // Leaving the zone blends back to the orbit behind the player.
    app.world_mut()
        .get_mut::<InCameraZones>(target)
        .unwrap()
        .0
        .clear();
    step(&mut app, 240);
    let (settings, state, transform) = camera(&app, entity);
    assert_eq!(state.zone_weight, 0.0);
    let focus =
        app.world().get::<Transform>(target).unwrap().translation + Vec3::Y * settings.focus_height;
    assert!((transform.translation.distance(focus) - settings.distance).abs() < 0.01);
}

#[test]
fn holding_the_toggle_switches_views_once() {
    let mut app = app();
    let (_, entity) = spawn(&mut app, Vec3::ZERO, Dir3::Y);
    app.add_systems(
        PreUpdate,
        (|mut actions: ResMut<InputActions>| actions.toggle_view.held = true)
            .after(InputSystems::Map)
            .before(struction_camera::CameraSystems::Input),
    );
    app.world_mut().resource_mut::<Script>().toggle = true;
    step(&mut app, 10);
    assert_eq!(camera(&app, entity).0.view, ViewMode::FirstPerson);
}
