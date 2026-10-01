# Character moves: dodge and combat

Timed moves are optional character capabilities, each an [extensor](authoring-direction.md#extensors)
a definition opts into. The roll was started by **MOON**; **SUN** moved it into the `dodge`
extensor and added the `combat` swing.

| Extensor | Supplies | Action | Playground control |
| --- | --- | --- | --- |
| `dodge` | `Roll` | `dodge/roll` | Left Shift: roll in the movement direction, or forward when idle |
| `combat` | `Attack` | `combat/attack`, `combat/hit` | Left click or F: swing in the movement direction, or forward when idle |

Both are opt-in. A component they own is refused unless the definition, an ancestor, a preset or
an override names its extensor. Naming one without the component adds it with default tuning:

```jsonc
// A project character
{
  "descendsFrom": "Actor",
  "extensors": ["dodge", "combat"],
  "components": {
    "PlayerControlled": {},
    // Only what differs from the defaults.
    "Attack": { "knockback": 6 }
  }
}
```

The engine's `Actor` names neither, so its `characters/sentry` walks but cannot roll or swing;
`characters/player` names both and spells out their refusal conditions. Games add the packages
they use: `DodgePlugin` and `CombatPlugin` from `struction_character` (`struction_scene`'s
`ScenePlugin` adds every engine package).

## Tuning

`Roll { duration: 0.65, peak_speed: 10, recovery: 0.2, blocked_while: ["Airborne", "Swimming", "Attacking", "Recovering"], cancel_into: [] }`.
Duration includes tucking and getting up. Speed follows a sine curve, so the defaults cover about
4.14 m on unobstructed flat ground. Walls can shorten it. Movement ignores surface traction while
rolling. The upright capsule keeps its shape, so the tuck does not fit under lower ceilings.
There are no immunity frames or stamina costs.

`Attack { duration: 0.5, reach: 1, radius: 0.6, knockback: 4, recovery: 0.15, blocked_while: ["Swimming", "Rolling", "Recovering"], cancel_into: [] }`.
The strike lands once, when the swing passes its strike phase (`struction_anim::moves::SWING_STRIKE`,
45% of the duration). It hits every solid body overlapping a sphere of `radius` centered `reach`
ahead of the body's center, excluding the attacker and sensor volumes. The character keeps
walking while swinging and faces the strike.

## Refusing and cancelling

Two lists per move, both plain data, answer different questions.

`blocked_while` lists the `CharacterCondition`s in which the move neither starts nor continues:
`Grounded`, `Airborne` (jumping or falling), `Swimming`, `Rolling`, `Attacking`, or `Recovering`
(the `recovery` seconds after a move ends; it only refuses starting, never cuts a move short). A
move ignores its own condition. Becoming true mid-move cancels it, for example on leaving the
ground mid-roll. The defaults keep one move at a time with a short recovery between them, but
that is authored: removing `Rolling` from `Attack` lets a character swing mid-roll, and removing
`Airborne` from `Roll` lets a roll started on the ground carry on through the air as a dash.
Jumps have the same list on the controller, `CharacterController.jump_blocked_while`
(`["Rolling", "Attacking"]`), besides needing ground and not swimming.

`cancel_into` (empty by default) opens cancel windows: `[{ "action": "Jump", "after": 0.3 }]` on
a roll lets a jump end it from 0.3 s in; `[{ "action": "Roll", "after": 0.1 }]` on an attack is an
attack cancel into a roll. Actions are `Jump`, `Roll` and `Attack`. A cancelled move ends without
recovery, in the `Cancel` stage before any move starts, so the action happens in the same tick,
still subject to its own `blocked_while`. Requests outside a window are refused as usual.

`validate()` on either component reports the invalid field: durations must be finite and
positive, everything else finite and nonnegative. Invalid tuning refuses the move and logs why.
Generic JSONC loading validates types, not these ranges.

## States switch components

A definition's `states` section (last in canonical order) enables and disables components while a
state holds; leaving the state restores what it replaced:

```jsonc
"states": {
  // A rolling character pulls what is near it, and cannot be pushed around by its own Surface.
  "Rolling": { "enable": { "GravityField": { "volume": { "Sphere": { "radius": 4 } } } }, "disable": ["Surface"] },
  // Slower while swimming: an enabled component is a whole value, defaults filling what is left out.
  "Swimming": { "enable": { "CharacterController": { "move_speed": 2 } } }
}
```

State names come from the extensors that contribute them: `character` (`Grounded`, `Airborne`,
`Swimming`, `Recovering`), `dodge` (`Rolling`) and `combat` (`Attacking`). An unknown state is an
error with its file and line, and a state or enabled component of an opt-in extensor needs that
extensor named, like any of its components. Rules apply after the moves and before the
controller each tick, so a state can retune either; two rules should not change the same
component. Live reload replaces the rules and undoes the old ones first.



The controller runs in four ordered `FixedPostUpdate` stages: `Sense` (ground probe, leaving
the ground, swimming, recovery countdown), `Cancel` (cancel windows), `Moves` (extensors), then
`Control` (walking, swimming, jumping, orientation). Moves hold the character through the shared
`CharacterMove`:

- `active`, the running moves as conditions (`Rolling`, `Attacking`);
- `velocity` a move drives this tick instead of walking or swimming (the roll does; the swing
  does not);
- `facing`, the heading a move holds while the character faces its movement;
- `recovery`, the seconds of `Recovering`, set when a move ends (not when it is cancelled).

The roll runs before the swing, so with the default lists a roll and an attack pressed on the
same tick roll. Presses
latch into `CharacterIntent::roll_requested`/`attack_requested` and are consumed even when
refused; holding the key does not repeat. AI and headless tools write the same intent or invoke
the actions. Reactions to `dodge/roll` and `combat/attack` observe the request, not success.

`Rolling` and `Attacking` snapshot the tuning they started with, so live edits apply to the next
move. Losing the capability cancels a running move. Directions are transported as local gravity
turns, so both work around the planet. Animation never moves the body.

A strike invokes `combat/hit` on each struck body with the attacker as `by`. The action knocks
dynamic bodies back along the attacker's ground with a little lift; it needs `CorePlugin`. Give a
definition a reaction to `combat/hit` to make it break, take damage or flee. There is no health
yet.

## Presentation

The bridge interpolates the running move's phase into the rig's `MovePose`. A roll blends the
authored `roll` tuck pose with a full turn around the pelvis, suppressing locomotion and
constraints while it tumbles. A swing blends the `swing_raise` and `swing_strike` base poses on
the arms and torso only, so the legs keep walking. Interrupted poses fade out over 0.12 seconds.
The rig root and capsule never turn over, so neither does the camera.

`--smoke-test` rolls at three seconds and attacks at four and a half. `--trace FILE.jsonl` records
`Rolling`, `Attacking` and `CharacterMove` alongside position, velocity and grounding. Headless
tests cover input latching, timing and recovery, direction capture, jumps, `blocked_while`,
invalid tuning, actions, walls, slopes, water, planet curvature, cancellation, frame rates,
knockback and the animation of both moves.
