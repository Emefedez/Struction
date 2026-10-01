//! Headless authoring operations shared by the editor and automation tools.

pub mod hierarchy;
pub mod history;
pub use hierarchy::HierarchyNode;
pub mod project;
pub mod protocol;
pub mod session;

pub use project::{
    AuthoringProject, DefinitionInspection, Diagnostic, EntityEntry, EntityInspection,
    ExtensorEntry, ExtensorWhy, SpawnSource, Unavailable,
};
pub use session::{Applied, EditRequest, EditSession, Field, SessionError};
