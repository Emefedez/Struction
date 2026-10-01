//! Versioned JSON commands for local tools; no GUI, network listener or background thread.

use std::io::{self, BufRead, Write};

use bevy::prelude::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use struction_core::{ActionDescriptor, ArgValue};

use crate::history::Transaction;
use crate::session::revision;
use crate::{AuthoringProject, Diagnostic, EditRequest, SessionError};

pub const PROTOCOL_VERSION: u32 = 1;

/// Bounds one request's work, so a typo cannot hang the session.
const MAX_STEP_TICKS: usize = 10_000;

const COMMANDS: [&str; 26] = [
    "master_hierarchy",
    "definition_hierarchy",
    "set_master",
    "create_spawn",
    "describe",
    "validate",
    "schema",
    "actions",
    "definitions",
    "inspect_definition",
    "add_extensor",
    "remove_extensor",
    "read",
    "entities",
    "inspect_entity",
    "edit",
    "move_spawn",
    "history",
    "undo",
    "redo",
    "end_group",
    "refresh",
    "create_definition",
    "start_play",
    "step_play",
    "stop_play",
];

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: Value,
    pub command: Command,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    MasterHierarchy {
        #[serde(default)]
        playing: bool,
    },
    DefinitionHierarchy {},
    SetMaster {
        path: String,
        master: Option<String>,
    },
    CreateSpawn {
        spawner: String,
        name: String,
        definition: String,
        #[serde(default)]
        master: Option<String>,
        #[serde(default)]
        offset: [f32; 3],
    },
    Describe {},
    Validate {},
    Schema {},
    Actions {},
    Definitions {},
    InspectDefinition {
        path: String,
    },
    AddExtensor {
        path: String,
        extensor: String,
    },
    RemoveExtensor {
        path: String,
        extensor: String,
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
    Ok(match command {
        Command::MasterHierarchy { playing } => json!(project.master_hierarchy(playing)?),
        Command::DefinitionHierarchy {} => json!(project.definition_hierarchy()),
        Command::SetMaster { path, master } => json!(project.set_master(&path, master.as_deref())?),
        Command::CreateSpawn {
            spawner,
            name,
            definition,
            master,
            offset,
        } => json!(project.create_spawn(
            &spawner,
            &name,
            &definition,
            master.as_deref(),
            Vec3::from_array(offset)
        )?),
        Command::Describe {} => json!({
            "protocol_version": PROTOCOL_VERSION,
            "commands": COMMANDS,
            "max_step_ticks": MAX_STEP_TICKS,
        }),
        Command::Validate {} => json!({ "diagnostics": project.validate() }),
        Command::Schema {} => project.schema(),
        Command::Actions {} => project.actions().into_iter().map(action).collect(),
        Command::Definitions {} => json!(project.definitions()),
        Command::InspectDefinition { path } => json!(project.inspect_definition(&path)?),
        Command::AddExtensor { path, extensor } => json!(project.add_extensor(&path, &extensor)?),
        Command::RemoveExtensor { path, extensor } => {
            json!(project.remove_extensor(&path, &extensor)?)
        }
        Command::Read { file } => {
            let source = project.session().read(&file)?;
            json!({ "revision": revision(&source), "source": source })
        }
        Command::Entities { playing } => json!(project.entities(playing)?),
        Command::InspectEntity { target, playing } => {
            json!(project.inspect_entity(&target, playing)?)
        }
        Command::Edit { edit } => json!(project.edit(edit)?),
        Command::MoveSpawn {
            path,
            position,
            group,
        } => json!(project.move_spawn(&path, Vec3::from_array(position), group)?),
        Command::History {} => {
            let history = project.session().history();
            let stack = |transactions: &[Transaction]| -> Vec<Value> {
                transactions
                    .iter()
                    .map(|t| json!({ "label": t.label, "files": t.files() }))
                    .collect()
            };
            json!({
                "undo": stack(history.undo_stack()),
                "redo": stack(history.redo_stack()),
                "grouping": history.is_grouping(),
                "playing": project.session().is_playing(),
            })
        }
        Command::Undo {} => json!(project.undo()?),
        Command::Redo {} => json!(project.redo()?),
        Command::EndGroup {} => {
            project.end_group();
            Value::Null
        }
        Command::Refresh {} => {
            project.refresh()?;
            Value::Null
        }
        Command::CreateDefinition { path, parent } => {
            project.create_definition(&path, &parent)?;
            Value::Null
        }
        Command::StartPlay {} => {
            project.start_play()?;
            Value::Null
        }
        Command::StepPlay { ticks } => {
            if ticks > MAX_STEP_TICKS {
                return Err(SessionError::InvalidOperation(format!(
                    "step_play accepts at most {MAX_STEP_TICKS} ticks per request"
                )));
            }
            project.step_play(ticks)?;
            Value::Null
        }
        Command::StopPlay {} => {
            project.stop_play();
            Value::Null
        }
    })
}

fn action(action: ActionDescriptor) -> Value {
    let params: Vec<_> = action
        .params
        .into_iter()
        .map(|param| {
            let default = param.default.map(|value| match value {
                ArgValue::Bool(v) => json!(v),
                ArgValue::Int(v) => json!(v),
                ArgValue::Float(v) => json!(v),
                ArgValue::Str(v) => json!(v),
                ArgValue::Entity(v) => json!({ "entity": v.to_bits().to_string() }),
            });
            json!({
                "name": param.name,
                "type": format!("{:?}", param.ty).to_lowercase(),
                "required": default.is_none(),
                "default": default,
            })
        })
        .collect();
    json!({ "name": action.name, "doc": action.doc, "params": params, "requires": action.requires })
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
