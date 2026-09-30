//! Headless character tests: fixed-step loops with a manual clock, no window or GPU. Keyboard
//! state is injected into `ButtonInput` the way the input plugin would leave it.

use avian3d::prelude::*;
use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*, time::TimeUpdateStrategy};
use core::time::Duration;
use struction_character::prelude::*;
use struction_physics::{prelude::*, testing::*};

const G: f32 = 9.81;
/// Distance from the capsule's center to its feet (half of the 1.0 cylinder plus the radius).
const FEET: f32 = 0.8;

fn app() -> App {
    let mut app = headless_app_with(CharacterPlugins);
    app.init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(AccumulatedMouseMotion::default());
    app
}

/// One frame, then what the input plugin does at the start of the next: forget the edges.
fn frame(app: &mut App) {
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
}

fn frames(app: &mut App, count: usize) {
    for _ in 0..count {
        frame(app);
    }
}

fn press(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
}

fn release(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(key);
}

fn scene_gravity(app: &mut App) {
    app.world_mut().spawn(GravityField::scene(Vec3::NEG_Y * G));
}

fn floor(app: &mut App, surface: Option<Surface>) -> Entity {
    let floor = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(400.0, 1.0, 400.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ))
        .id();
    if let Some(surface) = surface {
        app.world_mut().entity_mut(floor).insert(surface);
    }
    floor
}

fn player(app: &mut App, at: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            CharacterController::default(),
            PlayerControlled,
            Transform::from_translation(at),
        ))
        .id()
}

fn position(app: &App, entity: Entity) -> Vec3 {
    app.world().get::<Position>(entity).unwrap().0
}

fn velocity(app: &App, entity: Entity) -> Vec3 {
    app.world().get::<LinearVelocity>(entity).unwrap().0
}

fn state(app: &App, entity: Entity) -> CharacterState {
    *app.world().get::<CharacterState>(entity).unwrap()
}

/// Up axis of the body as the physics engine has it.
fn body_up(app: &App, entity: Entity) -> Vec3 {
    app.world().get::<Rotation>(entity).unwrap().0 * Vec3::Y
}

#[test]
fn keyboard_becomes_input_actions() {
    let mut app = app();
    press(&mut app, KeyCode::KeyW);
    press(&mut app, KeyCode::KeyD);
    frame(&mut app);
    let actions = app.world().resource::<InputActions>().clone();
    assert!(
        (actions.movement.length() - 1.0).abs() < 1e-6,
        "diagonals are normalized"
    );
    assert!(actions.movement.x > 0.0 && actions.movement.y > 0.0);

    release(&mut app, KeyCode::KeyD);
    press(&mut app, KeyCode::KeyA);
    press(&mut app, KeyCode::KeyD);
    frame(&mut app);
    assert_eq!(
        app.world().resource::<InputActions>().movement,
        Vec2::Y,
        "opposites cancel"
    );

    press(&mut app, KeyCode::Space);
    frame(&mut app);
    let jump = app.world().resource::<InputActions>().jump;
    assert!(jump.pressed && jump.held && !jump.released);
    frame(&mut app);
    let jump = app.world().resource::<InputActions>().jump;
    assert!(!jump.pressed && jump.held, "the press edge lasts one frame");
    release(&mut app, KeyCode::Space);
    frame(&mut app);
    let jump = app.world().resource::<InputActions>().jump;
    assert!(jump.released && !jump.held);

    app.insert_resource(AccumulatedMouseMotion {
        delta: Vec2::new(100.0, -50.0),
    });
    frame(&mut app);
    let look = app.world().resource::<InputActions>().look;
    assert!(look.abs_diff_eq(Vec2::new(0.3, -0.15), 1e-6), "{look}");
}

#[test]
fn bindings_can_be_swapped() {
    let mut app = app();
    app.world_mut().resource_mut::<InputMap>().jump = vec![Binding::Mouse(MouseButton::Right)];
    press(&mut app, KeyCode::Space);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Right);
    frame(&mut app);
    assert!(app.world().resource::<InputActions>().jump.pressed);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Right);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .clear();
    release(&mut app, KeyCode::Space);
    frame(&mut app);
    assert!(
        !app.world().resource::<InputActions>().jump.held,
        "Space is no longer bound"
    );
}

