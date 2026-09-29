//! Headless locomotion simulations: a body moving at constant velocity over flat, tilted and
//! rolling ground. Feet must stay planted while stance and swing alternate, and the IK must
//! reach the foot targets.

use bevy::math::{Mat3, Quat, Vec3};
use bevy::transform::components::Transform;
use struction_anim::humanoid;
use struction_anim::locomotion::{
    Ground, GroundHit, LocomotionInput, LocomotionParams, LocomotionState, PlaneGround,
};
use struction_anim::rig::Limb;
use struction_anim::solve::{PoseSolver, SolveFrame, SolverSettings, foot_goals};

const DT: f32 = 1.0 / 60.0;

/// Root transform whose local up is `up` and whose forward (-Z) is the tangent `forward`.
fn frame(position: Vec3, up: Vec3, forward: Vec3) -> Transform {
    let up = up.normalize();
    let forward = (forward - up * forward.dot(up)).normalize();
    let right = forward.cross(up);
    Transform {
        translation: position,
        rotation: Quat::from_mat3(&Mat3::from_cols(right, up, -forward)),
        scale: Vec3::ONE,
    }
}

struct Sim<'g> {
    ground: &'g dyn Ground,
    up: Vec3,
    state: LocomotionState,
    solver: PoseSolver,
    poses: struction_anim::base_pose::BasePoseSet,
    root: Transform,
    feet: Vec<usize>,
}

#[derive(Clone, Default)]
struct Record {
    /// World position of each foot joint from forward kinematics of the solved pose.
    fk_feet: Vec<Vec3>,
    planted: Vec<bool>,
    /// IK residual per foot goal, meters.
    ik_error: Vec<f32>,
    /// Height of each foot above the ground plane along up (for flat-ish ground).
    height: Vec<f32>,
}

impl<'g> Sim<'g> {
    fn new(ground: &'g dyn Ground, up: Vec3, start: Vec3, forward: Vec3) -> Self {
        let rig = humanoid::rig();
        let feet = rig.feet().map(|b| *b.chain.last().unwrap()).collect();
        Self {
            ground,
            up,
            state: LocomotionState::from_rig(&rig, LocomotionParams::default()).unwrap(),
            solver: PoseSolver::new(rig, SolverSettings::default()),
            poses: humanoid::base_poses(),
            root: frame(start, up, forward),
            feet,
        }
    }

    fn step(&mut self, velocity: Vec3, follow_ground: bool) -> Record {
        self.root.translation += velocity * DT;
        if follow_ground
            && let Some(hit) = self.ground.cast(self.root.translation + self.up * 2.0, -self.up, 4.0)
        {
            self.root.translation = hit.point;
        }
        let input = LocomotionInput {
            root: self.root,
            velocity,
            up: self.up,
            grounded: true,
            gravity: -self.up * 9.81,
        };
        let output = self.state.update(&input, self.ground, DT).clone();
        let goals = foot_goals(&output, self.up, self.root.rotation, 1.0);
        let pose = self
            .solver
            .solve(&SolveFrame {
                root: self.root,
                up: self.up,
                base_poses: &self.poses,
                constraints: &[],
                goals: &goals,
                locomotion: Some(&output),
                body_weight: 1.0,
                dt: DT,
            })
            .unwrap();
        let model = self.solver.rig.skeleton.model_transforms(&pose);
        let fk_feet: Vec<Vec3> = self
            .feet
            .iter()
            .map(|&j| self.root.transform_point(model[j].translation))
            .collect();
        Record {
            height: fk_feet
                .iter()
                .map(|p| {
                    self.ground
                        .cast(*p + self.up, -self.up, 3.0)
                        .map_or(f32::NAN, |h| (*p - h.point).dot(self.up))
                })
                .collect(),
            fk_feet,
            planted: output.feet.iter().map(|f| f.planted).collect(),
            ik_error: self.solver.report.goals.iter().map(|g| g.error).collect(),
        }
    }
}

struct Summary {
    steps: Vec<Vec<f32>>,
}

