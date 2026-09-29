//! JSONC sources: loading through Reflect, descendsFrom inheritance, presets, file:line errors, comment-preserving edits.
//!
//! A project directory holds `**/entity.jsonc` (the definition `<dir>`) and `presets/**.jsonc`.
//! A definition file has the canonical sections `descendsFrom`, `presets`, `transform`,
//! `components`, `constraints`, `reactions` (plus extra sections other crates own, kept raw).
//! `transform` is shorthand for the `Transform` component.
//!
//! Resolution layers, later over earlier, with objects deep-merged field by field and arrays and
//! scalars replaced: parent definition (recursively), the definition's presets in listed order,
//! the definition's own data, then scene/spawn overrides. A component set to `null` removes it.
//! Components are built from the merged data through `Reflect`; fields a component leaves out
//! come from its `Default`, and every value keeps the file and line it was written at.
//!
//! [`DefinitionStore`] is the entry point; [`edit`] is the write path for the editor and
//! [`schema`] generates the JSON Schema.

mod build;
mod definition;
pub mod edit;
pub mod error;
mod plugin;
pub mod schema;
pub mod source;
mod store;
mod typeinfo;

pub use build::ComponentValue;
pub use definition::{CANONICAL_ORDER, DEFAULT_EXTRA_SECTIONS, Lineage, Resolved};
pub use error::{DataError, ErrorKind, Location};
pub use plugin::{DataPlugin, DefinitionsChanged, reload_definition_file};
pub use source::{Node, NodeValue, parse_jsonc};
pub use store::{DefinitionStore, ReloadReport};
