//! Headless authoring operations shared by the editor and automation tools.

pub mod history;
pub mod project;
pub mod protocol;
pub mod session;

pub use project::{AuthoringProject, Diagnostic};
pub use session::{Applied, EditRequest, EditSession, Field, SessionError};
