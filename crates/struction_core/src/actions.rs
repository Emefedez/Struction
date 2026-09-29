//! The action registry: named, instantaneous behaviors implemented as Rust systems.
//!
//! An action is a one-shot system taking [`ActionCall`] as input, so it can use any system
//! parameters (queries, commands, resources). Registration also records metadata that the editor
//! and descriptor generation read, and that the compiler uses to validate references.

use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use bevy::ecs::system::SystemId;
use bevy::prelude::*;

/// Path of a registered action, such as `minions/ogre/die`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ActionName(Arc<str>);

impl ActionName {
    pub fn new(name: impl AsRef<str>) -> Self {
        Self(name.as_ref().into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ActionName {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for ActionName {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

impl From<&ActionName> for ActionName {
    fn from(name: &ActionName) -> Self {
        name.clone()
    }
}

/// A parameter value, as written in data.
#[derive(Clone, PartialEq, Debug)]
pub enum ArgValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Entity(Entity),
}

impl ArgValue {
    pub fn ty(&self) -> ParamType {
        match self {
            Self::Bool(_) => ParamType::Bool,
            Self::Int(_) => ParamType::Int,
            Self::Float(_) => ParamType::Float,
            Self::Str(_) => ParamType::Str,
            Self::Entity(_) => ParamType::Entity,
        }
    }
}

macro_rules! arg_from {
    ($($ty:ty => $variant:ident $(as $cast:ty)?),* $(,)?) => {$(
        impl From<$ty> for ArgValue {
            fn from(value: $ty) -> Self {
                Self::$variant(value $(as $cast)?)
            }
        }
    )*};
}

arg_from!(bool => Bool, i64 => Int, i32 => Int as i64, f64 => Float, f32 => Float as f64, String => Str, Entity => Entity);

impl From<&str> for ArgValue {
    fn from(value: &str) -> Self {
        Self::Str(value.into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamType {
    Bool,
    Int,
    Float,
    Str,
    Entity,
}

impl ParamType {
    /// Integers are accepted where a float is expected, since data often writes `3` for `3.0`.
    pub fn accepts(self, value: &ArgValue) -> bool {
        value.ty() == self || (self == Self::Float && matches!(value, ArgValue::Int(_)))
    }
}

/// Named arguments of an invocation.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct ActionArgs(BTreeMap<String, ArgValue>);

impl ActionArgs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: impl Into<String>, value: impl Into<ArgValue>) -> Self {
        self.0.insert(name.into(), value.into());
        self
    }

    pub fn get(&self, name: &str) -> Option<&ArgValue> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ArgValue)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            ArgValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            ArgValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn float(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            ArgValue::Float(v) => Some(*v),
            ArgValue::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            ArgValue::Str(v) => Some(v),
            _ => None,
        }
    }

    pub fn entity(&self, name: &str) -> Option<Entity> {
        match self.get(name)? {
            ArgValue::Entity(v) => Some(*v),
            _ => None,
        }
    }
}

/// Declared parameter of an action.
#[derive(Clone, PartialEq, Debug)]
pub struct ParamSpec {
    pub name: String,
    pub ty: ParamType,
    /// `None` makes the parameter required.
    pub default: Option<ArgValue>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RequiredComponent {
    pub type_id: TypeId,
    pub name: &'static str,
}

/// Metadata registered with an action.
#[derive(Clone, Debug)]
pub struct ActionMeta {
    pub name: ActionName,
    pub doc: String,
    pub params: Vec<ParamSpec>,
    /// Components the target must have for the action to run.
    pub requires: Vec<RequiredComponent>,
}

impl ActionMeta {
    pub fn new(name: impl Into<ActionName>) -> Self {
        Self {
            name: name.into(),
            doc: String::new(),
            params: Vec::new(),
            requires: Vec::new(),
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
        default: impl Into<ArgValue>,
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

    pub fn requires<C: Component>(mut self) -> Self {
        self.requires.push(RequiredComponent {
            type_id: TypeId::of::<C>(),
            name: core::any::type_name::<C>(),
        });
        self
    }

    /// Checks `args` against the declared parameters and fills in defaults.
    pub fn resolve_args(&self, args: &ActionArgs) -> Result<ActionArgs, ActionError> {
        let name = &self.name;
        if let Some((param, _)) = args
            .iter()
            .find(|(param, _)| !self.params.iter().any(|spec| spec.name == *param))
        {
            return Err(ActionError::UnknownArg {
                action: name.clone(),
                param: param.into(),
            });
        }
        let mut resolved = ActionArgs::new();
        for spec in &self.params {
            let value = match (args.get(&spec.name), &spec.default) {
                (Some(value), _) => value,
                (None, Some(default)) => default,
                (None, None) => {
                    return Err(ActionError::MissingArg {
                        action: name.clone(),
                        param: spec.name.clone(),
                    });
                }
            };
            if !spec.ty.accepts(value) {
                return Err(ActionError::ArgType {
                    action: name.clone(),
                    param: spec.name.clone(),
                    expected: spec.ty,
                    found: value.ty(),
                });
            }
            resolved.0.insert(spec.name.clone(), value.clone());
        }
        Ok(resolved)
    }

    pub fn descriptor(&self) -> ActionDescriptor {
        ActionDescriptor {
            name: self.name.as_str().into(),
            doc: self.doc.clone(),
            params: self.params.clone(),
            requires: self.requires.iter().map(|r| r.name.into()).collect(),
        }
    }
}

/// Plain-data view of an action's metadata, for the editor and generated descriptors.
#[derive(Clone, PartialEq, Debug)]
pub struct ActionDescriptor {
    pub name: String,
    pub doc: String,
    pub params: Vec<ParamSpec>,
    /// Rust type names of the required components.
    pub requires: Vec<String>,
}

/// What an action system receives.
#[derive(Clone, Debug)]
pub struct ActionCall {
    pub action: ActionName,
    pub target: Entity,
    /// Validated against the action's parameters, with defaults filled in.
    pub args: ActionArgs,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum ActionError {
    #[error("unknown action `{name}`")]
    Unknown { name: ActionName },
    #[error("action `{name}` is already registered")]
    Duplicate { name: ActionName },
    #[error("action `{action}` requires parameter `{param}`")]
    MissingArg { action: ActionName, param: String },
    #[error("action `{action}` has no parameter `{param}`")]
    UnknownArg { action: ActionName, param: String },
    #[error("parameter `{param}` of action `{action}` expects {expected:?}, got {found:?}")]
    ArgType {
        action: ActionName,
        param: String,
        expected: ParamType,
        found: ParamType,
    },
    #[error("action `{action}` needs component `{component}` on {target}")]
    MissingComponent {
        action: ActionName,
        target: Entity,
        component: &'static str,
    },
    #[error("action `{action}` targets {target}, which does not exist")]
    TargetMissing { action: ActionName, target: Entity },
    #[error(
        "action `{action}` on {target} exceeds the reaction depth limit of {limit}; \
         a reaction or action loop is likely"
    )]
    DepthExceeded {
        action: ActionName,
        target: Entity,
        limit: u32,
    },
    #[error("action `{action}` failed to run: {message}")]
    SystemFailed { action: ActionName, message: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ActionId(u32);

struct Entry {
    meta: ActionMeta,
    system: SystemId<In<ActionCall>>,
}

/// All registered actions. Registration order is not observable: lookups go by name.
#[derive(Resource, Default)]
pub struct ActionRegistry {
    entries: Vec<Entry>,
    by_name: HashMap<ActionName, ActionId>,
}

impl ActionRegistry {
    /// Registers `system` as the implementation of the action described by `meta`.
    pub fn register<M>(
        world: &mut World,
        meta: ActionMeta,
        system: impl IntoSystem<In<ActionCall>, (), M> + 'static,
    ) -> Result<ActionId, ActionError> {
        world.init_resource::<Self>();
        if world.resource::<Self>().by_name.contains_key(&meta.name) {
            return Err(ActionError::Duplicate { name: meta.name });
        }
        let system = world.register_system(system);
        let mut registry = world.resource_mut::<Self>();
        let id = ActionId(registry.entries.len() as u32);
        registry.by_name.insert(meta.name.clone(), id);
        registry.entries.push(Entry { meta, system });
        Ok(id)
    }

    /// Resolves a reference from data; the error names the missing action.
    pub fn resolve(&self, name: &ActionName) -> Result<ActionId, ActionError> {
        self.by_name
            .get(name)
            .copied()
            .ok_or_else(|| ActionError::Unknown { name: name.clone() })
    }

    pub fn meta(&self, id: ActionId) -> &ActionMeta {
        &self.entries[id.0 as usize].meta
    }

    pub fn system(&self, id: ActionId) -> SystemId<In<ActionCall>> {
        self.entries[id.0 as usize].system
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Descriptors sorted by name, so generated output is stable.
    pub fn descriptors(&self) -> Vec<ActionDescriptor> {
        let mut all: Vec<_> = self.entries.iter().map(|e| e.meta.descriptor()).collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Checks that a reference with arguments resolves and matches the action's signature.
    pub fn validate_call(&self, name: &ActionName, args: &ActionArgs) -> Result<(), ActionError> {
        let id = self.resolve(name)?;
        self.meta(id).resolve_args(args).map(drop)
    }
}

/// Registration on [`App`].
pub trait ActionAppExt {
    /// Registers an action.
    ///
    /// # Panics
    ///
    /// Panics if the name is already registered: duplicates are a build-time mistake.
    fn register_action<M>(
        &mut self,
        meta: ActionMeta,
        system: impl IntoSystem<In<ActionCall>, (), M> + 'static,
    ) -> &mut Self;
}

impl ActionAppExt for App {
    fn register_action<M>(
        &mut self,
        meta: ActionMeta,
        system: impl IntoSystem<In<ActionCall>, (), M> + 'static,
    ) -> &mut Self {
        if let Err(error) = ActionRegistry::register(self.world_mut(), meta, system) {
            panic!("{error}");
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Health;

    fn meta() -> ActionMeta {
        ActionMeta::new("minions/ogre/hit")
            .param("damage", ParamType::Float)
            .param_or("times", ParamType::Int, 1)
            .requires::<Health>()
    }

    #[test]
    fn args_fill_defaults_and_coerce_ints_for_floats() {
        let args = meta()
            .resolve_args(&ActionArgs::new().with("damage", 3))
            .unwrap();
        assert_eq!(args.float("damage"), Some(3.0));
        assert_eq!(args.int("times"), Some(1));
    }

    #[test]
    fn args_are_validated() {
        let meta = meta();
        assert!(matches!(
            meta.resolve_args(&ActionArgs::new()),
            Err(ActionError::MissingArg { param, .. }) if param == "damage"
        ));
        assert!(matches!(
            meta.resolve_args(&ActionArgs::new().with("damage", 1.0).with("x", 1)),
            Err(ActionError::UnknownArg { param, .. }) if param == "x"
        ));
        assert!(matches!(
            meta.resolve_args(&ActionArgs::new().with("damage", "lots")),
            Err(ActionError::ArgType {
                expected: ParamType::Float,
                found: ParamType::Str,
                ..
            })
        ));
    }

    #[test]
    fn descriptor_lists_signature_and_requirements() {
        let descriptor = meta().doc("Deals damage").descriptor();
        assert_eq!(descriptor.name, "minions/ogre/hit");
        assert_eq!(descriptor.params.len(), 2);
        assert_eq!(descriptor.requires.len(), 1);
        assert!(descriptor.requires[0].ends_with("Health"));
    }
}
