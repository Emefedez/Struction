use std::path::{Path, PathBuf};

use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;

use crate::store::{DefinitionStore, ReloadReport};

/// Loads the project at `root` into a [`DefinitionStore`] resource.
///
/// The load happens in `Plugin::finish`, after every plugin registered its component types, so
/// definitions can use components from any package. Declare primordials and extra sections with
/// [`DataPlugin::primordial`] and [`DataPlugin::section`].
pub struct DataPlugin {
    root: PathBuf,
    primordials: Vec<String>,
    sections: Vec<String>,
}

impl DataPlugin {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            primordials: Vec::new(),
            sections: Vec::new(),
        }
    }

    pub fn primordial(mut self, id: impl Into<String>) -> Self {
        self.primordials.push(id.into());
        self
    }

    pub fn section(mut self, name: impl Into<String>) -> Self {
        self.sections.push(name.into());
        self
    }
}

/// Sent after a reload changed live-relevant data. A runtime refreshes the instances of every
/// listed definition (their lineage already includes descendants) and drops removed ones.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct DefinitionsChanged {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl Plugin for DataPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppTypeRegistry>()
            .register_type::<Transform>()
            .add_message::<DefinitionsChanged>();
    }

    fn finish(&self, app: &mut App) {
        let mut store = DefinitionStore::new(self.root.clone());
        for id in &self.primordials {
            store.declare_primordial(id.clone());
        }
        for name in &self.sections {
            store.allow_section(name.clone());
        }
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let report = store.load(&registry.read());
        for error in &report.errors {
            error!("{error}");
        }
        app.insert_resource(store);
    }
}

/// Reloads one changed file into the world's store and announces the result. This is the hook a
/// file watcher (or the editor after an edit) calls.
pub fn reload_definition_file(world: &mut World, path: &Path) -> ReloadReport {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let report = world.resource_scope(|_, mut store: Mut<DefinitionStore>| {
        store.reload_file(path, &registry.read())
    });
    for error in &report.errors {
        error!("{error}");
    }
    if !report.changed.is_empty() || !report.removed.is_empty() {
        world.write_message(DefinitionsChanged {
            changed: report.changed.clone(),
            removed: report.removed.clone(),
        });
    }
    report
}
