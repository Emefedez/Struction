# Struction

A data-driven game engine derived from Bevy, focused on fast authoring through composition, procedural animation, and small integrated tools.

This describes the design and current implementation direction. The workspace has headless engine packages, world integration tests, a native physics playground and an editor ([development setup](docs/development.md)); detailed implementation status is tracked in [AGENTS.md](AGENTS.md). Rationale and adopted decisions are in the [design review](docs/design-review.md). Scope is sized for a solo developer. Linux is the reference development platform; macOS development is supported, while Windows and web remain follow-up targets.

## Core model

Entities hold components (states and capabilities); systems implement continuous behavior; actions implement discrete behavior. Volumes define regions where effects apply, constraints bind entities spatially, and presets package reusable component sets. Built-in and user content use the same mechanisms:

| Concept | Composition |
| --- | --- |
| Water | Volume, surface properties, buoyancy, particles, audio |
| Lava | Water preset with damage and different surface properties |
| Camera zone | Volume and camera constraints |
| Scene gravity | Gravity field with an infinite volume |
| Gravity planet | Collider, radial gravity field, surface |
| Grabbable object | Physics body and grip targets |

- An entity without health has no `Health` component. Surfaces describe friction, drag, and interaction responses instead of hardcoded material types. Cross-cutting traits such as `Flammable` are components.
- Gravity is not a global constant: bodies sum the gravity fields affecting them, which also defines each character's local up.
- **Constraints** share one authoring model (source, target, property, weight, priority, falloff) with domain-specific solvers: attachment (sword in hand, rider on mount), grip alignment, foot contact, gaze, gravity alignment, and camera positioning. Camera presets are ordinary constraints.
- Systems iterate components in batches; relations do not drive the update loop.

### Relations

Struction's own vocabulary, independent of Bevy's `ChildOf`:

| Relation | Between | Meaning |
| --- | --- | --- |
| `descendsFrom` | Definitions | Type lineage, like `extends` in Java. Single parent, transitive (`small_ogre` → `ogre` → `Actor`), inherits defaults. Never changes at runtime |
| `masterIs` | Instances | The ward is at its master's disposal. Changes at runtime (adoption, `join_party`) |

Definitions are classes; spawned instances are objects. Primordial engine types are capitalized (`Actor`, `Terrain`), user types lowercase. A master can:

- **Notify** its wards by invoking an action on all of them; wards can react to the master's actions.
- **Order** its wards; orders feed the ward's decision tree, which may still override them.
- **Grant capabilities**, declared in the master's definition and filtered by lineage. Granted components, and the actions that require them, are added when the relation starts and removed when it ends. They are derived, not saved. A ward may refuse or override grants.

```jsonc
// player/entity.jsonc
"grantsToWards": [
  { "to": "minions/ogre", "components": { "Follower": { "distance": 3 } }, "actions": ["fetch", "guard"] }
]
```

Groups (squads, encounters, fish schools) are master entities. Queries combine both relations: "wards of this player that descend from `minions/ogre`". Streaming residency is tracked separately, so crossing a streaming boundary never changes a relation.

## Actions and reactions

Actions are registered Rust functions referenced by entity data. They can update components, invoke other actions, or change relations; `die` and `join_party` are behaviors, not built-in states. Reactions bind to another entity's action:

```jsonc
"reactions": [
  // After my master's `die` completes, run my own `die`.
  { "source": "master", "after": "bosses/ogre_lord/die", "call": "minions/ogre/die" }
]
```

- The compiler resolves references and validates signatures; registered actions expose metadata to the editor.
- Reactions fire on action invocation, not on state changes: setting health to zero is not `die`.
- Death, removal, and streaming unload are distinct; unloading a boss never triggers its death.
- Reactions are queued and run at defined points, never while systems iterate.

## Procedural animation

```text
Gameplay state → animation intent → pose requests and constraints → solvers → pose
```

No animation clips. Authored **base poses** (idle, grip, fist, seated, aim) act as attractors: solvers move the body away from them and springs snap it back. Reusable dataflow graphs combine curves, springs, blending, bone masks, IK, and deformation. Objects expose affordances (`Grabbable` grips, `Sittable` targets, `Climbable` holds) that characters solve against. Locomotion derives from velocity, ground contact, gravity, and predicted landings; squash, stretch, and secondary motion respond to acceleration and impacts. Gameplay expresses intent (`player.hold(object)`), never bones.

## AI and sensing

Definitions reference a decision tree asset (`"brain"`), shared by simple enemies or unique to a boss. Leaves invoke registered actions; conditions read components, sensing, and orders from the master. Sensing filters by lineage, including descendants:

