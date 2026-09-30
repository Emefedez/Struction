//! Headless locomotion simulations: a body moving at constant velocity over flat, tilted,
//! rolling and spherical ground. Planted feet must not slide, stance and swing must alternate,
//! and the IK must reach the foot targets.

use bevy::math::{Mat3, Quat, Vec3};
use bevy::transform::components::Transform;
use struction_anim::base_pose::BasePoseSet;
use struction_anim::humanoid;
use struction_anim::locomotion::{
    Ground, GroundHit, LocomotionInput, LocomotionParams, LocomotionState, PlaneGround,
};
use struction_anim::rig::{Limb, LimbBinding, Rig};
use struction_anim::skeleton::{JointDef, Skeleton};
use struction_anim::solve::{PoseSolver, SolveFrame, SolverSettings, foot_goals};

const DT: f32 = 1.0 / 60.0;
const WARMUP: usize = 60;
/// Largest per-frame movement of a planted foot, meters.
const SLIDE_TOLERANCE: f32 = 1e-3;
/// Largest distance between a foot and its target after IK, meters.
const IK_TOLERANCE: f32 = 1e-3;

/// Root transform whose local up is `up` and whose forward (-Z) is along `forward`.
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
    state: LocomotionState,
    solver: PoseSolver,
    poses: BasePoseSet,
    root: Transform,
    feet: Vec<usize>,
}

struct Record {
    /// World position of each foot joint, from forward kinematics of the solved pose.
    fk_feet: Vec<Vec3>,
    planted: Vec<bool>,
    groups: Vec<u8>,
    /// Distance of each foot joint above the ground along up.
    height: Vec<f32>,
    ik_error: f32,
    body_offset: f32,
}

impl<'g> Sim<'g> {
    fn new(rig: Rig, ground: &'g dyn Ground, root: Transform) -> Self {
        let feet = rig.feet().map(|b| *b.chain.last().unwrap()).collect();
        Self {
            ground,
            state: LocomotionState::from_rig(&rig, LocomotionParams::default()).unwrap(),
            solver: PoseSolver::new(rig, SolverSettings::default()),
            poses: humanoid::base_poses(),
            root,
            feet,
        }
    }

    fn humanoid(ground: &'g dyn Ground, root: Transform) -> Self {
        Self::new(humanoid::rig(), ground, root)
    }

    /// Advances one frame with the root already placed by the caller.
    fn step(&mut self, velocity: Vec3) -> Record {
        let up = self.root.rotation * Vec3::Y;
        let input = LocomotionInput {
            root: self.root,
            velocity,
            up,
            grounded: true,
            gravity: -up * 9.81,
        };
        let output = self.state.update(&input, self.ground, DT).clone();
        let goals = foot_goals(&output, up, self.root.rotation, 1.0);
        let pose = self
            .solver
            .solve(&SolveFrame {
                root: self.root,
                up,
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
                        .cast(*p + up, -up, 3.0)
                        .map_or(f32::NAN, |h| (*p - h.point).dot(up))
                })
                .collect(),
            fk_feet,
            planted: output.feet.iter().map(|f| f.planted).collect(),
            groups: self.state.legs.iter().map(|l| l.spec.group).collect(),
            ik_error: self
                .solver
                .report
                .goals
                .iter()
                .map(|g| g.error)
                .fold(0.0, f32::max),
            body_offset: output.body_offset,
        }
    }

    /// Moves the root in a straight line, optionally snapping it to the ground below.
    fn walk(&mut self, velocity: Vec3, follow_ground: bool) -> Record {
        let up = self.root.rotation * Vec3::Y;
        self.root.translation += velocity * DT;
        if follow_ground
            && let Some(hit) = self.ground.cast(self.root.translation + up * 2.0, -up, 4.0)
        {
            self.root.translation = hit.point;
        }
        self.step(velocity)
    }
}

/// Invariant checker fed one record per frame.
#[derive(Default)]
struct Checker {
    frame: usize,
    previous: Option<Record>,
    /// (time, leg) of every lift-off.
    lifts: Vec<(f32, usize)>,
    max_slide: f32,
    max_ik: f32,
    max_swing_height: f32,
    lowest_body: f32,
    /// Planted feet must be at ankle height within this tolerance (NaN skips the check).
    height_tolerance: f32,
}

impl Checker {
    fn new(height_tolerance: f32) -> Self {
        Self {
            height_tolerance,
            ..Default::default()
        }
    }

