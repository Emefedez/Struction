//! Read-only language snapshots for code editors, using the game's registered types and the
//! authoring backend's validation. A custom game host can call [`analyze`] with its own factory.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use bevy::prelude::AppTypeRegistry;
use serde::Deserialize;
use serde_json::{Value, json};
use struction_core::{ActionRegistry, Participation};
use struction_data::DefinitionStore;
use struction_editor::{
    AuthoringProject, Diagnostic, ExtensorEntry, SessionError, SourceKind, protocol,
};

/// Library files are reported as `<package>:<path from the library root>`, which no client can
/// open; project files stay project-relative because the client knows the root it passed.
fn locatable(file: &str, libraries: &[(String, PathBuf)]) -> String {
    match file.split_once(':') {
        Some((library, relative)) => match libraries.iter().find(|(name, _)| name == library) {
            Some((_, root)) => root.join(relative).to_string_lossy().into_owned(),
            None => file.to_string(),
        },
        None => file.to_string(),
    }
}

/// JSON Schema for `scenes/**.jsonc`. The shape mirrors the grammar `struction_world::scene`
/// reads; `overrides` holds the definition schema, and `definition` lists the resolved paths, so a
/// client needs nothing else to complete or explain a scene.
pub fn scene_schema(store: &DefinitionStore, definition_schema: &Value) -> Value {
    let position = json!({
        "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3,
        "description": "Position as [x, y, z]: world coordinates for a zone, the zone's own for a spawner.",
    });
    let rotation = json!({
        "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3,
        "description": "Euler angles in degrees as [x, y, z], applied yaw then pitch then roll.",
    });
    let spawn = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["definition"],
        "properties": {
            "definition": { "type": "string", "enum": store.definitions().collect::<Vec<_>>(),
                "description": "Definition this spawn instantiates." },
            "offset": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3,
                "description": "Position in the spawner's frame, as [x, y, z]." },
            "rotation": rotation.clone(),
            "masterIs": { "type": "string",
                "description": "Authored master path. This spawn is the ward; placement is separate." },
            "overrides": { "$ref": "#/$defs/definition" },
        },
    });
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "Struction scene",
        "type": "object",
        "additionalProperties": false,
        "description": "Zones and the spawners placed in them.",
        "properties": {
            "$schema": { "type": "string" },
            "zones": {
                "type": "object",
                "description": "Zone origins in world coordinates, keyed by path. Undeclared zones sit at the origin.",
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": { "position": position.clone(), "rotation": rotation.clone() },
                },
            },
            "spawnerList": {
                "type": "object",
                "description": "Spawners keyed by name; each one's path is <zone>/<name>.",
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["zone", "position"],
                    "properties": {
                        "zone": { "type": "string", "description": "Zone this spawner is placed in." },
                        "position": position.clone(),
                        "rotation": rotation.clone(),
                        "spawns": { "type": "object", "description": "Spawns keyed by name.",
                            "additionalProperties": spawn },
                    },
                },
            },
        },
        "$defs": { "definition": definition_schema },
    })
}

/// An entire project snapshot: every supplied buffer replaces its disk source for this request.
/// Buffers omitted from the next request are read from disk again. No writes or simulation ticks.
///
/// A client sends every JSONC buffer it has open and asks here what each file is, what the
/// schema allows at a position and what the registrations say about a name, so none of the
/// engine's own vocabulary has to be repeated on the other side of the wire.
pub fn analyze(
    project: &mut AuthoringProject,
    sources: &BTreeMap<String, String>,
) -> Result<Value, SessionError> {
    let reported = project.validate_sources(sources)?;
    let world = project.preview();
    let types = world.resource::<AppTypeRegistry>().read();
    let store = world
        .resource::<DefinitionStore>()
        .preview_sources(sources, &types);
    let libraries: Vec<_> = store
        .library_roots()
        .map(|(name, root)| (name.to_string(), root.to_path_buf()))
        .collect();
    let diagnostics: Vec<_> = reported
        .iter()
        .map(|error| Diagnostic {
            file: error.file.as_ref().map(|file| locatable(file, &libraries)),
            ..error.clone()
        })
        .collect();
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
                "doc": store.doc(path),
                "lineage": resolved.lineage,
                "resolved": resolved.data(),
                "extensors": extensors,
                "components": resolved.components.iter().map(|c| c.type_path).collect::<Vec<_>>(),
            }))
        })
        .collect();
    let presets: Vec<_> = store
        .preset_names()
        .filter_map(|name| {
            let (source, library) = store.preset_source(name)?;
            Some(json!({ "name": name, "source": source, "library": library }))
        })
        .collect();
    // What each supplied buffer is, so a client serves a file without knowing the engine's
    // naming rules; a file nothing reads is left out.
    let files: Vec<_> = sources
        .keys()
        .filter_map(|file| {
            let kind = project.source_kind(file)?;
            let name = match &kind {
                SourceKind::Definition(name) | SourceKind::Preset(name) => Some(name.as_str()),
                SourceKind::Scene => None,
            };
            Some(json!({ "path": file, "kind": kind_name(&kind), "name": name }))
        })
        .collect();
    let extensors: Vec<_> = store
        .extensors()
        .iter()
        .map(|meta| {
            json!({
                "name": meta.name,
                "doc": meta.doc,
                "opt_in": meta.participation == Participation::OptIn,
                "requires": meta.requires,
                "states": meta.states.iter().map(|state| json!({
                    "name": state.name, "doc": state.doc,
                })).collect::<Vec<_>>(),
                "components": meta.components.iter().map(|c| json!({
                    "name": c.name, "type_path": c.type_path, "supplied": c.supplied,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let actions = world.resource::<ActionRegistry>();
    let schema = store.schema(&types, actions);
    let scene_schema = scene_schema(&store, &schema);
    drop(types);
    let actions = protocol::execute(
        project,
        protocol::Request {
            id: Value::Null,
            command: protocol::Command::Actions {},
        },
    )
    .result;
    Ok(
        json!({ "schema": schema, "scene_schema": scene_schema, "definitions": definitions,
        "presets": presets, "files": files, "extensors": extensors, "actions": actions,
        "diagnostics": diagnostics }),
    )
}

/// The vocabulary a file's name belongs to, in the snapshot's own words.
fn kind_name(kind: &SourceKind) -> &'static str {
    match kind {
        SourceKind::Definition(_) => "definition",
        SourceKind::Preset(_) => "preset",
        SourceKind::Scene => "scene",
    }
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
        let id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned())
            .unwrap_or(Value::Null);
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let result = match request.command {
                    Command::Describe {} => Ok(json!({ "protocol_version": 1,
                        "commands": ["describe", "analyze"] })),
                    Command::Analyze { sources } => analyze(project, &sources),
                };
                match result {
                    Ok(result) => {
                        json!({ "id": request.id, "ok": true, "result": result, "error": null })
                    }
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
