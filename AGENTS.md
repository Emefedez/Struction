# AGENTS.md

Working notes for coding agents (Claude, Codex) implementing Struction. The design lives in [README.md](README.md); rationale and the adopted resolutions of the README's open decisions are in [docs/design-review.md](docs/design-review.md) (the only Spanish document; everything else, including code, is English). Setup is in [docs/development.md](docs/development.md).

## Rules

- Pinned Bevy 0.19.1 and Rust 1.98.1. Bevy is newer than most model training data: check APIs in `~/.cargo/registry/src/*/bevy_*-0.19.1` instead of guessing.
- One crate per concern under `crates/`. Keep crates headless-testable; only applications enable rendering features. Shared dependencies go in `[workspace.dependencies]`.
- Before committing: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt --all --check`. Commit `Cargo.lock`.
- Systems use explicit system sets and ordering, never registration order. Simulation runs in `FixedUpdate` and never reads presentation state; randomness is seeded.
- Match the surrounding code: sparse comments explaining why, no speculative abstractions (README "Scope").
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

## Status

| Area | State |
| --- | --- |
| `struction_core` | Done, 53 tests |
| `struction_data` | Done, 64 tests. Defines its own `Lineage`, to be replaced by core's `Definition` in `struction_world` step 0 |
| gravity / physics / character | Written, interrupted before verification: build, test and review |
| `struction_anim` | Written, interrupted before verification: build, test, review, write the spike go/no-go report |
| `struction_ai` | Partial (behavior, condition, definition modules); sensing, boids, plugin, tests missing |
| `struction_world` | Not started |
| `struction_assets` | Not started |

## Next

1. Finish and verify the interrupted crates (physics/character, anim, ai).
2. `struction_world`: unify lineage, build grants/reactions from data, spawners, save/load, rename, boss/minion scenario test.
3. `struction_assets` (milestone 6).
4. Playground app (`apps/playground`): the validation scene from the README, with the physics, character, animation, AI, and data crates wired together, keyboard input, and hot reload.
5. Editor (milestone 4): egui hierarchy, inspector, gizmos, undo/redo over `struction_data::edit`, play mode on a world copy, spawn previews, templates.
6. Menus as entities; package validation (planetary gravity as an external package, milestone 7); streaming (milestone 8).
