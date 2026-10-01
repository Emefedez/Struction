import assert from 'node:assert/strict';
import { test } from 'node:test';
import { JSONSchema, Position } from 'vscode-json-languageservice';
import { definitionPath, describeDefinition, engineRange, Features, sourceFile, tokenAt } from '../src/features';
import { Snapshot } from '../src/types';

const schema: JSONSchema = {
  type: 'object',
  properties: {
    $schema: { type: 'string' },
    descendsFrom: { type: 'string', enum: ['Actor', 'characters/humanoid', 'guards/ogre'] },
    presets: { type: 'array', items: { type: 'string' } },
    extensors: { type: 'array', items: { oneOf: [
      { const: 'living', description: 'Anything that can be hurt' },
      { const: '-living', description: 'Drop the inherited living extensor and its components' },
      { const: 'armor', description: 'Protection' },
      { const: '-armor', description: 'Drop the inherited armor extensor and its components' },
    ] } },
    components: { type: 'object', properties: {
      Health: { anyOf: [{ $ref: '#/$defs/Health' }, { type: 'null' }], description: 'Object of fields, or null to remove the inherited component.' },
      Armor: { anyOf: [{ $ref: '#/$defs/Armor' }, { type: 'null' }] },
    }, additionalProperties: false },
    states: { type: 'object', propertyNames: { type: 'string', enum: ['Rolling', 'Guarded'] },
      additionalProperties: { type: 'object', properties: {
        disable: { type: 'array', items: { type: 'string' },
          description: 'Component names to disable while the state holds.' },
      } } },
    reactions: { type: 'array', items: {
      type: 'object', additionalProperties: false, required: ['call'],
      properties: {
        source: { type: 'string', enum: ['this', 'master', 'wards'], default: 'this',
          description: 'Who the hook watches: this definition, its master, or its wards.' },
        after: { type: 'string', enum: ['hurt', 'heal'], enumDescriptions: ['Lose hit points', 'Regain hit points'],
          description: 'Call after this action has run on the source.' },
        before: { type: 'string', enum: ['hurt', 'heal'], enumDescriptions: ['Lose hit points', 'Regain hit points'],
          description: 'Call before this action runs on the source.' },
        call: { type: 'string', enum: ['hurt', 'heal'], enumDescriptions: ['Lose hit points', 'Regain hit points'],
          description: 'The action to call.' },
        args: { type: 'object', description: 'Arguments for `call`, by parameter name.' },
      },
      oneOf: [{ required: ['after'] }, { required: ['before'] }],
    } },
    grantsToWards: { type: 'array', items: {
      type: 'object', additionalProperties: false, required: ['to'],
      properties: {
        to: { type: 'string', enum: ['Actor', 'characters/humanoid', 'guards/ogre'],
          description: 'Wards matching this definition, which receive the grant.' },
        components: { type: 'object' },
        actions: { type: 'array', items: { type: 'string', enum: ['hurt', 'heal'],
          enumDescriptions: ['Lose hit points', 'Regain hit points'] },
          description: 'Action names the wards gain.' },
      },
    } },
  },
  additionalProperties: false,
  $defs: {
    Health: { type: 'object', description: 'Hit points.', properties: { current: { type: 'number' } }, additionalProperties: false },
    Armor: { type: 'object', description: 'Damage reduction.', properties: { rating: { type: 'number' } }, additionalProperties: false },
  },
};

const vector = (description: string) => ({
  type: 'array', items: { type: 'number' }, minItems: 3, maxItems: 3, description,
});

