//! Headless simulation tests: fixed-step loops with a manual clock, no window or GPU.

use avian3d::prelude::*;
use bevy::{prelude::*, time::TimeUpdateStrategy};
use bevy_transform_interpolation::TranslationEasingState;
use core::time::Duration;
use struction_physics::{prelude::*, testing::*};

const G: f32 = 9.81;

fn scene_gravity(app: &mut App) {
    app.world_mut().spawn(GravityField::scene(Vec3::NEG_Y * G));
}

fn floor(app: &mut App, y: f32) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(200.0, 1.0, 200.0),
            Transform::from_xyz(0.0, y - 0.5, 0.0),
        ))
        .id()
}

fn cube(app: &mut App, position: Vec3, density: f32) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            ColliderDensity(density),
            Transform::from_translation(position),
        ))
        .id()
}

fn position(app: &App, entity: Entity) -> Vec3 {
    app.world().get::<Position>(entity).unwrap().0
}

fn physics_seconds(app: &App) -> f32 {
    app.world().resource::<Time<Physics>>().elapsed_secs()
}

#[test]
fn body_falls_under_scene_gravity() {
    let mut app = headless_app();
    scene_gravity(&mut app);
    let ball = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Transform::from_xyz(0.0, 100.0, 0.0),
        ))
        .id();
    step(&mut app, 60);

    let t = physics_seconds(&app);
    assert!(t > 0.9, "the fixed clock advanced: {t}");
    let expected = 100.0 - 0.5 * G * t * t;
    let y = position(&app, ball).y;
    assert!(
        (y - expected).abs() < 0.2,
        "fell to {y}, expected about {expected}"
    );
    assert!(app.world().get::<LocalUp>(ball).unwrap().0 == Dir3::Y);
}

#[test]
fn without_fields_there_is_no_gravity() {
    let mut app = headless_app();
    let ball = cube(&mut app, Vec3::new(0.0, 10.0, 0.0), 1.0);
    step(&mut app, 60);
    assert_eq!(position(&app, ball), Vec3::new(0.0, 10.0, 0.0));
}

#[test]
fn fields_add_up_on_a_body() {
    let mut app = headless_app();
    scene_gravity(&mut app);
    app.world_mut().spawn(GravityField::scene(Vec3::Y * G));
    let ball = cube(&mut app, Vec3::new(0.0, 10.0, 0.0), 1.0);
    step(&mut app, 60);
    assert!(position(&app, ball).y > 9.999, "opposite fields cancel");
}

fn spawn_planet(app: &mut App, radius: f32) {
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::sphere(radius),
        GravityField::planet(G, radius * 8.0),
        Transform::default(),
    ));
}

#[test]
fn body_falls_toward_a_planet_and_stays_on_it() {
    let mut app = headless_app();
    spawn_planet(&mut app, 5.0);
    let from_above = cube(&mut app, Vec3::new(0.0, 12.0, 0.0), 1.0);
    let from_the_side = cube(&mut app, Vec3::new(12.0, 0.0, 0.0), 1.0);
    let from_below = cube(&mut app, Vec3::new(0.0, -12.0, 0.0), 1.0);
    step(&mut app, 400);

    for (body, direction) in [
        (from_above, Vec3::Y),
        (from_the_side, Vec3::X),
        (from_below, Vec3::NEG_Y),
    ] {
        let p = position(&app, body);
        // Resting on a flat face: center at radius + half the cube.
        assert!(
            (p.length() - 5.5).abs() < 0.1,
            "{p} is not resting on the surface"
        );
        assert!(
            p.normalize().dot(direction) > 0.99,
            "{p} drifted along the surface"
        );
        let up = app.world().get::<LocalUp>(body).unwrap().0;
        assert!(up.dot(direction) > 0.99, "local up {up} is not radial");
    }
}

