//! `masterIs`: the runtime relation between instances, plus orders.
//!
//! `descendsFrom` is not here: it lives on definitions and is exposed through
//! [`Definition`](crate::Definition).
//!
//! `MasterIs` is a Bevy relationship without `linked_spawn`, so despawning or removing a master
//! only orphans its wards (the adopted default). A master that wants something else declares
//! reactions. Change the relation with the ordinary Bevy API: insert a new `MasterIs` to
//! re-assign (adoption, `join_party`), remove it to orphan.
//!
//! Streaming residency must never change a relation. Unloading is modeled by disabling entities
//! (Bevy's `Disabled`), which leaves both components untouched. Note that queries skip disabled
//! entities, so [`Relations`] does not report them.

use std::collections::VecDeque;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::actions::{ActionArgs, ActionName};
use crate::identity::{Definition, DefinitionPath};

/// The ward is at the master's disposal. At most one master per ward.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
#[relationship(relationship_target = Wards)]
pub struct MasterIs(pub Entity);

/// The wards of a master, maintained by Bevy from [`MasterIs`].
#[derive(Component, Debug, Default, PartialEq, Eq)]
#[relationship_target(relationship = MasterIs)]
pub struct Wards(Vec<Entity>);

impl core::ops::Deref for Wards {
    type Target = [Entity];

    fn deref(&self) -> &[Entity] {
        &self.0
    }
}

/// Queries combining both relations.
#[derive(SystemParam)]
pub struct Relations<'w, 's> {
    masters: Query<'w, 's, &'static MasterIs>,
    wards: Query<'w, 's, &'static Wards>,
    definitions: Query<'w, 's, &'static Definition>,
}

impl Relations<'_, '_> {
    pub fn master_of(&self, ward: Entity) -> Option<Entity> {
        self.masters.get(ward).ok().map(|master| master.0)
    }

    pub fn wards_of(&self, master: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.wards
            .get(master)
            .into_iter()
            .flat_map(|wards| wards.iter())
    }

    /// Whether `entity`'s definition is `ancestor` or descends from it.
    pub fn descends_from(&self, entity: Entity, ancestor: &DefinitionPath) -> bool {
        self.definitions
            .get(entity)
            .is_ok_and(|definition| definition.descends_from(ancestor))
    }

    /// "Wards of this master that descend from `minions/ogre`."
    pub fn wards_descending_from<'a>(
        &'a self,
        master: Entity,
        ancestor: &'a DefinitionPath,
    ) -> impl Iterator<Item = Entity> + 'a {
        self.wards_of(master)
            .filter(move |&ward| self.descends_from(ward, ancestor))
    }
}

/// An instruction from a master.
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub action: ActionName,
    pub args: ActionArgs,
    pub issuer: Entity,
}

/// Orders a ward has received, oldest first.
///
/// The ward's decision tree reads them and may still override them: nothing here executes an
/// order. Orders end with the relation; a ward that changes master loses the previous master's.
#[derive(Component, Default, Debug)]
pub struct Orders(VecDeque<Order>);

impl Orders {
    pub fn push(&mut self, order: Order) {
        self.0.push_back(order);
    }

    /// The order the ward should consider next.
    pub fn current(&self) -> Option<&Order> {
        self.0.front()
    }

    pub fn pop(&mut self) -> Option<Order> {
        self.0.pop_front()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &Order> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