/** Mirrors `struction_language::scene_schema`: the grammar `struction_world::scene` reads. */
const sceneSchema: JSONSchema = {
  $schema: 'http://json-schema.org/draft-07/schema#',
  title: 'Struction scene',
  type: 'object',
  additionalProperties: false,
  description: 'Zones and the spawners placed in them.',
  properties: {
    zones: { type: 'object', description: 'Zone origins in world coordinates, keyed by path.',
      additionalProperties: { type: 'object', additionalProperties: false, properties: {
        position: vector('Position as [x, y, z]: world coordinates for a zone, the zone\'s own for a spawner.'),
        rotation: vector('Euler angles in degrees as [x, y, z], applied yaw then pitch then roll.'),
      } } },
    spawnerList: { type: 'object', description: 'Spawners keyed by name; each path is <zone>/<name>.',
      additionalProperties: { type: 'object', additionalProperties: false, required: ['zone', 'position'], properties: {
        zone: { type: 'string', description: 'Zone this spawner is placed in.' },
        position: vector('Position as [x, y, z]: world coordinates for a zone, the zone\'s own for a spawner.'),
        rotation: vector('Euler angles in degrees as [x, y, z], applied yaw then pitch then roll.'),
        spawns: { type: 'object', description: 'Spawns keyed by name.', additionalProperties: {
          type: 'object', additionalProperties: false, required: ['definition'], properties: {
            definition: { type: 'string', enum: ['Actor', 'characters/humanoid', 'guards/ogre'],
              description: 'Definition this spawn instantiates.' },
            offset: vector('Position in the spawner\'s frame, as [x, y, z].'),
            rotation: vector('Euler angles in degrees as [x, y, z], applied yaw then pitch then roll.'),
            masterIs: { type: 'string', description: 'Authored master path. This spawn is the ward; placement is separate.' },
            overrides: { $ref: '#/$defs/definition' },
          } } },
      } } },
  },
  $defs: { definition: schema },
};

const snapshot: Snapshot = {
  schema,
  scene_schema: sceneSchema,
  diagnostics: [],
  definitions: [{
    path: 'guards/ogre',
    source: '/project/guards/ogre/entity.jsonc',
    library: 'engine',
    lineage: ['Actor'],
    resolved: { components: { Health: { current: 60 } } },
    components: ['test::Health', 'test::Armor'],
    extensors: [{ name: 'armor', reason: { required_by: 'living' }, supplied: [], components: ['Armor'] }],
  }, {
    path: 'characters/humanoid',
    source: null,
    library: null,
    lineage: ['Actor'],
    resolved: {},
    components: ['test::Health'],
    extensors: [{ name: 'living', reason: { owns: 'Health' }, supplied: [], components: ['Health'] }],
  }],
  extensors: [{
    name: 'armor',
    doc: 'Protection that reduces incoming hits',
    opt_in: true,
    requires: ['living'],
    states: ['Guarded'],
    components: [{ name: 'Armor', type_path: 'test::Armor', supplied: true }],
  }, {
    name: 'living',
    doc: 'Anything that can be hurt',
    opt_in: false,
    requires: [],
    states: [],
    components: [{ name: 'Health', type_path: 'test::Health', supplied: false }],
  }],
  actions: [{
    name: 'hurt',
    doc: 'Lose hit points',
    params: [{ name: 'by', type: 'entity', required: true, default: null }, { name: 'amount', type: 'float', required: false, default: 1 }],
    requires: ['test::Health'],
  }],
};

const features = new Features(snapshot);
const ENTITY = 'guards/ogre/entity.jsonc';

/**
 * Positions are found by their surrounding text so a fixture edit cannot shift a cursor silently.
 * A quoted marker puts the cursor inside the token, which is where the editor's hover lands.
 */
function at(file: string, text: string, marker: string) {
  const document = features.document(`file:///project/${file}`, text);
  const start = text.indexOf(marker);
  assert.notEqual(start, -1, `no ${marker} in the fixture`);
  return { document, position: document.positionAt(start + (marker.startsWith('"') ? 1 : 0)) };
}

async function hover(file: string, text: string, marker: string): Promise<string> {
  const { document, position } = at(file, text, marker);
  return (await features.hover(file, document, position))?.text ?? '';
}

/** Places the cursor inside the string value that follows a key marker. */
async function hoverValue(file: string, text: string, key: string): Promise<string> {
  const document = features.document(`file:///project/${file}`, text);
  const start = text.indexOf(key);
  assert.notEqual(start, -1, `no ${key} in the fixture`);
  const position = document.positionAt(text.indexOf('"', start + key.length + 1) + 1);
  return (await features.hover(file, document, position))?.text ?? '';
}

async function labels(file: string, text: string, marker: string): Promise<string[]> {
  const { document, position } = at(file, text, marker);
  const list = await features.complete(file, document, position);
  return (list?.items ?? []).map(item => String(item.label));
}

