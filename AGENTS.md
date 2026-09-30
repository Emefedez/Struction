# AGENTS.md

Working notes for coding agents (Claude, Codex) implementing Struction. The design lives in [README.md](README.md); rationale and the adopted resolutions of the README's open decisions are in [docs/design-review.md](docs/design-review.md) (the only Spanish document; everything else, including code, is English). Setup is in [docs/development.md](docs/development.md).

## Rules

- Pinned Bevy 0.19.1 and Rust 1.98.1. Bevy is newer than most model training data: check APIs in `~/.cargo/registry/src/*/bevy_*-0.19.1` instead of guessing.
- One crate per concern under `crates/`. Keep crates headless-testable; only applications enable rendering features. Shared dependencies go in `[workspace.dependencies]`.
- Before committing: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all --check`. Commit `Cargo.lock`.
- Systems use explicit system sets and ordering, never registration order. Simulation runs in `FixedUpdate` and never reads presentation state; randomness is seeded.
- Match the surrounding code: sparse comments explaining why, no speculative abstractions (README "Scope").
- AI tools are first-class authoring clients. Expose inspection, validation and edits through headless APIs over structured data; editor UI must call the same operations. Preserve comments, stable identities, actionable source errors and undoable changes. Do not make game authoring depend on GUI automation.
- Commit often with focused messages, staging only the files you touched (several agents share one checkout). Update the status table below when a milestone step lands.
- Before editing, read the Work ownership table and check `git status`. Claim the area under your agent name, including its scope; do not overwrite another active owner's work. Coordinate overlapping changes first. Update the entry when work lands or is handed off, and record who implemented or verified each status update. This agent is **MOON**.

## Work ownership

Check this table before proceeding with changes. An active claim covers only its stated scope; unlisted areas are unclaimed, and historical ownership does not block later work.

| Area | Owner | State / scope |
| --- | --- | --- |
| Headless editor / AI authoring | **MOON** | Landed: validated project edits, exact-source undo, inspection, spawn movement, isolated play and JSONL commands. 32 tests, Clippy and native protocol smoke verified; see `docs/authoring.md` |
| Data/world authoring hooks | **MOON** | Landed: read-only candidate-source validation, scene-source provenance and integration tests; 69 data / 22 world tests and Clippy verified |
| Asset pipeline verification | **MOON** | Landed: 23 tests, including actual Blender import/export, and Clippy verified; implementation predates this verification |
| Playground/debug fixes | **MOON** | Landed: structured tracing, water rendering, camera movement and dry camera-zone fixes from the previous work session |
| Planet gravity and camera obstruction | **SUN** (from **MOON**) | Active, handed to SUN while MOON is away; MOON's uncommitted work is being verified and finished as-is: gravity entry/exit hysteresis and escape tests; separate playground occlusion module for transparent blocking ground. SUN retains rig/animation work in playground; only minimal camera module wiring and planet tuning will overlap |
| Character animation integration | **SUN** | Landed (implemented and verified by **SUN**): `CharacterAnimationPlugin` bridge, `CustomGround` hook, slope fixes in the controller and locomotion, animated player and working HUD in `apps/playground`; native Vulkan smoke run verified |

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
| `struction_core` | 3 | Identity (`DefinitionPath`, `Definition`, `StableId`), `MasterIs`/`Wards`, grants, action registry, queued reactions, notify/order |
| `struction_data` | 2 | JSONC definitions through `Reflect`, `descendsFrom`, presets, overrides, `file:line` errors, JSON Schema, comment-preserving edits, hot reload |
| `struction_gravity` | 1 | Gravity fields summed per body, local up |
| `struction_physics` | 1 | Avian 0.7 wrapper, fixed timestep and interpolation, surfaces, volumes (water, buoyancy), camera zones |
| `struction_character` | 1 | Input actions and mapping, character controller with variable up |
| `struction_anim` | 0, 5 | Skeleton, poses, springs, IK, constraints, dataflow graph, procedural locomotion, affordances |
| `struction_ai` | 3 | Behavior trees, condition registry, sensing by lineage, boids |
| `struction_world` | 3 | Spawners, zones, save/load, path renaming, data↔core integration |
| `struction_assets` | 6 | Blender/glTF import, UVs, collision, LODs, rkyv binary format, Open in… |
| `struction_debug` | Cross-cutting | Opt-in headless component-change tracing, bounded event log and JSONL output for tools |

## Status

| Area | State |
| --- | --- |
| `struction_core` | Done, 53 tests |
| `struction_data` | Done, 69 tests. **MOON** added candidate-source validation without disk writes; inherited definitions, presets and source errors verified |
| gravity / physics / character | Done, 49 tests. Own input mapping; dynamic capsule controller; camera movement frames are captured as commands, headings follow changes in gravity. Only buoyant volumes cause submersion; camera zones remain dry. Grounded characters hold on walkable slopes (friction-scaled). `CharacterAnimationPlugin` drives `struction_anim` rigs with physics-raycast feet (**SUN**) |
| `struction_anim` | Done, 54 tests. Spike verdict: go on mechanics (planted feet, planets, hold/gaze/sit); rendered in the playground, looks still rough (see follow-ups). Runtime uses a fixed solve pipeline, not the dataflow graph yet. `CustomGround` lets bridges step locomotion with their own ground; feet no longer re-step at the reach limit on slopes (**SUN**) |
| `struction_ai` | Done, 23 tests. Loading `brain`/`sensing` from definitions and a physics line-of-sight are left to integration |
| `struction_world` | Verified, 22 tests. Spawners, save/load, aliases and boss/minion integration; **MOON** added candidate scene compilation and source provenance for authoring |
| `struction_assets` | Verified, 23 tests including real Blender 5.2.2 import/export, UVs, collision/LODs, compiled loading and hot reload; Clippy clean |
| `struction_debug` | Implemented, 5 tests and native trace verified. Fixed-tick snapshots and before/after changes, stable identities, component/activation/lifecycle changes, optional JSONL sink. See `docs/debugging.md` |
| `struction_editor` / `apps/editor` | Headless foundation verified by **MOON**, 32 tests and native JSONL smoke. Shared validation, source/history, hierarchy/inspection, world-space moves, templates and isolated play. GUI app remains a scaffold |
| `apps/playground` | Milestone 1 scene runnable with a procedurally walking humanoid player; HUD renders (`bevy_ui_render`, `default_font`) (**SUN**). Water surface replaces overlapping transparent box; camera-relative movement and overhead orientation covered by 2 tests; native Vulkan smoke run verified |

## Follow-ups

- Brain arguments use tagged values (`{ "Float": 0.25 }`); switch to plain JSON values typed by the condition/action parameter metadata.
- Animation looks: foot roll, hip sway, arm swing, longer strides (~110 steps/min), knee limits, turning in place.
- Hot reload of a master's `grantsToWards` does not update existing wards.
- World/physics integration must derive moving cells from simulation `Position` instead of interpolated `Transform`. Streaming still needs persistence of unloaded spawners that have not run yet; current saves record spawners after their first run.

## Next

1. Shared headless editor gate passed (**MOON**); resolve reported planet gravity/camera issues before beginning the first egui hierarchy, inspector, move gizmo and play controls. See `docs/authoring.md`.
2. Playground: integrate animation, AI and authored encounters through `struction_world`, then hot reload and the rest of the README validation scene.
3. Editor (milestone 4): build egui on `AuthoringProject`; extend the same headless operations for rotation/scale, source rename/delete and further authoring tools.
4. Menus as entities; package validation (planetary gravity as an external package, milestone 7); streaming (milestone 8).
