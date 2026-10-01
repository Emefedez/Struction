//! JSONC sources: loading through Reflect, descendsFrom inheritance, presets, file:line errors, comment-preserving edits.
//!
//! A project directory holds `**/entity.jsonc` (the definition `<dir>`) and `presets/**.jsonc`.
//! A definition file has the canonical sections `descendsFrom`, `presets`, `extensors`,
//! `transform`, `components`, `constraints`, `reactions` and `states`, plus extra sections other
//! crates own, kept raw. `transform` is shorthand for the `Transform` component. `extensors`
//! names the packages extending the definition (see `struction_core::ExtensorRegistry`); they
//! accumulate through inheritance, presets and overrides. `states` lists components switched
//! while a state an extensor contributes holds ([`Resolved::states`]). A store may also load
//! read-only libraries, such as the engine's base definitions, which the project descends from
//! and overrides.
//!
//! Resolution layers, later over earlier, with objects deep-merged field by field and arrays and
//! scalars replaced: parent definition (recursively), the definition's presets in listed order,
//! the definition's own data, then scene/spawn overrides. A component set to `null` removes it.
//! Components are built from the merged data through `Reflect`; fields a component leaves out
//! come from its `Default`, and every value keeps the file and line it was written at.
//!
//! Instances get the core `Definition` (path and lineage) on insertion. `reactions` and
//! `grantsToWards` become the core `Reactions` and `GrantsToWards` through
//! [`Resolved::reactions`] and [`Resolved::grants_to_wards`], once actions are registered.
//!
//! [`DefinitionStore`] is the entry point; [`edit`] is the write path for the editor and
//! [`schema`] generates the JSON Schema.

mod build;
mod definition;
pub mod edit;
pub mod error;
mod extensors;
mod plugin;
mod relations;
pub mod schema;
pub mod source;
mod store;
mod typeinfo;

pub use build::ComponentValue;
pub use definition::{CANONICAL_ORDER, DEFAULT_EXTRA_SECTIONS, Resolved};
pub use error::{DataError, ErrorKind, Location};
pub use extensors::{DroppedExtensor, ExtensorReason, ExtensorUse, Suggestion};
pub use plugin::{DataPlugin, DefinitionsChanged, reload_definition_file};
pub use relations::{grants_from_node, reactions_from_node};
pub use source::{Node, NodeValue, parse_jsonc};
pub use store::{DefinitionStore, ReloadReport};
