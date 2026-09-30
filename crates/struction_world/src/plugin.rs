use bevy::prelude::*;
use struction_core::{ActionRegistry, CoreSet};
use struction_data::DefinitionStore;

use crate::cells::{CellSize, update_cells};
use crate::rename::PathAliases;
use crate::save::{ReflectPersist, clear_world};
use crate::scene::SceneCatalog;
use crate::spawn::{WorldErrors, build_world, link_masters, record_removal, run_pending_spawners};

/// Phases of the world in `FixedUpdate`. `Spawn` and `Link` run before `CoreSet::Invoke`, so new
/// instances and relations exist before anything invokes actions on them; `Cells` runs after
/// `CoreSet::Post`, once this tick's movement is done.
#[derive(SystemSet, Clone, PartialEq, Eq, Hash, Debug)]
pub enum WorldSet {
    /// Loaded spawners that have not run yet create their instances.
    Spawn,
    /// Pending `masterIs` references whose target now exists become relations.
    Link,
    /// Derived cells follow moved entities.
    Cells,
}

/// Spawners, zones, saves and streaming cells. Add after `CorePlugin` and `DataPlugin`.
pub struct WorldPlugin {
    /// Edge length of a streaming cell, in meters.
    pub cell_size: f32,
}

impl Default for WorldPlugin {
    fn default() -> Self {
        Self {
            cell_size: CellSize::default().0,
        }
    }
}

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Transform>()
            .register_type_data::<Transform, ReflectPersist>()
            .insert_resource(CellSize(self.cell_size))
            .init_resource::<WorldErrors>()
            .init_resource::<SceneCatalog>()
            .init_resource::<PathAliases>()
            .configure_sets(
                FixedUpdate,
                (
                    (WorldSet::Spawn, WorldSet::Link)
                        .chain()
                        .before(CoreSet::Invoke),
                    WorldSet::Cells.after(CoreSet::Post),
                ),
            )
            .add_systems(Startup, setup_world)
            .add_systems(
                FixedUpdate,
                (
                    run_pending_spawners.in_set(WorldSet::Spawn),
                    link_masters.in_set(WorldSet::Link),
                    update_cells.in_set(WorldSet::Cells),
                ),
            )
            .add_observer(record_removal);
    }
}

/// Compiles the project's scenes and aliases, reports data problems (definitions, action
/// references, scenes) in [`WorldErrors`], and rebuilds the zones and spawners. Runs at
/// `Startup`; call again after the sources changed.
pub fn setup_world(world: &mut World) {
    let types = world.resource::<AppTypeRegistry>().clone();
    let types = types.read();
    let store = world.resource::<DefinitionStore>();
    let root = store.root().to_owned();
    let mut errors = store.errors();
    errors.extend(store.check_references(&types, world.resource::<ActionRegistry>()));
    let catalog = SceneCatalog::load(&root, store, &types);
    errors.extend(catalog.errors().iter().cloned());
    let aliases = PathAliases::load(&root).unwrap_or_else(|e| {
        errors.push(e);
        PathAliases::default()
    });
    drop(types);

    world.insert_resource(catalog);
    world.insert_resource(aliases);
    world.resource_mut::<WorldErrors>().record(errors);
    clear_world(world);
    build_world(world);
}