    fn push(&mut self, rec: Record) {
        let n = self.frame;
        self.frame += 1;
        if n >= WARMUP {
            assert!(
                rec.planted.iter().any(|p| *p),
                "frame {n}: no foot on the ground"
            );
            // Legs of different groups never swing together once the gait has settled.
            let swinging: Vec<u8> = (0..rec.planted.len())
                .filter(|&l| !rec.planted[l])
                .map(|l| rec.groups[l])
                .collect();
            assert!(
                swinging.windows(2).all(|w| w[0] == w[1]),
                "frame {n}: legs of different groups swing together"
            );
            self.max_ik = self.max_ik.max(rec.ik_error);
            self.lowest_body = self.lowest_body.min(rec.body_offset);
        }
        if let Some(prev) = &self.previous {
            for leg in 0..rec.planted.len() {
                if prev.planted[leg] && rec.planted[leg] {
                    let slide = prev.fk_feet[leg].distance(rec.fk_feet[leg]);
                    self.max_slide = self.max_slide.max(slide);
                }
                if prev.planted[leg] && !rec.planted[leg] {
                    self.lifts.push((n as f32 * DT, leg));
                }
                if !rec.planted[leg] {
                    self.max_swing_height = self.max_swing_height.max(rec.height[leg]);
                }
                if rec.planted[leg] && n >= WARMUP && !self.height_tolerance.is_nan() {
                    assert!(
                        (rec.height[leg] - humanoid::ANKLE_HEIGHT).abs() < self.height_tolerance,
                        "frame {n}: planted foot {leg} floats or sinks: {}",
                        rec.height[leg]
                    );
                }
            }
        }
        self.previous = Some(rec);
    }

    fn finish(&self, name: &str) {
        eprintln!(
            "{name}: {} steps, max slide {:.1e} m/frame, max IK error {:.1e} m, swing height {:.3} m, lowest pelvis offset {:.3} m",
            self.lifts.len(),
            self.max_slide,
            self.max_ik,
            self.max_swing_height,
            self.lowest_body
        );
        assert!(
            self.max_slide < SLIDE_TOLERANCE,
            "planted feet slid {} m in one frame",
            self.max_slide
        );
        assert!(
            self.max_ik < IK_TOLERANCE,
            "IK missed a foot target by {} m",
            self.max_ik
        );
        assert!(
            self.max_swing_height > humanoid::ANKLE_HEIGHT + 0.05,
            "swing arcs lift the feet (max {})",
            self.max_swing_height
        );
        let legs = self.previous.as_ref().map_or(0, |r| r.planted.len());
        for leg in 0..legs {
            let count = self.lifts.iter().filter(|(_, l)| *l == leg).count();
            assert!(count >= 4, "leg {leg} stepped only {count} times");
        }
    }

    /// Biped check: after `after` seconds lifts strictly alternate and are evenly spaced.
    fn assert_even_alternation(&self, after: f32) {
        let events: Vec<_> = self.lifts.iter().filter(|(t, _)| *t > after).collect();
        assert!(events.len() >= 6);
        for w in events.windows(2) {
            assert_ne!(
                w[0].1, w[1].1,
                "same leg stepped twice in a row: {events:?}"
            );
        }
        let gaps: Vec<f32> = events.windows(2).map(|w| w[1].0 - w[0].0).collect();
        let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
        for gap in &gaps {
            // Two frames of quantization on either side.
            assert!(
                (gap - mean).abs() <= 2.0 * DT + 1e-4,
                "uneven gait (limp): {gaps:?}"
            );
        }
    }
}

fn flat() -> PlaneGround {
    PlaneGround {
        point: Vec3::ZERO,
        normal: Vec3::Y,
    }
}

#[test]
fn constant_velocity_on_flat_ground_plants_feet_and_alternates() {
    let ground = flat();
    let mut sim = Sim::humanoid(&ground, Transform::IDENTITY);
    let mut check = Checker::new(1e-3);
    for _ in 0..(8.0 / DT) as usize {
        check.push(sim.walk(Vec3::new(0.0, 0.0, -1.4), false));
    }
    check.finish("flat");
    check.assert_even_alternation(1.0);
}

