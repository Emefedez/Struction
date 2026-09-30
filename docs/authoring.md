# Headless authoring and the first GUI

`struction_editor` is the shared backend for AI tools and the future egui editor. It exposes source edits, project validation, definition and instance inspection, scene previews, world-space spawn movement, undo/redo and isolated play. `apps/editor` still contains only its scaffold.

MOON implemented and verified this foundation. The initial GUI can now bind its hierarchy, inspector, move gizmo and play controls to these operations. It does not need a second mutation or validation implementation.

## Run the tool host

The example host registers a small game with `Health` and fixed-step regeneration. Work on a copy of the example because successful edits write through to source files:

```bash
cp -r examples/authoring /tmp/struction-authoring
cargo run --locked -p struction_editor --example authoring -- /tmp/struction-authoring
```

Send one JSON object per line on stdin. Each nonblank line gets one JSON response on stdout. The host uses no window or GPU. Game hosts must keep logs on stderr or a separate sink.

```jsonl
{"id":1,"command":{"op":"describe"}}
{"id":2,"command":{"op":"validate"}}
{"id":3,"command":{"op":"inspect_definition","path":"guards/ogre"}}
{"id":4,"command":{"op":"entities"}}
{"id":5,"command":{"op":"read","file":"guards/ogre/entity.jsonc"}}
{"id":6,"command":{"op":"edit","edit":{"op":"set","file":"guards/ogre/entity.jsonc","path":["components","Health","current"],"value":30,"label":"Set guard health"}}}
{"id":7,"command":{"op":"undo"}}
{"id":8,"command":{"op":"redo"}}
{"id":9,"command":{"op":"move_spawn","path":"Court/guards/ogre","position":[8,2,6],"group":"drag-1"}}
{"id":10,"command":{"op":"end_group"}}
{"id":11,"command":{"op":"start_play"}}
{"id":12,"command":{"op":"step_play","ticks":60}}
{"id":13,"command":{"op":"inspect_entity","target":"Court/guards/ogre","playing":true}}
{"id":14,"command":{"op":"stop_play"}}
```

Protocol version 1 returns `{ "id": ..., "ok": true, "result": ..., "error": null }` on success. Failures return `ok: false`, `result: null` and an error containing `code`, `message` and `diagnostics`. Validation diagnostics include project-relative `file`, `line` and `column` when available. Invalid JSON, unknown operations and extra fields are rejected without ending the session. `validate` succeeds as an inspection operation even when its `diagnostics` array contains problems.

For read-modify-write clients, pass the opaque `revision` returned by `read` in the subsequent `set` or `remove` request. A mismatch returns `conflict` without writing. Paths are arrays of object keys and numeric array indices, so names containing dots do not require escaping. Removing an override reveals inherited/default data; it is distinct from assigning a JSON `null`.

## Operations

| Command | Additional fields | Result or behavior |
| --- | --- | --- |
| `describe` | — | Protocol version, command names and step limit |
| `schema` | — | Definition JSON Schema using the host's registered component types |
| `actions` | — | Registered action names, documentation, typed parameters, defaults and required components |
| `definitions` | — | Loaded definition paths |
| `inspect_definition` | `path` | Lineage, resolved authored data and reflected components including defaults |
| `read` | `file` | Exact source text and revision token |
| `validate` | — | Current source diagnostics, including external edits |
| `entities` | Optional `playing` | Paths, definition, identity, placement, source location, disabled state and master path |
| `inspect_entity` | `target`, optional `playing` | Reflected component values; uninspectable components are reported explicitly |
| `edit` | `edit` | Validates a `set` or `remove` before writing; returns affected files |
| `move_spawn` | `path`, `position`, optional `group` | World-space translation converted into an authored spawn offset |
| `history` | — | Undo/redo labels and files, grouping and play state |
| `undo`, `redo` | — | Restore exact source snapshots after validating the candidate project |
| `end_group` | — | End a drag/group so subsequent edits form a new transaction |
| `refresh` | — | Revalidate external edits and rebuild the preview |
| `create_definition` | `path`, `parent` | Create a validated, user-owned definition scaffold without overwriting files |
| `start_play` | — | Build a separate game world from current sources |
| `step_play` | `ticks` | Advance the play app with its fixed timestep; maximum 10,000 per request |
| `stop_play` | — | Discard play state and reenable edits |

