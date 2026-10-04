//! Spawners, zones, save/load, path renaming.
//!
//! - [`scene`]: spawn descriptions in `scenes/**.jsonc` (zones and a `spawnerList`), compiled
//!   into a [`SceneCatalog`] with the streaming cell derived from positions.
//! - [`spawn`]: zone and spawner entities, instances created through
//!   `DefinitionStore::instantiate`, identity (paths for authored things, [`StableId`]s for
//!   instances) and `masterIs` resolution by path in any order.
//! - [`save`]: the save-data layer as versioned JSON; loading rebuilds the world from definitions
//!   and spawners, then applies the save.
//! - [`rename`]: renaming a path across the project sources, with an alias for old saves.
//! - [`cells`]: derived streaming cells; unloading disables entities (neither death nor removal).
//! - [`live`]: applies saved sources to a running world, keeping runtime state.
//!
//! [`WorldPlugin`] needs `CorePlugin` and `DataPlugin`. It builds the world at `Startup` and runs
//! [`WorldSet`] in `FixedUpdate`.
//!
//! [`StableId`]: struction_core::StableId

pub mod cells;
mod identity;
pub mod live;
mod plugin;
pub mod rename;
pub mod save;
pub mod scene;
pub mod spawn;

pub use cells::{Cell, CellSize, load_cell, unload_cell};
pub use identity::{EntityPath, EntityRef, find_entity};
pub use live::{Authored, LiveReloadPlugin, LiveReloaded, reload_sources};
pub use plugin::{WorldPlugin, WorldSet, setup_world};
pub use rename::{PathAliases, RenameError, RenameReport, rename_path};
pub use save::{
    LoadReport, ReflectPersist, SAVE_VERSION, SaveData, SaveError, clear_world, load_world,
    save_world,
};
pub use scene::{SceneCatalog, is_scene_file};
pub use spawn::{
    PendingMaster, RuntimeCreated, Spawned, Spawner, WorldEntity, WorldErrors, Zone, build_world,
    link_masters, run_pending_spawners, run_spawner, spawn_instance, spawn_runtime,
};
