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
| `struction_data` | Done, 68 tests. Lineage unified with core's `Definition`; reflected grants/reactions available to world integration |
| gravity / physics / character | Done, 43 tests. Own input mapping; dynamic capsule controller; camera movement frames are captured as commands, headings follow changes in gravity. Only buoyant volumes cause submersion; camera zones remain dry |
| `struction_anim` | Done, 52 tests. Spike verdict: go on mechanics (planted feet, planets, hold/gaze/sit); visual quality unproven until rendered. Runtime uses a fixed solve pipeline, not the dataflow graph yet |
| `struction_ai` | Done, 23 tests. Loading `brain`/`sensing` from definitions and a physics line-of-sight are left to integration |
| `struction_world` | Spawners, save/load, aliases and boss/minion integration verified with 21 tests. Rejects inconsistent save identities and rename key collisions; renamed/removed spawns keep correct save bookkeeping |
| `struction_assets` | Verified, 23 tests including real Blender 5.2.2 import/export, UVs, collision/LODs, compiled loading and hot reload; Clippy clean |
| `struction_debug` | Implemented, 5 tests and native trace verified. Fixed-tick snapshots and before/after changes, stable identities, component/activation/lifecycle changes, optional JSONL sink. See `docs/debugging.md` |
| `struction_editor` / `apps/editor` | History/session source present but not exported; editor app remains a scaffold |
| `apps/playground` | Milestone 1 scene runnable. Water surface replaces overlapping transparent box; camera-relative movement and overhead orientation covered by 2 tests; native Vulkan smoke run verified |

## Follow-ups

- Brain arguments use tagged values (`{ "Float": 0.25 }`); switch to plain JSON values typed by the condition/action parameter metadata.
- Animation integration: character writes `AnimMotion` and `LocalUp`; a physics-raycast `GroundQuery`; order physics interpolation before `AnimSystems::Motion`. Looks: foot roll, hip sway, arm swing, longer strides (~110 steps/min), knee limits, turning in place.
- Hot reload of a master's `grantsToWards` does not update existing wards.
- World/physics integration must derive moving cells from simulation `Position` instead of interpolated `Transform`. Streaming still needs persistence of unloaded spawners that have not run yet; current saves record spawners after their first run.

## Next

1. Finish the shared headless editor backend and verify inspect → edit → validate → preview → undo/redo → isolated play before starting GUI implementation.
2. Playground: integrate animation, AI and authored encounters through `struction_world`, then hot reload and the rest of the README validation scene.
3. Editor (milestone 4): finish and expose the headless session/history API for both AI tools and egui; hierarchy, inspector, gizmos, undo/redo, play mode on a world copy, spawn previews, templates. Add structured command/validation entry points as the operations become available.
4. Menus as entities; package validation (planetary gravity as an external package, milestone 7); streaming (milestone 8).
