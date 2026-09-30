mod common;

use bevy::prelude::*;
use common::*;
use struction_ai::*;
use struction_core::*;

/// SplitMix64, so the scattered start is the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

/// Stands in for the movement system: follow the desired velocity exactly.
fn swim(time: Res<Time<Fixed>>, mut fish: Query<(&DesiredVelocity, &mut Transform)>) {
    let dt = time.timestep().as_secs_f32();
    for (velocity, mut transform) in &mut fish {
        transform.translation += velocity.0 * dt;
    }
}

fn school(seed: u64) -> (App, Vec<Entity>) {
    let mut app = test_app();
    app.add_systems(FixedUpdate, swim.after(AiSet::Act).in_set(CoreSet::Invoke));
    let master = app.world_mut().spawn(Flock::default()).id();
    let mut rng = Rng(seed);
    let fish = (0..12)
        .map(|_| {
            let position = Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 2.0;
            let heading = Vec3::new(rng.signed(), rng.signed(), rng.signed()).normalize() * 2.0;
            app.world_mut()
                .spawn((
                    Boid,
                    MasterIs(master),
                    Transform::from_translation(position),
                    DesiredVelocity(heading),
                ))
                .id()
        })
        .collect();
    (app, fish)
}

/// 1 when every fish swims the same way, near 0 when headings cancel out.
fn polarization(app: &App, fish: &[Entity]) -> f32 {
    let sum: Vec3 = fish
        .iter()
        .map(|&f| {
            app.world()
                .get::<DesiredVelocity>(f)
                .unwrap()
                .0
                .normalize_or_zero()
        })
        .sum();
    sum.length() / fish.len() as f32
}

fn positions(app: &App, fish: &[Entity]) -> Vec<Vec3> {
    fish.iter()
        .map(|&f| app.world().get::<Transform>(f).unwrap().translation)
        .collect()
}

#[test]
fn a_school_converges_to_a_common_heading_and_stays_together() {
    let (mut app, fish) = school(42);
    let start = polarization(&app, &fish);
    assert!(start < 0.6, "start {start}");

    steps(&mut app, 64 * 10);

    let end = polarization(&app, &fish);
    assert!(end > 0.95, "polarization {start} -> {end}");
    let positions = positions(&app, &fish);
    let center = positions.iter().sum::<Vec3>() / positions.len() as f32;
    for p in &positions {
        assert!(
            p.distance(center) < 4.0,
            "a fish strayed: {p} from {center}"
        );
    }
    for (i, a) in positions.iter().enumerate() {
        for b in &positions[i + 1..] {
            assert!(a.distance(*b) > 0.2, "fish collided");
        }
    }
    let flock = Flock::default();
    for &f in &fish {
        let speed = app.world().get::<DesiredVelocity>(f).unwrap().0.length();
        assert!(speed >= flock.min_speed - 1e-4 && speed <= flock.max_speed + 1e-4);
    }
}

#[test]
fn flocking_replays_exactly_and_ignores_other_groups() {
    let (mut a, fish_a) = school(7);
    let (mut b, fish_b) = school(7);
    // A boid with another master does not affect this school.
    let stranger_master = b.world_mut().spawn(Flock::default()).id();
    b.world_mut().spawn((
        Boid,
        MasterIs(stranger_master),
        Transform::default(),
        DesiredVelocity(Vec3::X),
    ));
    steps(&mut a, 200);
    steps(&mut b, 200);
    assert_eq!(positions(&a, &fish_a), positions(&b, &fish_b));
}