async function problems(file: string, text: string): Promise<string[]> {
  const document = features.document(`file:///project/${file}`, text);
  return (await features.diagnostics(file, document)).map(diagnostic => String(diagnostic.message));
}

test('classifies the files the engine reads', () => {
  assert.equal(sourceFile('guards/ogre/entity.jsonc'), true);
  assert.equal(sourceFile('presets/swift.jsonc'), true);
  assert.equal(sourceFile('scenes/yard.jsonc'), true);
  assert.equal(sourceFile('scenes/yard.json'), false);
  assert.equal(sourceFile('notes.md'), false);
  assert.equal(sourceFile('guards/ogre/readme.jsonc'), false);
  assert.equal(definitionPath('guards/ogre/entity.jsonc'), 'guards/ogre');
  assert.equal(definitionPath('scenes/yard.jsonc'), undefined);
});

test('reports keys and values apart from the cursor', () => {
  const text = '{\n  "descendsFrom": "Actor"\n}\n';
  const key = tokenAt(text, text.indexOf('descendsFrom'));
  const value = tokenAt(text, text.indexOf('"Actor"') + 2);
  assert.deepEqual(key?.path, ['descendsFrom']);
  assert.equal(key?.key, true);
  assert.deepEqual(value?.path, ['descendsFrom']);
  assert.equal(value?.key, false);
});

test('hovers a definition path with its lineage', async () => {
  const inherited = '{\n  "descendsFrom": "characters/humanoid"\n}\n';
  assert.match(await hover(ENTITY, inherited, '"characters/humanoid"'), /Actor → characters\/humanoid/);
  const library = '{\n  "descendsFrom": "guards/ogre"\n}\n';
  const shown = await hover(ENTITY, library, '"guards/ogre"');
  assert.match(shown, /Actor → guards\/ogre/);
  assert.match(shown, /engine library/);
});

test('hovers an extensor with how it is taken and dropped', async () => {
  const named = '{\n  "extensors": ["armor"]\n}\n';
  const dropped = '{\n  "extensors": ["-armor"]\n}\n';
  const inferred = '{\n  "extensors": ["living"]\n}\n';
  assert.match(await hover(ENTITY, named, '"armor"'), /Opt-in capability/);
  assert.match(await hover(ENTITY, named, '"armor"'), /Requires: living\. Supplies: Armor\./);
  assert.match(await hover(ENTITY, dropped, '"-armor"'), /Drops the inherited extensor/);
  assert.match(await hover(ENTITY, inferred, '"living"'), /Inferred from owned components/);
});

test('hovers the state an extensor contributes', async () => {
  const text = '{\n  "states": {\n    "Guarded": {}\n  }\n}\n';
  assert.match(await hover(ENTITY, text, '"Guarded"'), /contributed by \*\*armor\*\*/);
});

test('hovers an action call with its parameters', async () => {
  const text = '{\n  "reactions": [\n    { "source": "wards", "after": "hurt", "call": "hurt" }\n  ]\n}\n';
  const shown = await hoverValue(ENTITY, text, '"call":');
  assert.match(shown, /Lose hit points/);
  assert.match(shown, /by: entity \(required\)/);
  assert.match(shown, /amount: float = 1/);
  assert.match(shown, /Requires: test::Health/);
});

test('hovers a component key with its package and the resolved authored value', async () => {
  const armor = '{\n  "components": {\n    "Armor": { "rating": 3 }\n  }\n}\n';
  const shown = await hover(ENTITY, armor, '"Armor"');
  assert.match(shown, /Damage reduction/);
  assert.match(shown, /Package: armor\. Protection that reduces incoming hits/);
  const health = '{\n  "components": {\n    "Health": { "current": 3 }\n  }\n}\n';
  const resolved = await hover(ENTITY, health, '"Health"');
  assert.match(resolved, /Hit points/);
  assert.match(resolved, /Package: living\. Anything that can be hurt/);
  assert.match(resolved, /"current": 60/);
});

