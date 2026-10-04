//! The authoring operations as tools for language models: names, descriptions and JSON Schemas
//! for each, called through [`protocol::execute`] so a model edits, validates and undoes exactly
//! as the JSONL protocol and the editor do. The MCP server ([`crate::mcp`]) and the editor's
//! assistant both list these.

use serde_json::{Map, Value, json};

use crate::AuthoringProject;
use crate::protocol::{self, Failure, Request};

/// What a model should know before calling the tools; MCP sends it as the server's
/// `instructions`, the editor's assistant as its system prompt.
pub const INSTRUCTIONS: &str = "\
Struction projects are JSONC sources edited through validated, undoable operations; never \
write project files another way.

- A definition is `<path>/entity.jsonc` (for example `characters/player/entity.jsonc`): \
`descendsFrom` names its parent, so a lineage reads `Actor -> characters/humanoid -> \
characters/player`. `components` holds reflected component values, `extensors` opts into \
packages of behavior (`dodge`, `combat`) and `states` enables or disables components while a \
character state such as `Walking` or `Rolling` holds.
- Scenes (`scenes/*.jsonc`) hold spawners with named spawns; an instance is addressed by its \
entity path, such as `Playground/guards/ogre`, and overrides its definition's components.
- Engine definitions come from a read-only library. Editing one writes a project override at \
the same path that keeps the engine's parent and reaches every descendant.
- Field paths are arrays of object keys and list indices: `[\"components\", \"Health\", \
\"current\"]`.

Work like this: look before editing (`inspect_definition`, `inspect_entity`, `entities`, \
`read`); ask `field_options` what a field accepts; prefer `edit_field`, `add_field`, \
`add_entry` and `remove_entry`, which keep inherited entries and comments. Every edit is \
checked against the whole project and refused with diagnostics (file, line, column) if it \
would break it; fix the cause rather than retrying the same edit. Each successful edit is one \
undo step. `validate` reports problems already in the sources.";

/// One operation offered to a model.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema of the arguments object.
    pub input_schema: Value,
    /// Starts, drives or stops isolated play, which a GUI host may track itself.
    pub play: bool,
    command: Builder,
}

