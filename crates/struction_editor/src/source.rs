//! Openable locations shared by the native editor and headless clients.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use struction_data::{DefinitionStore, parse_jsonc};
use struction_world::SceneCatalog;

use crate::{AuthoringProject, SessionError};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum SourceTarget {
    Definition(String),
    Entity(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceLocation {
    /// Absolute, openable path, including for read-only library content.
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
}

impl AuthoringProject {
    pub fn source_location(&self, target: &SourceTarget) -> Result<SourceLocation, SessionError> {
        let missing =
            || SessionError::InvalidOperation(format!("No authored source for {target:?}"));
        let (file, line, column) = match target {
            SourceTarget::Definition(id) => {
                let relative = format!("{id}/entity.jsonc");
                let local = self.session().path_of(&relative)?;
                let file = if local.is_file() {
                    local
                } else {
                    self.preview()
                        .resource::<DefinitionStore>()
                        .library_file(id)
                        .ok_or_else(missing)?
                };
                let text = std::fs::read_to_string(&file).map_err(|source| SessionError::Io {
                    file: file.display().to_string(),
                    source,
                })?;
                // Broken definitions must still be openable for repair.
                let (line, column) = parse_jsonc(&relative, &text).map_or((1, 1), |node| {
                    (node.span.start.line, node.span.start.column)
                });
                (file, line, column)
            }
            SourceTarget::Entity(target) => {
                let scenes = self.preview().resource::<SceneCatalog>();
                // Catalog lookup also finds entries which have not spawned yet.
                let span = scenes.spawners().find_map(|spawner| {
                    if spawner.path.to_string() == *target {
                        Some(&spawner.source)
                    } else {
                        spawner
                            .spawns
                            .iter()
                            .find(|s| s.path.to_string() == *target)
                            .map(|s| &s.source)
                    }
                });
                if let Some(span) = span {
                    (
                        self.session().path_of(&span.file)?,
                        span.start.line,
                        span.start.column,
                    )
                } else {
                    let inspected = self.inspect_entity(target, self.play_world().is_some())?;
                    let source = inspected.entity.source.ok_or_else(missing)?;
                    (self.session().path_of(&source.file)?, source.line, 1)
                }
            }
        };
        let file = std::path::absolute(&file).map_err(|source| SessionError::Io {
            file: file.display().to_string(),
            source,
        })?;
        Ok(SourceLocation { file, line, column })
    }
}
