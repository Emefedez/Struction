//! Errors of behavior trees: reference validation at registration, failures while running.

use bevy::prelude::*;
use struction_core::ActionError;

use crate::condition::ConditionError;

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum BrainError {
    #[error("tree `{tree}`, node {node}: {error}")]
    Action {
        tree: String,
        node: String,
        error: ActionError,
    },
    #[error("tree `{tree}`, node {node}: {error}")]
    Condition {
        tree: String,
        node: String,
        error: ConditionError,
    },
    #[error("tree `{tree}`, node {node}: {problem}")]
    Shape {
        tree: String,
        node: String,
        problem: String,
    },
    #[error("unknown behavior tree `{name}`")]
    UnknownTree { name: String },
    #[error("{entity} uses behavior tree `{name}`, which is not registered")]
    MissingTree { entity: Entity, name: String },
    #[error("condition `{condition}` on {entity} failed to run: {message}")]
    ConditionFailed {
        condition: String,
        entity: Entity,
        message: String,
    },
    #[error("{entity} cannot perform order `{action}`: it is not in its action set")]
    OrderRefused { entity: Entity, action: String },
}

/// Failures met while trees run, oldest first. They are also logged.
#[derive(Resource, Default, Debug)]
pub struct BrainErrors(Vec<BrainError>);

impl BrainErrors {
    const CAPACITY: usize = 128;

    pub(crate) fn record(&mut self, error: BrainError) {
        error!("{error}");
        if self.0.len() == Self::CAPACITY {
            self.0.remove(0);
        }
        self.0.push(error);
    }

    pub fn iter(&self) -> impl Iterator<Item = &BrainError> {
        self.0.iter()
    }

    pub fn drain(&mut self) -> Vec<BrainError> {
        std::mem::take(&mut self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
