# Authoring direction

Struction is intended to make small 3D projects and game-jam projects fast to author while keeping the result readable, human-editable and extensible. AI is a first-class authoring client, but it should work through the same structured concepts as a human using the editor.

The goal is not to generate opaque game code. The goal is to let a human or an AI compose a useful object from definitions, capabilities and authored placement, then inspect exactly why that object behaves as it does.

## Human first, AI second

The human is the author of the game. AI is a close authoring companion: it understands the project, prepares changes, notices inconsistencies, explains what is available and helps test ideas. It should make the human faster without taking ownership of the design.

This relationship is deliberately asymmetric:

- the human chooses the project's direction, rules and desired behavior;
- the AI may infer structure and make suggestions from registered metadata;
- changes that alter authored intent are proposed as explicit edits;
- the human can inspect, modify, accept, reject or undo every accepted change.

The AI should behave like a knowledgeable assistant, not like an invisible second runtime. Automatic discovery is useful when it reduces repetition, but it must remain deterministic and explainable.

## Definitions, instances and runtime state

These are three different things:

- A **definition** describes what an entity is. It lives in an `entity.jsonc` file and can inherit from another definition with `descendsFrom`.
- An **authored instance** describes where a definition exists in a level. It lives in a scene spawner and receives a stable authored path.
- **Runtime state** is what happens after the game starts: health, velocity, targets, relationships and other mutable state.

For example, `props/pushable_cube/entity.jsonc` defines a pushable cube. A scene can then create one instance by referring to `props/pushable_cube` from a spawner. The definition is reusable; the scene controls whether there are zero, one or many cubes and where they appear.

Keeping these layers separate makes several workflows possible:

- one definition can be reused by many scenes and spawns;
- scene placement can change without changing the entity's type;
- runtime state can be saved without rewriting authored definitions;
- live reload can update authored values while preserving unrelated runtime state;
- an AI can inspect an object as both a definition and a situated instance.

## Extensors

An entity definition should be able to declare the packages that extend its authored meaning or participate in its generation. The proposed field is `extensors`:

```jsonc
{
  "descendsFrom": "Prop",
  "extensors": [
    "physics",
    "interaction"
  ],
  "components": {
    "RigidBody": "Dynamic",
    "Shape": { "Box": { "size": [1, 1, 1] } },
    "ColliderDensity": 80,
    "Look": { "color": [0.96, 0.52, 0.20] }
  }
}
```

An extensor is not simply an import list. A package may contribute:

- component types and defaults;
- actions and conditions;
- continuous systems;
- affordances and constraints;
- generator or expansion rules;
- validation rules;
- editor inspectors and presentation metadata;
- descriptions that let AI tools explain the resulting capabilities.

The compiler should retain provenance for these contributions. An inspector or AI client should be able to answer:

```text
props/pushable_cube
  descends from: Prop
  physics contributes: RigidBody, Shape, ColliderDensity
  interaction contributes: pushable affordance
```

Package participation should be explicit when it affects generation. The registry may also infer ownership from component and action metadata, so users do not need to repeat a package name unnecessarily. Explicit `extensors` are valuable when a package contributes generator behavior, defaults or validation that is not obvious from one component.

This gives the author two levels of control:

```text
Explicit:   "extensors": ["character", "combat"]
Inferred:   physics, because CharacterController requires physics integration
Suggested:  dodge, because this actor has locomotion but no dodge capability
```

Inferred participation may explain an entity or enable package-owned metadata, but it should not silently add gameplay behavior. A package must declare whether an inference is informational, required for an existing component, or an opt-in generator action. The editor and AI should show that distinction and provide a preview before applying generated data.

There is an intended relations resolver that does not use LLM AI but rather and inferred set of rules and connections. Inherited extensors should normally be inherited just like inherited components. If an Actor itself adds combat, every descendant may receive it, which could be undesirable.

The resolved view should expose provenance rather than flattening it away:

```text
Extensors
  Explicit: character
  Inferred: physics (required by CharacterController)
  Available: combat, dodge, climbing
```

This lets the human write a small definition while still seeing the complete explanation of what the engine knows about it.

Extensors must be opt-in. Extending `Actor` should not automatically give every actor every ability provided by the character, combat or interaction packages. This keeps definitions light and makes absence meaningful.

## Capabilities are optional

An entity should only pay for the capabilities it uses. A character can be an actor without being able to attack, roll, climb, swim or interact.

For example:

```jsonc
{
  "descendsFrom": "Actor",
  "extensors": ["character"],
  "components": {
    "CharacterController": {},
    "Health": { "max": 20 }
  }
}
```

This character has movement and health, but no `Attack`, `WeaponUser` or `Roller` capability. The combat and roll packages may be available to the project without being active on this entity.

A more capable character opts in explicitly:

```jsonc
{
  "descendsFrom": "Actor",
  "extensors": ["character", "combat", "dodge"],
  "components": {
    "CharacterController": {},
    "Health": { "max": 50 },
    "WeaponUser": { "slot": "right_hand" },
    "Roller": { "distance": 4 }
  }
}
```

The presence of a capability should be enough for systems, actions, the editor and AI tools to discover what the entity can do. The absence of a capability should be equally clear: the AI should not invent an attack action for an entity that has no combat extension or required components.

## Spawners and authored placement

A spawner is the authored bridge between a definition and a level instance. It answers:

- where an instance is placed;
- when it becomes active;
- how many named instances it creates;
- which definition each instance uses;
- which per-instance overrides apply;
- which authored relationship, such as `masterIs`, it receives.

Scenes contain zones and a `spawnerList`. A simple object still uses a spawner; it merely has one named spawn:

