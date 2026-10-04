import { findNodeAtOffset, getNodePath, parseTree, Node } from 'jsonc-parser';
import { getLanguageService, JSONSchema, LanguageService, TextDocument, Position, Range, JSONDocument, ObjectASTNode, PropertyASTNode } from 'vscode-json-languageservice';
import { Snapshot, Definition, Preset, Extensor, Action, SourceFile } from './types';

/** The service's AST types mark properties readonly; dropping one of a document's own keys is local. */
interface OpenObject extends ObjectASTNode {
  properties: PropertyASTNode[];
}

export function tokenAt(text: string, offset: number): { node: Node; path: (string | number)[]; key: boolean } | undefined {
  const root = parseTree(text);
  const node = root && findNodeAtOffset(root, offset, true);
  if (!node) return;
  const key = node.parent?.type === 'property' && node.parent.children?.[0] === node;
  const path = key ? [...getNodePath(node.parent!.parent!), node.value] : getNodePath(node);
  return { node, path, key };
}

// Rust source columns count Unicode scalar values; VS Code positions count UTF-16 units.
export function engineRange(text: string, line: number | null, column: number | null): Range {
  const lines = text.split('\n');
  const row = Math.min(Math.max((line ?? 1) - 1, 0), lines.length - 1);
  const character = [...lines[row]].slice(0, Math.max((column ?? 1) - 1, 0)).join('').length;
  const start = { line: row, character };
  const document = TextDocument.create('file:///diagnostic.jsonc', 'jsonc', 0, text);
  const at = document.offsetAt(start);
  const node = tokenAt(text, at)?.node;
  const end = node ? document.positionAt(node.offset + node.length) : { line: row, character: Math.min(character + 1, lines[row].length) };
  return { start, end };
}