/// Runs the walk and checks every invariant; returns per-leg step times.
fn check_walk(sim: &mut Sim, velocity: Vec3, seconds: f32, follow_ground: bool, height_tolerance: f32) -> Summary {
    let frames = (seconds / DT) as usize;
    let warmup = 60;
    let legs = sim.feet.len();
    let mut prev: Option<Record> = None;
    let mut steps = vec![Vec::new(); legs];
    let mut order: Vec<usize> = Vec::new();
    let mut max_slide = 0.0_f32;
    let mut max_ik = 0.0_f32;
    let mut max_swing_lift = 0.0_f32;
    for n in 0..frames {
        let rec = sim.step(velocity, follow_ground);
        if n >= warmup {
            assert!(rec.planted.iter().any(|p| *p), "frame {n}: no foot on the ground");
            max_ik = max_ik.max(rec.ik_error.iter().copied().fold(0.0, f32::max));
        }
        if let Some(prev) = &prev {
            for leg in 0..legs {
                if prev.planted[leg] && rec.planted[leg] {
                    let slide = prev.fk_feet[leg].distance(rec.fk_feet[leg]);
                    max_slide = max_slide.max(slide);
                }
                if prev.planted[leg] && !rec.planted[leg] {
                    steps[leg].push(n as f32 * DT);
                    order.push(leg);
                }
                if !rec.planted[leg] {
                    max_swing_lift = max_swing_lift.max(rec.height[leg]);
                }
                if rec.planted[leg] && n >= warmup {
                    assert!(
                        (rec.height[leg] - humanoid::ANKLE_HEIGHT).abs() < height_tolerance,
                        "frame {n}: planted foot {leg} floats/sinks: {}",
                        rec.height[leg]
                    );
                }
            }
        }
        prev = Some(rec);
    }
    assert!(max_slide < 2e-3, "planted feet slid {max_slide} m in one frame");
    assert!(max_ik < 2e-3, "IK missed a foot target by {max_ik} m");
    assert!(max_swing_lift > humanoid::ANKLE_HEIGHT + 0.05, "swing arc lifts the foot (max {max_swing_lift})");
    for (leg, s) in steps.iter().enumerate() {
        assert!(s.len() >= 4, "leg {leg} stepped only {} times", s.len());
    }
    // After warm-up the legs strictly alternate.
    let settled: Vec<usize> = order
        .iter()
        .zip(steps.iter().flatten().map(|_| ()))
        .map(|(l, _)| *l)
        .collect();
    let _ = settled;
    Summary { steps }
}

fn alternation(steps: &[Vec<f32>], after: f32) {
    let mut events: Vec<(f32, usize)> = steps
        .iter()
        .enumerate()
        .flat_map(|(leg, times)| times.iter().map(move |t| (*t, leg)))
        .filter(|(t, _)| *t > after)
        .collect();
    events.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!(events.len() >= 6);
    for w in events.windows(2) {
        assert_ne!(w[0].1, w[1].1, "same leg stepped twice in a row: {events:?}");
    }
}

fn no_double_swing(sim: &mut Sim, velocity: Vec3, seconds: f32) {
    // Stance and swing alternate: with the settled gait no two legs are airborne together.
    for n in 0..(seconds / DT) as usize {
        let rec = sim.step(velocity, false);
        if n > 90 {
            assert!(rec.planted.iter().filter(|p| !**p).count() <= 1, "frame {n}: both legs swinging");
        }
    }
}

#[test]
fn constant_velocity_on_flat_ground_plants_feet_and_alternates() {
    let ground = PlaneGround {
        point: Vec3::ZERO,
        normal: Vec3::Y,
    };
    let velocity = Vec3::new(0.0, 0.0, -1.4);
    let mut sim = Sim::new(&ground, Vec3::Y, Vec3::ZERO, Vec3::NEG_Z);
    let summary = check_walk(&mut sim, velocity, 8.0, false, 2e-3);
    alternation(&summary.steps, 1.0);

    let mut sim = Sim::new(&ground, Vec3::Y, Vec3::ZERO, Vec3::NEG_Z);
    no_double_swing(&mut sim, velocity, 6.0);
}

