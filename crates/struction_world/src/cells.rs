//! Streaming cells, derived from positions. Cells organize loading only, never gameplay: unloading
//! disables entities (Bevy's `Disabled`), which keeps their state and relations. It is neither
//! death nor removal, and saves record it separately.

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;

use crate::spawn::WorldEntity;

/// Edge length of a streaming cell, in meters.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct CellSize(pub f32);

impl Default for CellSize {
    fn default() -> Self {
        Self(64.0)
    }
}

impl CellSize {
    pub fn cell_of(&self, position: Vec3) -> Cell {
        Cell((position / self.0).floor().as_ivec3())
    }
}

/// The streaming cell an entity is in. Derived from its position, never authored.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cell(pub IVec3);

pub(crate) fn update_cells(
    size: Res<CellSize>,
    mut moved: Query<(&Transform, &mut Cell), Changed<Transform>>,
) {
    for (transform, mut cell) in &mut moved {
        cell.set_if_neq(size.cell_of(transform.translation));
    }
}

/// Unloads every world entity in `cell`. Returns how many were unloaded.
pub fn unload_cell(world: &mut World, cell: IVec3) -> usize {
    let mut query =
        world.query_filtered::<(Entity, &Cell), (With<WorldEntity>, Without<Disabled>)>();
    let entities: Vec<Entity> = query
        .iter(world)
        .filter(|(_, c)| c.0 == cell)
        .map(|(e, _)| e)
        .collect();
    for &entity in &entities {
        world.entity_mut(entity).insert(Disabled);
    }
    entities.len()
}

/// Loads back every unloaded world entity in `cell`. Returns how many were loaded.
pub fn load_cell(world: &mut World, cell: IVec3) -> usize {
    let mut query = world.query_filtered::<(Entity, &Cell), (With<WorldEntity>, With<Disabled>)>();
    let entities: Vec<Entity> = query
        .iter(world)
        .filter(|(_, c)| c.0 == cell)
        .map(|(e, _)| e)
        .collect();
    for &entity in &entities {
        world.entity_mut(entity).remove::<Disabled>();
    }
    entities.len()
}
