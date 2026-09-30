# Character dodge roll

Implemented by **MOON**. In the playground, press **Left Shift** to roll in the movement
direction, or forward when idle. The binding is part of `InputMap`, alongside jump. It works
with the blood knight and the fallback humanoid shapes, in either camera view.

The player definition (`apps/playground/project/Player/entity.jsonc`) opts in with:

```json
"RollAbility": { "duration": 0.65, "peak_speed": 10.0, "recovery": 0.2 }
```

Duration includes tucking and getting up. Speed follows a sine curve, so these defaults cover
about 4.14 m on unobstructed flat ground, followed by 0.2 s before another roll can start.
Walls can shorten the distance. Movement ignores surface traction during the roll; ice still
affects ordinary movement and slope forces. The upright capsule keeps its original shape,
so the visual tuck does not let the character fit under lower ceilings. There are no immunity
frames or stamina costs.

## Simulation and authoring

`RollAbility` is reflected and supports definition edits and live reload. Its `validate()`
operation reports the invalid field: duration and peak speed must be finite and positive,
recovery finite and nonnegative. The controller rejects invalid tuning on a request and logs
the reason. Generic JSONC loading currently validates types, not these numeric constraints;
headless clients can call `validate()` before applying tuning.

Keyboard presses latch into `CharacterIntent::roll_requested`. AI and headless tools can write
the same intent. Games using `CorePlugin` can add `RollActionsPlugin`, which registers
`character/roll` with capability metadata. This instantaneous action requests a roll; reactions
to it observe the request, not successful entry or completion. Gameplay can inspect `Rolling`
and `RollRecovery`; separate success/completion actions are not supplied in this first version.

The controller consumes requests after current ground and water checks in its existing
`FixedPostUpdate` physics preparation set. It accepts only grounded, non-swimming characters
with the capability and no active roll/recovery. Rejected presses are consumed. Holding the
key does not repeat. Rolling takes priority over a simultaneous jump and locks travel direction,
while transporting it as local gravity changes. Speed is integrated over each fixed tick,
including a partial final tick. Physics resolves contacts; animation never moves the body.

`Rolling` snapshots the tuning at entry, so edits apply to the next roll. Leaving walkable
ground, entering swimming water, or losing the capability cancels the roll and starts recovery.
Ground loss preserves momentum into falling. Normal walking resumes during the cooldown.

## Presentation

The bridge reads the simulation timer and interpolates its phase into `RollPose`. The animation
solver blends the authored `roll` tuck pose with a full turn around the pelvis, suppressing
ordinary locomotion and constraint weights during the tumble. Foot contacts are refreshed
while rolling so they do not remain at the takeoff point. Interrupted poses fade out over
0.12 seconds. The rig root and physics capsule never somersault, so neither does the camera.

`--smoke-test` includes a roll at three seconds. `--trace FILE.jsonl` records the request,
active roll and recovery alongside position, velocity and grounding. Headless tests cover the
input latch, timing and recovery, direction capture, jumps, invalid tuning, actions, walls,
slopes, water, planet curvature, cancellation, frame rates and animation recovery.