```jsonc
// minions/small_ogre/entity.jsonc
{
  "descendsFrom": "minions/ogre",
  "brain": "ai/simple_ogre",
  "sensing": { "sees": ["player"], "flocksWith": ["minions/ogre"] }
}
```

## Input

Devices produce raw input that a mapping turns into input actions (`Jump`, `Move`); gameplay only sees input actions. The first version maps the keyboard (and mouse in the editor); gamepads are another device mapping.

## Data

```text
JSONC sources → asset compiler → compiled binary → runtime ECS
```

- **Schemas:** Rust component structs with `Reflect` are the source of truth; JSON Schema is generated for validation and editor completion.
- **Sources:** JSONC with comments and a canonical field order (identity and `descendsFrom`, transform, components, constraints, reactions). The order is a convention, not execution semantics. The editor writes through a syntax-preserving parser that keeps comments and formatting. Non-entity assets use existing formats.
- **Compiled binary:** versioned layouts with fixed field offsets and strides, read in place by offset instead of parsed; variable-sized data lives in payloads referenced by file-relative offsets. Components are copied into ECS storage at spawn. The format is designed once schemas stabilize; until then the runtime loads JSONC directly.
- **Identity:** authored files, zones, spawners, and named spawns are identified by path (`Fortress/LeftCourtYard/courtyard_guards`); runtime-created entities by UUID. Spawners record internally which UUIDs they created, so saves restore them and dead spawns do not reappear.
- **Errors** point to `file:line`. When a schema changes incompatibly, loading old data fails with a clear error; automated migrations come later.
- **Hot reload** covers entities, presets, models, textures, materials, animation graphs, and levels.

| Layer | Holds |
| --- | --- |
| Definition | Shared components, defaults, constraints, action references |
| Scene / spawn | Placement, instance identity, overrides |
| Runtime state | Current health, position, targets |
| Save data | Persistent state, stable IDs, created and removed entities |

Many minions share one definition; taking damage changes memory only.

## Spawners

A definition says what an entity is; a spawn description says where, when, and how many. Players, bosses, and single objects use the same system. Spawners are organized under named zones and each spawn has a local offset rotated by the spawner:

```jsonc
"spawnerList": {
  "courtyard_guards": {
    "zone": "Fortress/LeftCourtYard",
    "tile": [0, 2, 3],
    "position": [4, 0, 6],
    "spawns": {
      "fireman1": {
        "definition": "minions/fireman",
        "offset": [0.5, -0.2, 0.0],
        "masterIs": "Fortress/Keep/ogre_lord"
      }
    }
  }
}
```

Several spawners may share a tile. The editor shows **draggable translucent model previews** (drag a preview to edit its offset, drag the origin to move the group) plus XYZ fields and sliders, all editing the same source values. Spawned entities move independently unless a constraint attaches them.

## Project layout

```text
project/
├ project.toml
├ minions/ogre/
│  ├ entity.jsonc
│  ├ scripting/actions.rs
│  └ assets/
├ bosses/ogre_lord/
├ scenes/
├ menus/
├ templates/
└ build/generated/
```

Templates scaffold enemies, spawners, and menus with ordered definitions, comments, and starter functions; scaffolded files become user-owned. Generated action descriptors live in `build/generated/`.

## Foundation and packages

- Rust with a pinned Bevy version, reusing its ECS, scheduling, reflection, assets, rendering, windowing, and input. Subsystems may be replaced; upstream compatibility is not a goal.
- A **package** is a Rust crate with assets and a manifest, compiled with the game. It bundles components with the actions and systems that apply to them, plus presets, constraints, importers, build steps, and editor extensions. Systems declare explicit phases and dependencies, never relying on registration order.
- Physics comes from an existing library wrapped as a package; gravity fields are a separate package.
- C/C++ libraries (UV unwrapping, simplification, compression) are called through coarse buffer-level interfaces with explicit ownership and no unwinding across the boundary, or run as subprocesses.
- Coordinates follow Godot and Bevy: Y up, right-handed, −Z forward, meters. Blender's Z-up is converted on import.

## Editor and toolboxes

The toolbox contains lightweight mini-programs that open in separate native
windows, plus click-to-run utilities with a few choices. Each provides a complete
simple workflow with useful controls, feedback and revision. Its backend may
use a small library, a narrow internal toolset or a full app such as headless
Blender when that makes the task straightforward without requiring the user to
delve into the larger app. Merely launching an operation is not sufficient.
**Open in…** separately offers the full application's interface when the user
needs more power. The
[toolbox plan](docs/toolbox.md) maps tools to their bases, handoffs,
implementation gaps and MOON's planned work, including attribution for reused
or modified open-source tools.

Keep the toolset narrow: reuse existing editor controls, add mini-programs only
for concrete tasks, and leave vector editing and SVG support out of scope.