An `edit` is `{ "op": "set", "file": "...", "path": [...], "value": ..., "label": "...", "group": "optional drag key", "revision": "optional token" }`, or `{ "op": "remove", "file": "...", "path": [...], "label": "...", "revision": "optional token" }`. Only entity, preset and scene JSONC sources are accepted by the project API. Reserved `presets` and `scenes` directories cannot contain definition scaffolds.

Authored entities are addressed by their paths; runtime-created entities can be inspected by `StableId`. ECS entity numbers are diagnostic strings, not persistent addresses. Preview rebuilds recreate ECS entities, so GUI selections must retain authored paths. Source provenance on spawners and named spawns supplies the file and structured field path for instance overrides. `master` is a gameplay relation, separate from the authored hierarchy.

## Embed in a game or GUI

`AuthoringProject::open(root, factory)` accepts a factory returning a configured, **unstarted headless** Bevy `App`. Register `CorePlugin`, `DataPlugin`, `WorldPlugin` and all game component/action types in that factory. Reuse the same game registration code in the rendered host. Do not give the factory a window, a process-global logger, file-writing startup systems or other external side effects: it runs again for previews and play. `MinimalPlugins` are added if the host has no time plugin.

The project initializes authored spawns without advancing fixed simulation. Game simulation belongs in explicitly ordered `FixedUpdate` systems. The example uses `CoreSet::Invoke`; other games should use their established system sets. The default fixed clock advances once per `step_play` iteration; hosts that pause, rescale or replace that clock own those semantics.

The Rust API is in `project`, `session` and `protocol`. `protocol::execute` handles one typed request; `protocol::serve` is a synchronous JSONL adapter over any `BufRead` and `Write`. An egui host can call `AuthoringProject` directly. `EditSession` is a lower-level text/history primitive; use the project API for game-aware validation.

For runtime change streams, add `struction_debug` to the game's factory and configure its bounded log or a separate JSONL sink. See [debugging.md](debugging.md). Inspection provides current state; tracing records how that state changed.

## Guarantees and current limits

- Validation resolves candidate definitions, inheritance, presets, scene overrides, action references, master references and aliases before writes. Invalid data leaves files and history unchanged. Invalid projects can open for diagnosis; play requires valid sources. JSON syntax damage can be repaired externally, followed by `refresh`.
- Field edits preserve surrounding JSONC comments. Undo restores original bytes, including removed array entries, parent objects, ordering and formatting. Grouped drags across files undo together. An undo that would reintroduce invalid data is rejected with history intact.
- Each file replacement is staged and atomic. Multi-file history restoration preflights every file and rolls back on ordinary write failures; this is not a crash-recovery journal or a distributed filesystem transaction. Use one serialized authoring host per project. Revision checks detect stale edits but are not an interprocess lock.
- Outside edits invalidate affected history. `refresh` retains the last valid preview if the new sources fail validation. Successful edits rebuild the whole preview, including master grants; large-project incremental preview updates are future work.
- Play uses a separate app rebuilt from authored source, not a snapshot of arbitrary runtime state. Play edits and undo are blocked. Stopping discards simulated state. File creation produces a user-owned scaffold and is currently outside undo history.
- The first gizmo operation is translation of named spawns. Rotation/scale gizmos, source rename/delete transactions in the editor, viewport rendering and GUI widgets remain to implement through the same backend. Existing `struction_world::rename_path` is not yet an editor command.
- Physics/animation/AI scene integration, runtime grant hot reload, streaming persistence and procedural animation's visual quality remain separate work. These are not prerequisites for starting the first hierarchy/inspector GUI, but this backend is not a claim that the complete README validation scene is finished.

## Verification

```bash
cargo test --locked -p struction_data -p struction_world -p struction_editor
cargo clippy --locked -p struction_data -p struction_world -p struction_editor --all-targets -- -D warnings
cargo fmt --all --check
```

Tests exercise the full inspect/edit/validate/preview/undo/play loop, inherited fields, grants, source locations, rotated placement frames, stable identity across moves, grouped history, stale revisions, external invalidation, disabled entities, empty/invalid projects and JSONL error recovery. Asset readiness was also checked with all 23 `struction_assets` tests, including actual headless Blender import/export.
