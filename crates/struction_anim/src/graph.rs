//! Small dataflow graph that produces a pose: curves, springs, blends, bone masks, IK and a
//! deformation stub, evaluated in dependency order. Graphs are plain data (serde) so the
//! animation toolbox can author them; `compile` resolves names, checks types and orders nodes.

use std::collections::HashMap;

use bevy::math::Vec3;
use serde::{Deserialize, Serialize};

use crate::base_pose::{BasePoseSet, ResolvedBasePose};
use crate::constraint::smoothstep;
use crate::error::AnimError;
use crate::ik;
use crate::pose::{BoneMask, Pose};
use crate::rig::{Limb, Rig};
use crate::spring::{SpringF32, SpringParams};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurveKey {
    pub t: f32,
    pub v: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interpolation {
    #[default]
    Linear,
    Smooth,
}

/// Which joints a mask node affects, by data-level names or by limb role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MaskSpec {
    /// The named joint and everything below it.
    Subtree(String),
    Joints(Vec<String>),
    /// The end of a limb and everything below it (a hand and its fingers).
    Limb(Limb),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Const {
        value: f32,
    },
    /// Scalar from the runtime inputs.
    Param {
        name: String,
        #[serde(default)]
        default: f32,
    },
    Vec3Const {
        value: Vec3,
    },
    Vec3Param {
        name: String,
        #[serde(default)]
        default: Vec3,
    },
    /// Piecewise curve over a scalar input.
    Curve {
        input: String,
        keys: Vec<CurveKey>,
        #[serde(default)]
        interpolation: Interpolation,
    },
    Spring {
        input: String,
        params: SpringParams,
    },
    /// Rest pose with the named base pose applied.
    BasePose {
        name: String,
    },
    /// `a` to `b` by a scalar weight.
    Blend {
        a: String,
        b: String,
        weight: String,
    },
    /// `layer` over `base` on the masked joints, scaled by a scalar weight.
    Mask {
        base: String,
        layer: String,
        mask: MaskSpec,
        weight: String,
    },
    /// Two-bone (or chain) IK of a limb toward a model-space target.
    Ik {
        input: String,
        limb: Limb,
        target: String,
        weight: String,
    },
    /// Stub for deformation: volume-preserving stretch along Y of one joint. Mesh-level
    /// deformation is not part of the spike.
    Deformation {
        input: String,
        joint: String,
        stretch: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeDef {
    pub name: String,
    #[serde(flatten)]
    pub node: NodeKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphDef {
    pub nodes: Vec<NodeDef>,
    /// Name of the pose node that is the graph result.
    pub output: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueType {
    Scalar,
    Vec3,
    Pose,
}

impl ValueType {
    fn name(self) -> &'static str {
        match self {
            ValueType::Scalar => "scalar",
            ValueType::Vec3 => "vec3",
            ValueType::Pose => "pose",
        }
    }
}

impl NodeKind {
    fn output_type(&self) -> ValueType {
        match self {
            NodeKind::Const { .. }
            | NodeKind::Param { .. }
            | NodeKind::Curve { .. }
            | NodeKind::Spring { .. } => ValueType::Scalar,
            NodeKind::Vec3Const { .. } | NodeKind::Vec3Param { .. } => ValueType::Vec3,
            _ => ValueType::Pose,
        }
    }

    /// Named inputs in evaluation order with their expected types.
    fn inputs(&self) -> Vec<(&str, ValueType)> {
        use ValueType::{Pose as P, Scalar as S, Vec3 as V};
        match self {
            NodeKind::Const { .. }
            | NodeKind::Param { .. }
            | NodeKind::Vec3Const { .. }
            | NodeKind::Vec3Param { .. }
            | NodeKind::BasePose { .. } => vec![],
            NodeKind::Curve { input, .. } | NodeKind::Spring { input, .. } => vec![(input, S)],
            NodeKind::Blend { a, b, weight } => vec![(a, P), (b, P), (weight, S)],
            NodeKind::Mask {
                base,
                layer,
                weight,
                ..
            } => vec![(base, P), (layer, P), (weight, S)],
            NodeKind::Ik {
                input,
                target,
                weight,
                ..
            } => vec![(input, P), (target, V), (weight, S)],
            NodeKind::Deformation { input, stretch, .. } => vec![(input, P), (stretch, S)],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GraphInputs {
    pub scalars: HashMap<String, f32>,
    pub vec3s: HashMap<String, Vec3>,
}

#[derive(Clone, Debug)]
enum Value {
    Scalar(f32),
    Vec3(Vec3),
    Pose(Pose),
}

impl Value {
    fn scalar(&self) -> f32 {
        match self {
            Value::Scalar(s) => *s,
            _ => unreachable!("types are checked at compile time"),
        }
    }

    fn vec3(&self) -> Vec3 {
        match self {
            Value::Vec3(v) => *v,
            _ => unreachable!("types are checked at compile time"),
        }
    }

    fn pose(&self) -> &Pose {
        match self {
            Value::Pose(p) => p,
            _ => unreachable!("types are checked at compile time"),
        }
    }
}

/// Node data resolved against a rig at compile time.
enum Resolved {
    Plain,
    BasePose(ResolvedBasePose),
    Mask(BoneMask),
    Ik(Vec<usize>, Vec3),
    Deformation(usize),
}

struct CompiledNode {
    kind: NodeKind,
    inputs: Vec<usize>,
    resolved: Resolved,
}

pub struct CompiledGraph {
    rig: Rig,
    nodes: Vec<CompiledNode>,
    /// Evaluation order, indices into `nodes`.
    order: Vec<usize>,
    output: usize,
    springs: Vec<SpringF32>,
    spring_ready: Vec<bool>,
}

impl GraphDef {
    pub fn compile(&self, rig: &Rig, base_poses: &BasePoseSet) -> Result<CompiledGraph, AnimError> {
        let mut index = HashMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if index.insert(n.name.as_str(), i).is_some() {
                return Err(AnimError::DuplicateNode(n.name.clone()));
            }
        }
        let lookup = |node: &str, input: &str| {
            index
                .get(input)
                .copied()
                .ok_or_else(|| AnimError::UnknownNode {
                    node: node.to_owned(),
                    input: input.to_owned(),
                })
        };

        let mut nodes = Vec::with_capacity(self.nodes.len());
        for def in &self.nodes {
            let mut inputs = Vec::new();
            for (input, expected) in def.node.inputs() {
                let i = lookup(&def.name, input)?;
                let found = self.nodes[i].node.output_type();
                if found != expected {
                    return Err(AnimError::TypeMismatch {
                        node: def.name.clone(),
                        input: input.to_owned(),
                        expected: expected.name(),
                        found: found.name(),
                    });
                }
                inputs.push(i);
            }
            let skeleton = &rig.skeleton;
            let resolved = match &def.node {
                NodeKind::BasePose { name } => {
                    Resolved::BasePose(base_poses.resolve(name, skeleton)?)
                }
                NodeKind::Mask { mask, .. } => Resolved::Mask(match mask {
                    MaskSpec::Subtree(name) => skeleton.subtree_mask(skeleton.joint_id(name)?),
                    MaskSpec::Joints(names) => {
                        let mut m = BoneMask::uniform(skeleton.len(), 0.0);
                        for name in names {
                            m.weights[skeleton.joint_id(name)?] = 1.0;
                        }
                        m
                    }
                    MaskSpec::Limb(limb) => rig.limb_end_mask(*limb)?,
                }),
                NodeKind::Ik { limb, .. } => {
                    let binding = rig.binding(*limb)?;
                    Resolved::Ik(binding.chain.clone(), binding.pole)
                }
                NodeKind::Deformation { joint, .. } => {
                    Resolved::Deformation(skeleton.joint_id(joint)?)
                }
                _ => Resolved::Plain,
            };
            nodes.push(CompiledNode {
                kind: def.node.clone(),
                inputs,
                resolved,
            });
        }

        let output = lookup("<output>", &self.output)?;
        if nodes[output].kind.output_type() != ValueType::Pose {
            return Err(AnimError::TypeMismatch {
                node: "<output>".into(),
                input: self.output.clone(),
                expected: "pose",
                found: nodes[output].kind.output_type().name(),
            });
        }

        // Depth-first topological order with cycle detection.
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            New,
            Visiting,
            Done,
        }
        fn visit(
            i: usize,
            nodes: &[CompiledNode],
            marks: &mut [Mark],
            order: &mut Vec<usize>,
            names: &[NodeDef],
        ) -> Result<(), AnimError> {
            match marks[i] {
                Mark::Done => return Ok(()),
                Mark::Visiting => return Err(AnimError::Cycle(names[i].name.clone())),
                Mark::New => {}
            }
            marks[i] = Mark::Visiting;
            for &dep in &nodes[i].inputs {
                visit(dep, nodes, marks, order, names)?;
            }
            marks[i] = Mark::Done;
            order.push(i);
            Ok(())
        }
        let mut marks = vec![Mark::New; nodes.len()];
        let mut order = Vec::new();
        // Only what the output depends on is evaluated, but every node is still validated above.
        visit(output, &nodes, &mut marks, &mut order, &self.nodes)?;
        for i in 0..nodes.len() {
            if marks[i] == Mark::New {
                visit(i, &nodes, &mut marks, &mut Vec::new(), &self.nodes)?;
            }
        }

        let count = nodes.len();
        Ok(CompiledGraph {
            rig: rig.clone(),
            nodes,
            order,
            output,
            springs: vec![SpringF32::default(); count],
            spring_ready: vec![false; count],
        })
    }
}

fn curve(keys: &[CurveKey], x: f32, interpolation: Interpolation) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return 0.0;
    };
    if x <= first.t {
        return first.v;
    }
    if x >= last.t {
        return last.v;
    }
    let i = keys.partition_point(|k| k.t <= x) - 1;
    let (a, b) = (keys[i], keys[i + 1]);
    let f = (x - a.t) / (b.t - a.t);
    let f = match interpolation {
        Interpolation::Linear => f,
        Interpolation::Smooth => smoothstep(f),
    };
    a.v + (b.v - a.v) * f
}

impl CompiledGraph {
    /// Evaluates the graph and returns the pose of the output node.
    pub fn evaluate(&mut self, inputs: &GraphInputs, dt: f32) -> Pose {
        let skeleton = &self.rig.skeleton;
        let mut values: Vec<Option<Value>> = vec![None; self.nodes.len()];
        for &i in &self.order {
            let node = &self.nodes[i];
            let arg = |k: usize| {
                values[node.inputs[k]]
                    .as_ref()
                    .expect("dependencies evaluate first")
            };
            let value = match (&node.kind, &node.resolved) {
                (NodeKind::Const { value }, _) => Value::Scalar(*value),
                (NodeKind::Param { name, default }, _) => {
                    Value::Scalar(inputs.scalars.get(name).copied().unwrap_or(*default))
                }
                (NodeKind::Vec3Const { value }, _) => Value::Vec3(*value),
                (NodeKind::Vec3Param { name, default }, _) => {
                    Value::Vec3(inputs.vec3s.get(name).copied().unwrap_or(*default))
                }
                (
                    NodeKind::Curve {
                        keys,
                        interpolation,
                        ..
                    },
                    _,
                ) => Value::Scalar(curve(keys, arg(0).scalar(), *interpolation)),
                (NodeKind::Spring { params, .. }, _) => {
                    let target = arg(0).scalar();
                    if !self.spring_ready[i] {
                        self.springs[i] = SpringF32::at(target);
                        self.spring_ready[i] = true;
                    }
                    self.springs[i].step(target, *params, dt);
                    Value::Scalar(self.springs[i].value)
                }
                (NodeKind::BasePose { .. }, Resolved::BasePose(pose)) => {
                    let mut out = skeleton.rest_pose();
                    pose.apply(&mut out, 1.0, None);
                    Value::Pose(out)
                }
                (NodeKind::Blend { .. }, _) => {
                    Value::Pose(arg(0).pose().blended(arg(1).pose(), arg(2).scalar(), None))
                }
                (NodeKind::Mask { .. }, Resolved::Mask(mask)) => Value::Pose(
                    arg(0)
                        .pose()
                        .blended(arg(1).pose(), arg(2).scalar(), Some(mask)),
                ),
                (NodeKind::Ik { .. }, Resolved::Ik(chain, pole)) => {
                    let mut pose = arg(0).pose().clone();
                    let mut model = skeleton.model_transforms(&pose);
                    let (target, weight) = (arg(1).vec3(), arg(2).scalar());
                    if chain.len() == 3 {
                        ik::apply_two_bone(
                            skeleton,
                            &mut pose,
                            &mut model,
                            [chain[0], chain[1], chain[2]],
                            target,
                            *pole,
                            weight,
                        );
                    } else {
                        ik::apply_chain(skeleton, &mut pose, &mut model, chain, target, weight);
                    }
                    Value::Pose(pose)
                }
                (NodeKind::Deformation { .. }, Resolved::Deformation(joint)) => {
                    let mut pose = arg(0).pose().clone();
                    let along = (1.0 + arg(1).scalar()).max(0.05);
                    let across = along.sqrt().recip();
                    pose.locals[*joint].scale = Vec3::new(across, along, across);
                    Value::Pose(pose)
                }
                _ => unreachable!("resolution matches node kinds"),
            };
            values[i] = Some(value);
        }
        match values[self.output].take() {
            Some(Value::Pose(pose)) => pose,
            _ => unreachable!("output is a pose node"),
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::Quat;

    use super::*;
    use crate::humanoid;

    fn setup() -> (Rig, BasePoseSet) {
        (humanoid::rig(), humanoid::base_poses())
    }

    const GRAPH: &str = r#"{
        "output": "final",
        "nodes": [
            { "name": "grip_amount", "Param": { "name": "grip", "default": 0.0 } },
            { "name": "smooth_grip", "Spring": { "input": "grip_amount", "params": { "frequency": 4.0, "damping_ratio": 1.0 } } },
            { "name": "idle", "BasePose": { "name": "idle" } },
            { "name": "fist", "BasePose": { "name": "fist" } },
            { "name": "hand_layer", "Mask": { "base": "idle", "layer": "fist", "mask": { "Limb": "RightHand" }, "weight": "smooth_grip" } },
            { "name": "reach_target", "Vec3Param": { "name": "reach", "default": [0.3, 1.2, -0.3] } },
            { "name": "one", "Const": { "value": 1.0 } },
            { "name": "final", "Ik": { "input": "hand_layer", "limb": "RightHand", "target": "reach_target", "weight": "one" } }
        ]
    }"#;

    #[test]
    fn json_graph_compiles_and_evaluates() {
        let (rig, poses) = setup();
        let def: GraphDef = serde_json::from_str(GRAPH).unwrap();
        let again: GraphDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(def, again);

        let mut graph = def.compile(&rig, &poses).unwrap();
        let mut inputs = GraphInputs::default();
        let open = graph.evaluate(&inputs, 1.0 / 60.0);
        inputs.scalars.insert("grip".into(), 1.0);
        let mut closed = open.clone();
        for _ in 0..120 {
            closed = graph.evaluate(&inputs, 1.0 / 60.0);
        }
        let finger = rig.skeleton.joint_id("fingers_1_r").unwrap();
        let other_hand = rig.skeleton.joint_id("fingers_1_l").unwrap();
        assert!(
            closed.locals[finger]
                .rotation
                .angle_between(open.locals[finger].rotation)
                > 1.0
        );
        assert!(
            closed.locals[other_hand]
                .rotation
                .angle_between(open.locals[other_hand].rotation)
                < 1e-2
        );

        let arm = rig.binding(Limb::RightHand).unwrap();
        let model = rig.skeleton.model_transforms(&closed);
        assert!(
            model[arm.chain[2]]
                .translation
                .distance(Vec3::new(0.3, 1.2, -0.3))
                < 1e-3
        );
    }

    #[test]
    fn spring_node_has_state_across_evaluations() {
        let (rig, poses) = setup();
        let def: GraphDef = serde_json::from_str(GRAPH).unwrap();
        let mut graph = def.compile(&rig, &poses).unwrap();
        let mut inputs = GraphInputs::default();
        graph.evaluate(&inputs, 1.0 / 60.0);
        inputs.scalars.insert("grip".into(), 1.0);
        let finger = rig.skeleton.joint_id("fingers_1_r").unwrap();
        let a = graph.evaluate(&inputs, 1.0 / 60.0).locals[finger].rotation;
        let mut last = a;
        for _ in 0..10 {
            last = graph.evaluate(&inputs, 1.0 / 60.0).locals[finger].rotation;
        }
        // Still ramping (not snapped), and moving in one direction.
        assert!(a.angle_between(Quat::IDENTITY) < last.angle_between(Quat::IDENTITY));
    }

    #[test]
    fn curve_node_interpolates() {
        let keys = [
            CurveKey { t: 0.0, v: 0.0 },
            CurveKey { t: 1.0, v: 2.0 },
            CurveKey { t: 2.0, v: 2.0 },
        ];
        assert!((curve(&keys, 0.5, Interpolation::Linear) - 1.0).abs() < 1e-6);
        assert!((curve(&keys, 0.25, Interpolation::Smooth) - 2.0 * smoothstep(0.25)).abs() < 1e-6);
        assert_eq!(curve(&keys, -3.0, Interpolation::Linear), 0.0);
        assert_eq!(curve(&keys, 9.0, Interpolation::Linear), 2.0);
    }

    #[test]
    fn compile_reports_errors() {
        let (rig, poses) = setup();
        let node = |name: &str, node: NodeKind| NodeDef {
            name: name.into(),
            node,
        };
        let cyc = GraphDef {
            output: "a".into(),
            nodes: vec![
                node(
                    "a",
                    NodeKind::Blend {
                        a: "b".into(),
                        b: "b".into(),
                        weight: "w".into(),
                    },
                ),
                node(
                    "b",
                    NodeKind::Blend {
                        a: "a".into(),
                        b: "a".into(),
                        weight: "w".into(),
                    },
                ),
                node("w", NodeKind::Const { value: 0.5 }),
            ],
        };
        assert!(matches!(
            cyc.compile(&rig, &poses),
            Err(AnimError::Cycle(_))
        ));

        let bad_type = GraphDef {
            output: "a".into(),
            nodes: vec![
                node(
                    "a",
                    NodeKind::Blend {
                        a: "w".into(),
                        b: "w".into(),
                        weight: "w".into(),
                    },
                ),
                node("w", NodeKind::Const { value: 0.5 }),
            ],
        };
        assert!(matches!(
            bad_type.compile(&rig, &poses),
            Err(AnimError::TypeMismatch { .. })
        ));

        let missing = GraphDef {
            output: "nope".into(),
            nodes: vec![],
        };
        assert!(matches!(
            missing.compile(&rig, &poses),
            Err(AnimError::UnknownNode { .. })
        ));
    }

    #[test]
    fn deformation_stub_stretches_joint_preserving_volume() {
        let (rig, poses) = setup();
        let def = GraphDef {
            output: "d".into(),
            nodes: vec![
                NodeDef {
                    name: "p".into(),
                    node: NodeKind::BasePose {
                        name: "idle".into(),
                    },
                },
                NodeDef {
                    name: "s".into(),
                    node: NodeKind::Const { value: 0.2 },
                },
                NodeDef {
                    name: "d".into(),
                    node: NodeKind::Deformation {
                        input: "p".into(),
                        joint: "spine".into(),
                        stretch: "s".into(),
                    },
                },
            ],
        };
        let pose = def
            .compile(&rig, &poses)
            .unwrap()
            .evaluate(&GraphInputs::default(), 0.016);
        let s = pose.locals[rig.skeleton.joint_id("spine").unwrap()].scale;
        assert!((s.x * s.y * s.z - 1.0).abs() < 1e-5 && s.y > 1.19);
    }
}
