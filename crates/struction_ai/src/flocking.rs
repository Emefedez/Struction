//! Boids for groups such as fish schools.
//!
//! A school is a master entity with a [`Flock`]; its members are the wards that have a [`Boid`].
//! Each fixed tick, in [`AiSet::Act`](crate::AiSet::Act), every member steers by separation,
//! alignment and cohesion with the members within `neighbor_radius`, and the result is written
//! to its [`DesiredVelocity`]. Whatever moves the body (a character controller, a swimming
//! system) follows it. The previous desired velocity is the boid's heading, so the rule needs no
//! physics velocity and replays exactly: all members read the same snapshot of the tick.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use struction_core::Wards;

/// Tuning of a group, on its master.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
#[reflect(Component, Default)]
pub struct Flock {
    /// Members closer than this influence each other. Meters.
    pub neighbor_radius: f32,
    /// Members closer than this push each other away. Meters.
    pub separation_radius: f32,
    pub separation: f32,
    pub alignment: f32,
    pub cohesion: f32,
    /// Meters per second.
    pub min_speed: f32,
    pub max_speed: f32,
    /// Largest change of velocity per second.
    pub max_acceleration: f32,
}

impl Default for Flock {
    fn default() -> Self {
        Self {
            neighbor_radius: 4.0,
            separation_radius: 1.0,
            separation: 1.5,
            alignment: 1.0,
            cohesion: 0.5,
            min_speed: 1.0,
            max_speed: 3.0,
            max_acceleration: 4.0,
        }
    }
}

/// Marks a ward that flocks with its master's other boids.
#[derive(Component, Reflect, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[reflect(Component, Default)]
#[require(DesiredVelocity)]
pub struct Boid;

/// The velocity the AI wants; the movement system decides how to achieve it.
#[derive(Component, Reflect, Default, Clone, Copy, PartialEq, Debug)]
#[reflect(Component, Default)]
pub struct DesiredVelocity(pub Vec3);

pub(crate) fn flock(
    time: Res<Time<Fixed>>,
    flocks: Query<(&Flock, &Wards)>,
    mut boids: Query<(&Transform, &mut DesiredVelocity), With<Boid>>,
) {
    let dt = time.timestep().as_secs_f32();
    for (flock, wards) in &flocks {
        let members: Vec<(Entity, Vec3, Vec3)> = wards
            .iter()
            .filter_map(|ward| {
                let (transform, velocity) = boids.get(ward).ok()?;
                Some((ward, transform.translation, velocity.0))
            })
            .collect();
        for &(entity, position, velocity) in &members {
            let steer = steering(flock, position, velocity, &members);
            let mut next = velocity + (steer * dt).clamp_length_max(flock.max_acceleration * dt);
            let speed = next.length();
            if speed > 0.0 {
                next *= speed.clamp(flock.min_speed, flock.max_speed) / speed;
            }
            if let Ok((_, mut desired)) = boids.get_mut(entity) {
                desired.0 = next;
            }
        }
    }
}

fn steering(
    flock: &Flock,
    position: Vec3,
    velocity: Vec3,
    members: &[(Entity, Vec3, Vec3)],
) -> Vec3 {
    let mut away = Vec3::ZERO;
    let mut heading = Vec3::ZERO;
    let mut center = Vec3::ZERO;
    let mut neighbors = 0;
    for &(_, other, other_velocity) in members {
        let offset = position - other;
        let distance = offset.length();
        if distance == 0.0 || distance > flock.neighbor_radius {
            continue;
        }
        neighbors += 1;
        heading += other_velocity;
        center += other;
        if distance < flock.separation_radius {
            // Stronger the closer they are.
            away +=
                offset / distance * (flock.separation_radius - distance) / flock.separation_radius;
        }
    }
    if neighbors == 0 {
        return Vec3::ZERO;
    }
    let n = neighbors as f32;
    let alignment = heading / n - velocity;
    let cohesion = center / n - position;
    away * flock.separation * flock.max_speed
        + alignment * flock.alignment
        + cohesion * flock.cohesion
}
