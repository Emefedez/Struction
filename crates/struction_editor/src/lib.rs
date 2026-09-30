//! Headless authoring operations shared by the editor and automation tools.

pub mod history;
pub mod project;
pub mod protocol;
pub mod session;

pub use project::{
    AuthoringProject, DefinitionInspection, Diagnostic, EntityEntry, EntityInspection, SpawnSource,
    Unavailable,
};
pub use session::{Applied, EditRequest, EditSession, Field, SessionError};