#[test]
fn only_player_controlled_characters_read_input() {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, None);
    let human = player(&mut app, Vec3::new(-3.0, FEET, 0.0));
    // A non-player character is driven by writing its intent, as AI or the network would.
    let bot = app
        .world_mut()
        .spawn((
            CharacterController::default(),
            Transform::from_xyz(3.0, FEET, 0.0),
        ))
        .id();
    frames(&mut app, 30);

    press(&mut app, KeyCode::KeyW);
    app.world_mut()
        .get_mut::<CharacterIntent>(bot)
        .unwrap()
        .movement = Vec2::X;
    frames(&mut app, 60);

    assert!(position(&app, human).z < -3.0, "the player walked forward");
    assert!(position(&app, bot).x > 5.0, "the bot followed its intent");
    assert!(
        position(&app, bot).z.abs() < 0.05,
        "and ignored the keyboard"
    );
    let intent = app.world().get::<CharacterIntent>(bot).unwrap();
    assert_eq!(intent.movement, Vec2::X);
}

#[test]
fn walks_on_a_flat_floor() {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, None);
    let hero = player(&mut app, Vec3::new(0.0, FEET + 1.0, 0.0));
    frames(&mut app, 60);
    let rest = position(&app, hero);
    assert!((rest.y - FEET).abs() < 0.05, "standing at {rest}");
    assert!(state(&app, hero).grounded);
    assert!(velocity(&app, hero).length() < 0.05);

    press(&mut app, KeyCode::KeyW);
    frames(&mut app, 120);
    let p = position(&app, hero);
    let controller = CharacterController::default();
    assert!((velocity(&app, hero).length() - controller.move_speed).abs() < 0.2);
    assert!(p.z < -8.0 && p.x.abs() < 0.05, "walked forward to {p}");
    assert!((p.y - FEET).abs() < 0.05, "stayed on the floor: {p}");
    assert!((body_up(&app, hero) - Vec3::Y).length() < 0.01);

    release(&mut app, KeyCode::KeyW);
    frames(&mut app, 30);
    assert!(
        velocity(&app, hero).length() < 0.05,
        "stops on a grippy floor"
    );
}

#[test]
fn movement_is_relative_to_the_heading() {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, None);
    let hero = player(&mut app, Vec3::new(0.0, FEET, 0.0));
    frames(&mut app, 20);

    // Turn right a quarter turn with the mouse: forward becomes +X.
    let quarter_turn = core::f32::consts::FRAC_PI_2;
    app.insert_resource(AccumulatedMouseMotion {
        delta: Vec2::new(quarter_turn / InputMap::default().look_sensitivity, 0.0),
    });
    frame(&mut app);
    app.insert_resource(AccumulatedMouseMotion::default());
    press(&mut app, KeyCode::KeyW);
    frames(&mut app, 60);

    let p = position(&app, hero);
    assert!(p.x > 3.0 && p.z.abs() < 0.2, "walked along +X: {p}");
    let look = app.world().get::<CharacterLook>(hero).unwrap();
    assert!(look.forward.abs_diff_eq(Vec3::X, 1e-3), "{}", look.forward);
    // The body turned with it: local -Z points along the heading.
    let facing = app.world().get::<Rotation>(hero).unwrap().0 * Vec3::NEG_Z;
    assert!(facing.dot(Vec3::X) > 0.99, "{facing}");
}

#[test]
fn jump_reaches_the_configured_height() {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, None);
    let hero = player(&mut app, Vec3::new(0.0, FEET, 0.0));
    frames(&mut app, 30);
    let ground = position(&app, hero).y;

    press(&mut app, KeyCode::Space);
    frame(&mut app);
    release(&mut app, KeyCode::Space);
    let mut apex = ground;
    let mut left_the_ground = false;
    for _ in 0..90 {
        frame(&mut app);
        apex = apex.max(position(&app, hero).y);
        left_the_ground |= !state(&app, hero).grounded;
    }
    let height = apex - ground;
    let expected = CharacterController::default().jump_height;
    assert!(left_the_ground);
    assert!(
        (height - expected).abs() < 0.1,
        "jumped {height}, expected {expected}"
    );
    assert!(state(&app, hero).grounded, "landed again");
    assert!((position(&app, hero).y - ground).abs() < 0.05);

    // Pressing jump in the air does nothing later either: one press, one jump.
    frames(&mut app, 30);
    assert!((position(&app, hero).y - ground).abs() < 0.05);
}