The native editor is built with egui and is Linux-first. It opens a project, shows the scene and definition hierarchy, edits reflected fields and lists, provides toolbox workflows, and supports selection, spawn transforms, undo/redo and isolated Play mode. Edits are recorded as undoable changes (file, field, previous and next value); continuous edits such as a gizmo drag form one transaction. Play mode runs on a copy of the world and is excluded from history. **Open in…** hands a source file to the full application; saving triggers reimport and hot reload. `.blend` files are converted through headless Blender and glTF; shipped games do not need Blender.

**AI authoring is a first-class requirement and is implemented through the same APIs.** Inspection, validation and editing operations are available without a window over structured JSONL and MCP transports. Component schemas, action metadata and extensor provenance make capabilities discoverable; edits preserve comments and stable identities, participate in undo/redo, and return actionable source errors. The editor can serve its open project over localhost MCP and includes an assistant integration for installed, signed-in coding agents. An AI can inspect a game, change its definitions or scene, and verify the result without automating mouse clicks.

| Toolbox | Tasks | Version |
| --- | --- | --- |
| Animation and constraints | Base poses, procedural graphs, interaction targets, constraints | First |
| Mesh preparation | UVs, collision shapes, LODs | First |
| World properties | Surfaces, camera behavior | First |
| Texture painting | Auto-unwrap, paint on model, brushes, layers | Later |
| Audio | Volume, sound zones, clip trimming | Later |

## Menus

Menus are entities with layout, visibility, labels, focus, input scope, and action bindings. Buttons invoke registered actions such as `resume_game`. UI containment propagates layout, visibility, and lifetime without world transforms.

## Streaming, web, and multiplayer

- Streaming uses spatial cells and explicit asset demand. Cells organize loading only, never gameplay relations. Volumes can prefetch teleport destinations and encounters. Bundles are dependency-aware, with hysteresis and LRU eviction.
- Native and browser builds share game code; the asset layer abstracts disk, packages, and browser caches.
- Multiplayer is not in the first version, but the structure must allow it: fixed-timestep simulation with render interpolation, stable IDs, input converted to commands before reaching the simulation, no simulation reads from presentation, and seeded randomness. Headless builds need no window, GPU, or audio.

## Open decisions

- Whether every definition must descend from a primordial type.
- What happens to wards when their master dies or is removed (orphaned, despawned, or reassigned), and how orders are weighted against the ward's own tree.
- Whether actions are instantaneous, with durations modeled as state components; this settles the start vs. completion hook for reactions.
- Whether spawners author tile coordinates or world/zone positions with derived tiles.
- How renaming a path preserves references and saves.
- Editor UI toolkit.
- Runtime scripting language, if any.
- Exact schema and reference syntax; one descriptor per action or a combined catalog; save format.

## Development sequence

Each milestone ends in something runnable on Linux.

0. **Risk spikes:** procedural legs with IK, springs, and base poses; Bevy hotpatching (done: opt-in, see [live reload](docs/live-reload.md)); comment-preserving JSONC edits; physics library compatibility.
1. **Physics playground:** physics and gravity-field packages, character controller with variable up, keyboard input, cube, slippery floor, water, camera zone, fixed timestep with interpolation, headless tests.
2. **Data:** JSONC loading through `Reflect`, `descendsFrom` inheritance, presets, overrides, hot reload, `file:line` errors.
3. **Actions, AI, and encounters:** action registry and reactions, `masterIs` with grants, decision trees and sensing, spawners, boss and minions, save/load.
4. **Editor:** hierarchy, inspector, gizmos, undo/redo, play mode, spawn previews, templates.
5. **Character and animation toolbox:** base poses, gaze, grab/carry/drop, jumping, squash/stretch, sitting.
6. **Asset pipeline:** Blender import, compiled binary format, generated collision and LODs, Open in….
7. **Package validation:** move planetary gravity into an external package.
8. **Streaming.**

After the first version: web, Windows and macOS, multiplayer, texture painting, audio toolbox, video playback.

The validation playground has a player, cube, chair, water, slippery floor, gravity planet, camera zone, teleporter, a boss/minion encounter, and a menu. It must demonstrate procedural walking, jumping, looking, grabbing, carrying, and dropping, with important behavior inspectable and editable as data.

## Scope

Prefer composition over new primitives and existing libraries over custom infrastructure; add abstractions only for a current problem.

Non-goals: AAA rendering, full creative-tool replacements, stable binary plugin ABI, giant worlds, general visual scripting, lockstep multiplayer, custom GPU backends, tracking every Bevy release.

Success means building unusual interactive scenes substantially faster by combining inspectable data and reusable behavior.
