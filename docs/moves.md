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
// characters/player
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

`Actor` names neither, so the sentry NPC descending from it walks but cannot roll or swing.
Games add the packages they use: `DodgePlugin` and `CombatPlugin` from `struction_character`
(the playground's `ScenePlugin` does this).

## Tuning

`Roll { duration: 0.65, peak_speed: 10, recovery: 0.2, blocked_while: ["Airborne", "Swimming"] }`.
Duration includes tucking and getting up. Speed follows a sine curve, so the defaults cover about
4.14 m on unobstructed flat ground. Walls can shorten it. Movement ignores surface traction while
rolling. The upright capsule keeps its shape, so the tuck does not fit under lower ceilings.
There are no immunity frames or stamina costs.

`Attack { duration: 0.5, reach: 1, radius: 0.6, knockback: 4, recovery: 0.15, blocked_while: ["Swimming"] }`.
The strike lands once, when the swing passes its strike phase (`struction_anim::moves::SWING_STRIKE`,
45% of the duration). It hits every solid body overlapping a sphere of `radius` centered `reach`
ahead of the body's center, excluding the attacker and sensor volumes. The character keeps
walking while swinging and faces the strike.

`blocked_while` lists the `CharacterCondition`s in which the move neither starts nor continues:
`Grounded`, `Airborne` (jumping or falling) or `Swimming`. Becoming true mid-move cancels it,
for example on leaving the ground mid-roll. Removing `Airborne` from a roll lets a roll started
on the ground carry on through the air as a dash.

`validate()` on either component reports the invalid field: durations must be finite and
positive, everything else finite and nonnegative. Invalid tuning refuses the move and logs why.
Generic JSONC loading validates types, not these ranges.

## Simulation

The controller runs in three ordered `FixedPostUpdate` stages: `Sense` (ground probe, leaving
the ground, swimming, recovery countdown), `Moves` (extensors), then `Control` (walking,
swimming, jumping, orientation). Moves hold the character through the shared `CharacterMove`:

- `busy` while a move runs; only one runs at a time, and the controller refuses jumps;
- `velocity` the move drives this tick instead of walking or swimming (the roll does; the
  swing does not);
- `facing`, the heading the move holds while the character faces its movement;
- `recovery`, the seconds before the next move may start, set when a move ends or is cancelled.

The roll runs before the swing, so a roll and an attack pressed on the same tick roll. Presses
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