#[test]
fn a_jump_press_between_fixed_ticks_is_not_lost() {
    // 240 Hz frames over a 60 Hz simulation: three of four frames run no tick.
    for phase in 0..4 {
        let mut app = app();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 240.0,
        )));
        scene_gravity(&mut app);
        floor(&mut app, None);
        let hero = player(&mut app, Vec3::new(0.0, FEET, 0.0));
        frames(&mut app, 120 + phase);
        let ground = position(&app, hero).y;

        press(&mut app, KeyCode::Space);
        frame(&mut app);
        release(&mut app, KeyCode::Space);
        let mut apex = ground;
        for _ in 0..240 {
            frame(&mut app);
            apex = apex.max(position(&app, hero).y);
        }
        assert!(
            apex - ground > 1.0,
            "phase {phase}: jumped only {}",
            apex - ground
        );
    }
}

/// The character is shoved to 5 m/s and then left alone: how far does it get?
fn distance_when_shoved(surface: Option<Surface>) -> f32 {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, surface);
    let hero = player(&mut app, Vec3::new(0.0, FEET, 0.0));
    frames(&mut app, 20);
    let start = position(&app, hero).z;
    app.world_mut().get_mut::<LinearVelocity>(hero).unwrap().0 = Vec3::NEG_Z * 5.0;
    frames(&mut app, 90);
    start - position(&app, hero).z
}

#[test]
fn slippery_floor_makes_the_character_slide() {
    let grippy = distance_when_shoved(None);
    let slippery = distance_when_shoved(Some(Surface::slippery()));
    assert!(grippy < 0.5, "stops quickly on grippy floor: {grippy}");
    assert!(slippery > 5.0, "slides on ice: {slippery}");
}

#[test]
fn slippery_floor_limits_acceleration() {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, Some(Surface::slippery()));
    let hero = player(&mut app, Vec3::new(0.0, FEET, 0.0));
    frames(&mut app, 20);
    press(&mut app, KeyCode::KeyW);
    frames(&mut app, 30);
    assert_eq!(state(&app, hero).ground_friction, 0.02);
    let speed = velocity(&app, hero).length();
    assert!(speed < 1.0, "barely gets going on ice: {speed}");
}

fn spawn_planet(app: &mut App, radius: f32) {
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::sphere(radius),
        GravityField::planet(G, radius * 10.0),
        Transform::default(),
    ));
}

#[test]
fn walks_around_a_small_planet_keeping_up_aligned() {
    let radius = 6.0;
    let mut app = app();
    spawn_planet(&mut app, radius);
    // Start on the side and the top, so nothing about "up" is world Y.
    let hero = player(&mut app, Vec3::new(radius + FEET + 0.5, 0.0, 0.0));
    frames(&mut app, 60);
    let rest = position(&app, hero);
    assert!(
        (rest.length() - (radius + FEET)).abs() < 0.05,
        "standing at {rest}"
    );

    press(&mut app, KeyCode::KeyW);
    let mut travelled = 0.0;
    let mut previous = position(&app, hero).normalize();
    for i in 0..600 {
        frame(&mut app);
        let p = position(&app, hero);
        let radial = p.normalize();
        travelled += previous.angle_between(radial);
        previous = radial;

        let height = p.length() - radius;
        assert!(
            (height - FEET).abs() < 0.15,
            "tick {i}: hopped off the surface: {height}"
        );
        let up = app.world().get::<LocalUp>(hero).unwrap().0;
        assert!(
            up.dot(radial) > 0.999,
            "tick {i}: local up {up} is not radial"
        );
        assert!(
            body_up(&app, hero).dot(radial) > 0.98,
            "tick {i}: body up {} lags radial {radial}",
            body_up(&app, hero)
        );
        assert!(state(&app, hero).grounded, "tick {i}: lost the ground");
    }
    // 10 s at 5 m/s over a radius of 6.8: more than 3/4 of the way around.
    assert!(
        travelled > 1.5 * core::f32::consts::PI,
        "only went {travelled} rad around"
    );
}

#[test]
fn jumps_along_local_up_on_a_planet() {
    let radius = 6.0;
    let mut app = app();
    spawn_planet(&mut app, radius);
    let hero = player(&mut app, Vec3::new(0.0, radius + FEET, 0.0));
    frames(&mut app, 60);
    press(&mut app, KeyCode::Space);
    frame(&mut app);
    release(&mut app, KeyCode::Space);
    let mut apex: f32 = 0.0;
    for _ in 0..90 {
        frame(&mut app);
        apex = apex.max(position(&app, hero).length() - radius - FEET);
    }
    assert!((apex - 1.2).abs() < 0.15, "jumped {apex} above the surface");
    let p = position(&app, hero);
    assert!(p.xz().length() < 0.1, "went straight up and down: {p}");
}

