# Structured simulation traces

Tracing is opt-in and produces JSON Lines for AI tools or scripts. Run the playground with a **new** output filename:

```bash
cargo run --locked -p struction-playground -- --trace /tmp/struction-trace.jsonl
# Or run the ten-second scripted scene:
cargo run --locked -p struction-playground -- --smoke-test --trace /tmp/struction-smoke.jsonl
```

Existing files are not overwritten. Normal renderer/application logs stay separate. Without `--trace`, the trace plugin and file writer are not installed.

The playground tracks the player, cubes, gravity planet, water volume, floors and camera zone. Ground and zone references can be matched to those entities' names in the trace. It captures physics `Position` and `LinearVelocity`, `CharacterState` (ground contact and swimming), `CharacterLook`, `CharacterIntent`, `LocalUp`, `Submersion`, and `InCameraZones`. It reads simulation positions rather than interpolated render transforms. Registration is explicit so a game can choose the components relevant to its investigation.

Each line contains a format `version`, fixed `tick` number, `identity`, and `kind`:

| Kind | Meaning |
| --- | --- |
| `spawned` | First observation of a tracked entity, with its initial component values and active state |
| `changed` | Changed components with `before` and `after` values; an optional `active` flag records unloading/reloading |
| `removed` | A previously tracked entity was despawned |
| `untracked` | The entity still exists but is no longer selected for tracing |
| `error` | A selected component could not be serialized; includes its type and the error |

Component map keys are full Rust type paths. A missing component is represented by `null` in a change's `before` or `after` field. Identity includes Bevy entity bits with generation, an optional stable UUID, name, and definition path. Use stable UUIDs across saves; entity bits identify entities only within one run. Named world spawns also carry their authored path in `Name`.

Capture happens in `FixedLast`, after the fixed simulation. Unchanged entities emit no lines. These are end-of-tick observations: intermediate changes that cancel within a tick are not an action history. Starting or re-enabling tracing produces a fresh baseline, not a claim that all observed entities were just created.

Positions and contact normals can change every tick. To extract just grounded/swimming transitions with `jq`:

```bash
jq -c 'select(.kind == "changed") | . as $event | .components | to_entries[] | select(.key | endswith("::CharacterState")) | select(.value.before.grounded != .value.after.grounded or .value.before.swimming != .value.after.swimming) | {tick: $event.tick, entity: $event.identity, change: .value}' /tmp/struction-trace.jsonl
```

## Engine API

`struction_debug` is headless and does not depend on physics or rendering. Install `DebugTracePlugin`, then call `TraceAppExt::trace_component::<C>()` for reflected component types. Entities with core `StableId` are included automatically; add `TraceEntity` for other entities. This works for custom health, AI, inventory and encounter components as well as position.

A tool can drain `TraceLog` directly without any file. It retains the last 1,024 events by default; `TraceSettings.capacity` changes the bound and `TraceSettings.enabled` toggles capture. A `TraceWriter` accepts a `Write + Send + Sync` sink, such as a buffered file. File output receives every event even when the memory log is full. A write error is logged and exposed through `TraceWriter::error()`; the sink is disabled while in-memory capture continues.

Changes to simulation in `FixedLast` must run before `TraceSystems`. Games otherwise using the existing fixed simulation phases need no additional ordering.

```rust,ignore
use struction_debug::{DebugTracePlugin, TraceAppExt, TraceEntity, TraceLog};

app.add_plugins(DebugTracePlugin)
    .trace_component::<Health>()
    .trace_component::<Position>();
app.world_mut().entity_mut(player).insert(TraceEntity);

// After stepping the simulation, expose the events to a tool or test.
let events: Vec<_> = app.world_mut().resource_mut::<TraceLog>().drain().collect();
```

Regression checks: `cargo test -p struction_debug --locked`.