#[test]
fn slippery_floor_slides_and_grippy_floor_stops() {
    fn slide_distance(surface: Surface) -> f32 {
        let mut app = headless_app();
        scene_gravity(&mut app);
        let floor = floor(&mut app, 0.0);
        app.world_mut().entity_mut(floor).insert(surface);
        let block = cube(&mut app, Vec3::new(0.0, 0.5, 0.0), 1.0);
        step(&mut app, 30);
        app.world_mut().get_mut::<LinearVelocity>(block).unwrap().0 = Vec3::X * 5.0;
        let start = position(&app, block).x;
        step(&mut app, 120);
        position(&app, block).x - start
    }

    let grippy = slide_distance(Surface::default());
    let slippery = slide_distance(Surface::slippery());
    assert!(grippy < 3.0, "grippy floor stops the block: {grippy}");
    assert!(slippery > 8.0, "slippery floor barely slows it: {slippery}");
}

#[test]
fn surface_is_mapped_to_avian_material() {
    let mut app = headless_app();
    let floor = floor(&mut app, 0.0);
    app.world_mut().entity_mut(floor).insert(Surface {
        friction: 0.1,
        restitution: 0.5,
        drag: 0.0,
    });
    step(&mut app, 2);
    let friction = app.world().get::<Friction>(floor).unwrap();
    assert_eq!(friction.dynamic_coefficient, 0.1);
    assert_eq!(friction.combine_rule, CoefficientCombine::Multiply);
    assert_eq!(
        app.world().get::<Restitution>(floor).unwrap().coefficient,
        0.5
    );
}

#[test]
fn surface_drag_slows_bodies_sliding_on_it() {
    fn speed_after(drag: f32) -> f32 {
        let mut app = headless_app();
        scene_gravity(&mut app);
        let floor = floor(&mut app, 0.0);
        app.world_mut().entity_mut(floor).insert(Surface {
            friction: 0.0,
            restitution: 0.0,
            drag,
        });
        let block = cube(&mut app, Vec3::new(0.0, 0.5, 0.0), 1.0);
        step(&mut app, 30);
        app.world_mut().get_mut::<LinearVelocity>(block).unwrap().0 = Vec3::X * 5.0;
        step(&mut app, 60);
        app.world().get::<LinearVelocity>(block).unwrap().0.x
    }
    assert!(speed_after(0.0) > 4.9);
    assert!(speed_after(3.0) < 1.0);
}

const POOL_TOP: f32 = 0.0;

/// A 10x4x10 pool whose surface is at y = 0 over a floor at y = -4.
fn pool(app: &mut App, preset: impl Bundle) -> Entity {
    scene_gravity(app);
    floor(app, -4.0);
    app.world_mut()
        .spawn((preset, Transform::from_xyz(0.0, POOL_TOP - 2.0, 0.0)))
        .id()
}

fn pool_shape() -> VolumeShape {
    VolumeShape::Box {
        half_extents: Vec3::new(5.0, 2.0, 5.0),
    }
}

#[test]
fn buoyant_cube_floats_in_water() {
    let mut app = headless_app();
    pool(&mut app, water(pool_shape()));
    // Half the density of water: floats half submerged.
    let block = cube(&mut app, Vec3::new(0.0, 2.0, 0.0), 500.0);
    step(&mut app, 600);

    let p = position(&app, block);
    assert!((p.y - POOL_TOP).abs() < 0.05, "floating at {p}");
    assert!(p.xz().length() < 0.05, "drifted sideways: {p}");
    let submerged = app.world().get::<Submersion>(block).unwrap().0;
    assert!((submerged - 0.5).abs() < 0.05, "submersion {submerged}");
    assert!(app.world().get::<LinearVelocity>(block).unwrap().0.length() < 0.05);
}

#[test]
fn dense_cube_sinks_in_water() {
    let mut app = headless_app();
    pool(&mut app, water(pool_shape()));
    let block = cube(&mut app, Vec3::new(0.0, 2.0, 0.0), 2000.0);
    step(&mut app, 600);

    let p = position(&app, block);
    assert!(
        (p.y - -3.5).abs() < 0.05,
        "resting on the pool floor at {p}"
    );
    assert_eq!(app.world().get::<Submersion>(block).unwrap().0, 1.0);
}

