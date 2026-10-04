use std::time::Duration;

use avian3d::prelude::*;
use bevy::{prelude::*, time::TimeUpdateStrategy};
use struction_character::prelude::*;
use struction_core::{ActionArgs, ActionInvocation, ActionQueue, CorePlugin};
use struction_physics::{prelude::*, testing::*};

fn scene() -> (App, Entity, Entity) {
    let mut app = headless_app_with((CharacterPlugins, DodgePlugin, CorePlugin::default()));
    app.world_mut()
        .spawn(GravityField::scene(Vec3::NEG_Y * 9.81));
    let floor = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(100.0, 1.0, 100.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ))
        .id();
    let body = app
        .world_mut()
        .spawn((
            CharacterController::default(),
            Roll::default(),
            Transform::from_xyz(0.0, 0.8, 0.0),
        ))
        .id();
    step(&mut app, 30);
    assert!(
        app.world().get::<CharacterState>(body).unwrap().grounded,
        "settling: {:?} {:?} {:?}",
        app.world().get::<CharacterState>(body),
        app.world().get::<Position>(body),
        app.world().get::<CharacterMove>(body)
    );
    (app, body, floor)
}

fn request(app: &mut App, body: Entity) {
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .roll_requested = true;
}

fn position(app: &App, body: Entity) -> Vec3 {
    app.world().get::<Position>(body).unwrap().0
}

#[test]
fn idle_roll_has_a_bounded_distance_and_recovers() {
    let (mut app, body, _) = scene();
    let start = position(&app, body);
    request(&mut app, body);
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_some());
    assert!(
        !app.world()
            .get::<CharacterIntent>(body)
            .unwrap()
            .roll_requested
    );
    let mut ticks = 1;
    while app.world().get::<Rolling>(body).is_some() {
        step(&mut app, 1);
        ticks += 1;
        assert!(ticks < 45);
    }
    let distance = (position(&app, body) - start).length();
    let tuning = Roll::default();
    let expected = 2.0 * tuning.peak_speed * tuning.duration / std::f32::consts::PI;
    assert!(
        (distance - expected).abs() < 0.05,
        "distance {distance}, expected {expected}"
    );
    assert!(app.world().get::<CharacterMove>(body).unwrap().recovery > 0.0);
    request(&mut app, body);
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_none());
    step(&mut app, 30);
    assert!(
        app.world().get::<Rolling>(body).is_none(),
        "rejected presses are consumed"
    );
    request(&mut app, body);
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_some());
}

#[test]
fn direction_is_captured_and_jump_cannot_interrupt() {
    let (mut app, body, _) = scene();
    {
        let mut intent = app.world_mut().get_mut::<CharacterIntent>(body).unwrap();
        intent.movement = Vec2::Y;
        intent.movement_forward = Some(Vec3::X);
        intent.roll_requested = true;
        intent.jump_requested = true;
        intent.face_movement = true;
    }
    step(&mut app, 1);
    {
        let mut intent = app.world_mut().get_mut::<CharacterIntent>(body).unwrap();
        intent.movement_forward = Some(Vec3::NEG_Z);
        intent.roll_requested = true;
        intent.jump_requested = true;
    }
    step(&mut app, 20);
    let active = app.world().get::<Rolling>(body).unwrap();
    assert!(
        active.elapsed > 0.3,
        "repeated request must not restart the timer"
    );
    assert!(active.direction.abs_diff_eq(Vec3::X, 1e-5));
    let at = position(&app, body);
    assert!(
        at.x > 2.0 && at.z.abs() < 0.02 && (at.y - 0.8).abs() < 0.03,
        "{at}"
    );
    assert!(app.world().get::<CharacterState>(body).unwrap().grounded);
}

#[test]
fn roll_requires_capability_valid_tuning_and_ground() {
    let (mut app, body, _) = scene();
    app.world_mut().entity_mut(body).remove::<Roll>();
    request(&mut app, body);
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_none());
    for duration in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        app.world_mut().entity_mut(body).insert(Roll {
            duration,
            ..default()
        });
        request(&mut app, body);
        step(&mut app, 1);
        assert!(app.world().get::<Rolling>(body).is_none());
        assert!(position(&app, body).is_finite());
    }
    app.world_mut().entity_mut(body).insert(Roll::default());
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .jump_requested = true;
    step(&mut app, 10);
    request(&mut app, body);
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_none());
}