test('completes definition paths, extensors and component names', async () => {
  const inherits = '{\n  "descendsFrom": "gu"\n}\n';
  assert.deepEqual(await labels(ENTITY, inherits, '"gu'), ['"Actor"', '"characters/humanoid"', '"guards/ogre"']);
  const extensors = '{\n  "extensors": [""]\n}\n';
  assert.deepEqual(await labels(ENTITY, extensors, '""]'), ['"living"', '"-living"', '"armor"', '"-armor"']);
  const components = '{\n  "components": { "": {}}\n}\n';
  assert.deepEqual(await labels(ENTITY, components, '""'), ['Health', 'Armor']);
});

test('reports schema violations against the registered types', async () => {
  assert.deepEqual(await problems(ENTITY, '{"descendsFrom":"Actor","components":{"Health":{"current":1}}}'), []);
  assert.deepEqual(await problems(ENTITY, '{"descendsFrom":"guards/ogre"}'), []);
  assert.match((await problems(ENTITY, '{"components":{"Nonsense":{}}}')).join('\n'), /Nonsense/);
  assert.match((await problems(ENTITY, '{"components":{"Health":{"current":"lots"}}}')).join('\n'), /number/);
  assert.match((await problems(ENTITY, '{"shapes":{}}')).join('\n'), /shapes/);
});

test('completes every field a scene accepts', async () => {
  const scene = 'scenes/yard.jsonc';
  assert.deepEqual((await labels(scene, '{\n  "": {}\n}\n', '""')).sort(), ['spawnerList', 'zones']);
  const zone = '{\n  "zones": { "Court": { "": {} } }\n}\n';
  assert.deepEqual(await labels(scene, zone, '""'), ['position', 'rotation']);
  const spawner = '{\n  "spawnerList": { "guards": { "": {} } }\n}\n';
  assert.deepEqual(await labels(scene, spawner, '""'), ['zone', 'position', 'rotation', 'spawns']);
  const spawn = '{\n  "spawnerList": { "guards": { "spawns": { "ogre": { "": {} } } } }\n}\n';
  assert.deepEqual(await labels(scene, spawn, '""'),
    ['definition', 'offset', 'rotation', 'masterIs', 'overrides']);
});

test('explains scene fields on hover', async () => {
  const scene = 'scenes/yard.jsonc';
  const position = '{\n  "zones": { "Court": { "position": [1, 2, 3] } }\n}\n';
  assert.match(await hover(scene, position, '"position"'), /world coordinates for a zone/);
  const rotation = '{\n  "spawnerList": { "guards": { "rotation": [0, 90, 0] } }\n}\n';
  assert.match(await hover(scene, rotation, '"rotation"'), /Euler angles in degrees/);
  const definition = '{\n  "spawnerList": { "guards": { "spawns": { "ogre": { "definition": "guards/ogre" } } } }\n}\n';
  assert.match(await hover(scene, definition, '"guards/ogre"'), /Definition this spawn instantiates/);
  const overrides = '{\n  "spawnerList": { "guards": { "spawns": { "ogre": { "overrides": { "descendsFrom": "Actor" } } } } }\n}\n';
  assert.match(await hover(scene, overrides, '"descendsFrom"'), /Inherits this definition/);
});

test('completes every field a reaction accepts', async () => {
  const keys = '{\n  "reactions": [{ "": {} }]\n}\n';
  assert.deepEqual((await labels(ENTITY, keys, '""')).sort(),
    ['after', 'args', 'before', 'call', 'source']);
  const source = '{\n  "reactions": [{ "source": "" }]\n}\n';
  assert.deepEqual(await labels(ENTITY, source, '""'), ['"this"', '"master"', '"wards"']);
  const call = '{\n  "reactions": [{ "call": "" }]\n}\n';
  assert.deepEqual(await labels(ENTITY, call, '""'), ['"hurt"', '"heal"']);
});

test('completes every field a grant accepts', async () => {
  const keys = '{\n  "grantsToWards": [{ "": {} }]\n}\n';
  assert.deepEqual((await labels(ENTITY, keys, '""')).sort(), ['actions', 'components', 'to']);
  const to = '{\n  "grantsToWards": [{ "to": "" }]\n}\n';
  assert.deepEqual(await labels(ENTITY, to, '""'), ['"Actor"', '"characters/humanoid"', '"guards/ogre"']);
  const actions = '{\n  "grantsToWards": [{ "actions": [""] }]\n}\n';
  assert.deepEqual(await labels(ENTITY, actions, '""'), ['"hurt"', '"heal"']);
});

