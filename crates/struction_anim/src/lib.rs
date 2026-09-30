//! Procedural animation: springs, IK, base poses, dataflow graphs.
//!
//! ```text
//! gameplay state -> animation intent -> pose requests and constraints -> solvers -> pose
//! ```
//!
//! No clips. Authored base poses are attractors; solvers move joints away from them and springs
//! snap them back. The math modules are pure and Bevy-free (headless testable); `plugin` is the
//! thin ECS layer on top.

pub mod affordance;
pub mod base_pose;
pub mod constraint;
pub mod error;
pub mod graph;
pub mod humanoid;
pub mod ik;
pub mod locomotion;
pub mod plugin;
pub mod pose;
pub mod rig;
pub mod roll;
pub mod skeleton;
pub mod solve;
pub mod spring;

pub use error::AnimError;
pub use plugin::{AnimPlugin, AnimSystems};
