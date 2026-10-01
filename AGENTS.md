# AGENTS.md

Working notes for coding agents (Claude, Codex) implementing Struction. The design lives in [README.md](README.md); rationale and the adopted resolutions of the README's open decisions are in [docs/design-review.md](docs/design-review.md) (the only Spanish document; everything else, including code, is English). Setup (Linux or macOS) is in [docs/development.md](docs/development.md).

## Rules

- Pinned Bevy 0.19.1 and Rust 1.98.1. Bevy is newer than most model training data: check APIs in `~/.cargo/registry/src/*/bevy_*-0.19.1` instead of guessing.
- One crate per concern under `crates/`. Keep crates headless-testable; only applications enable rendering features. Shared dependencies go in `[workspace.dependencies]`.
- Before committing: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all --check`. Commit `Cargo.lock`.
- Systems use explicit system sets and ordering, never registration order. Simulation runs in `FixedUpdate` and never reads presentation state; randomness is seeded.
- Match the surrounding code: sparse comments explaining why, no speculative abstractions (README "Scope").
- AI tools are first-class authoring clients. Expose inspection, validation and edits through headless APIs over structured data; editor UI must call the same operations. Preserve comments, stable identities, actionable source errors and undoable changes. Do not make game authoring depend on GUI automation.
- Commit often with focused messages, staging only the files you touched (several agents share one checkout). Update the status table below when a milestone step lands.
- Before editing, read the Work ownership table and check `git status`. Claim the area under your agent name, including its scope; do not overwrite another active owner's work. Coordinate overlapping changes first. Update the entry when work lands or is handed off, and record who implemented or verified each status update. One agent is **MOON**, the other is **SUN**. If deemed necessary, **SUN** can "overthrow" **MOON**'s choices, but this cannot happen the other way around.


## Work ownership

Check this table before proceeding with changes. An active claim covers only its stated scope; unlisted areas are unclaimed, and historical ownership does not block later work.

| Area | Owner | State / scope |
| --- | --- | --- |
| Editor loading, actor master hierarchy and Blender selection | **MOON** | Active (backend implemented and verified by **MOON**: 36 tests and Clippy): repair editor project/type discovery and actionable errors; expose master/ward hierarchy when inspecting and creating actors through shared headless operations; configurable Blender executable and Linux verification. Coordination: game types stay in SUN’s `apps/playground/src/scene.rs`; MOON adds a library entry point exposing that module and its headless factory, used by the editor. Preserve SUN’s extensor/model changes and toolbox work |
| Playground frame pacing | **MOON** | Active: measure native frame times, fix verified gameplay/presentation stalls. The melee attack was handed to **SUN** at the user's direction and landed as the `combat` extensor (see the extensors row). Scope: playground and character/animation integration; asset toolbox changes remain with their current owner |
| macOS playground source paths | **MOON** | Active: fix case-colliding `Ground`/`ground` and `Gravity`/`gravity` directories reported by the user's native run; update authored references, scene tests and development instructions |
| Character dodge roll | **MOON**, then **SUN** | Landed: **MOON** implemented the roll (fixed-step state, procedural presentation, input/HUD, headless tests). At the user's direction **SUN** moved it out of the controller into the opt-in `dodge` extensor (`Roll`, `Rolling`, `dodge/roll`), with recovery shared through `CharacterMove`; MOON's roll tests still pass. See `docs/moves.md` |
| Extensors, generic model shapes, dodge/combat extensors | **SUN** (user's direction) | Landed (implemented and verified by **SUN**): `extensors` in definitions per `docs/authoring-direction.md` (registry in `struction_core`, opt-in refusal, supplied defaults, inference, provenance in `Resolved::extensors` and editor inspection, schema completions); timed moves through `CharacterMove` with `dodge` and `combat` extensors and `blocked_while` conditions; `MovePose` roll/swing presentation; `Shape::Humanoid { model }` for any actor, `figures.rs` replacing the knight-only code; `Actor`, `characters/player`, `characters/sentry`. Headless tests and Clippy; not yet run natively |
| Extensor authoring: drops, suggestions, operations and editor panel | **SUN** (user's direction) | Landed (implemented and verified by **SUN**): `"-name"` drops an inherited extensor and its components; suggestions from registered requirements; `add_extensor`/`remove_extensor` as single undoable steps (`EditSession::apply_fields_checked`) and JSONL commands; an Extensors section in the editor's definition inspector on the same operations. Headless tests and Clippy; the panel has not been looked at natively |
| Toolbox plan and asset-tool integration | **MOON** | Plan revised by **MOON**: separate mini-program windows and quick utilities providing complete simple workflows; small libraries or full-app backends where they simplify the task, with a separate optional Open in… handoff; see `docs/toolbox.md`. Next: collision/LOD workflows, shared tool sessions, mesh inspection and UV preparation using the existing Blender backend. Texture repair proposed for later. Implementation not started; collision/LOD utilities and the mesh preview window handed to **SUN** (next row) |
| Toolbox: collision/LOD utilities and mesh preview window | **SUN** (from **MOON**'s plan) | Active: steps 1–2 of `docs/toolbox.md`, taken on at the user's direction while MOON works on the roll: asset recipes, collision/LOD presets and a preview/apply/undo preparation session in `struction_assets`; an Assets panel, the collision/LOD quick utilities and a separate mesh preview window in `apps/editor`. UV preparation, the Open in… round trip and later inventory items stay in MOON's plan |
| Headless editor / AI authoring | **MOON** | Landed: validated project edits, exact-source undo, inspection, spawn movement, isolated play and JSONL commands. 32 tests, Clippy and native protocol smoke verified; see `docs/authoring.md` |
| Data/world authoring hooks | **MOON** | Landed: read-only candidate-source validation, scene-source provenance and integration tests; 69 data / 22 world tests and Clippy verified |
| Asset pipeline verification | **MOON** | Landed: 23 tests, including actual Blender import/export, and Clippy verified; implementation predates this verification |
| Playground/debug fixes | **MOON** | Landed: structured tracing, water rendering, camera movement and dry camera-zone fixes from the previous work session |
| Planet gravity and camera obstruction | **SUN** (from **MOON**) | Landed: MOON implemented gravity entry/exit hysteresis, simulation-owned `GravityPose` and `camera_obstructions`; SUN verified them, fixed the controller judging leaving the ground along up, and replaced whole-material fading with a sight-line shader cut-out (`apps/playground/src/sight_fade.wgsl`) for ground and water, reviewed natively with the user |
| Character animation integration | **SUN** | Landed (implemented and verified by **SUN**): `CharacterAnimationPlugin` bridge, `CustomGround` hook, slope fixes in the controller and locomotion, animated player and working HUD in `apps/playground`; native Vulkan smoke run verified |
| Playground scene as data | **SUN** | Landed: Milestone 1 scene authored in `apps/playground/project` and spawned through `struction_data`/`struction_world`; `Shape` (boxes, spheres, humanoids) and `Look` authoring components in `apps/playground/src/scene.rs`. Implemented and verified by **SUN**: 3 scene tests, and a `--smoke-test` run on llvmpipe under Xvfb that ends on the planet like `main` (37 vs 38 footfalls); not yet looked at on a real GPU |
| Player camera | **SUN** | Landed (implemented and verified by **SUN**): `struction_camera` with third-person orbit (zoom, collision pull-in, zones) and first-person views only, no free camera; `face_movement` turning in the controller; `ToggleView`/`Zoom` input actions; playground wiring. 7 camera tests, 1 controller test, Clippy; not yet run natively |
| Live reload / hotpatching spike | **SUN** | Landed (implemented and verified by **SUN**): `struction_world::live` applies saved sources to a running world field by field; the playground reloads its own project (`apps/playground/project`, or `--project DIR`) live, redrawing and re-colliding edited shapes and looks; subsecond hotpatching measured with `dx` 0.7.10. Headless tests only; see `docs/live-reload.md` |
| Blender bridge and player model | **SUN** | Landed (implemented and verified by **SUN**): macOS Blender discovery, `struction-assets build` for generator scripts, material factors in `.smesh` (format 2), and the Blender-generated blood knight dressing the playground rig with hot reload (`apps/playground/src/knight.rs`, `apps/playground/assets/models/`). Verified with Blender 5.0 (bpy) and a software-rendered playground run; not yet run on macOS |
| Editor GUI | **SUN** | Landed (implemented and verified by **SUN**): egui (`bevy_egui`) chosen over Dear ImGui, custom theme; `apps/editor` bound to `AuthoringProject` with project open, scene tree, inspector edits (instance overrides, definition fields, reset), problems, undo/redo, viewport selection and drag moves, and play/pause/step. 7 tests; a native run checked the hierarchy, inspectors and markers. Mouse picking/dragging and play were verified headlessly only |
| macOS development setup | **SUN** | Landed (implemented by **SUN**): macOS host section in `docs/development.md` and a macOS branch in `tools/check-environment.sh`; **Open in…** uses `open` on macOS and `STRUCTION_BLENDER` for `.blend` (struction_assets 23 tests, Clippy). Type-checked for `aarch64-apple-darwin` from Linux (all but `struction_assets`); native Mac run not yet verified |
| Code quality pass | **SUN** | Landed (implemented and verified by **SUN**): typed `EntityEntry`/inspection results from `AuthoringProject` instead of raw JSON (protocol output unchanged), undo without cloning history, dead history API removed, shared `ParamSpec::resolve` for actions and conditions, unused editor dependencies dropped |

## Adopted decisions

From the design review's pending recommendations, treated as decided:

- Actions are instantaneous; durations are state components (`Dying { timer }`) that invoke another action on completion. A shared system invokes `die` when `Health <= 0`.
- Spawners author world/zone positions; the streaming cell is derived.
- Wards of a dead or removed master become orphans; masters declare reactions for anything else.
- Every lineage ends in a primordial (capitalized) type.
- Renaming a path rewrites references and records an `old -> new` alias for saves.
- Editor UI: egui (`bevy_egui`). Scripting: Rust only. AI: behavior trees with a running status; boids for schools.

## Crates

| Crate | Milestone | Content |
| --- | --- | --- |
| `struction_core` | 3 | Identity (`DefinitionPath`, `Definition`, `StableId`), `MasterIs`/`Wards`, grants, action and extensor registries, queued reactions, notify/order |
| `struction_data` | 2 | JSONC definitions through `Reflect`, `descendsFrom`, presets, extensors, overrides, `file:line` errors, JSON Schema, comment-preserving edits, hot reload |
| `struction_gravity` | 1 | Gravity fields summed per body, local up |
| `struction_physics` | 1 | Avian 0.7 wrapper, fixed timestep and interpolation, surfaces, volumes (water, buoyancy), camera zones |
| `struction_character` | 1 | Input actions and mapping, character controller with variable up, `dodge` and `combat` move extensors |
| `struction_camera` | 1 | Player camera: third-person orbit and first-person views, variable up, collision pull-in, camera zones |
| `struction_anim` | 0, 5 | Skeleton, poses, springs, IK, constraints, dataflow graph, procedural locomotion, affordances |
| `struction_ai` | 3 | Behavior trees, condition registry, sensing by lineage, boids |
| `struction_world` | 3 | Spawners, zones, save/load, path renaming, data↔core integration |
| `struction_assets` | 6 | Blender/glTF import, UVs, collision, LODs, rkyv binary format, Open in… |
| `struction_debug` | Cross-cutting | Opt-in headless component-change tracing, bounded event log and JSONL output for tools |

## Status

| Area | State |
| --- | --- |
| `struction_core` | Done, 55 tests. Extensor registry (**SUN**) |
| `struction_data` | Done, 77 tests. `extensors` resolution and provenance; optional enum-variant fields may be left out (**SUN**). **MOON** added candidate-source validation without disk writes; inherited definitions, presets and source errors verified |
| gravity / physics / character | Done, 80 tests. Moves (`dodge` roll, `combat` swing with `combat/hit` knockback) run in `CharacterSystems::Moves` through `CharacterMove`, each with `blocked_while` conditions; physics, volumes, gravity and character register inferred extensors (**SUN**). Own input mapping; dynamic capsule controller; camera movement frames are captured as commands, headings follow changes in gravity. Only buoyant volumes cause submersion; camera zones remain dry. Grounded characters hold on walkable slopes (friction-scaled) and judge leaving the ground along its normal. Gravity fields support exit hysteresis and sample simulation poses (**MOON**); `camera_obstructions` reports blockers of a camera's view (**MOON**). `CharacterAnimationPlugin` drives `struction_anim` rigs with physics-raycast feet (**SUN**). Characters can turn toward their movement (`face_movement`) for free-orbit cameras (**SUN**) |
| `struction_camera` | Implemented, 7 tests (**SUN**). Third person (orbit, zoom, sphere-cast pull-in, fixed/follow zones) and first person (eyes, look turns the body), switched by V or zooming; follows local up. No free camera by design |
| `struction_anim` | Done, 57 tests. `MovePose` draws rolls and upper-body swings (**SUN**). Spike verdict: go on mechanics (planted feet, planets, hold/gaze/sit); rendered in the playground, looks still rough (see follow-ups). Runtime uses a fixed solve pipeline, not the dataflow graph yet. `CustomGround` lets bridges step locomotion with their own ground; feet no longer re-step at the reach limit on slopes (**SUN**) |
| `struction_ai` | Done, 23 tests. Loading `brain`/`sensing` from definitions and a physics line-of-sight are left to integration |
| `struction_world` | Verified, 26 tests. `LiveReloadPlugin`/`reload_sources` refresh live instances from saved sources, keeping runtime state (**SUN**). Spawners, save/load, aliases and boss/minion integration; **MOON** added candidate scene compilation and source provenance for authoring |
| `struction_assets` | Verified, 24 tests including real Blender 5.2.2 import/export, UVs, collision/LODs, compiled loading and hot reload; Clippy clean. `.smesh` format 2 carries material factors; Blender found on macOS; `build` runs generator scripts (**SUN**) |
| `struction_debug` | Implemented, 5 tests and native trace verified. Fixed-tick snapshots and before/after changes, stable identities, component/activation/lifecycle changes, optional JSONL sink. See `docs/debugging.md` |
| `struction_editor` / `apps/editor` | Headless foundation verified by **MOON**, 32 tests and native JSONL smoke. Shared validation, source/history, hierarchy/inspection, world-space moves, templates and isolated play. First egui GUI on it (**SUN**, 7 tests): scene tree, inspector edits, problems, undo/redo, viewport moves and play controls; see `docs/authoring.md`. Definition inspection lists extensors in use, dropped, suggested and available; `add_extensor`/`remove_extensor` operations, commands and inspector section (**SUN**) |
| `apps/playground` | Scene loaded from JSONC data (`apps/playground/project`) through `struction_world` (**SUN**). Milestone 1 scene runnable with a procedurally walking humanoid player dressed as the Blender-authored blood knight (rigid pieces per joint, hot-reloaded from the `.blend`) (**SUN**); HUD renders (`bevy_ui_render`, `default_font`) (**SUN**). Water surface replaces overlapping transparent box; planet escape/walking and sight-line fading covered by tests (camera movement and overhead orientation moved to `struction_camera`); third/first person player camera attached to the data-spawned player (**SUN**); occluding ground and the water surface get a soft shader cut-out around the player (**SUN**); planet field ends 1 m above the surface with a 0.5 m exit margin; native Vulkan smoke run verified. Characters descend from `Actor` and opt into `dodge`/`combat`; every `Shape::Humanoid` gets a rig dressed with its own model (compiled in the background) or simple shapes, so the guarding `characters/sentry` NPC wears the knight too; attack on left click/F (**SUN**) |

## Follow-ups

- Extensors: suggestions only come from requirements; there are no generator previews beyond listing supplied components. Move conditions (`blocked_while`) are a closed list in `struction_character`; a state vocabulary other packages can contribute (stunned, carrying, ...) would extend it.
- Moves: the roll, the swing and the guarding sentry still need a native look; there is no health, so `combat/hit` only knocks back unless a definition reacts to it.
- Brain arguments use tagged values (`{ "Float": 0.25 }`); switch to plain JSON values typed by the condition/action parameter metadata.
- Animation looks: foot roll, hip sway, arm swing, longer strides (~110 steps/min), knee limits, turning in place.
- Hot reload of a master's `grantsToWards` does not update existing wards.
- Playground camera: the third-person camera now pulls in when solid ground blocks it, which should stop it sinking under the approach floor on the planet's near side; confirm natively.
- Live reload: a new spawn in a spawner that already ran needs a restart; re-placed dynamic bodies are untested; run the playground loop natively. The editor cannot open the playground project until a game crate gives it the playground's types. Rust hotpatching silently keeps old code when a system's parameters change (`docs/live-reload.md`).
- World/physics integration must derive moving cells from simulation `Position` instead of interpolated `Transform`. Streaming still needs persistence of unloaded spawners that have not run yet; current saves record spawners after their first run.

## Next

1. First egui editor landed on `AuthoringProject` (**SUN**). Next in it: render instances' own meshes, rotation/scale through new backend operations, a game crate supplying the editor's factory, and a native pass on mouse picking/dragging.
2. Playground: the scene is data now; integrate AI and authored encounters through `struction_world`, then hot reload and the rest of the README validation scene.
3. Editor (milestone 4): build egui on `AuthoringProject`; extend the same headless operations for rotation/scale, source rename/delete and further authoring tools.
4. Menus as entities; package validation (planetary gravity as an external package, milestone 7); streaming (milestone 8).