```jsonc
{
  "zones": {
    "Playground": { "position": [0, 0, 0] }
  },
  "spawnerList": {
    "start": {
      "zone": "Playground",
      "position": [0, 0, 8],
      "spawns": {
        "player": {
          "definition": "characters/player"
        }
      }
    }
  }
}
```

The spawner itself is not the entity definition. It is placement and lifecycle data. This allows the same definition to be used by a player spawn, an enemy encounter, a test fixture or a generated wave without duplicating the entity's components.

Spawner paths are also useful authoring identities. The editor and AI can move `Playground/start/player`, inspect its definition, change its overrides and undo those changes without confusing it with another instance of the same definition.

### Runtime-created entities

Not every entity needs an authored spawner. Gameplay systems can create a runtime instance from a definition when something happens:

- an enemy is summoned;
- a projectile is fired;
- a dropped item is created;
- a procedural encounter generates a creature;
- a temporary effect becomes an entity.

These entities receive a generated stable ID rather than an authored path. They can still use the same definition, components, actions and package capabilities. The distinction is provenance and persistence, not a separate object model.

Authored spawns are appropriate for things a human wants to place, inspect and revise in a scene. Runtime creation is appropriate for things produced by simulation or gameplay. Saves can record runtime-created entities when their package marks the relevant state as persistent.

## Human-readable organization

Field order has no execution meaning. The compiler and runtime can resolve components independently of their position in a JSONC file. The authoring tool should nevertheless organize definitions consistently for humans and AI.

The preferred order is semantic rather than merely alphabetical:

1. identity and inheritance;
2. extensors;
3. static configuration;
4. physical and visual data;
5. capabilities and affordances;
6. relationships, actions and reactions;
7. continuous or ticking behavior.

The last group includes capabilities such as flammability when they participate in ongoing simulation. A range value does not by itself make a capability continuous; its update behavior and package metadata determine the group.

The editor can offer an explicit organize or format operation. It should preserve comments and intentional source edits, while compiled and inspector views may always use canonical ordering.

## AI authoring contract

AI should work through the same operations as the editor. It is a close collaborator in the authoring loop, but the project remains human-owned:

- inspect definitions, extensors, resolved components and capabilities;
- inspect authored spawns and their source locations;
- create or derive definitions;
- add or remove package extensions;
- place, duplicate or remove spawns;
- apply instance overrides;
- validate before writing;
- preview and step the game;
- explain diagnostics and capability provenance;
- undo every accepted change.

For a request such as “give this character a dodge roll”, the AI should first inspect the definition and report the smallest change: add the dodge extensor, add or suggest its authored tuning, identify the systems and input it requires, and show the resulting diff. It should not add combat, animation or unrelated components merely because those packages are available.

For a request such as “make this a simple walking character”, it should preserve the absence of `Attack`, `WeaponUser` and `Roller` rather than filling in a generic character template. Absence is authored information.

The AI should not need to edit Rust or manipulate GUI controls for ordinary authoring. Rust packages remain the extensibility boundary for genuinely new engine behavior; JSONC definitions, presets and scenes remain the human-owned layer for composing that behavior into a game.

## Design principle

The engine should make a small entity easy to understand because it contains only what it needs:

```text
definition = identity + selected extensors + authored components
instance = definition + scene placement + local overrides
runtime = instance + simulation state
```

This keeps documents small, avoids unwanted character abilities, gives packages a clear way to extend the engine, and gives both humans and AI a reliable explanation of what an entity is and why it can do something.

The desired authoring loop is:

```text
human intent
  -> inspect current definition and inferred capabilities
  -> AI explains options and prepares a minimal diff
  -> human reviews or edits the diff
  -> validate, preview, run and undo through shared APIs
```
## Current implementation

Implemented by **SUN**; the rest of this document remains direction.

- `extensors` is a definition section, between `presets` and `transform` in canonical order. Names accumulate through `descendsFrom`, presets and scene overrides. `"-combat"` drops an inherited extensor together with the components it owns inherited so far; naming it again later brings it back with its defaults. `Resolved::dropped` records who dropped what.
- Packages register extensors in `struction_core`'s `ExtensorRegistry` with `register_extensor`: the components they own, the ones they supply with defaults when named, whether they are opt-in, and the extensors they build on.
- `struction_data` refuses a component of an opt-in extensor that is not named (`Roll belongs to the opt-in extensor "dodge"; add it to "extensors"`), adds supplied components a definition leaves out, infers the other extensors from owned components and requirements, and reports unknown names. `Resolved::extensors` keeps the provenance: named by which definition, preset or override, owning which component, or required by which extensor. The generated schema offers registered names (and their `-name` forms) with their descriptions.
- Suggestions come from the same metadata, without any model: an opt-in extensor not in use whose requirements are all in use is suggested (`dodge` and `combat` for anything with a character), unless the definition dropped it. `Resolved::suggested_extensors` explains which extensors it builds on and what it would supply; nothing is added until the author accepts.
- Registered now: `physics`, `volumes` and `gravity` (inferred, `struction_physics`), `character` (inferred), and the opt-in `dodge` and `combat` (`struction_character`, see [moves](moves.md)). Inferences only explain; nothing inferred adds behavior.
- Moves carry their own refusal conditions (`blocked_while: ["Airborne", "Swimming"]`), drawn from a closed list of character states. A shared vocabulary of states that any package could contribute does not exist yet.
- `AuthoringProject::inspect_definition` (and the JSONL `inspect_definition` command) lists the extensors in use, dropped, suggested and available. `add_extensor` and `remove_extensor` (operations and JSONL commands) edit only the definition's own source as one undoable step: removing also takes out the definition's own components of that extensor, drops an inherited one with `"-name"`, and is refused while another extensor in use builds on it. The editor's definition inspector has an Extensors section calling the same operations.
- Not yet: generator previews beyond listing supplied components.
