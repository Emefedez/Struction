# Headless authoring and the first GUI

`struction_editor` is the shared backend for AI tools and the egui editor. It exposes source edits, project validation, definition and instance inspection, scene previews, world-space spawn movement, undo/redo and isolated play.

MOON implemented and verified this foundation. SUN built the first GUI on it (`apps/editor`, see [The editor GUI](#the-editor-gui)); it has no second mutation or validation implementation.

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
| `definitions` | — | Definition paths, including broken sources that need repair |
| `source_location` | `target: {kind: "definition" or "entity", path: "…"}` | Absolute file, line and column for a definition, spawn or spawner; library definitions resolve to their real files |
| `field_options` | `file`, `path` | Schema fragment (with `$defs`), effective value and locally authored value for a definition field or spawn override |
| `add_field` | `file`, `path` (parent), `key`, optional `value` | Add a field absent from this source; defaults to the effective value or a schema-generated starting value |
| `edit_field` | `file`, `path`, `value`, optional `group` | Edit an effective field, materializing an inherited list before changing an entry; preserves sibling entries |
| `remove_entry` | `file`, `path` (list), `index` | Remove one entry, preserving inherited siblings and neighboring authored comments; fixed-size arrays refuse removal |
| `add_entry` | `file`, `path` (list), optional `value` | Append a schema-guided entry, preserving inherited entries; fixed-size arrays refuse extra entries |
| `inspect_definition` | `path` | Lineage, resolved authored data and reflected components including defaults, plus extensors: those in use with their reason (`named_by`, `owns`, `required_by`) and supplied components, dropped ones, suggested ones and the rest available |
| `add_extensor` | `path`, `extensor` | Name an extensor in the definition's own source (or remove its own `"-name"` drop); validated and undoable |
| `remove_extensor` | `path`, `extensor` | Take an extensor and the definition's own components of it out, dropping it with `"-name"` when inherited; refused while another extensor in use builds on it |
| `read` | `file` | Exact source text and revision token |
| `validate` | — | Current source diagnostics, including external edits |
| `entities` | Optional `playing` | Paths, definition, identity, placement, source location, disabled state and master path |
| `master_hierarchy` | Optional `playing` | Instance tree by `masterIs`; roots have no master in the tree |
| `definition_hierarchy` | — | Definition tree by `descendsFrom`, including broken source entries |
| `set_master` | `path`, `master` (path or `null`) | Reparent or release a named spawn; validated and undoable |
| `create_spawn` | `spawner`, `name`, `definition`, optional `master`, `offset` | Add a named spawn in an existing spawner; offset is local; undo restores the scene exactly |
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

Authored entities are addressed by their paths; runtime-created entities can be inspected by `StableId`. ECS entity numbers are diagnostic strings, not persistent addresses. Preview rebuilds recreate ECS entities, so GUI selections must retain authored paths. Source provenance on spawners and named spawns supplies the file and structured field path for instance overrides. `master` is a gameplay relation, separate from the placement hierarchy. The authored field remains `masterIs`: the actor containing it is the ward, and its value identifies the master. `master_hierarchy` nests wards under masters; `definition_hierarchy` shows type inheritance separately. Self-parenting and relationship cycles fail validation.

## Embed in a game or GUI

`AuthoringProject::open(root, factory)` accepts a factory returning a configured, **unstarted headless** Bevy `App`. Register `CorePlugin`, `DataPlugin`, `WorldPlugin` and all game component/action types in that factory. Reuse the same game registration code in the rendered host. Do not give the factory a window, a process-global logger, file-writing startup systems or other external side effects: it runs again for previews and play. `MinimalPlugins` are added if the host has no time plugin.

The project initializes authored spawns without advancing fixed simulation. Game simulation belongs in explicitly ordered `FixedUpdate` systems. The example uses `CoreSet::Invoke`; other games should use their established system sets. The default fixed clock advances once per `step_play` iteration; hosts that pause, rescale or replace that clock own those semantics.

The Rust API is in `project`, `session` and `protocol`. `protocol::execute` handles one typed request; `protocol::serve` is a synchronous JSONL adapter over any `BufRead` and `Write`. An egui host can call `AuthoringProject` directly. `EditSession` is a lower-level text/history primitive; use the project API for game-aware validation.

For runtime change streams, add `struction_debug` to the game's factory and configure its bounded log or a separate JSONL sink. See [debugging.md](debugging.md). Inspection provides current state; tracing records how that state changed.

## The editor GUI

```bash
cp -r examples/authoring /tmp/struction-authoring
cargo run -p struction-editor -- /tmp/struction-authoring
# The playground data is supported by the same game registration as its native host:
cargo run -p struction-editor -- apps/playground/project
# The repository directory or apps/playground also resolves to that data directory.
# Use the editor’s registrations through JSONL without opening a window:
cargo run -p struction-editor -- apps/playground/project --headless
```

Without an argument the editor asks for a project directory. The UI is egui (`bevy_egui`) with its own theme (`apps/editor/src/theme.rs`); egui was chosen over Dear ImGui for being pure Rust with a maintained Bevy 0.19 integration and egui-native docking, gizmo and node-graph crates. Every widget emits a `Command` (`apps/editor/src/state.rs`) that calls the `AuthoringProject` operation of the same name, so the GUI and the JSONL protocol share validation, history and source formatting:

- **Scene**: switch between **Masters & wards** (`masterIs`) and **Placement** (zones, spawners, named spawns). Definitions form a separate inheritance tree (`descendsFrom`). **New actor (instance)** chooses a definition, placement spawner and optional master from the hierarchy; the created actor appears beneath its master and can be undone. New definitions descend from the selected definition; the parent is shown explicitly.
- **Colors**: blue instances, gold masters, green wards, purple definitions and peach assets; role labels and indentation carry the same meaning. Source problems are red, rejected operations amber, and locally authored fields have amber dots.
- **Inspector**: an entity's definition, source `file:line`, StableId, master selector and placement; its authored components are edited as scene `overrides` on the spawn. A definition shows its lineage, its **Extensors** (in use and why, dropped ones with Restore, suggestions as `+` buttons and an **Add extensor…** menu, each calling `add_extensor`/`remove_extensor`) and resolved components; edits write its own file. **Reset** removes a local field override to reveal its inherited/default value. Drags on a number form one undo group.
- **Viewport**: instances as capsules, zones and spawners as gizmos. Click selects; dragging a named spawn moves it on the ground (Shift: height) through `move_spawn`, one undo step per drag. Right-drag orbits, middle-drag pans, the wheel zooms, F frames the selection.
- **Problems**: `validate` diagnostics and the last rejected operation without duplicating identical diagnostics. Broken definitions stay listed and show their original source with an **Open source in editor…** action; definition locations select their definition.
- **Narrow windows**: Scene, Inspector and Viewport become tabs; selecting an object opens its inspector. Problems stays across the bottom and toolbar controls wrap.
- **Top bar**: Undo/Redo (Ctrl+Z, Ctrl+Shift+Z), Refresh (also on window focus, for outside edits), Play/Pause/Step/Stop (Ctrl+P). Play steps the separate play world by the game's fixed timestep; the panels then inspect that world read-only.

The editor builds projects with `struction_scene::authoring_app`: every engine package headless, over the engine's base definitions, plus the small example's `Health` behavior. Physics, gravity, character components, moves and game actions are therefore available to preview, validation and isolated play. Engine definitions are listed with the project's and are read-only: the inspector says so, and the first edit (or extensor change) creates the project's override file at the same path, removed again if the edit is rejected. Instances render as markers rather than their meshes, and rotation/scale are read-only, like the backend.

The Assets panel reads a project's sibling `assets/` directory when it has one; the engine's models live in `crates/struction_scene/content/assets`. `--mesh` paths are relative to the asset directory shown by this layout.

## The VS Code extension

`crates/struction_language` is a read-only host for editors, and `extensions/vscode` is the client built on it. The host answers two JSONL commands and never writes a source file:

```bash
cargo run -p struction_language --bin struction-language -- apps/playground/project
```

```jsonl
{"id":1,"command":{"op":"describe"}}
{"id":2,"command":{"op":"analyze","sources":{"characters/player/entity.jsonc":"{\n  \"descendsFrom\": \"characters/humanoid\"\n}\n"}}}
```

`describe` returns `protocol_version` 1 and the supported commands, so a client can refuse a host it does not understand. `analyze` takes every open buffer of a Struction source, keyed by project-relative path, and returns one snapshot:

- `schema`: the JSON Schema generated from the registered types, which drives completions and diagnostics.
- `scene_schema`: the same schema for `scenes/**.jsonc` — `zones`, `spawnerList`, and each spawn's `definition`, `offset`, `rotation`, `masterIs` and `overrides`, with the definition schema placed at override sites. `struction_world::scene` is the authority for that grammar and a test compares the two, so a scene completes and explains itself the way a definition does. Scene *semantics* (references, masters, overrides) stay the backend's: the client does not re-report them.
- `definitions`: every resolved path with its lineage, resolved data, components, why each extensor is in use, its library and the absolute file it came from.
- `extensors`: every registered package with its doc, opt-in flag, requirements, contributed states and components.
- `actions`: every registered action with its doc, typed parameters and requirements.
- `diagnostics`: the backend's validation of the project with the supplied buffers. Files are project-relative, except library files, which are absolute so a client can open them.

Buffers replace disk sources for that request only, and the next request without them reads disk again, so nothing reaches the filesystem. A definition that does not resolve is absent from `definitions` but still reported in `diagnostics` with its position; that is how a broken file stays repairable. The extension (`extensions/vscode`, see [development.md](development.md#vs-code-extension)) adds hovers, completions, go-to-definition and problems on top of the snapshot, and its `Struction: Inspect Definition` command renders the same description the hover uses.

## Choosing Blender

**Programs… → Blender executable → Browse… → Save** stores a machine-local executable path. Linux accepts the actual executable, including custom installations; macOS accepts the executable inside the application bundle (pasting a `.app` path also resolves it). Spaces are passed literally, without shell parsing. The same choice drives mesh imports, collision/LOD preparation and **Open in Blender**. Close an open mesh tool before saving a different program, then reopen the asset.

A one-run override is also available:

```bash
cargo run -p struction-editor -- apps/playground/project --blender /path/to/blender
```

The order is `--blender`, saved editor choice, then automatic discovery (`STRUCTION_BLENDER`, `PATH`, macOS application directories). **Detect** fills the discovered path; **Save** activates it. Settings live in `$XDG_CONFIG_HOME/struction/programs.json` or `~/.config/struction/programs.json` on Linux, and `~/Library/Application Support/struction/programs.json` on macOS. These paths stay out of project sources. The asset CLI still accepts `STRUCTION_BLENDER` for its own processes.

## Source navigation and adding fields

Right-click a definition, instance or spawner in the scene tree and choose **Open in IDE**,
or use the inspector's button. Instances open their spawn entry; definitions open their
own file (the library file until a project override exists). Broken definitions remain
openable for repair. Runtime entities without authored provenance have no source button.

**Programs… → IDE command** is machine-local alongside Blender. The initial command comes
from `$VISUAL`, then `$EDITOR`, or `code --goto {file}:{line}:{column}`. Use `{file}`, `{line}`
and `{column}` placeholders, and quote executable paths with spaces. Without `{file}`, the
path is appended. Commands launch directly without shell expansion; terminal editors need
a terminal launcher in the template. Saving either program preserves the other setting.

Inspector controls follow the registered schema. Known values such as movement states and
cancellation actions use dropdowns, flags use checkboxes, and numbers use draggable numeric
inputs (sliders only when the schema supplies both bounds). Fixed-size vectors keep their
length. Optional values can be enabled or cleared. A numeric drag is one undo step.

Object sections offer **Add field…** for missing fields; map sections also accept a new key.
Inherited/default fields already shown can be edited directly. **Add entry…** opens a typed
form: a `cancel_into` entry has an action dropdown and an `after` number, rather than a JSON
text box. Nested objects and list entries expose the same controls. List size limits from
the schema constrain additions and removals.

**Remove entry** deletes one list element; **Clear list** explicitly overrides the list with
an empty one. **Reset** removes a locally authored field, revealing inherited/default data
if present. Resetting a list restores inheritance; clearing it leaves it empty. Required
fields inside list entries cannot be individually removed: edit the entry, remove it, or
reset its owning list. All edits validate before writing and are disabled during Play.
Editing inherited entries first materializes their list so siblings survive. Authored
entry removals preserve neighboring comments; undo restores the exact previous source.

The main Toolbox menu and asset inspector open the same three workspaces as the mesh tool:
**Prepare** (model inspection, LODs and collision), **Surface** (UVs and materials), and
**Poses** (poses and sequences).

The same operations are available over JSONL:

```jsonl
{"id":1,"command":{"op":"source_location","target":{"kind":"definition","path":"characters/player"}}}
{"id":2,"command":{"op":"field_options","file":"characters/player/entity.jsonc","path":["components","Roll"]}}
{"id":3,"command":{"op":"add_field","file":"characters/player/entity.jsonc","path":["components","Roll"],"key":"cancel_into","value":[]}}
{"id":4,"command":{"op":"add_entry","file":"characters/player/entity.jsonc","path":["components","Roll","blocked_while"],"value":"Attacking"}}
{"id":5,"command":{"op":"add_entry","file":"characters/player/entity.jsonc","path":["components","Roll","cancel_into"],"value":{"action":"Attack","after":0.25}}}
{"id":6,"command":{"op":"edit_field","file":"characters/player/entity.jsonc","path":["components","Roll","cancel_into",0,"action"],"value":"Jump"}}
{"id":7,"command":{"op":"remove_entry","file":"characters/player/entity.jsonc","path":["components","Roll","cancel_into"],"index":0}}
{"id":8,"command":{"op":"undo"}}
```

Scene override paths begin with `spawnerList`, the spawner name, `spawns`, the spawn name,
`overrides`, then the definition field path. Schema-generated initial values are starting
points: required references or cross-field constraints may need an explicit value.

## Guarantees and current limits

- Validation resolves candidate definitions, inheritance, presets, scene overrides, action references, master references and aliases before writes. Invalid data leaves files and history unchanged. Invalid projects can open for diagnosis; play requires valid sources. JSON syntax damage can be repaired externally, followed by `refresh`.
- Field edits preserve surrounding JSONC comments. Undo restores original bytes, including removed array entries, parent objects, ordering and formatting. Grouped drags across files undo together. An undo that would reintroduce invalid data is rejected with history intact.
- Each file replacement is staged and atomic. Multi-file history restoration preflights every file and rolls back on ordinary write failures; this is not a crash-recovery journal or a distributed filesystem transaction. Use one serialized authoring host per project. Revision checks detect stale edits but are not an interprocess lock.
- Outside edits invalidate affected history. `refresh` retains the last valid preview if the new sources fail validation. Successful edits rebuild the whole preview, including master grants; large-project incremental preview updates are future work.
- Play uses a separate app rebuilt from authored source, not a snapshot of arbitrary runtime state. Play edits and undo are blocked. Stopping discards simulated state. File creation produces a user-owned scaffold and is currently outside undo history.
- The first gizmo operation is translation of named spawns. Rotation/scale gizmos, source rename/delete transactions and rendering instances' own meshes remain to implement through the same backend. Existing `struction_world::rename_path` is not yet an editor command.
- Physics/animation/AI scene integration, runtime grant hot reload, streaming persistence and procedural animation's visual quality remain separate work. These are not prerequisites for starting the first hierarchy/inspector GUI, but this backend is not a claim that the complete README validation scene is finished.

## Verification

```bash
cargo test --locked -p struction_data -p struction_world -p struction_editor
cargo clippy --locked -p struction_data -p struction_world -p struction_editor --all-targets -- -D warnings
cargo fmt --all --check
```

Tests exercise the full inspect/edit/validate/preview/undo/play loop, inherited fields, grants, source locations, rotated placement frames, stable identity across moves, grouped history, stale revisions, external invalidation, disabled entities, empty/invalid projects and JSONL error recovery. Asset readiness was also checked with all 23 `struction_assets` tests, including actual headless Blender import/export.

## Scene presentation and Play controls

MOON separated `SceneVisualsPlugin` (geometry, materials, model compilation and rig dressing)
from the gameplay-only camera fading in `SceneRenderPlugin`. Both applications use the
same visuals. `SceneRigPlugin` builds and retires skeletons without requiring a GPU. The
editor mirrors authored shapes, looks and transforms into its render world; in Play it
copies the isolated simulation's solved joint poses and player-camera transform. It does
not run a second physics world or solve animation twice. Component changes rebuild only
affected visuals; transforms and poses update independently. Mesh bounds drive picking
and selection outlines, including broad floors and the individual pieces of a rig.

Play uses the same character and camera plugins as the playground. Click the viewport to
capture controls: WASD/arrows move, mouse looks, Space jumps, Shift rolls, F/left click
attacks, V changes view and the wheel zooms. Escape releases the pointer. Pausing, losing
window focus or leaving the viewport tab releases held and pending commands. Stop returns
to the unchanged authored world. Projects without `PlayerControlled` can simulate but
have no player to steer; the viewport states that explicitly.

The headless equivalent is `play_input` with an `input` object (`movement: [x, forward]`,
`look: [yaw, pitch]`, `jump_held`, `jump_pressed`, `roll_pressed`, `attack_pressed`,
`toggle_view_pressed`, `zoom`). Held movement persists until replaced; button edges, look
and zoom accumulate until the next simulation update and are consumed once. Use
`release_play_input` to clear queued/held input, then `step_play` as usual. Factories opt in
with `PlayInputPlugin`. Neither operation changes sources or history.

Spatial guides show gravity influence boundaries and arrows sampled from the actual
field function (purple). Infinite fields get a local symbol and an explicit label instead
of an invented boundary. Camera volumes are cyan, with a vector camera icon and their
fixed/follow constraints, weight and priority. Fixed cameras link to their world position.
Toggle Spatial guides to hide them; gameplay hides authoring guides automatically.

## Model tools and state targets

The permanent **Toolbox** menu lists model sources and opens the model-tool window. Its tabs
are three workspaces, each holding work that belongs together so it needs no switching:
**Prepare** (LODs and collision, both preparation-recipe settings), **Surface** (UVs and
material of one part) and **Poses** (keys `1`–`3`). A model summary (parts, triangles,
materials, recipe) stays above every workspace and folds out per mesh. One part selection is
shared: click a part in the view, or pick it from the list, and it glows in every workspace.
Overlays are switches at the bottom of the view usable in any workspace (wireframe, hull,
trimesh, convex parts, LOD 0 beside the chosen level, pose ghosts); each workspace keeps its
own. One bar at the bottom lists what is not applied yet, across workspaces, with one
**Apply** (`Ctrl+Enter`) and **Revert** (`Esc`); **Undo**/**Redo** walk back the tool's applied
edits newest first, whether recipe or project edits, and refuse a project edit that is no
longer the project's latest. Collision presets and detail/decomposition controls preview
before Apply. **Surface** draws the selected part's UV triangles over a checker tile beside the
3D view, can generate missing coordinates through the preparation recipe, and opens the part
in Blender's **UV Editing** workspace for seams, repacking or re-unwrapping.

**Poses** selects a definition using that model and edits its overrides of the rig's pose
library ([pose sequences](moves.md#pose-sequences)) in one workspace; every name, description
and use it shows comes from that data and the definition. The **timeline** under the view shows
the chosen sequence over its real length (the move's `duration`, such as `Attack.duration`,
editable beside "Played by"; a loop's `seconds` otherwise): keys and events are markers to drag,
the fades are shaded, and the playhead can be scrubbed, played (`Space`), repeated or stepped
key to key (`,` `.`). Clicking a key edits its pose in the sidebar while the view keeps the
sequence at that moment; a key the sequence is still fading at is shown alone ("Key pose
alone"). Faint ghosts show the keys the playhead is between. Joints are picked by clicking the
model, from the list, or by walking the skeleton (`↑` parent, `↓` child, `Tab` other side); the
chosen joint glows, joints the pose sets are marked, and three rings turn it about its own
axes by dragging, beside exact XYZ angles in degrees. **Mirror** copies the pose's joints on
the selected side to the other, mirrored. Keys can also be added at the playhead, removed and
reordered, and looping, length, takeover, fades, events and the pelvis tumble edited under
Sequence settings. Apply checks the overrides against the rig, then writes sparse
`PoseTargets { poses, sequences }` as one validated, undoable source edit (the JSONL `edit`
command does the same); engine definitions get project overrides. Runtime rigs merge them with
the library when they change, and the swing's `strike` event times its hit.

**Materials** identifies each mesh part and material slot and opens Blender **Shading**
with that object and slot active. The configured Blender executable is used for these
handoffs, scene model compilation and mesh preparation on all platforms. Save in Blender
and Refresh the tool to reimport. For glTF, export back to the same source path. The current
`.smesh` format carries material factors, not texture images; texture-node changes remain
in the source and will need texture support in the asset format to appear in the runtime.