fn pool(app: &mut App, depth: f32) {
    scene_gravity(app);
    let floor_top = -depth;
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 1.0, 100.0),
        Transform::from_xyz(0.0, floor_top - 0.5, 0.0),
    ));
    app.world_mut().spawn((
        water(VolumeShape::Box {
            half_extents: Vec3::new(20.0, depth / 2.0, 20.0),
        }),
        Transform::from_xyz(0.0, -depth / 2.0, 0.0),
    ));
}

#[test]
fn character_sinks_slowly_and_swims_up_when_jump_is_held() {
    let mut app = app();
    pool(&mut app, 6.0);
    let hero = player(&mut app, Vec3::new(0.0, 1.0, 0.0));
    // Only a little denser than water, so it sinks at well under a meter per second.
    frames(&mut app, 720);
    let bottom = position(&app, hero).y;
    assert!(
        (bottom - (-6.0 + FEET)).abs() < 0.1,
        "denser than water: rests on the bottom at {bottom}"
    );
    assert!(state(&app, hero).swimming);
    assert_eq!(app.world().get::<Submersion>(hero).unwrap().0, 1.0);

    press(&mut app, KeyCode::Space);
    frames(&mut app, 180);
    let y = position(&app, hero).y;
    assert!(y > -1.5, "swam up to {y}");
    assert!(
        velocity(&app, hero).length() < 4.0,
        "water drag keeps speeds low"
    );

    // Swimming is horizontal too.
    release(&mut app, KeyCode::Space);
    press(&mut app, KeyCode::KeyW);
    let z = position(&app, hero).z;
    frames(&mut app, 60);
    assert!(position(&app, hero).z < z - 1.0);
}

#[test]
fn light_character_floats_at_the_surface() {
    let mut app = app();
    pool(&mut app, 6.0);
    let hero = app
        .world_mut()
        .spawn((
            CharacterController::default(),
            ColliderDensity(500.0),
            Transform::from_xyz(0.0, -4.0, 0.0),
        ))
        .id();
    frames(&mut app, 600);
    // Half of the 1.6 m capsule below the surface.
    let y = position(&app, hero).y;
    assert!(y.abs() < 0.15, "floating at {y}");
    let submersion = app.world().get::<Submersion>(hero).unwrap().0;
    assert!((submersion - 0.5).abs() < 0.1, "{submersion}");
}

#[test]
fn characters_are_deterministic() {
    fn run() -> Vec<u32> {
        let mut app = app();
        spawn_planet(&mut app, 6.0);
        let hero = player(&mut app, Vec3::new(6.8, 0.0, 0.0));
        frames(&mut app, 30);
        press(&mut app, KeyCode::KeyW);
        frames(&mut app, 50);
        press(&mut app, KeyCode::Space);
        frame(&mut app);
        release(&mut app, KeyCode::Space);
        frames(&mut app, 200);
        let mut bits: Vec<u32> = position(&app, hero).to_array().map(f32::to_bits).to_vec();
        bits.extend(velocity(&app, hero).to_array().map(f32::to_bits));
        bits
    }
    assert_eq!(run(), run());
}

#[test]
fn camera_movement_frame_is_captured_as_a_command_without_a_camera_in_simulation() {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, None);
    let hero = player(&mut app, Vec3::new(0.0, FEET, 0.0));
    app.add_systems(
        PreUpdate,
        (|mut actions: ResMut<InputActions>| {
            actions.movement_forward = Some(Vec3::X);
        })
        .after(struction_character::InputSystems::Map)
        .before(struction_character::InputSystems::Command),
    );
    frames(&mut app, 20);
    press(&mut app, KeyCode::KeyW);
    frames(&mut app, 60);
    let p = position(&app, hero);
    assert!(p.x > 3.0 && p.z.abs() < 0.1, "camera-relative forward: {p}");
    assert_eq!(
        app.world()
            .get::<CharacterIntent>(hero)
            .unwrap()
            .movement_forward,
        Some(Vec3::X)
    );
    assert!(
        app.world()
            .get::<CharacterLook>(hero)
            .unwrap()
            .forward
            .abs_diff_eq(Vec3::NEG_Z, 1e-4)
    );
    release(&mut app, KeyCode::KeyW);
    press(&mut app, KeyCode::KeyD);
    frames(&mut app, 60);
    assert!(
        position(&app, hero).z > 3.0,
        "screen right follows the command frame"
    );
}