function markdown(text: string): string {
  return text.replace(/[\\`*_{}\[\]()<>#!|]/g, '\\$&');
}

/** Case-insensitive edit distance, used only to suggest the nearest registered value. */
function distance(a: string, b: string): number {
  const left = [...a.toLowerCase()];
  const right = [...b.toLowerCase()];
  // Row i is a prefix of `left`, column j a prefix of `right`, so cell (i, j) compares
  // `left[i - 1]` with `right[j - 1]`.
  let row: number[] = Array.from({ length: right.length + 1 }, (_, j) => j);
  for (let i = 1; i <= left.length; i++) {
    // Column 0 of the previous row is read before it becomes this row's.
    let diagonal = row[0];
    row[0] = i;
    for (let j = 1; j <= right.length; j++) {
      const above = row[j];
      row[j] = Math.min(above + 1, row[j - 1] + 1,
        diagonal + (left[i - 1] === right[j - 1] ? 0 : 1));
      diagonal = above;
    }
  }
  return row[right.length];
}

export function describeDefinition(definition: Definition): string {
  const extensors = definition.extensors.map(e => {
    const why = Object.entries(e.reason).map(([kind, value]) => `${kind.replaceAll('_', ' ')} ${value}`).join(', ');
    return `- **${markdown(e.name)}**: ${markdown(why)}${e.supplied.length ? `; supplies ${e.supplied.map(markdown).join(', ')}` : ''}`;
  }).join('\n');
  // The engine's lineage starts at the primordial type, so the definition itself goes last.
  const prose = definition.doc ? `${definition.doc}\n\n` : '';
  return `${prose}**${markdown(definition.path)}**${definition.library ? ` · ${markdown(definition.library)} library` : ''}\n\n` +
    `Lineage: ${[...definition.lineage, definition.path].map(markdown).join(' → ')}\n\n` +
    `Components: ${definition.components.map(c => markdown(c.split('::').at(-1)!)).join(', ') || 'none'}` +
    (extensors ? `\n\nExtensors:\n${extensors}` : '');
}

/** What a token's value names, so a hover says what the engine calls it rather than guessing. */
type Name =
  | { kind: 'definition'; definition: Definition }
  | { kind: 'preset'; preset: Preset }
  | { kind: 'extensor'; extensor: Extensor; dropped: boolean }
  | { kind: 'state'; state: { name: string; doc: string }; extensor: Extensor }
  | { kind: 'action'; action: Action }
  | { kind: 'component'; component: string; extensor: Extensor };

/** Every vocabulary the engine's registrations put a name in, each with what to show for it. */
export class Features {
  private readonly entityService: LanguageService;
  private readonly sceneService: LanguageService;
  private readonly entitySchema: JSONSchema;
  private readonly sceneSchema: JSONSchema;
  private readonly files = new Map<string, SourceFile>();
  private readonly definitions = new Map<string, Definition>();
  private readonly presets = new Map<string, Preset>();
  private readonly extensors = new Map<string, Extensor>();
  private readonly states = new Map<string, { state: { name: string; doc: string }; extensor: Extensor }>();
  private readonly actions = new Map<string, Action>();
  private readonly components = new Map<string, Extensor>();

  constructor(readonly snapshot: Snapshot) {
    this.entitySchema = structuredClone(snapshot.schema);
    // Scenes get the host's own schema, which places the definition schema at override sites and
    // lists the definitions a spawn may name; scene validity stays the backend's to decide.
    this.sceneSchema = structuredClone(snapshot.scene_schema);
    const overrides = this.sceneSchema.$defs?.definition;
    if (overrides && typeof overrides === 'object') this.describe(overrides);
    this.describe(this.entitySchema);
    const service = (schema: JSONSchema) => {
      const language = getLanguageService({});
      language.configure({ schemas: [{ uri: 'struction://schema/current', fileMatch: ['*'], schema }] });
      return language;
    };
    this.entityService = service(this.entitySchema);
    this.sceneService = service(this.sceneSchema);
    for (const file of snapshot.files) this.files.set(file.path, file);
    for (const definition of snapshot.definitions) this.definitions.set(definition.path, definition);
    for (const preset of snapshot.presets) this.presets.set(preset.name, preset);
    for (const action of snapshot.actions) this.actions.set(action.name, action);
    for (const extensor of snapshot.extensors) {
      this.extensors.set(extensor.name, extensor);
      for (const state of extensor.states) this.states.set(state.name, { state, extensor });
      // Definitions write either name, and every owned component has exactly one package.
      for (const component of extensor.components) {
        this.components.set(component.name, extensor);
        this.components.set(component.type_path, extensor);
      }
    }
  }

  /**
   * Adds what the schema cannot say about itself: which package owns each component. Everything
   * else — what each section means, what each name stands for — is read from the schema's own
   * descriptions and the registrations, so a client never repeats the engine's vocabulary. A
   * scene's overrides are described the same way, so both schemas share one vocabulary.
   */
  private describe(schema: JSONSchema): void {
    const properties = schema.properties ?? {};
    const componentSection = properties.components;
    if (componentSection && typeof componentSection !== 'boolean') {
      for (const [name, component] of Object.entries(componentSection.properties ?? {})) {
        if (typeof component === 'boolean') continue;
        const reference = component.anyOf?.[0];
        const ref = typeof reference === 'object' ? reference.$ref : undefined;
        const target = ref?.startsWith('#/$defs/') ? this.entitySchema.$defs?.[ref.slice('#/$defs/'.length)] : undefined;
        component.description = [typeof target === 'object' ? target.description : undefined,
          this.packageOf(name)].filter(Boolean).join('\n\n');
      }
    }
  }

  /** Whether the engine reads this file, and what it calls it. */
  file(file: string): SourceFile | undefined {
    return this.files.get(file);
  }

  /** The definition the file being edited holds, for its resolved values. */
  private own(file: string): Definition | undefined {
    const name = this.files.get(file)?.name;
    return name ? this.definitions.get(name) : undefined;
  }

  private service(file: string): LanguageService {
    return this.files.get(file)?.kind === 'scene' ? this.sceneService : this.entityService;
  }

  private schema(file: string): JSONSchema {
    return this.files.get(file)?.kind === 'scene' ? this.sceneSchema : this.entitySchema;
  }

  /** The registered package that supplies or infers a component, when one owns it. */
  private packageOf(name: string): string | undefined {
    const owner = this.components.get(name);
    return owner ? `Package: ${owner.name}. ${owner.doc}` : undefined;
  }

  /**
   * The values the schema allows at `path`: a numeric step is an index into an array's items and
   * a name is one of an object's properties, so a key is looked for in the object that holds it
   * and a value in its own schema. An empty list means the field is open.
   */
  private allowed(file: string, path: (string | number)[], key: boolean): string[] {
    const nodes = this.nodesAt(file, key ? path.slice(0, -1) : path);
    const found: string[] = [];
    for (const node of nodes) found.push(...(key ? this.keyNames(node) : this.valueNames(node)));
    return [...new Set(found)];
  }

  /** Every schema the path may land in. */
  private nodesAt(file: string, path: (string | number)[]): unknown[] {
    let nodes: unknown[] = [this.schema(file)];
    for (const step of path) {
      nodes = nodes.flatMap(node =>
        this.alternativeSteps(node, step).flatMap(found => this.expand(found)));
      if (!nodes.length) return [];
    }
    return nodes;
  }

  /** One step down a schema: a property, an array's items, or the entry holding a name. */
  private alternativeSteps(node: unknown, key: string | number): unknown[] {
    if (!node || typeof node !== 'object') return [];
    const object = node as Record<string, unknown>;
    if (typeof key === 'number') {
      return typeof object.items === 'object' ? [object.items] : [];
    }
    const properties = object.properties;
    if (properties && typeof properties === 'object') {
      const named = (properties as Record<string, unknown>)[key];
      if (named !== undefined) return [named];
    }
    // A named entry holding the array: `{"spawns": {"additionalProperties": <item>}}`.
    if (typeof object.additionalProperties === 'object') return [object.additionalProperties];
    return [];
  }

  /**
   * What a schema points at rather than describes: the target of a `$ref`, or the branches of
   * `anyOf`/`oneOf` when the schema names no fields of its own. One that does name fields keeps
   * them, since its alternatives only add requirements to the same object.
   */
  private expand(node: unknown): unknown[] {
    if (!node || typeof node !== 'object') return [];
    const object = node as Record<string, unknown>;
    if (typeof object.$ref === 'string') {
      const reference = object.$ref;
      if (!reference.startsWith('#/$defs/')) return [];
      const target = this.entitySchema.$defs?.[reference.slice('#/$defs/'.length)];
      return target === undefined ? [] : this.expand(target);
    }
    const namesFields = ['properties', 'items', 'additionalProperties', 'propertyNames']
      .some(name => object[name] && typeof object[name] === 'object');
    if (namesFields) return [object];
    const branches = ['anyOf', 'oneOf'].map(name => object[name]).find(Array.isArray);
    return Array.isArray(branches) ? branches.flatMap(branch => this.expand(branch)) : [object];
  }

  /** The names a key may hold: the ones `propertyNames` allows and its siblings' own. */
  private keyNames(node: unknown): string[] {
    if (!node || typeof node !== 'object') return [];
    const object = node as Record<string, unknown>;
    const names: string[] = [];
    const propertyNames = object.propertyNames;
    if (propertyNames && typeof propertyNames === 'object') {
      names.push(...this.valueNames(propertyNames));
    }
    const properties = object.properties;
    if (properties && typeof properties === 'object') names.push(...Object.keys(properties));
    return names;
  }

  /** The values a value schema allows: its `enum`, or the `const` of each alternative. */
  private valueNames(node: unknown): string[] {
    if (!node || typeof node !== 'object') return [];
    const object = node as Record<string, unknown>;
    const names: string[] = [];
    if (Array.isArray(object.enum)) {
      names.push(...object.enum.filter((value): value is string => typeof value === 'string'));
    }
    if (typeof object.const === 'string') names.push(object.const);
    return names;
  }

  /**
   * What `value` names, in the order the engine's vocabularies are looked up. A position whose
   * schema closes the set says which vocabulary it expects, so a name outside it stays unknown
   * instead of matching whatever else happens to share it.
   */
  private name(value: string, allowed: string[]): Name | undefined {
    const takes = allowed.length === 0 || allowed.includes(value);
    const definition = takes ? this.definitions.get(value) : undefined;
    if (definition) return { kind: 'definition', definition };
    const dropped = value.startsWith('-') ? this.extensors.get(value.slice(1)) : undefined;
    const extensor = takes ? dropped ?? this.extensors.get(value) : undefined;
    if (extensor) return { kind: 'extensor', extensor, dropped: dropped !== undefined };
    const action = takes ? this.actions.get(value) : undefined;
    if (action) return { kind: 'action', action };
    const state = this.states.get(value);
    if (state && takes) return { kind: 'state', ...state };
    const owner = this.components.get(value);
    if (owner && takes) return { kind: 'component', component: value, extensor: owner };
    const preset = this.presets.get(value);
    if (preset && takes) return { kind: 'preset', preset };
    return undefined;
  }

  /** What a token names at its own position, or `undefined` when it names nothing registered. */
  private nameAt(file: string, value: string, path: (string | number)[], key: boolean): Name | undefined {
    return this.name(value, this.allowed(file, path, key));
  }

  /** The nearest registered value, so a typo reads as the one it was meant to be. */
  private suggestion(value: string, options: string[]): string {
    const nearest = options
      .map(option => ({ option, distance: distance(value, option) }))
      .sort((a, b) => a.distance - b.distance)[0];
    const { option, distance: cost } = nearest;
    return cost <= Math.max(2, Math.floor(option.length / 3))
      ? `“${option}” is not a registered value.`
      : 'Not a registered value.';
  }

  /** What the engine says about a name, beyond what the schema's own description already says. */
  private explain(name: Name): string {
    switch (name.kind) {
      case 'definition':
        return describeDefinition(name.definition);
      case 'preset':
        return `**${markdown(name.preset.name)}** · reusable layer${name.preset.library ? ` from the ${markdown(name.preset.library)} library` : ''}`;
      case 'extensor':
        return `**${markdown(name.extensor.name)}**: ${markdown(name.extensor.doc)}\n\n` +
          (name.dropped ? 'Drops the inherited extensor and its components.'
            : name.extensor.opt_in ? 'Opt-in capability; must be named before using its components.'
              : 'Inferred from owned components or requirements.') +
          `\n\nRequires: ${name.extensor.requires.map(markdown).join(', ') || 'none'}.` +
          ` Supplies: ${name.extensor.components.filter(c => c.supplied).map(c => markdown(c.name)).join(', ') || 'none'}.`;
      case 'state':
        return `**${markdown(name.state.name)}** is contributed by **${markdown(name.extensor.name)}**` +
          (name.state.doc ? `. ${markdown(name.state.doc)}` : '.');
      case 'action':
        return `**${markdown(name.action.name)}**: ${markdown(name.action.doc)}\n\n` +
          `Parameters: ${name.action.params.map(p => `${markdown(p.name)}: ${p.type}${p.required ? ' (required)' : ` = ${markdown(JSON.stringify(p.default))}`}`).join(', ') || 'none'}.\n\n` +
          `Requires: ${name.action.requires.map(markdown).join(', ') || 'none'}.`;
      case 'component':
        return `Package: ${markdown(name.extensor.name)}. ${markdown(name.extensor.doc)}`;
    }
  }

  private parse(file: string, document: TextDocument): JSONDocument {
    const json = this.service(file).parseJSONDocument(document);
    // Runtime registrations take precedence over stale or remote $schema links in sources.
    const root = json.root;
    if (root?.type === 'object') {
      const properties = root.properties.filter(p => p.keyNode.value !== '$schema');
      if (properties.length !== root.properties.length) {
        json.root = { ...root, properties } as OpenObject;
      }
    }
    return json;
  }

  document(uri: string, text: string, version = 0): TextDocument {
    return TextDocument.create(uri, 'jsonc', version, text);
  }

  async hover(file: string, document: TextDocument, position: Position): Promise<{ text: string; range?: Range } | undefined> {
    const json = this.parse(file, document);
    const schemaHover = await this.service(file).doHover(document, position, json);
    const contents = schemaHover?.contents;
    let text = typeof contents === 'string' ? contents : Array.isArray(contents)
      ? contents.map(c => typeof c === 'string' ? c : c.value).join('\n\n') : contents?.value ?? '';
    const token = tokenAt(document.getText(), document.offsetAt(position));
    if (!token) return text ? { text, range: schemaHover?.range } : undefined;
    const { node, path, key } = token;
    if (!key && node.type === 'string') {
      const named = this.nameAt(file, node.value, path, false);
      if (named) text += `\n\n${this.explain(named)}`;
      // A value the engine rejects still names what it should have been: the schema knows the
      // options, so say them instead of leaving the field unexplained.
      const options = this.allowed(file, path, false);
      if (text.trim() && options.length && !options.includes(node.value)) {
        text += `\n\n${this.suggestion(node.value, options)}\n\nOptions: ${options.map(markdown).join(', ')}.`;
      }
    }
    if (key && node.type === 'string') {
      const named = this.nameAt(file, node.value, path, true);
      if (named) text += `\n\n${this.explain(named)}`;
      // The authored value the engine resolved for this component, which the editor shows too.
      const resolved = (this.own(file)?.resolved.components as Record<string, unknown> | undefined)?.[node.value];
      if (resolved !== undefined) {
        text += `\n\nResolved authored value:\n\n\`\`\`json\n${JSON.stringify(resolved, null, 2)}\n\`\`\``;
      }
    }
    return text.trim() ? { text: text.trim(), range: {
      start: document.positionAt(node.offset), end: document.positionAt(node.offset + node.length),
    } } : undefined;
  }

  async complete(file: string, document: TextDocument, position: Position) {
    const json = this.parse(file, document);
    return this.service(file).doComplete(document, position, json);
  }

  async diagnostics(file: string, document: TextDocument) {
    const json = this.parse(file, document);
    return this.service(file).doValidation(document, json, {
      comments: 'ignore', trailingCommas: 'ignore',
      // A scene's grammar is the backend's to validate; a definition's is the schema's.
      schemaValidation: this.files.get(file)?.kind === 'scene' ? 'ignore' : 'error',
    }, this.schema(file));
  }

  /** The file a name is authored in, when it names one: a definition or a preset. */
  reference(file: string, text: string, offset: number): string | undefined {
    const token = tokenAt(text, offset);
    if (!token || token.key || token.node.type !== 'string') return;
    const named = this.nameAt(file, token.node.value, token.path, false);
    if (named?.kind === 'definition') return named.definition.source ?? undefined;
    if (named?.kind === 'preset') return named.preset.source ?? undefined;
    return;
  }
}