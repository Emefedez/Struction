# Struction

A data-driven game engine designed for fast authoring through composition, exclusively procedural animation, and small integrated tools.

This document describes the intended architecture and development sequence, not implemented features. Keep the scope manageable for a solo developer: reuse mature infrastructure and spend custom work on gameplay composition and authoring.

## Core model

Entities contain identity and components. Components describe capabilities; systems implement behavior. Relationships and constraints connect entities, while volumes define regions where effects apply. Resources hold shared data, actions express gameplay intent, presets package reusable configurations, and packages extend the engine.

Built-in and user-defined content use the same mechanisms:

| Concept | Composition |
| --- | --- |
| Water | Volume, surface properties, buoyancy, particles, audio |
| Lava | Water-like preset with damage and different surface properties |
| Camera zone | Volume and camera constraints |
| Gravity planet | Collider, gravity source, surface |
| Grabbable object | Physics body and grip targets |

An entity without health simply has no `Health` component. Surfaces describe friction, drag, movement modifiers, and interaction responses instead of relying on hardcoded material types. Presets expose their components for inspection and customization.

Constraints share an authoring model—source, target, property, weight, priority, and falloff—while using appropriate solvers for each domain. They support grip alignment, foot contact, gaze, gravity alignment, and camera positioning. Camera presets combine ordinary constraints, with spatial influences blending by weight, priority, and falloff.

## Exclusively procedural animation

```text
Gameplay state → animation intent → pose requests and constraints → solvers → pose
```

Animation uses no authored animation clips. Reusable dataflow graphs combine state, curves, springs, math, blending, bone masks, IK, and deformation controls.

Objects describe their interaction affordances: `Grabbable` exposes grips, `Sittable` exposes body targets, and `Climbable` exposes handholds and footholds. Characters solve interactions from these targets rather than requiring object-specific animations.

Locomotion derives from velocity, ground contact, gravity, and predicted landings. Squash, stretch, and secondary motion respond to acceleration and impacts through dedicated deformation controls. Gameplay APIs express intent, such as `player.hold(object)`, rather than manipulating bones directly.

## Authoring and runtime data

Use existing formats wherever they fit. Create a custom format only when an existing one cannot meet a concrete requirement. The editor modifies readable, diff-friendly source files, which remain the authoring source of truth.

```text
Editable sources → asset compiler → compiled assets → runtime ECS
```

Compiled entity/component data uses schema-defined layouts with known field offsets and fixed strides where practical. Variable-sized data lives in separate payloads referenced by offsets, counts, and layout metadata. A schema can reserve a reference field for extension data; readers must be able to distinguish references from inline values. Serialized references are file-relative offsets, not process memory pointers.

Addon components can use fixed layouts when their schemas permit; extensions do not force ordinary fields into sequential parsing. Compiled layouts are versioned and validated during loading. Runtime ECS storage remains independent and optimized for component iteration. Parse or decode assets during loading and hot reload, rather than repeatedly reading serialized records during simulation.

Hot reload is a core workflow for entities, presets, models, textures, materials, animation graphs, and levels. Keep shared resources separate from instance state so updates can reach existing instances where feasible.

## Foundation and packages

Use Rust and a pinned Bevy revision. Retain useful infrastructure such as ECS, scheduling, reflection, tasks, asset handling, rendering, windows, and input. Modify or replace subsystems when necessary; future upstream compatibility is optional.

Ordinary authoring uses the engine's vocabulary. Advanced integrations may access the underlying Bevy world. A runtime scripting language remains undecided and should be added only if it improves iteration.

Packages can provide components, systems, constraints, presets, importers, build steps, script APIs, and editor extensions. An engine-specific manifest can register Bevy plugins internally. Systems declare explicit scheduling phases and dependencies; ordering must not depend on plugin registration order.

Reuse C/C++ libraries for expensive operations such as UV unwrapping, mesh simplification, compression, and import. Use coarse operations over buffers through C-compatible interfaces, with explicit ownership and errors, minimal copying, and no panics or exceptions crossing the boundary. Import and build tools may run as subprocesses.

## Editor and toolboxes

The native editor targets **Linux, Windows, and macOS**. It integrates filesystem access, file watching, compilers, and external applications.

A toolbox brings the main or simplest functions of a full creative tool into the editor:

| Toolbox | Common tasks |
| --- | --- |
| Texture painting | Create a texture, auto-unwrap, paint on a model; basic brushes, erase, and layers |
| Mesh preparation | Generate UVs, collision shapes, and LODs |
| Animation and constraints | Edit procedural graphs, interaction targets, and constraints |
| World properties | Edit surfaces and camera behavior |
| Audio | Trim clips |

When a toolbox is insufficient, **Open in…** launches the appropriate full application with the working source file. Prefer compatible formats and shared source assets so users can continue their work externally. Saving triggers reimport and hot reload. Toolboxes may reuse libraries or external processes rather than reimplementing mature algorithms.

Support `.blend` sources through a headless Blender exporter, initially using glTF as an intermediate if suitable. Ship compiled assets; release games do not require Blender.

## Streaming and exports

Support native desktop and browser games through shared game code. The asset layer abstracts disk, package files, and browser/network caches.

Streaming uses spatial cells and explicit asset demand rather than checking every entity's distance each frame. Volumes and gameplay events can prefetch teleport destinations, encounters, or other resources. Request dependency-aware bundles, retain shared assets while needed, and cache unused assets until memory pressure warrants eviction. Use hysteresis and least-recently-used eviction for eligible resources.

Separate simulation from presentation from the start. Headless servers must not require a window, GPU, audio, or editor. Add replication, snapshots, prediction, and interpolation after the core engine works. Component schemas may describe network roles, but wire formats remain separate from runtime layouts and handle versioning, quantization, and endianness explicitly. Clients derive cosmetic poses from gameplay state; gameplay-relevant pose information needs authoritative treatment. Single-player simulation requires no network transport.

## Development sequence

1. **Foundation:** pinned Bevy base, component schemas, source loading, inspector, presets, volumes, constraints, and hot reload.
2. **Procedural character:** skeleton access, arm/leg IK, gaze, grabbing, gait, jumping, springs, and squash/stretch.
3. **World systems:** surfaces, camera volumes, gravity, teleporters, interaction particles, and user presets.
4. **Asset pipeline and toolboxes:** Blender import, compilation, painting, UVs, collision generation, graph editing, and external-tool handoff.
5. **Streaming and web:** world cells, bundles, residency, eviction, semantic prefetch, and browser export.
6. **Package validation:** move planetary gravity into an external package containing components, constraints, systems, editor tools, and presets.
7. **Multiplayer:** headless server, replication, snapshots, prediction, interpolation, and network metadata.

Validate the design in a small playground with a player, cube, chair, water, slippery floor, gravity planet, camera zone, and teleporter. Demonstrate procedural walking, jumping, looking, grabbing, carrying, and dropping, alongside surface effects, camera blending, gravity, and streaming hints. Important behavior must remain inspectable and editable through data.

## Scope

Prefer composition before adding primitives, events and change detection where appropriate, and existing libraries before custom infrastructure. Introduce abstractions when they solve a current problem.

Initial non-goals: AAA rendering, full creative-tool replacements, stable binary plugin ABI, giant-world technology, general visual scripting, deterministic multiplayer lockstep, custom GPU backends, and compatibility with every future Bevy release.

Success means building unusual interactive scenes substantially faster by combining inspectable data and reusable behavior.
