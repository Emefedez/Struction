use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AnimError {
    #[error("joint {joint} has parent index {parent}, which does not precede it")]
    BadParent { joint: usize, parent: usize },
    #[error("unknown joint `{0}`")]
    UnknownJoint(String),
    #[error("unknown base pose `{0}`")]
    UnknownPose(String),
    #[error("rig has no limb {0:?}")]
    UnknownLimb(crate::rig::Limb),
    #[error("graph node `{node}` refers to unknown input `{input}`")]
    UnknownNode { node: String, input: String },
    #[error("graph node `{0}` is defined twice")]
    DuplicateNode(String),
    #[error("graph has a cycle through `{0}`")]
    Cycle(String),
    #[error("graph node `{node}` expects a {expected} input but `{input}` is a {found}")]
    TypeMismatch {
        node: String,
        input: String,
        expected: &'static str,
        found: &'static str,
    },
}