#[test]
fn gait_scales_with_speed_and_stops_when_the_body_stops() {
    let ground = flat();
    let steps = |speed: f32| {
        let mut sim = Sim::humanoid(&ground, Transform::IDENTITY);
        let mut check = Checker::new(1e-3);
        for _ in 0..(8.0 / DT) as usize {
            check.push(sim.walk(Vec3::new(0.0, 0.0, -speed), false));
        }
        check.finish(&format!("speed {speed}"));
        check.lifts.len()
    };
    let slow = steps(0.6);
    let fast = steps(2.2);
    assert!(
        fast > slow,
        "faster walking steps more often ({slow} vs {fast})"
    );

    let mut sim = Sim::humanoid(&ground, Transform::IDENTITY);
    for _ in 0..180 {
        sim.walk(Vec3::new(0.0, 0.0, -1.4), false);
    }
    let mut previous: Option<Vec<bool>> = None;
    let mut steps_after_stop = 0;
    for n in 0..120 {
        let rec = sim.walk(Vec3::ZERO, false);
        if let Some(prev) = &previous {
            steps_after_stop += (0..2).filter(|&l| prev[l] && !rec.planted[l]).count();
        }
        if n > 60 {
            assert!(
                rec.planted.iter().all(|p| *p),
                "still stepping while standing"
            );
        }
        previous = Some(rec.planted);
    }
    assert!(
        steps_after_stop <= 2,
        "at most a settling step after stopping ({steps_after_stop})"
    );
}

#[test]
fn walking_with_a_tilted_up_vector() {
    // A planet surface far from the origin: up is nowhere near world Y.
    let up = Vec3::new(0.4, 1.0, 0.3).normalize();
    let ground = PlaneGround {
        point: Vec3::new(3.0, -2.0, 1.0),
        normal: up,
    };
    let forward = Vec3::new(1.0, -0.2, -1.0);
    let root = frame(ground.point, up, forward);
    let velocity = (root.rotation * Vec3::NEG_Z) * 1.4;
    let mut sim = Sim::humanoid(&ground, root);
    let mut check = Checker::new(1e-3);
    for _ in 0..(8.0 / DT) as usize {
        check.push(sim.walk(velocity, false));
    }
    check.finish("tilted");
    check.assert_even_alternation(1.0);
}

#[test]
fn walking_around_a_small_planet() {
    // The local up rotates continuously as the body walks along a great circle.
    let radius = 6.0_f32;
    let ground = move |origin: Vec3, dir: Vec3, max: f32| -> Option<GroundHit> {
        let b = origin.dot(dir);
        let c = origin.length_squared() - radius * radius;
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let near = -b - disc.sqrt();
        let t = if near >= 0.0 { near } else { -b + disc.sqrt() };
        (0.0..=max).contains(&t).then(|| {
            let point = origin + dir * t;
            GroundHit {
                point,
                normal: point.normalize(),
            }
        })
    };
    let start = Vec3::Y * radius;
    let mut sim = Sim::humanoid(&ground, frame(start, Vec3::Y, Vec3::NEG_Z));
    let mut check = Checker::new(1e-3);
    let speed = 1.4;
    for _ in 0..(10.0 / DT) as usize {
        let forward = sim.root.rotation * Vec3::NEG_Z;
        let position = (sim.root.translation + forward * speed * DT).normalize() * radius;
        let up = position.normalize();
        let travelled = position - sim.root.translation;
        sim.root = frame(position, up, travelled);
        check.push(sim.step(forward * speed));
    }
    check.finish("planet");
    check.assert_even_alternation(1.0);
    assert!(
        sim.root.translation.angle_between(start) > 1.0,
        "went around the planet"
    );
}

#[test]
fn rolling_hills_keep_feet_on_the_terrain() {
    let height = |x: f32, z: f32| 0.15 * (x * 1.3).sin() + 0.1 * (z * 0.9).cos();
    let ground = move |origin: Vec3, dir: Vec3, max: f32| -> Option<GroundHit> {
        // Only vertical rays are cast here (up is world Y).
        assert!((dir + Vec3::Y).length() < 1e-4);
        let h = height(origin.x, origin.z);
        let e = 1e-3;
        let normal = Vec3::new(
            -(height(origin.x + e, origin.z) - height(origin.x - e, origin.z)) / (2.0 * e),
            1.0,
            -(height(origin.x, origin.z + e) - height(origin.x, origin.z - e)) / (2.0 * e),
        )
        .normalize();
        (0.0..=max).contains(&(origin.y - h)).then(|| GroundHit {
            point: Vec3::new(origin.x, h, origin.z),
            normal,
        })
    };
    let start = Vec3::new(0.0, height(0.0, 0.0), 0.0);
    let mut sim = Sim::humanoid(&ground, Transform::from_translation(start));
    // Planted feet sit on the sloped surface along its normal, so their vertical distance to
    // the terrain differs slightly from the ankle height: the height check is skipped.
    let mut check = Checker::new(f32::NAN);
    for _ in 0..(8.0 / DT) as usize {
        check.push(sim.walk(Vec3::new(0.3, 0.0, -1.2), true));
    }
    check.finish("hills");
}

