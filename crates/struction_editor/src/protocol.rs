//! Versioned JSON commands for local tools; no GUI, network listener or background thread.

use std::io::{self, BufRead, Write};

use bevy::prelude::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{AuthoringProject, Diagnostic, EditRequest, SessionError};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: Value,
    pub command: Command,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Describe {},
    Validate {},
    Schema {},
    Actions {},
    Definitions {},
    InspectDefinition {
        path: String,
    },
    Read {
        file: String,
    },
    Entities {
        #[serde(default)]
        playing: bool,
    },
    InspectEntity {
        target: String,
        #[serde(default)]
        playing: bool,
    },
    Edit {
        edit: EditRequest,
    },
    MoveSpawn {
        path: String,
        position: [f32; 3],
        #[serde(default)]
        group: Option<String>,
    },
    History {},
    Undo {},
    Redo {},
    EndGroup {},
    Refresh {},
    CreateDefinition {
        path: String,
        parent: String,
    },
    StartPlay {},
    StepPlay {
        ticks: usize,
    },
    StopPlay {},
}

#[derive(Debug, Serialize)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub id: Value,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<Failure>,
}

impl From<SessionError> for Failure {
    fn from(error: SessionError) -> Self {
        let code = match &error {
            SessionError::Playing => "playing",
            SessionError::InvalidPath(_) => "invalid_path",
            SessionError::InvalidOperation(_) => "invalid_operation",
            SessionError::Conflict(_) => "conflict",
            SessionError::Validation(_) => "validation",
            SessionError::Io { .. } => "io",
            SessionError::Edit { .. } => "edit",
            SessionError::Rollback(_) => "rollback",
        };
        let diagnostics = match &error {
            SessionError::Validation(errors) => errors.iter().map(Diagnostic::from).collect(),
            _ => vec![],
        };
        Self {
            code,
            message: error.to_string(),
            diagnostics,
        }
    }
}

pub fn execute(project: &mut AuthoringProject, request: Request) -> Response {
    match apply(project, request.command) {
        Ok(result) => Response {
            id: request.id,
            ok: true,
            result: Some(result),
            error: None,
        },
        Err(error) => Response {
            id: request.id,
            ok: false,
            result: None,
            error: Some(error.into()),
        },
    }
}

fn apply(project: &mut AuthoringProject, command: Command) -> Result<Value, SessionError> {
    match command {
        Command::Describe {} => {
            return Ok(json!({"protocol_version":PROTOCOL_VERSION,
            "commands":["describe","validate","schema","actions","definitions","inspect_definition","read","entities","inspect_entity","edit","move_spawn","history","undo","redo","end_group","refresh","create_definition","start_play","step_play","stop_play"],
            "max_step_ticks":10000}));
        }
        Command::Validate {} => return Ok(json!({"diagnostics":project.validate()})),
        Command::Schema {} => return Ok(project.schema()),
        Command::Actions {} => return Ok(Value::Array(project.actions().into_iter().map(|action| {
            let params: Vec<_> = action.params.into_iter().map(|param| {
                let default = param.default.as_ref().map(|value| match value {
                    struction_core::ArgValue::Bool(v) => json!(v),
                    struction_core::ArgValue::Int(v) => json!(v),
                    struction_core::ArgValue::Float(v) => json!(v),
                    struction_core::ArgValue::Str(v) => json!(v),
                    struction_core::ArgValue::Entity(v) => json!({"entity":v.to_bits().to_string()}),
                });
                json!({"name":param.name,"type":format!("{:?}",param.ty).to_lowercase(),"required":default.is_none(),"default":default})
            }).collect();
            json!({"name":action.name,"doc":action.doc,"params":params,"requires":action.requires})
        }).collect())),
        Command::Definitions {} => return Ok(json!(project.definitions())),
        Command::InspectDefinition { path } => return project.inspect_definition(&path),
        Command::Read { file } => {
            let source = project.session().read(&file)?;
            return Ok(json!({"revision":crate::session::revision(&source),"source":source}));
        }
        Command::Entities { playing } => return Ok(json!(project.entities(playing)?)),
        Command::InspectEntity { target, playing } => {
            return project.inspect_entity(&target, playing);
        }
        Command::Edit { edit } => return Ok(json!(project.edit(edit)?)),
        Command::MoveSpawn {
            path,
            position,
            group,
        } => {
            return Ok(json!(project.move_spawn(
                &path,
                Vec3::from_array(position),
                group
            )?));
        }
        Command::History {} => {
            let history = project.session().history();
            let stack = |transactions: &[crate::history::Transaction]| {
                transactions
                    .iter()
                    .map(|t| json!({"label":t.label,"files":t.files()}))
                    .collect::<Vec<_>>()
            };
            return Ok(
                json!({"undo":stack(history.undo_stack()),"redo":stack(history.redo_stack()),"grouping":history.is_grouping(),"playing":project.session().is_playing()}),
            );
        }
        Command::Undo {} => return Ok(json!(project.undo()?)),
        Command::Redo {} => return Ok(json!(project.redo()?)),
        Command::EndGroup {} => project.end_group(),
        Command::Refresh {} => project.refresh()?,
        Command::CreateDefinition { path, parent } => project.create_definition(&path, &parent)?,
        Command::StartPlay {} => project.start_play()?,
        Command::StepPlay { ticks } => {
            if ticks > 10000 {
                return Err(SessionError::InvalidOperation(
                    "step_play accepts at most 10000 ticks per request".into(),
                ));
            }
            project.step_play(ticks)?;
        }
        Command::StopPlay {} => project.stop_play(),
    }
    Ok(Value::Null)
}

/// One response per nonblank input line, flushed immediately. Invalid requests do not terminate
/// the session. The host must keep game logs off this output stream (normally stdout).
pub fn serve(
    project: &mut AuthoringProject,
    input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => execute(project, request),
            Err(error) => Response {
                id: serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                    .unwrap_or(Value::Null),
                ok: false,
                result: None,
                error: Some(Failure {
                    code: "invalid_request",
                    message: error.to_string(),
                    diagnostics: vec![],
                }),
            },
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
