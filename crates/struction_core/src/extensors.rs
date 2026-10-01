//! Extensors: the packages a definition is extended by, named in its `extensors` list.
//!
//! A package registers what it contributes to definitions: the components it owns, those it
//! supplies with their defaults when named, and the extensors it builds on. `struction_data`
//! reads the registry to validate definitions and to explain where each capability comes from.

use std::any::TypeId;
use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy::reflect::TypePath;

/// How a definition comes to use an extensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Participation {
    /// Inferred from the components it owns (or from an extensor requiring it). Naming it is
    /// allowed and changes nothing but its supplied defaults.
    Inferred,
    /// Gameplay a definition asks for: its components are refused unless a definition names it,
    /// so an entity without it visibly lacks the capability.
    OptIn,
}

/// A component an extensor owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedComponent {
    pub type_id: TypeId,
    /// The name definitions write, such as `Roll`.
    pub name: &'static str,
    /// Added with its default when a definition names the extensor without giving it.
    pub supplied: bool,
}

/// A package's contribution to definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensorMeta {
    pub name: String,
    pub doc: String,
    pub participation: Participation,
    pub components: Vec<OwnedComponent>,
    /// Extensors this one builds on. Inferred ones follow; opt-in ones must be named as well.
    pub requires: Vec<String>,
}

impl ExtensorMeta {
    /// An extensor inferred from its components.
    pub fn inferred(name: impl Into<String>) -> Self {
        Self::new(name.into(), Participation::Inferred)
    }

    /// An extensor definitions must name before using its components.
    pub fn opt_in(name: impl Into<String>) -> Self {
        Self::new(name.into(), Participation::OptIn)
    }

    fn new(name: String, participation: Participation) -> Self {
        Self {
            name,
            doc: String::new(),
            participation,
            components: Vec::new(),
            requires: Vec::new(),
        }
    }

    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = doc.into();
        self
    }

    pub fn owns<C: Component + TypePath>(self) -> Self {
        self.component::<C>(false)
    }

    /// Owns `C` and adds its default to definitions that name this extensor without giving it.
    /// `C` must reflect `Default`.
    pub fn supplies<C: Component + TypePath>(self) -> Self {
        self.component::<C>(true)
    }

    fn component<C: Component + TypePath>(mut self, supplied: bool) -> Self {
        self.components.push(OwnedComponent {
            type_id: TypeId::of::<C>(),
            name: C::short_type_path(),
            supplied,
        });
        self
    }

    pub fn requires(mut self, extensor: impl Into<String>) -> Self {
        self.requires.push(extensor.into());
        self
    }
}

/// Every registered extensor, by name.
#[derive(Resource, Clone, Debug, Default)]
pub struct ExtensorRegistry {
    extensors: BTreeMap<String, ExtensorMeta>,
    owners: BTreeMap<TypeId, String>,
}

impl ExtensorRegistry {
    /// # Panics
    ///
    /// Panics if the name is taken or a component already has an owner: both are build-time
    /// mistakes.
    pub fn register(&mut self, meta: ExtensorMeta) {
        assert!(
            !self.extensors.contains_key(&meta.name),
            "extensor {} is registered twice",
            meta.name
        );
        for component in &meta.components {
            if let Some(owner) = self.owners.insert(component.type_id, meta.name.clone()) {
                panic!(
                    "{} is owned by both the {owner} and {} extensors",
                    component.name, meta.name
                );
            }
        }
        self.extensors.insert(meta.name.clone(), meta);
    }

    pub fn get(&self, name: &str) -> Option<&ExtensorMeta> {
        self.extensors.get(name)
    }

    /// The extensor owning a component type.
    pub fn owner(&self, type_id: TypeId) -> Option<&ExtensorMeta> {
        self.owners.get(&type_id).and_then(|name| self.get(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = &ExtensorMeta> {
        self.extensors.values()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.extensors.keys().map(String::as_str)
    }
}

pub trait ExtensorAppExt {
    /// Registers an extensor, creating the registry if needed so packages need no plugin order.
    fn register_extensor(&mut self, meta: ExtensorMeta) -> &mut Self;
}

impl ExtensorAppExt for App {
    fn register_extensor(&mut self, meta: ExtensorMeta) -> &mut Self {
        self.init_resource::<ExtensorRegistry>()
            .world_mut()
            .resource_mut::<ExtensorRegistry>()
            .register(meta);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component, Reflect)]
    struct Roll;

    #[derive(Component, Reflect)]
    struct Controller;

    #[test]
    fn components_know_their_owner() {
        let mut app = App::new();
        app.register_extensor(ExtensorMeta::inferred("character").owns::<Controller>())
            .register_extensor(
                ExtensorMeta::opt_in("dodge")
                    .supplies::<Roll>()
                    .requires("character"),
            );
        let registry = app.world().resource::<ExtensorRegistry>();
        let dodge = registry.owner(TypeId::of::<Roll>()).unwrap();
        assert_eq!(dodge.name, "dodge");
        assert_eq!(dodge.participation, Participation::OptIn);
        assert_eq!(dodge.components[0].name, "Roll");
        assert!(dodge.components[0].supplied);
        assert_eq!(registry.names().collect::<Vec<_>>(), ["character", "dodge"]);
    }

    #[test]
    #[should_panic(expected = "owned by both")]
    fn a_component_has_one_owner() {
        let mut registry = ExtensorRegistry::default();
        registry.register(ExtensorMeta::opt_in("dodge").owns::<Roll>());
        registry.register(ExtensorMeta::opt_in("tumble").owns::<Roll>());
    }
}
