//! Capability grants: what a master gives its wards while the relation lasts.
//!
//! Grants are declared on the master ([`GrantsToWards`]), filtered by the ward's lineage, applied
//! when `MasterIs` is inserted and removed when it is discarded (removed, replaced, or either
//! entity despawned). They are derived, never saved. Each ward keeps a [`GrantRecord`] of exactly
//! what was added so the end of the relation only removes that: a component the ward already had
//! is left alone, and so is its value. A ward can refuse grants with [`RefusesGrants`].
//!
//! Grants are applied when the relation starts: the ward needs its [`Definition`] and the master
//! its [`GrantsToWards`] by then. Changing a master's rules afterwards does not touch existing
//! wards.

use std::any::TypeId;

use bevy::ecs::lifecycle::{Discard, Insert};
use bevy::prelude::*;
use bevy::reflect::PartialReflect;

use crate::actions::ActionName;
use crate::identity::{Definition, DefinitionPath};
use crate::relations::{MasterIs, Orders};

/// One entry of `grantsToWards`.
pub struct GrantRule {
    /// Wards whose definition is, or descends from, this path receive the grant.
    pub to: DefinitionPath,
    /// Component values, cloned onto each ward. The types must be registered with reflection
    /// and `#[reflect(Component)]`.
    pub components: Vec<Box<dyn PartialReflect>>,
    pub actions: Vec<ActionName>,
}

impl GrantRule {
    pub fn new(to: impl Into<DefinitionPath>) -> Self {
        Self {
            to: to.into(),
            components: Vec::new(),
            actions: Vec::new(),
        }
    }

    pub fn component<C: PartialReflect>(mut self, component: C) -> Self {
        self.components.push(Box::new(component));
        self
    }

    pub fn action(mut self, action: impl Into<ActionName>) -> Self {
        self.actions.push(action.into());
        self
    }
}

/// Declared in the master's definition.
#[derive(Component, Default)]
pub struct GrantsToWards(pub Vec<GrantRule>);

/// Actions an entity can be told to perform: its definition's plus those granted.
#[derive(Component, Default, Clone, PartialEq, Eq, Debug)]
pub struct ActionSet(Vec<ActionName>);

impl ActionSet {
    pub fn contains(&self, action: &ActionName) -> bool {
        self.0.contains(action)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ActionName> {
        self.0.iter()
    }

    /// Returns whether the action was newly added.
    pub fn add(&mut self, action: ActionName) -> bool {
        let is_new = !self.contains(&action);
        if is_new {
            self.0.push(action);
        }
        is_new
    }

    fn remove(&mut self, action: &ActionName) {
        self.0.retain(|a| a != action);
    }
}

impl FromIterator<ActionName> for ActionSet {
    fn from_iter<T: IntoIterator<Item = ActionName>>(iter: T) -> Self {
        let mut set = Self::default();
        for action in iter {
            set.add(action);
        }
        set
    }
}

/// A ward's opt-out from grants.
#[derive(Component, Default, Clone, Debug)]
pub struct RefusesGrants {
    pub all: bool,
    pub components: Vec<TypeId>,
    pub actions: Vec<ActionName>,
}

impl RefusesGrants {
    pub fn everything() -> Self {
        Self {
            all: true,
            ..default()
        }
    }

    pub fn component<C: Component>(mut self) -> Self {
        self.components.push(TypeId::of::<C>());
        self
    }

    pub fn action(mut self, action: impl Into<ActionName>) -> Self {
        self.actions.push(action.into());
        self
    }

    fn refuses_component(&self, type_id: TypeId) -> bool {
        self.all || self.components.contains(&type_id)
    }

    fn refuses_action(&self, action: &ActionName) -> bool {
        self.all || self.actions.contains(action)
    }
}

/// What the current master actually added to this ward. Derived state, not saved.
#[derive(Component, Default, Debug)]
pub struct GrantRecord {
    pub components: Vec<TypeId>,
    pub actions: Vec<ActionName>,
}

pub(crate) fn on_master_inserted(event: On<Insert, MasterIs>, mut commands: Commands) {
    let ward = event.entity;
    commands.queue(move |world: &mut World| apply_grants(world, ward));
}

pub(crate) fn on_master_discarded(event: On<Discard, MasterIs>, mut commands: Commands) {
    let ward = event.entity;
    commands.queue(move |world: &mut World| {
        revoke_grants(world, ward);
        if let Ok(mut ward) = world.get_entity_mut(ward) {
            ward.remove::<Orders>();
        }
    });
}

fn apply_grants(world: &mut World, ward: Entity) {
    let Some(master) = world.get::<MasterIs>(ward).map(|master| master.0) else {
        return;
    };
    let Some(definition) = world.get::<Definition>(ward) else {
        return;
    };
    let Some(GrantsToWards(rules)) = world.get::<GrantsToWards>(master) else {
        return;
    };
    let mut components = Vec::new();
    let mut actions = Vec::new();
    for rule in rules
        .iter()
        .filter(|rule| definition.descends_from(&rule.to))
    {
        components.extend(rule.components.iter().map(|value| value.to_dynamic()));
        actions.extend(rule.actions.iter().cloned());
    }
    if components.is_empty() && actions.is_empty() {
        return;
    }

    let refusals = world
        .get::<RefusesGrants>(ward)
        .cloned()
        .unwrap_or_default();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut record = GrantRecord::default();
    let mut ward_mut = world.entity_mut(ward);

    for value in &components {
        let Some(type_id) = value.get_represented_type_info().map(|info| info.type_id()) else {
            error!("grant from {master} carries a component without type information");
            continue;
        };
        if refusals.refuses_component(type_id) {
            continue;
        }
        let Some(reflect) = registry.get_type_data::<ReflectComponent>(type_id) else {
            error!(
                "grant from {master}: component type {type_id:?} is not registered as a reflected component"
            );
            continue;
        };
        // The ward's own component wins, and stays after the grant ends.
        if reflect.contains(&ward_mut) {
            continue;
        }
        reflect.insert(&mut ward_mut, value.as_ref(), &registry);
        record.components.push(type_id);
    }

    let actions: Vec<_> = actions
        .into_iter()
        .filter(|action| !refusals.refuses_action(action))
        .collect();
    if !actions.is_empty() {
        let mut set = ward_mut.take::<ActionSet>().unwrap_or_default();
        for action in actions {
            if set.add(action.clone()) {
                record.actions.push(action);
            }
        }
        ward_mut.insert(set);
    }

    ward_mut.insert(record);
}

fn revoke_grants(world: &mut World, ward: Entity) {
    let Ok(mut ward_mut) = world.get_entity_mut(ward) else {
        return;
    };
    let Some(record) = ward_mut.take::<GrantRecord>() else {
        return;
    };
    if let Some(mut set) = ward_mut.get_mut::<ActionSet>() {
        for action in &record.actions {
            set.remove(action);
        }
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut ward_mut = world.entity_mut(ward);
    for type_id in record.components {
        if let Some(reflect) = registry.get_type_data::<ReflectComponent>(type_id) {
            reflect.remove(&mut ward_mut);
        }
    }
}
