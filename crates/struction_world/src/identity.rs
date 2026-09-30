//! Identity of world entities: authored paths for zones, spawners and named spawns, and core's
//! [`StableId`] (a UUID) for every instance, so saves can refer to either.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use struction_core::StableId;

/// Authored path of a zone (`Fortress/LeftCourtYard`), spawner
/// (`Fortress/LeftCourtYard/courtyard_guards`) or named spawn
/// (`Fortress/LeftCourtYard/courtyard_guards/fireman1`).
#[derive(Component, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct EntityPath(Arc<str>);

impl EntityPath {
    pub fn new(path: impl AsRef<str>) -> Self {
        Self(path.as_ref().into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, name: &str) -> Self {
        Self::new(format!("{}/{name}", self.0))
    }
}

impl fmt::Display for EntityPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for EntityPath {
    fn from(path: &str) -> Self {
        Self::new(path)
    }
}

/// A reference to a world entity that survives saves: by path for authored ones, by id otherwise.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum EntityRef {
    Path(EntityPath),
    Id(StableId),
}

impl fmt::Display for EntityRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => path.fmt(f),
            Self::Id(id) => id.fmt(f),
        }
    }
}

/// Lookup of entities by path and id, unloaded (disabled) ones included: streaming must not
/// change what a reference resolves to.
#[derive(Default)]
pub(crate) struct Index {
    paths: HashMap<EntityPath, Entity>,
    ids: HashMap<StableId, Entity>,
}

impl Index {
    pub fn build(world: &mut World) -> Self {
        let mut index = Self::default();
        let mut query = world.query_filtered::<
            (Entity, Option<&EntityPath>, Option<&StableId>),
            Allow<Disabled>,
        >();
        for (entity, path, id) in query.iter(world) {
            if let Some(path) = path {
                index.paths.insert(path.clone(), entity);
            }
            if let Some(id) = id {
                index.ids.insert(*id, entity);
            }
        }
        index
    }

    pub fn get(&self, target: &EntityRef) -> Option<Entity> {
        match target {
            EntityRef::Path(path) => self.paths.get(path).copied(),
            EntityRef::Id(id) => self.ids.get(id).copied(),
        }
    }
}

/// Finds the entity a reference points at, loaded or not.
pub fn find_entity(world: &mut World, target: &EntityRef) -> Option<Entity> {
    Index::build(world).get(target)
}