#[test]
fn drag_slows_bodies_in_a_volume() {
    fn speed_after(preset: impl Bundle) -> f32 {
        let mut app = headless_app();
        app.world_mut().spawn((preset, Transform::default()));
        let body = cube(&mut app, Vec3::ZERO, 1000.0);
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = Vec3::X * 5.0;
        step(&mut app, 60);
        app.world().get::<LinearVelocity>(body).unwrap().0.x
    }
    let shape = VolumeShape::Sphere { radius: 3.0 };
    let dry = speed_after(());
    let wet = speed_after(water(shape));
    assert!(dry > 4.99);
    assert!(wet < 1.5, "water drags: {wet}");
}

#[test]
fn radial_volume_buoys_against_local_gravity() {
    // An ocean ball around a planet: buoyancy follows each body's own gravity.
    let mut app = headless_app();
    spawn_planet(&mut app, 4.0);
    app.world_mut().spawn((
        water(VolumeShape::Sphere { radius: 6.0 }),
        Transform::default(),
    ));
    let block = cube(&mut app, Vec3::new(0.0, 9.0, 0.0), 500.0);
    step(&mut app, 900);
    let p = position(&app, block);
    // Half submerged in an ocean of radius 6 (the planet's floor is 4, cube half size 0.5).
    assert!((p.length() - 6.0).abs() < 0.1, "floating at {p}");
}

#[test]
fn lava_emits_damage_for_other_packages() {
    #[derive(Resource, Default)]
    struct Taken(f32, Vec<Entity>);

    let mut app = headless_app();
    pool(&mut app, lava(pool_shape()));
    app.init_resource::<Taken>().add_observer(
        |damage: On<VolumeDamage>, mut taken: ResMut<Taken>| {
            taken.0 += damage.amount;
            taken.1.push(damage.body);
        },
    );
    let block = cube(&mut app, Vec3::new(0.0, 2.0, 0.0), 500.0);
    let dry = cube(&mut app, Vec3::new(20.0, 2.0, 0.0), 500.0);
    step(&mut app, 300);

    let taken = app.world().resource::<Taken>();
    assert!(taken.0 > 25.0, "took {} damage in lava", taken.0);
    assert!(
        taken.1.iter().all(|&body| body == block),
        "only the body in lava is hurt"
    );
    assert!(!taken.1.contains(&dry));
}

fn camera_zone(app: &mut App, center: Vec3, priority: i32) -> Entity {
    app.world_mut()
        .spawn((
            CameraZone::bundle(
                Volume {
                    shape: VolumeShape::Box {
                        half_extents: Vec3::new(2.0, 2.0, 2.0),
                    },
                },
                CameraConstraint {
                    mode: CameraMode::Follow {
                        distance: 6.0,
                        pitch: 0.4,
                    },
                    weight: 1.0,
                    priority,
                },
            ),
            Transform::from_translation(center),
        ))
        .id()
}

#[test]
fn camera_zone_detects_the_tracked_entity() {
    let mut app = headless_app();
    let low = camera_zone(&mut app, Vec3::new(10.0, 0.0, 0.0), 0);
    let high = camera_zone(&mut app, Vec3::new(10.0, 0.0, 0.0), 5);
    let player = cube(&mut app, Vec3::new(0.0, 0.0, 0.0), 1.0);
    let bystander = cube(&mut app, Vec3::new(10.0, 0.0, 0.0), 1.0);
    app.world_mut()
        .entity_mut(player)
        .insert((CameraTarget, LinearVelocity(Vec3::X * 5.0)));

    step(&mut app, 5);
    assert_eq!(
        app.world().get::<InCameraZones>(player).unwrap().active(),
        None
    );
    assert!(app.world().get::<InCameraZones>(bystander).is_none());

    // Enters at x = 8 after about 1.2 s.
    step(&mut app, 90);
    let zones = app.world().get::<InCameraZones>(player).unwrap();
    assert_eq!(zones.0, vec![high, low], "highest priority first");
    assert_eq!(zones.active(), Some(high));
    let constraint = app.world().get::<CameraZone>(high).unwrap().constraint;
    assert_eq!(constraint.priority, 5);

    // Leaves at x = 12 after about 2.5 s.
    step(&mut app, 120);
    assert_eq!(
        app.world().get::<InCameraZones>(player).unwrap().active(),
        None
    );
}