#[test]
fn feet_lie_flat_on_a_slope_under_an_upright_body() {
    let normal = Vec3::new(0.3, 1.0, 0.2).normalize();
    let ground = PlaneGround {
        point: Vec3::ZERO,
        normal,
    };
    let mut sim = Sim::humanoid(&ground, Transform::IDENTITY);
    for _ in 0..30 {
        sim.step(Vec3::ZERO);
    }
    let pose = sim.solver.solve(&SolveFrame {
        root: sim.root,
        up: Vec3::Y,
        base_poses: &sim.poses,
        constraints: &[],
        goals: &foot_goals(sim.state.output(), Vec3::Y, sim.root.rotation, 1.0),
        locomotion: Some(sim.state.output()),
        body_weight: 1.0,
        dt: DT,
    });
    let model = sim.solver.rig.skeleton.model_transforms(&pose.unwrap());
    for limb in [Limb::LeftFoot, Limb::RightFoot] {
        let foot = *sim.solver.rig.binding(limb).unwrap().chain.last().unwrap();
        let foot_up = model[foot].rotation * Vec3::Y;
        assert!(
            foot_up.angle_between(normal) < 1e-3,
            "{limb:?} up {foot_up} vs ground {normal}"
        );
    }
}

/// Four legs, two per side, trotting in diagonal pairs.
fn quadruped() -> Rig {
    let mut joints = vec![
        JointDef {
            name: "root".into(),
            parent: None,
            rest: Transform::IDENTITY,
        },
        JointDef {
            name: "body".into(),
            parent: Some(0),
            rest: Transform::from_xyz(0.0, 0.6, 0.0),
        },
    ];
    let mut limbs = vec![LimbBinding {
        limb: Limb::Pelvis,
        chain: vec![1],
        pole: Vec3::ZERO,
    }];
    // Front-left and back-right share a group, as do front-right and back-left.
    let legs = [
        (Limb::LeftFoot, -0.15, -0.35, Vec3::NEG_Z),
        (Limb::RightFoot, 0.15, -0.35, Vec3::NEG_Z),
        (Limb::ExtraFoot(1), -0.15, 0.35, Vec3::Z),
        (Limb::ExtraFoot(0), 0.15, 0.35, Vec3::Z),
    ];
    for (limb, x, z, pole) in legs {
        let hip = joints.len();
        joints.push(JointDef {
            name: format!("{limb:?}_hip"),
            parent: Some(1),
            rest: Transform::from_xyz(x, 0.0, z),
        });
        joints.push(JointDef {
            name: format!("{limb:?}_knee"),
            parent: Some(hip),
            rest: Transform::from_xyz(0.0, -0.27, 0.0),
        });
        joints.push(JointDef {
            name: format!("{limb:?}_foot"),
            parent: Some(hip + 1),
            rest: Transform::from_xyz(0.0, -(0.6 - 0.27 - humanoid::ANKLE_HEIGHT), 0.0),
        });
        limbs.push(LimbBinding {
            limb,
            chain: vec![hip, hip + 1, hip + 2],
            pole,
        });
    }
    Rig {
        skeleton: Skeleton::new(joints).unwrap(),
        pelvis: 1,
        root: 0,
        limbs,
    }
}

#[test]
fn quadruped_trots_with_diagonal_pairs() {
    let ground = flat();
    let mut sim = Sim::new(quadruped(), &ground, Transform::IDENTITY);
    sim.solver.settings.idle_pose = "none".into();
    sim.poses.poses.insert("none".into(), Default::default());
    let mut check = Checker::new(1e-3);
    for _ in 0..(8.0 / DT) as usize {
        check.push(sim.walk(Vec3::new(0.0, 0.0, -1.0), false));
    }
    check.finish("quadruped");
}
