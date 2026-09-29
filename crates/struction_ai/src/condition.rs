//! The condition registry: named Rust predicates that behavior trees reference from data.
//!
//! Mirrors the action registry of `struction_core`: a condition is a one-shot system taking a
//! [`ConditionCall`] and returning whether it holds, registered with metadata that the editor
//! reads and that validates references (unknown names, wrong or missing parameters).
//! Conditions only observe: they never change components.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use struction_core::{ActionArgs, ActionError, ActionMeta, ParamSpec, ParamType};

/// Name of a registered condition, such as `sensing/sees_any`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ConditionName(Arc<str>);

impl ConditionName {
    pub fn new(name: impl AsRef<str>) -> Self {
        Self(name.as_ref().into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConditionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ConditionName {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for ConditionName {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum ConditionError {
    #[error("unknown condition `{name}`")]
    Unknown { name: ConditionName },
    #[error("condition `{name}` is already registered")]
    Duplicate { name: ConditionName },
    #[error("condition `{condition}` requires parameter `{param}`")]
    MissingArg {
        condition: ConditionName,
        param: String,
    },
    #[error("condition `{condition}` has no parameter `{param}`")]
    UnknownArg {
        condition: ConditionName,
        param: String,
    },
    #[error("parameter `{param}` of condition `{condition}` expects {expected:?}, got {found:?}")]
    ArgType {
        condition: ConditionName,
        param: String,
        expected: ParamType,
        found: ParamType,
    },
}

/// Metadata registered with a condition.
#[derive(Clone, Debug)]
pub struct ConditionMeta {
    pub name: ConditionName,
    pub doc: String,
    pub params: Vec<ParamSpec>,
}

impl ConditionMeta {
    pub fn new(name: impl Into<ConditionName>) -> Self {
        Self {
            name: name.into(),
            doc: String::new(),
            params: Vec::new(),
        }
    }

    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = doc.into();
        self
    }

    pub fn param(mut self, name: impl Into<String>, ty: ParamType) -> Self {
        self.params.push(ParamSpec {
            name: name.into(),
            ty,
            default: None,
        });
        self
    }

    pub fn param_or(
        mut self,
        name: impl Into<String>,
        ty: ParamType,
        default: impl Into<struction_core::ArgValue>,
    ) -> Self {
        let default = default.into();
        debug_assert!(
            ty.accepts(&default),
            "default does not match parameter type"
        );
        self.params.push(ParamSpec {
            name: name.into(),
            ty,
            default: Some(default),
        });
        self
    }

    /// Checks `args` against the declared parameters and fills in defaults, with the same rules
    /// as actions.
    pub fn resolve_args(&self, args: &ActionArgs) -> Result<ActionArgs, ConditionError> {
        let as_action = ActionMeta {
            params: self.params.clone(),
            ..ActionMeta::new(self.name.as_str())
        };
        let condition = self.name.clone();
        as_action.resolve_args(args).map_err(|error| match error {
            ActionError::MissingArg { param, .. } => ConditionError::MissingArg { condition, param },
            ActionError::UnknownArg { param, .. } => ConditionError::UnknownArg { condition, param },
            ActionError::ArgType {
                param,
                expected,
                found,
                ..
            } => ConditionError::ArgType {
                condition,
                param,
                expected,
                found,
            },
            other => unreachable!("argument resolution cannot fail with {other}"),
        })
    }

    pub fn descriptor(&self) -> ConditionDescriptor {
        ConditionDescriptor {
            name: self.name.as_str().into(),
            doc: self.doc.clone(),
            params: self.params.clone(),
        }
    }
}

/// Plain-data view of a condition's metadata, for the editor and generated descriptors.
#[derive(Clone, PartialEq, Debug)]
pub struct ConditionDescriptor {
    pub name: String,
    pub doc: String,
    pub params: Vec<ParamSpec>,
}

/// What a condition system receives.
#[derive(Clone, Debug)]
pub struct ConditionCall {
    pub condition: ConditionName,
    /// The entity whose tree is being evaluated.
    pub entity: Entity,
    /// Validated against the condition's parameters, with defaults filled in.
    pub args: ActionArgs,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConditionId(u32);

struct Entry {
    meta: ConditionMeta,
    system: SystemId<In<ConditionCall>, bool>,
}

/// All registered conditions. Lookups go by name.
#[derive(Resource, Default)]
pub struct ConditionRegistry {
    entries: Vec<Entry>,
    by_name: HashMap<ConditionName, ConditionId>,
}

impl ConditionRegistry {
    pub fn register<M>(
        world: &mut World,
        meta: ConditionMeta,
        system: impl IntoSystem<In<ConditionCall>, bool, M> + 'static,
    ) -> Result<ConditionId, ConditionError> {
        world.init_resource::<Self>();
        if world.resource::<Self>().by_name.contains_key(&meta.name) {
            return Err(ConditionError::Duplicate { name: meta.name });
        }
        let system = world.register_system(system);
        let mut registry = world.resource_mut::<Self>();
        let id = ConditionId(registry.entries.len() as u32);
        registry.by_name.insert(meta.name.clone(), id);
        registry.entries.push(Entry { meta, system });
        Ok(id)
    }

    /// Resolves a reference from data; the error names the missing condition.
    pub fn resolve(&self, name: &ConditionName) -> Result<ConditionId, ConditionError> {
        self.by_name
            .get(name)
            .copied()
            .ok_or_else(|| ConditionError::Unknown { name: name.clone() })
    }

    pub fn meta(&self, id: ConditionId) -> &ConditionMeta {
        &self.entries[id.0 as usize].meta
    }

    pub(crate) fn system(&self, id: ConditionId) -> SystemId<In<ConditionCall>, bool> {
        self.entries[id.0 as usize].system
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Descriptors sorted by name, so generated output is stable.
    pub fn descriptors(&self) -> Vec<ConditionDescriptor> {
        let mut all: Vec<_> = self.entries.iter().map(|e| e.meta.descriptor()).collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Checks that a reference with arguments resolves and matches the signature.
    pub fn validate_call(
        &self,
        name: &ConditionName,
        args: &ActionArgs,
    ) -> Result<(), ConditionError> {
        let id = self.resolve(name)?;
        self.meta(id).resolve_args(args).map(drop)
    }
}

/// Registration on [`App`].
pub trait ConditionAppExt {
    /// Registers a condition.
    ///
    /// # Panics
    ///
    /// Panics if the name is already registered: duplicates are a build-time mistake.
    fn register_condition<M>(
        &mut self,
        meta: ConditionMeta,
        system: impl IntoSystem<In<ConditionCall>, bool, M> + 'static,
    ) -> &mut Self;
}

impl ConditionAppExt for App {
    fn register_condition<M>(
        &mut self,
        meta: ConditionMeta,
        system: impl IntoSystem<In<ConditionCall>, bool, M> + 'static,
    ) -> &mut Self {
        if let Err(error) = ConditionRegistry::register(self.world_mut(), meta, system) {
            panic!("{error}");
        }
        self
    }
}
