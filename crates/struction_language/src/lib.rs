//! Read-only language snapshots for code editors, using the game's registered types and the
//! authoring backend's validation. A custom game host can call [`analyze`] with its own factory.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use bevy::prelude::AppTypeRegistry;
use serde::Deserialize;
use serde_json::{Value, json};
use struction_core::Participation;
use struction_data::DefinitionStore;
use struction_editor::{AuthoringProject, ExtensorEntry, SessionError, protocol};

/// An entire project snapshot: every supplied buffer replaces its disk source for this request.
/// Buffers omitted from the next request are read from disk again. No writes or simulation ticks.
pub fn analyze(
    project: &mut AuthoringProject,
    sources: &BTreeMap<String, String>,
) -> Result<Value, SessionError> {
    let diagnostics = project.validate_sources(sources)?;
    let world = project.preview();
    let types = world.resource::<AppTypeRegistry>().read();
    let store = world.resource::<DefinitionStore>().preview_sources(sources, &types);
    let definitions: Vec<_> = store
        .definitions()
        .filter_map(|path| {
            let resolved = store.get(path)?;
            let source = if store.in_project(path) {
                Some(project.session().root().join(path).join("entity.jsonc"))
            } else {
                store.library_file(path)
            };
            let extensors: Vec<_> = resolved.extensors.iter().map(ExtensorEntry::from).collect();
            Some(json!({
                "path": path,
                "source": source,
                "library": store.library_of(path),
                "lineage": resolved.lineage,
                "resolved": resolved.data(),
                "extensors": extensors,
                "components": resolved.components.iter().map(|c| c.type_path).collect::<Vec<_>>(),
            }))
        })
        .collect();
    let extensors: Vec<_> = store.extensors().iter().map(|meta| json!({
        "name": meta.name,
        "doc": meta.doc,
        "opt_in": meta.participation == Participation::OptIn,
        "requires": meta.requires,
        "states": meta.states,
        "components": meta.components.iter().map(|c| json!({
            "name": c.name, "type_path": c.type_path, "supplied": c.supplied,
        })).collect::<Vec<_>>(),
    })).collect();
    let schema = store.schema(&types);
    drop(types);
    let actions = protocol::execute(project, protocol::Request {
        id: Value::Null,
        command: protocol::Command::Actions {},
    }).result;
    Ok(json!({ "schema": schema, "definitions": definitions, "extensors": extensors,
        "actions": actions, "diagnostics": diagnostics }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: Value,
    command: Command,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Describe {},
    Analyze {
        #[serde(default)]
        sources: BTreeMap<String, String>,
    },
}

/// JSONL transport, kept separate from the editor's mutation protocol. Logging belongs on stderr.
pub fn serve(project: &mut AuthoringProject, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        let id = serde_json::from_str::<Value>(&line).ok()
            .and_then(|v| v.get("id").cloned()).unwrap_or(Value::Null);
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let result = match request.command {
                    Command::Describe {} => Ok(json!({ "protocol_version": 1,
                        "commands": ["describe", "analyze"] })),
                    Command::Analyze { sources } => analyze(project, &sources),
                };
                match result {
                    Ok(result) => json!({ "id": request.id, "ok": true, "result": result, "error": null }),
                    Err(error) => json!({ "id": request.id, "ok": false, "result": null,
                        "error": protocol::Failure::from(error) }),
                }
            }
            Err(error) => json!({ "id": id, "ok": false, "result": null,
                "error": { "code": "invalid_request", "message": error.to_string(), "diagnostics": [] } }),
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