#[test]
fn heading_is_transported_across_a_sharp_gravity_change() {
    let mut app = app();
    let field = app
        .world_mut()
        .spawn(GravityField::scene(Vec3::NEG_Y * G))
        .id();
    let hero = player(&mut app, Vec3::ZERO);
    frames(&mut app, 2);
    let turn = Quat::from_rotation_x(2.0);
    let up = turn * Vec3::Y;
    app.world_mut()
        .entity_mut(field)
        .insert(GravityField::scene(-up * G));
    frame(&mut app);
    let look = app.world().get::<CharacterLook>(hero).unwrap();
    assert!(
        look.forward.abs_diff_eq(turn * Vec3::NEG_Z, 1e-4),
        "{}",
        look.forward
    );
    assert!(look.forward.dot(up).abs() < 1e-4);
    assert!(look.up.abs_diff_eq(up, 1e-4));
}

/// Where a character left idle on a 20 degree slope ends up after three seconds.
fn drift_on_slope(surface: Surface) -> Vec3 {
    let mut app = app();
    scene_gravity(&mut app);
    let tilt = Quat::from_rotation_z(20f32.to_radians());
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 40.0),
        surface,
        Transform::from_translation(tilt * Vec3::NEG_Y * 0.5).with_rotation(tilt),
    ));
    let hero = player(&mut app, Vec3::new(0.0, FEET + 0.1, 0.0));
    frames(&mut app, 60);
    let settled = position(&app, hero);
    frames(&mut app, 180);
    position(&app, hero) - settled
}

#[test]
fn standing_still_holds_on_walkable_slopes() {
    let drift = drift_on_slope(Surface::default());
    assert!(drift.length() < 0.02, "crept {drift} downhill");
}

#[test]
fn slippery_slopes_still_slide() {
    let drift = drift_on_slope(Surface::slippery());
    assert!(drift.x < -1.0, "slid {drift}");
}

#[test]
fn walking_up_a_ramp_stays_grounded() {
    let mut app = app();
    scene_gravity(&mut app);
    // Rises toward -Z, the default heading.
    let tilt = Quat::from_rotation_x(20f32.to_radians());
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 40.0),
        Transform::from_translation(tilt * Vec3::NEG_Y * 0.5).with_rotation(tilt),
    ));
    let hero = player(&mut app, Vec3::new(0.0, FEET + 0.1, 0.0));
    frames(&mut app, 30);
    let start = position(&app, hero);
    press(&mut app, KeyCode::KeyW);
    for tick in 0..90 {
        frame(&mut app);
        assert!(
            state(&app, hero).grounded,
            "airborne on the ramp at tick {tick}"
        );
    }
    let climbed = position(&app, hero) - start;
    assert!(climbed.z < -4.0 && climbed.y > 1.4, "climbed {climbed}");
}

#[test]
fn a_freely_orbiting_camera_turns_the_body_toward_its_movement() {
    let mut app = app();
    scene_gravity(&mut app);
    floor(&mut app, None);
    let hero = player(&mut app, Vec3::new(0.0, FEET, 0.0));
    app.add_systems(
        PreUpdate,
        (|mut actions: ResMut<InputActions>| {
            actions.movement_forward = Some(Vec3::X);
            actions.face_movement = true;
        })
        .after(struction_character::InputSystems::Map)
        .before(struction_character::InputSystems::Command),
    );
    frames(&mut app, 20);
    let heading = |app: &App| app.world().get::<CharacterLook>(hero).unwrap().forward;
    assert!(
        heading(&app).abs_diff_eq(Vec3::NEG_Z, 1e-4),
        "no movement, no turn"
    );
    // Screen-left of a camera facing +X is -Z: the body keeps facing it, then turns to +X.
    press(&mut app, KeyCode::KeyA);
    frames(&mut app, 30);
    assert!(
        heading(&app).abs_diff_eq(Vec3::NEG_Z, 1e-4),
        "{}",
        heading(&app)
    );
    release(&mut app, KeyCode::KeyA);
    press(&mut app, KeyCode::KeyW);
    frame(&mut app);
    let partial = heading(&app);
    assert!(partial.dot(Vec3::NEG_Z) > 0.5, "turns gradually: {partial}");
    frames(&mut app, 30);
    assert!(
        heading(&app).abs_diff_eq(Vec3::X, 1e-3),
        "{}",
        heading(&app)
    );
    assert!(position(&app, hero).x > 1.0);
}