#[test]
fn camera_volumes_do_not_submerge_bodies_but_overlapping_water_does() {
    let mut app = headless_app();
    let zone = camera_zone(&mut app, Vec3::ZERO, 0);
    let body = cube(&mut app, Vec3::ZERO, 1000.0);
    app.world_mut().entity_mut(body).insert(CameraTarget);
    step(&mut app, 5);
    assert_eq!(
        app.world().get::<InCameraZones>(body).unwrap().active(),
        Some(zone)
    );
    assert_eq!(app.world().get::<Submersion>(body).unwrap().0, 0.0);

    let fluid = app
        .world_mut()
        .spawn((
            water(VolumeShape::Box {
                half_extents: Vec3::splat(2.0),
            }),
            Transform::default(),
        ))
        .id();
    step(&mut app, 5);
    assert_eq!(app.world().get::<Submersion>(body).unwrap().0, 1.0);
    app.world_mut().entity_mut(fluid).despawn();
    step(&mut app, 5);
    assert_eq!(app.world().get::<Submersion>(body).unwrap().0, 0.0);
    assert_eq!(
        app.world().get::<InCameraZones>(body).unwrap().active(),
        Some(zone)
    );
}

#[test]
fn bodies_are_interpolated_between_fixed_ticks() {
    // Render at 144 Hz over a 60 Hz simulation.
    let mut app = headless_app();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 144.0,
    )));
    scene_gravity(&mut app);
    let ball = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Transform::from_xyz(0.0, 100.0, 0.0),
        ))
        .id();
    assert!(app.world().get::<TranslationInterpolation>(ball).is_some());

    let mut between_ticks = 0;
    for _ in 0..120 {
        app.update();
        let simulated = position(&app, ball).y;
        let shown = app.world().get::<Transform>(ball).unwrap().translation.y;
        // Presentation trails the simulation while the body falls, never leads it.
        assert!(
            shown >= simulated - 1e-4,
            "shown {shown} ahead of simulated {simulated}"
        );
        if shown > simulated + 1e-4 {
            between_ticks += 1;
        }
    }
    assert!(
        between_ticks > 50,
        "only {between_ticks} frames were interpolated"
    );

    let state = app.world().get::<TranslationEasingState>(ball).unwrap();
    assert!(state.start.is_some() && state.end.is_some());
}

/// Small deterministic generator so runs can be compared bit for bit.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

fn scatter_and_run() -> Vec<[u32; 3]> {
    let mut app = headless_app();
    spawn_planet(&mut app, 5.0);
    pool(&mut app, water(pool_shape()));
    let mut random = Lcg(7);
    let bodies: Vec<Entity> = (0..12)
        .map(|_| {
            let at = Vec3::new(
                random.next() * 8.0 - 4.0,
                random.next() * 8.0,
                random.next() * 8.0 - 4.0,
            );
            let density = 300.0 + random.next() * 2500.0;
            cube(&mut app, at + Vec3::Y * 8.0, density)
        })
        .collect();
    step(&mut app, 240);
    bodies
        .into_iter()
        .map(|body| position(&app, body).to_array().map(f32::to_bits))
        .collect()
}

#[test]
fn simulation_is_identical_from_run_to_run() {
    let first = scatter_and_run();
    let second = scatter_and_run();
    assert_eq!(first, second);
}