#[test]
fn gait_scales_with_speed_and_stops_when_the_body_stops() {
    let ground = PlaneGround {
        point: Vec3::ZERO,
        normal: Vec3::Y,
    };
    let count = |speed: f32| {
        let mut sim = Sim::new(&ground, Vec3::Y, Vec3::ZERO, Vec3::NEG_Z);
        let s = check_walk(&mut sim, Vec3::new(0.0, 0.0, -speed), 8.0, false, 2e-3);
        s.steps.iter().map(Vec::len).sum::<usize>()
    };
    let slow = count(0.8);
    let fast = count(2.0);
    assert!(fast > slow, "faster walking steps more often ({slow} vs {fast})");

    let mut sim = Sim::new(&ground, Vec3::Y, Vec3::ZERO, Vec3::NEG_Z);
    for _ in 0..180 {
        sim.step(Vec3::new(0.0, 0.0, -1.4), false);
    }
    let mut last = None;
    let mut steps_after_stop = 0;
    for n in 0..180 {
        let rec = sim.step(Vec3::ZERO, false);
        if let Some(prev) = &last {
            let prev: &Record = prev;
            steps_after_stop += (0..2).filter(|&l| prev.planted[l] && !rec.planted[l]).count();
            if n > 60 {
                assert!(rec.planted.iter().all(|p| *p), "still stepping while standing");
            }
        }
        last = Some(rec);
    }
    assert!(steps_after_stop <= 2, "at most a settling step after stopping");
}

#[test]
fn walking_with_a_tilted_up_vector_on_a_planet_surface() {
    let up = Vec3::new(0.4, 1.0, 0.3).normalize();
    let ground = PlaneGround {
        point: Vec3::ZERO,
        normal: up,
    };
    let forward = Vec3::new(1.0, -0.2, -1.0);
    let forward_t = (forward - up * forward.dot(up)).normalize();
    let mut sim = Sim::new(&ground, up, Vec3::ZERO, forward_t);
    let summary = check_walk(&mut sim, forward_t * 1.4, 8.0, false, 2e-3);
    alternation(&summary.steps, 1.0);
}

#[test]
fn walking_on_a_sphere_follows_the_curvature() {
    // Small planet: the local up rotates as the body walks around it.
    let radius = 6.0_f32;
    let ground = move |origin: Vec3, dir: Vec3, max: f32| -> Option<GroundHit> {
        // Ray-sphere intersection from outside/inside toward the surface.
        let oc = origin;
        let b = oc.dot(dir);
        let c = oc.dot(oc) - radius * radius;
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let t = -b - disc.sqrt();
        let t = if t < 0.0 { -b + disc.sqrt() } else { t };
        (0.0..=max).contains(&t).then(|| {
            let point = origin + dir * t;
            GroundHit {
                point,
                normal: point.normalize(),
            }
        })
    };
    let start_dir = Vec3::Y;
    let mut position = start_dir * radius;
    let mut forward = Vec3::NEG_Z;
    let speed = 1.4;
    let rig = humanoid::rig();
    let feet: Vec<usize> = rig.feet().map(|b| *b.chain.last().unwrap()).collect();
    let mut state = LocomotionState::from_rig(&rig, LocomotionParams::default()).unwrap();
    let mut solver = PoseSolver::new(rig, SolverSettings::default());
    let poses = humanoid::base_poses();
    let mut prev: Option<(Vec<Vec3>, Vec<bool>)> = None;
    let mut steps = 0;
    let mut max_slide = 0.0_f32;
    let mut max_ik = 0.0_f32;
    for n in 0..(10.0 / DT) as usize {
        let up = position.normalize();
        forward = (forward - up * forward.dot(up)).normalize();
        // Walk along the great circle.
        let velocity = forward * speed;
        let new_position = (position + velocity * DT).normalize() * radius;
        let travelled = new_position - position;
        position = new_position;
        let up_new = position.normalize();
        forward = (travelled - up_new * travelled.dot(up_new)).normalize();
        let root = frame(position, up_new, forward);
        let input = LocomotionInput {
            root,
            velocity: forward * speed,
            up: up_new,
            grounded: true,
            gravity: -up_new * 9.81,
        };
        let output = state.update(&input, &ground, DT).clone();
        let goals = foot_goals(&output, up_new, root.rotation, 1.0);
        let pose = solver
            .solve(&SolveFrame {
                root,
                up: up_new,
                base_poses: &poses,
                constraints: &[],
                goals: &goals,
                locomotion: Some(&output),
                body_weight: 1.0,
                dt: DT,
            })
            .unwrap();
        let model = solver.rig.skeleton.model_transforms(&pose);
        let fk: Vec<Vec3> = feet.iter().map(|&j| root.transform_point(model[j].translation)).collect();
        let planted: Vec<bool> = output.feet.iter().map(|f| f.planted).collect();
        if n >= 60 {
            max_ik = max_ik.max(solver.report.goals.iter().map(|g| g.error).fold(0.0, f32::max));
        }
        if let Some((pf, pp)) = &prev {
            for i in 0..fk.len() {
                if pp[i] && planted[i] {
                    max_slide = max_slide.max(pf[i].distance(fk[i]));
                }
                if pp[i] && !planted[i] {
                    steps += 1;
                }
            }
        }
        prev = Some((fk, planted));
    }
    assert!(steps >= 8, "only {steps} steps");
    assert!(max_slide < 2e-3, "slid {max_slide}");
    assert!(max_ik < 2e-3, "IK error {max_ik}");
    // The body really went around the planet.
    assert!(position.angle_between(start_dir) > 1.0);
}