#[test]
fn a_wall_stops_the_capsule() {
    let (mut app, body, _) = scene();
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(10.0, 4.0, 0.2),
        Transform::from_xyz(0.0, 1.0, -1.5),
    ));
    step(&mut app, 2);
    request(&mut app, body);
    for _ in 0..55 {
        step(&mut app, 1);
        assert!(position(&app, body).z > -1.15, "passed through wall");
    }
    assert!(app.world().get::<Rolling>(body).is_none());
}

#[test]
fn ground_loss_cancels_without_erasing_momentum() {
    let (mut app, body, floor) = scene();
    request(&mut app, body);
    step(&mut app, 14);
    let before = app.world().get::<LinearVelocity>(body).unwrap().0;
    app.world_mut().despawn(floor);
    step(&mut app, 2);
    assert!(app.world().get::<Rolling>(body).is_none());
    let after = app.world().get::<LinearVelocity>(body).unwrap().0;
    assert!(
        after.z < -5.0 && (after.z - before.z).abs() < 1.0,
        "{before} -> {after}"
    );
    assert!(after.y < 0.0);
}

#[test]
fn water_cancels_and_rejects_rolls() {
    let (mut app, body, _) = scene();
    request(&mut app, body);
    step(&mut app, 8);
    app.world_mut().spawn((
        water(VolumeShape::Box {
            half_extents: Vec3::splat(10.0),
        }),
        Transform::default(),
    ));
    step(&mut app, 3);
    assert!(app.world().get::<CharacterState>(body).unwrap().swimming);
    assert!(app.world().get::<Rolling>(body).is_none());
    step(&mut app, 20);
    request(&mut app, body);
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_none());
}

#[test]
fn roll_follows_planet_curvature() {
    let mut app = headless_app_with((CharacterPlugins, DodgePlugin));
    let radius = 6.0;
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::sphere(radius),
        GravityField::planet(9.81, 60.0),
        Transform::default(),
    ));
    let body = app
        .world_mut()
        .spawn((
            CharacterController::default(),
            Roll::default(),
            Transform::from_xyz(0.0, radius + 0.8, 0.0),
        ))
        .id();
    step(&mut app, 40);
    request(&mut app, body);
    for tick in 0..35 {
        step(&mut app, 1);
        let roll = app
            .world()
            .get::<Rolling>(body)
            .unwrap_or_else(|| panic!("cancelled at {tick}"));
        let up = *app.world().get::<LocalUp>(body).unwrap().0;
        assert!(roll.direction.dot(up).abs() < 1e-4);
        assert!((position(&app, body).length() - (radius + 0.8)).abs() < 0.15);
    }
    assert!(position(&app, body).z < -3.0);
}

#[test]
fn a_press_between_fixed_ticks_is_latched_and_holding_does_not_repeat() {
    let (mut app, body, _) = scene();
    app.world_mut().entity_mut(body).insert(PlayerControlled);
    app.init_resource::<ButtonInput<KeyCode>>();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 600.0,
    )));
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
    step(&mut app, 1);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    assert!(
        app.world()
            .get::<CharacterIntent>(body)
            .unwrap()
            .roll_requested
    );
    step(&mut app, 11);
    assert!(app.world().get::<Rolling>(body).is_some());
    step(&mut app, 700);
    assert!(app.world().get::<Rolling>(body).is_none());
}

#[test]
fn registered_action_uses_the_same_simulation_request() {
    let (mut app, body, _) = scene();
    app.world_mut()
        .resource_mut::<ActionQueue>()
        .invoke(ActionInvocation::new("dodge/roll", body, ActionArgs::new()));
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_some());
}