test('reports what a reaction and a grant left out', async () => {
  assert.deepEqual(await problems(ENTITY, '{"reactions":[{"call":"hurt","after":"hurt"}]}'), []);
  assert.match((await problems(ENTITY, '{"reactions":[{"after":"hurt"}]}')).join('\n'), /call/);
  // Exactly one hook: naming both is refused, as `reactions_from_node` refuses it too.
  assert.notDeepEqual(
    await problems(ENTITY, '{"reactions":[{"after":"hurt","before":"hurt","call":"hurt"}]}'), []);
  assert.match((await problems(ENTITY, '{"reactions":[{"nope":1,"call":"hurt","after":"hurt"}]}')).join('\n'), /nope/);
  assert.deepEqual(await problems(ENTITY, '{"grantsToWards":[{"to":"guards/ogre"}]}'), []);
  assert.match((await problems(ENTITY, '{"grantsToWards":[{"components":{}}]}')).join('\n'), /to/);
  assert.match((await problems(ENTITY, '{"grantsToWards":[{"to":"nope","nope":1}]}')).join('\n'), /nope/);
});

test('explains a reaction field and the action it names', async () => {
  const source = '{\n  "reactions": [{ "source": "wards" }]\n}\n';
  assert.match(await hover(ENTITY, source, '"source"'), /this definition, its master, or its wards/);
  const call = '{\n  "reactions": [{ "call": "hurt" }]\n}\n';
  const shown = await hoverValue(ENTITY, call, '"call":');
  assert.match(shown, /The action to call/);
  assert.match(shown, /Lose hit points/);
  // The schema's own enum documentation names the value under the cursor.
  const other = '{\n  "reactions": [{ "call": "heal" }]\n}\n';
  assert.match(await hoverValue(ENTITY, other, '"call":'), /Regain hit points/);
  const grant = '{\n  "grantsToWards": [{ "to": "guards/ogre" }]\n}\n';
  assert.match(await hoverValue(ENTITY, grant, '"to":'), /Wards matching this definition/);
  const disable = '{\n  "states": { "Guarded": { "disable": ["Armor"] } }\n}\n';
  assert.match(await hover(ENTITY, disable, '"disable"'), /Component names to disable/);
});

test('lists the options of a field whose value the engine rejects', async () => {
  const call = '{\n  "reactions": [{ "call": "hur" }]\n}\n';
  assert.match(await hoverValue(ENTITY, call, '"call":'), /“hurt” is not a registered value/);
  assert.match(await hoverValue(ENTITY, call, '"call":'), /Options: hurt, heal\./);
  // A source the engine does not accept, likewise.
  const source = '{\n  "reactions": [{ "source": "everyone" }]\n}\n';
  const shown = await hoverValue(ENTITY, source, '"source":');
  assert.match(shown, /Options: this, master, wards/);
  // Nothing to suggest when the value is not near any option.
  const far = '{\n  "reactions": [{ "call": "zzzzzzzz" }]\n}\n';
  assert.match(await hoverValue(ENTITY, far, '"call":'), /Not a registered value\./);
  // A field without a closed set is left alone.
  const open = '{\n  "reactions": [{ "args": { "amount": 1 } }]\n}\n';
  assert.equal(await hover(ENTITY, open, '"amount"'), '');
  // An accepted value is not questioned.
  assert.doesNotMatch(await hoverValue(ENTITY, '{\n  "reactions": [{ "call": "hurt" }]\n}\n', '"call":'),
    /not a registered value/);
});

test('keeps scene files free of entity schema errors', async () => {
  assert.deepEqual(await problems('scenes/yard.jsonc', '{"spawnerList":{"guards":{"spawns":{"ogre":{"definition":"guards/ogre"}}}}}'), []);
  assert.deepEqual(await problems('scenes/yard.jsonc', '{"zones":{"Yard":{}}}'), []);
  assert.match((await problems('scenes/yard.jsonc', '{"spawnerList":{')).join('\n'), /Expected/);
});

