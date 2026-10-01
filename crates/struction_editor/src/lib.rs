//! Headless authoring operations shared by the editor and automation tools.

pub mod extensors;
pub mod hierarchy;
pub mod history;
pub use hierarchy::HierarchyNode;
pub mod play;
pub mod project;
pub use play::{PlayInput, PlayInputPlugin};
pub mod protocol;
pub mod session;

pub use extensors::{DroppedEntry, ExtensorEntry, ExtensorWhy, SuggestedExtensor};
pub use project::{
    AuthoringProject, DefinitionInspection, Diagnostic, EntityEntry, EntityInspection, SpawnSource,
    Unavailable,
};
pub use session::{Applied, EditRequest, EditSession, Field, SessionError};