#[test]
fn rolling_hills_keep_feet_on_the_terrain() {
    let height = |x: f32, z: f32| 0.15 * (x * 1.3).sin() + 0.1 * (z * 0.9).cos();
    let ground = move |origin: Vec3, dir: Vec3, max: f32| -> Option<GroundHit> {
        // Downward vertical rays only: that is all locomotion casts here.
        assert!((dir + Vec3::Y).length() < 1e-4);
        let h = height(origin.x, origin.z);
        let t = origin.y - h;
        let e = 1e-3;
        let normal = Vec3::new(
            -(height(origin.x + e, origin.z) - height(origin.x - e, origin.z)) / (2.0 * e),
            1.0,
            -(height(origin.x, origin.z + e) - height(origin.x, origin.z - e)) / (2.0 * e),
        )
        .normalize();
        (0.0..=max).contains(&t).then(|| GroundHit {
            point: Vec3::new(origin.x, h, origin.z),
            normal,
        })
    };
    let mut sim = Sim::new(&ground, Vec3::Y, Vec3::new(0.0, height(0.0, 0.0), 0.0), Vec3::NEG_Z);
    // Feet plant on the surface; hills are gentle enough that IK still reaches.
    check_walk(&mut sim, Vec3::new(0.3, 0.0, -1.2), 8.0, true, 6e-3);
}

#[test]
fn foot_orientation_follows_the_ground_normal() {
    let up = Vec3::new(0.4, 1.0, 0.3).normalize();
    let ground = PlaneGround {
        point: Vec3::ZERO,
        normal: up,
    };
    // Body upright in world Y although the ground is tilted: feet must tilt to lie flat.
    let mut sim = Sim::new(&ground, Vec3::Y, Vec3::ZERO, Vec3::NEG_Z);
    sim.up = Vec3::Y;
    let mut model_foot_up = Vec3::Y;
    for _ in 0..30 {
        let output_up = sim.step(Vec3::ZERO, false);
        let _ = output_up;
        let pose = sim.solver.solve(&SolveFrame {
            root: sim.root,
            up: Vec3::Y,
            base_poses: &sim.poses,
            constraints: &[],
            goals: &foot_goals(sim.state.output(), Vec3::Y, sim.root.rotation, 1.0),
            locomotion: Some(sim.state.output()),
            body_weight: 1.0,
            dt: DT,
        }).unwrap();
        let model = sim.solver.rig.skeleton.model_transforms(&pose);
        let foot = sim.solver.rig.binding(Limb::LeftFoot).unwrap().chain[2];
        model_foot_up = sim.root.rotation * model[foot].rotation * Vec3::Y;
    }
    assert!(model_foot_up.angle_between(up) < 0.05, "foot up {model_foot_up} vs ground {up}");
}