/// How a tool's arguments become a protocol command.
#[derive(Clone, Copy)]
enum Builder {
    /// The arguments are the command's fields.
    Command,
    /// The arguments are an `edit` request of this operation.
    Edit(&'static str),
    /// The definition schema, narrowed: the whole is too large for a model's context.
    Schema,
}

impl Tool {
    fn command(&self, arguments: Value) -> Value {
        let mut fields = match arguments {
            Value::Object(fields) => fields,
            Value::Null => Map::new(),
            other => return other,
        };
        match self.command {
            Builder::Command => {
                fields.insert("op".into(), self.name.into());
                Value::Object(fields)
            }
            Builder::Edit(op) => {
                fields.insert("op".into(), op.into());
                json!({ "op": "edit", "edit": fields })
            }
            Builder::Schema => unreachable!("answered without a protocol command"),
        }
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn none() -> Value {
    object(json!({}), &[])
}

fn string(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn field_path(description: &str) -> Value {
    json!({
        "type": "array",
        "items": { "anyOf": [{ "type": "string" }, { "type": "integer", "minimum": 0 }] },
        "description": description,
    })
}

fn any_value(description: &str) -> Value {
    json!({ "description": description })
}

fn vector(description: &str) -> Value {
    json!({
        "type": "array",
        "items": { "type": "number" },
        "minItems": 3,
        "maxItems": 3,
        "description": description,
    })
}

fn playing() -> Value {
    json!({ "type": "boolean", "description": "Read the running play world instead of the edit preview" })
}

const FILE: &str =
    "Project-relative source file, such as `characters/player/entity.jsonc` or `scenes/main.jsonc`";
const DEFINITION: &str = "Definition path, such as `characters/player`";
const GROUP: &str = "Edits sharing a group key merge into one undo step until `end_group`";

fn tool(name: &'static str, description: &'static str, input_schema: Value) -> Tool {
    Tool {
        name,
        description,
        input_schema,
        play: false,
        command: Builder::Command,
    }
}

/// Every tool, read-only ones first.
pub fn tools() -> Vec<Tool> {
    let mut tools = vec![
        tool(
            "validate",
            "Problems in the project's current sources, with file, line and column; an empty list means it loads cleanly.",
            none(),
        ),
        tool(
            "definitions",
            "Every definition path, including broken sources that need repair.",
            none(),
        ),
        tool(
            "definition_hierarchy",
            "Definitions as a tree by `descendsFrom`, from the primordial types down.",
            none(),
        ),
        tool(
            "inspect_definition",
            "A definition's lineage, its own and resolved data, its reflected components with defaults, and its extensors: those in use with why, dropped, suggested and available.",
            object(json!({ "path": string(DEFINITION) }), &["path"]),
        ),
        tool(
            "entities",
            "Authored instances: entity path, definition, identity, placement, source location, disabled state and master.",
            object(json!({ "playing": playing() }), &[]),
        ),
        tool(
            "master_hierarchy",
            "Instances as a tree by `masterIs` (wards under their masters), a gameplay relation separate from placement.",
            object(json!({ "playing": playing() }), &[]),
        ),
        tool(
            "inspect_entity",
            "An instance's reflected component values, by entity path or stable id.",
            object(
                json!({
                    "target": string("Entity path such as `Playground/guards/ogre`, or a stable id"),
                    "playing": playing(),
                }),
                &["target"],
            ),
        ),
        tool(
            "read",
            "A source file's exact text and its revision token, to pass to `set` or `remove`.",
            object(json!({ "file": string(FILE) }), &["file"]),
        ),
        tool(
            "source_location",
            "Absolute file, line and column where a definition, spawn or spawner is authored.",
            object(
                json!({
                    "target": {
                        "type": "object",
                        "properties": {
                            "kind": { "enum": ["definition", "entity"] },
                            "path": string("Definition path or entity path"),
                        },
                        "required": ["kind", "path"],
                        "additionalProperties": false,
                    },
                }),
                &["target"],
            ),
        ),
        tool(
            "field_options",
            "What a field accepts: its schema fragment (with `$defs`), effective value and the value authored in this file. Ask before editing an unfamiliar field.",
            object(
                json!({ "file": string(FILE), "path": field_path("Field path") }),
                &["file", "path"],
            ),
        ),
        tool(
            "actions",
            "Registered actions: names, documentation, typed parameters with defaults, and required components.",
            none(),
        ),
        Tool {
            command: Builder::Schema,
            ..tool(
                "schema",
                "Without arguments, the definition's top-level fields and every registered component name; with `component`, that component's JSON Schema and the definitions it refers to. `field_options` answers for one field of one file.",
                object(
                    json!({ "component": string("Component name, such as `CharacterController`") }),
                    &[],
                ),
            )
        },
        tool(
            "history",
            "Undo and redo stacks with labels and files, whether a group is open, and whether play is running.",
            none(),
        ),
        tool(
            "edit_field",
            "Set a field's value, materializing an inherited list before changing one entry and keeping sibling entries and comments.",
            object(
                json!({
                    "file": string(FILE),
                    "path": field_path("Field path"),
                    "value": any_value("New JSON value"),
                    "group": string(GROUP),
                }),
                &["file", "path", "value"],
            ),
        ),
        tool(
            "add_field",
            "Add a field this source does not author yet, under `path`; without `value` it starts from the effective or a schema-generated value.",
            object(
                json!({
                    "file": string(FILE),
                    "path": field_path("Path of the parent object, such as [\"components\"]"),
                    "key": string("Field or component name to add"),
                    "value": any_value("Optional starting value"),
                }),
                &["file", "path", "key"],
            ),
        ),
        tool(
            "add_entry",
            "Append an entry to a list, keeping inherited entries; without `value` a schema-guided starting entry.",
            object(
                json!({
                    "file": string(FILE),
                    "path": field_path("Path of the list"),
                    "value": any_value("Optional entry"),
                }),
                &["file", "path"],
            ),
        ),
        tool(
            "remove_entry",
            "Remove one list entry by index, keeping inherited siblings and neighboring comments.",
            object(
                json!({
                    "file": string(FILE),
                    "path": field_path("Path of the list"),
                    "index": { "type": "integer", "minimum": 0 },
                }),
                &["file", "path", "index"],
            ),
        ),
        Tool {
            command: Builder::Edit("set"),
            ..tool(
                "set",
                "Write a JSON value at a path as one labeled undo step. Pass `revision` from `read` to refuse the write if the file changed since.",
                object(
                    json!({
                        "file": string(FILE),
                        "path": field_path("Field path"),
                        "value": any_value("New JSON value"),
                        "label": string("Undo label, such as `Set guard health`"),
                        "group": string(GROUP),
                        "revision": string("Revision token from `read`"),
                    }),
                    &["file", "path", "value", "label"],
                ),
            )
        },
        Tool {
            command: Builder::Edit("remove"),
            ..tool(
                "remove",
                "Remove an authored value, revealing the inherited or default one (unlike setting null).",
                object(
                    json!({
                        "file": string(FILE),
                        "path": field_path("Field path"),
                        "label": string("Undo label"),
                        "revision": string("Revision token from `read`"),
                    }),
                    &["file", "path", "label"],
                ),
            )
        },
        tool(
            "add_extensor",
            "Name an extensor in a definition's own source (or lift its `\"-name\"` drop).",
            object(
                json!({ "path": string(DEFINITION), "extensor": string("Extensor name, such as `combat`") }),
                &["path", "extensor"],
            ),
        ),
        tool(
            "remove_extensor",
            "Take an extensor and the definition's own components of it out, dropping it with `\"-name\"` when inherited.",
            object(
                json!({ "path": string(DEFINITION), "extensor": string("Extensor name") }),
                &["path", "extensor"],
            ),
        ),
        tool(
            "create_definition",
            "Create a definition scaffold descending from `parent`, without overwriting files.",
            object(
                json!({
                    "path": string("New definition path, such as `characters/archer`"),
                    "parent": string("Parent definition path or primordial type, such as `characters/humanoid`"),
                }),
                &["path", "parent"],
            ),
        ),
        tool(
            "create_spawn",
            "Add a named spawn of a definition to an existing spawner.",
            object(
                json!({
                    "spawner": string("Spawner entity path"),
                    "name": string("Name of the new spawn"),
                    "definition": string(DEFINITION),
                    "master": string("Optional master entity path"),
                    "offset": vector("Offset from the spawner, in its local space"),
                }),
                &["spawner", "name", "definition"],
            ),
        ),
        tool(
            "move_spawn",
            "Place a named spawn at a world-space position, written as its authored offset.",
            object(
                json!({
                    "path": string("Entity path of the spawn"),
                    "position": vector("World-space position in meters"),
                    "group": string(GROUP),
                }),
                &["path", "position"],
            ),
        ),
        tool(
            "rotate_spawn",
            "Turn a named spawn to a world-space rotation, written into its authored placement.",
            object(
                json!({
                    "path": string("Entity path of the spawn"),
                    "rotation": vector("World Euler angles in degrees, [X, Y, Z], applied in YXZ order"),
                    "group": string(GROUP),
                }),
                &["path", "rotation"],
            ),
        ),
        tool(
            "scale_spawn",
            "Scale a named spawn along world axes, written as its transform override.",
            object(
                json!({
                    "path": string("Entity path of the spawn"),
                    "scale": vector("Positive scale per world axis"),
                    "group": string(GROUP),
                }),
                &["path", "scale"],
            ),
        ),
        tool(
            "set_master",
            "Make an instance the ward of a master, or release it with null.",
            object(
                json!({
                    "path": string("Entity path of the ward"),
                    "master": { "type": ["string", "null"], "description": "Entity path of the master, or null" },
                }),
                &["path", "master"],
            ),
        ),
        tool(
            "undo",
            "Undo the latest edit, restoring the exact previous sources.",
            none(),
        ),
        tool("redo", "Redo the latest undone edit.", none()),
        tool(
            "end_group",
            "Close the open edit group so later edits form a new undo step.",
            none(),
        ),
        tool(
            "refresh",
            "Re-read sources changed outside these tools and rebuild the preview.",
            none(),
        ),
    ];
    let play = [
        tool(
            "start_play",
            "Build a separate game world from the current sources and play it; edits are refused until `stop_play`.",
            none(),
        ),
        tool(
            "step_play",
            "Advance play by whole fixed ticks (at most 10000 per call).",
            object(
                json!({ "ticks": { "type": "integer", "minimum": 0, "maximum": 10000 } }),
                &["ticks"],
            ),
        ),
        tool(
            "play_input",
            "Hold player input for the following ticks: movement and look axes, presses consumed by the next tick.",
            object(
                json!({
                    "input": {
                        "type": "object",
                        "properties": {
                            "movement": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[right, forward], length at most 1" },
                            "look": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                            "jump_held": { "type": "boolean" },
                            "jump_pressed": { "type": "boolean" },
                            "roll_pressed": { "type": "boolean" },
                            "attack_pressed": { "type": "boolean" },
                            "toggle_view_pressed": { "type": "boolean" },
                            "zoom": { "type": "number" },
                        },
                        "additionalProperties": false,
                    },
                }),
                &["input"],
            ),
        ),
        tool(
            "release_play_input",
            "Let go of all held player input.",
            none(),
        ),
        tool(
            "stop_play",
            "Discard the play world and allow edits again.",
            none(),
        ),
    ];
    tools.extend(play.into_iter().map(|tool| Tool { play: true, ..tool }));
    tools
}

/// Runs the tool named `name` with `arguments` (a JSON object) on `project`.
pub fn call(
    project: &mut AuthoringProject,
    name: &str,
    arguments: Value,
) -> Result<Value, Failure> {
    let Some(tool) = tools().into_iter().find(|tool| tool.name == name) else {
        return Err(Failure {
            code: "unknown_tool",
            message: format!("No tool named `{name}`"),
            diagnostics: vec![],
        });
    };
    if let Builder::Schema = tool.command {
        return schema(project, arguments);
    }
    let request = json!({ "id": null, "command": tool.command(arguments) });
    let request: Request = serde_json::from_value(request).map_err(|error| Failure {
        code: "invalid_arguments",
        message: format!("Arguments for `{name}` do not match its schema: {error}"),
        diagnostics: vec![],
    })?;
    let response = protocol::execute(project, request);
    match response.error {
        Some(failure) => Err(failure),
        None => Ok(response.result.unwrap_or(Value::Null)),
    }
}

fn schema(project: &AuthoringProject, arguments: Value) -> Result<Value, Failure> {
    let invalid = |message: String| Failure {
        code: "invalid_arguments",
        message,
        diagnostics: vec![],
    };
    let component = match arguments.get("component") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(name.clone()),
        Some(_) => return Err(invalid("`component` must be a string".into())),
    };
    if let Value::Object(fields) = &arguments
        && fields.keys().any(|key| key != "component")
    {
        return Err(invalid("`schema` only accepts `component`".into()));
    }
    let schema = project.schema();
    let components = &schema["properties"]["components"]["properties"];
    let Some(component) = component else {
        let names = |value: &Value| -> Vec<String> {
            value
                .as_object()
                .map(|fields| fields.keys().cloned().collect())
                .unwrap_or_default()
        };
        return Ok(json!({
            "fields": names(&schema["properties"]),
            "components": names(components),
        }));
    };
    let Some(found) = components.get(&component) else {
        return Err(invalid(format!(
            "No registered component `{component}`; call `schema` without arguments for the names"
        )));
    };
    // Only the definitions it refers to, followed transitively.
    let mut defs = Map::new();
    let mut pending = vec![found.clone()];
    while let Some(value) = pending.pop() {
        let mut references = Vec::new();
        collect_references(&value, &mut references);
        for name in references {
            if !defs.contains_key(&name)
                && let Some(def) = schema["$defs"].get(&name)
            {
                defs.insert(name, def.clone());
                pending.push(def.clone());
            }
        }
    }
    Ok(json!({ "component": component, "schema": found, "$defs": defs }))
}

fn collect_references(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                match (key.as_str(), value) {
                    ("$ref", Value::String(reference)) => {
                        if let Some(name) = reference.strip_prefix("#/$defs/") {
                            out.push(name.to_owned());
                        }
                    }
                    _ => collect_references(value, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_references(item, out)),
        _ => {}
    }
}