#[test]
fn live_tuning_changes_only_the_next_roll() {
    let (mut app, body, _) = scene();
    request(&mut app, body);
    step(&mut app, 1);
    app.world_mut().get_mut::<Roll>(body).unwrap().duration = 0.1;
    step(&mut app, 15);
    assert_eq!(
        app.world().get::<Rolling>(body).unwrap().tuning.duration,
        0.65
    );
    app.world_mut().entity_mut(body).remove::<Roll>();
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_none());
}

#[test]
fn roll_climbs_a_walkable_slope() {
    let (mut app, body, floor) = scene();
    app.world_mut().despawn(floor);
    let tilt = Quat::from_rotation_z(20_f32.to_radians());
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 1.0, 100.0),
        Transform::from_translation(tilt * Vec3::NEG_Y * 0.5).with_rotation(tilt),
    ));
    step(&mut app, 60);
    let start = position(&app, body);
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .movement = Vec2::X;
    request(&mut app, body);
    step(&mut app, 25);
    let delta = position(&app, body) - start;
    assert!(app.world().get::<Rolling>(body).is_some());
    assert!(delta.x > 2.0 && delta.y > 0.7, "{delta}");
}

#[test]
fn render_frame_rate_does_not_change_roll_distance() {
    let mut distances = Vec::new();
    for hz in [30.0, 60.0, 144.0] {
        let (mut app, body, _) = scene();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / hz,
        )));
        request(&mut app, body);
        step(&mut app, hz as usize);
        distances.push(position(&app, body).z);
    }
    assert!(
        distances.iter().all(|d| (d - distances[0]).abs() < 0.01),
        "{distances:?}"
    );
}

#[test]
fn a_jump_pressed_late_in_a_roll_waits_for_it_to_end() {
    let (mut app, body, _) = scene();
    app.world_mut().get_mut::<Roll>(body).unwrap().recovery = 0.0;
    request(&mut app, body);
    step(&mut app, 1);
    let duration = Roll::default().duration;
    while app.world().get::<Rolling>(body).unwrap().elapsed < duration - 0.1 {
        step(&mut app, 1);
    }
    // One press, a tenth of a second before the roll ends, is buffered and lands as a jump.
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .jump_requested = true;
    let mut rolled_out = false;
    for _ in 0..20 {
        step(&mut app, 1);
        rolled_out |= app.world().get::<Rolling>(body).is_none();
        if app.world().get::<LinearVelocity>(body).unwrap().0.y > 1.0 {
            assert!(
                rolled_out,
                "the roll is not cut short without a cancel window"
            );
            return;
        }
    }
    panic!("the buffered jump never happened");
}

#[test]
fn a_cancel_window_turns_a_press_into_a_jump_that_keeps_the_rolls_speed() {
    let (mut app, body, _) = scene();
    app.world_mut().get_mut::<Roll>(body).unwrap().cancel_into = vec![CancelInto {
        action: CharacterAction::Jump,
        after: 0.4,
    }];
    request(&mut app, body);
    step(&mut app, 1);
    // Pressed before the window opens: buffered until it does.
    while app.world().get::<Rolling>(body).unwrap().elapsed < 0.3 {
        step(&mut app, 1);
    }
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .jump_requested = true;
    for _ in 0..12 {
        step(&mut app, 1);
        let velocity = app.world().get::<LinearVelocity>(body).unwrap().0;
        if velocity.y > 1.0 {
            assert!(app.world().get::<Rolling>(body).is_none());
            assert!(
                Vec2::new(velocity.x, velocity.z).length() > 3.0,
                "{velocity}"
            );
            return;
        }
    }
    panic!("the press never opened the cancel window");
}

#[test]
fn a_refused_press_is_dropped_after_the_input_buffer() {
    let (mut app, body, _) = scene();
    request(&mut app, body);
    step(&mut app, 1);
    // Mid-roll, far from its end: the press expires instead of jumping later.
    app.world_mut()
        .get_mut::<CharacterIntent>(body)
        .unwrap()
        .jump_requested = true;
    step(&mut app, 40);
    assert!(app.world().get::<Rolling>(body).is_none());
    assert!(app.world().get::<LinearVelocity>(body).unwrap().0.y.abs() < 0.5);
    assert!(
        !app.world()
            .get::<CharacterIntent>(body)
            .unwrap()
            .jump_requested
    );
}