test('offers scene spawn definitions and the override schema', async () => {
  const scene = 'scenes/yard.jsonc';
  const text = '{\n  "spawnerList": { "guards": { "spawns": { "ogre": { "definition": "guard" } } } }\n}\n';
  assert.deepEqual(await labels(scene, text, '"guard"'), ['"Actor"', '"characters/humanoid"', '"guards/ogre"']);
  const overrides = '{\n  "spawnerList": { "guards": { "spawns": { "ogre": { "overrides": { "descendsFrom": "Actor" } } } } }\n}\n';
  assert.deepEqual(await problems(scene, overrides), []);
});

test('follows a path to the definition or preset it names', () => {
  const inherits = '{\n  "descendsFrom": "guards/ogre"\n}\n';
  const grant = '{\n  "grantsToWards": [{ "to": "guards/ogre" }]\n}\n';
  const preset = '{\n  "presets": ["swift"]\n}\n';
  const component = '{\n  "components": { "Health": { "current": 1 } }\n}\n';
  assert.equal(features.reference(ENTITY, inherits, inherits.indexOf('"guards/ogre"') + 2), '/project/guards/ogre/entity.jsonc');
  assert.equal(features.reference(ENTITY, grant, grant.indexOf('"guards/ogre"') + 2), '/project/guards/ogre/entity.jsonc');
  assert.equal(features.reference(ENTITY, preset, preset.indexOf('"swift"') + 2), 'presets/swift.jsonc');
  assert.equal(features.reference(ENTITY, component, component.indexOf('Health') + 1), undefined);
  assert.equal(features.reference(ENTITY, inherits, inherits.indexOf('descendsFrom') + 1), undefined);
});

test('maps engine positions onto the token they point at', () => {
  // Columns are 1-based and count characters, while editor positions count UTF-16 units.
  const text = '{\n  "descendsFrom": "Actor"\n}\n';
  const range = engineRange(text, 2, 20);
  assert.deepEqual(range.start, { line: 1, character: 19 });
  assert.deepEqual(range.end, { line: 1, character: 25 });
  // An astral character before the column is one Rust column but two editor units.
  const astral = '{\n  "descendsFrom": "🌍guards/ogre"\n}\n';
  assert.deepEqual(engineRange(astral, 2, 24).start, { line: 1, character: 24 });
  assert.deepEqual(engineRange(astral, 2, 20).start, { line: 1, character: 19 });
  assert.deepEqual(engineRange('{}\n', 99, 1).start, { line: 1, character: 0 });
  assert.deepEqual(engineRange('{}\n', null, null).start, { line: 0, character: 0 });
});

test('reads a lineage from the primordial type down to the definition', () => {
  const chain = describeDefinition({ ...snapshot.definitions[0], path: 'characters/player',
    lineage: ['Actor', 'characters/humanoid'] });
  assert.match(chain, /Lineage: Actor → characters\/humanoid → characters\/player/);
  // A primordial definition is its own root, with nothing to chain.
  assert.match(describeDefinition({ ...snapshot.definitions[1], path: 'Actor', lineage: [] }),
    /Lineage: Actor/);
});

test('describes a definition for inspection', () => {
  const described = describeDefinition(snapshot.definitions[0]);
  assert.match(described, /\*\*guards\/ogre\*\* · engine library/);
  assert.match(described, /Lineage: Actor → guards\/ogre/);
  assert.match(described, /Components: Health, Armor/);
  assert.match(described, /Extensors:\n- \*\*armor\*\*: required by living/);
});

test('escapes authored text so markdown cannot be injected through it', () => {
  const escaped = describeDefinition({ ...snapshot.definitions[0], path: 'guards/[o]gre' });
  assert.ok(escaped.includes('guards/\\[o\\]gre'), escaped);
});

test('ignores a source-level $schema in favor of the registrations', async () => {
  const text = '{\n  "$schema": "https://example.invalid/schema.json",\n  "descendsFrom": "Nonsense"\n}\n';
  const shown = await hover(ENTITY, text, '"Nonsense"');
  assert.equal(shown.includes('https://example.invalid'), false);
  assert.match((await problems(ENTITY, text)).join('\n'), /not accepted/);
});

test('position helper reads document positions', () => {
  const document = features.document('file:///project/guards/ogre/entity.jsonc', '{\n  "a": 1\n}\n', 3);
  assert.equal(document.version, 3);
  assert.deepEqual(document.positionAt(4) as Position, { line: 1, character: 2 });
});