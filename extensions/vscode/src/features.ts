import { findNodeAtOffset, getNodePath, parseTree, Node } from 'jsonc-parser';
import { getLanguageService, JSONSchema, LanguageService, TextDocument, Position, Range, JSONDocument, ObjectASTNode, PropertyASTNode } from 'vscode-json-languageservice';
import { Snapshot, Definition } from './types';

/** The service's AST types mark properties readonly; dropping one of a document's own keys is local. */
interface OpenObject extends ObjectASTNode {
  properties: PropertyASTNode[];
}

export function sourceFile(file: string): boolean {
  return file.endsWith('/entity.jsonc') || /^(presets|scenes)\/.+\.jsonc$/.test(file);
}

export function definitionPath(file: string): string | undefined {
  return file.endsWith('/entity.jsonc') ? file.slice(0, -'/entity.jsonc'.length) : undefined;
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
  return `**${markdown(definition.path)}**${definition.library ? ` · ${markdown(definition.library)} library` : ''}\n\n` +
    `Lineage: ${[...definition.lineage, definition.path].map(markdown).join(' → ')}\n\n` +
    `Components: ${definition.components.map(c => markdown(c.split('::').at(-1)!)).join(', ') || 'none'}` +
    (extensors ? `\n\nExtensors:\n${extensors}` : '');
}

export class Features {
  private readonly entityService: LanguageService;
  private readonly sceneService: LanguageService;
  private readonly entitySchema: JSONSchema;
  private readonly sceneSchema: JSONSchema;

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
  }

  /**
   * Adds what the engine's schema cannot say about itself: what each section means, and which
   * package contributes each component. A scene's overrides are described the same way, so both
   * schemas share one vocabulary.
   */
  private describe(schema: JSONSchema): void {
    const properties = schema.properties ?? {};
    const docs: Record<string, string> = {
      descendsFrom: 'Inherits this definition, its components and named extensors. Every lineage ends in a primordial type.',
      presets: 'Named reusable layers applied before this definition’s own overrides.',
      components: 'Registered Rust components. Omitted fields inherit; null removes an inherited component.',
      reactions: 'Instantaneous action hooks: source is this, master or wards; choose after or before, then call with typed args.',
      grantsToWards: 'Capabilities granted by a master to wards matching a definition lineage.',
    };
    for (const [name, doc] of Object.entries(docs)) {
      const property = properties[name];
      if (property && typeof property !== 'boolean') property.description ??= doc;
    }
    const componentSection = properties.components;
    if (componentSection && typeof componentSection !== 'boolean') {
      for (const [name, component] of Object.entries(componentSection.properties ?? {})) {
        if (typeof component === 'boolean') continue;
        const reference = component.anyOf?.[0];
        const ref = typeof reference === 'object' ? reference.$ref : undefined;
        const target = ref?.startsWith('#/$defs/') ? this.entitySchema.$defs?.[ref.slice('#/$defs/'.length)] : undefined;
        component.description = [typeof target === 'object' ? target.description : undefined,
          this.packageOf(name), 'Omitted fields inherit; null removes the inherited component.'].filter(Boolean).join('\n\n');
      }
    }
  }

  private service(file: string): LanguageService {
    return file.startsWith('scenes/') ? this.sceneService : this.entityService;
  }

  /** The registered package that supplies or infers a component, when one owns it. */
  private packageOf(name: string): string | undefined {
    const owner = this.snapshot.extensors.find(e => e.components.some(c => c.name === name || c.type_path === name));
    return owner ? `Package: ${owner.name}. ${owner.doc}` : undefined;
  }

  /** The values the schema allows at `path`: a numeric step is an array index, so its items. */
  private optionsAt(schema: JSONSchema, path: (string | number)[]): string[] {
    let node: unknown = schema;
    for (const step of path) {
      node = this.step(node, step);
      if (node === undefined) return [];
    }
    const options = (node as { enum?: unknown } | undefined)?.enum;
    return Array.isArray(options) ? options.filter((option): option is string => typeof option === 'string') : [];
  }

  /** One step down the schema: a property name, or an array's items for an index. */
  private step(section: unknown, key: string | number): unknown {
    if (!section || typeof section !== 'object') return undefined;
    const object = section as Record<string, unknown>;
    if (typeof key === 'number') return object.items;
    const property = object.properties;
    if (property && typeof property === 'object') {
      const named = (property as Record<string, unknown>)[key];
      if (named !== undefined) return named;
    }
    // A named entry holding the array: `{"spawns": {"additionalProperties": <item>}}`.
    if (typeof object.additionalProperties === 'object') return object.additionalProperties;
    return undefined;
  }

  /** Whether a value is one the schema allows; an empty option set means the field is open. */
  private accepted(schema: JSONSchema, value: string, path: (string | number)[]): boolean {
    const options = this.optionsAt(schema, path);
    return options.length === 0 || options.includes(value);
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

  private schema(file: string): JSONSchema {
    return file.startsWith('scenes/') ? this.sceneSchema : this.entitySchema;
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
    const field = path.at(-1);
    if (!key && node.type === 'string') {
      if (field === 'descendsFrom' || field === 'definition' || (field === 'to' && path.includes('grantsToWards'))) {
        const definition = this.snapshot.definitions.find(d => d.path === node.value);
        if (definition) text += `\n\n${describeDefinition(definition)}`;
      }
      if (path.at(-2) === 'extensors') {
        const dropped = node.value.startsWith('-');
        const extensor = this.snapshot.extensors.find(e => e.name === (dropped ? node.value.slice(1) : node.value));
        if (extensor) text += `\n\n**${markdown(extensor.name)}**: ${markdown(extensor.doc)}\n\n` +
          (dropped ? 'Drops the inherited extensor and its components.' : extensor.opt_in ? 'Opt-in capability; must be named before using its components.' : 'Inferred from owned components or requirements.') +
          `\n\nRequires: ${extensor.requires.map(markdown).join(', ') || 'none'}. Supplies: ${extensor.components.filter(c => c.supplied).map(c => markdown(c.name)).join(', ') || 'none'}.`;
      }
      if ((path.includes('reactions') && ['after', 'before', 'call'].includes(String(field))) || path.at(-2) === 'actions') {
        const action = this.snapshot.actions.find(a => a.name === node.value);
        if (action) text += `\n\n**${markdown(action.name)}**: ${markdown(action.doc)}\n\n` +
          `Parameters: ${action.params.map(p => `${markdown(p.name)}: ${p.type}${p.required ? ' (required)' : ` = ${markdown(JSON.stringify(p.default))}`}`).join(', ') || 'none'}.\n\n` +
          `Requires: ${action.requires.map(markdown).join(', ') || 'none'}.`;
      }
      // A value the engine rejects still names what it should have been: the schema knows the
      // options, so say them instead of leaving the field unexplained.
      const schema = this.schema(file);
      if (text.trim() && !this.accepted(schema, node.value, path)) {
        const options = this.optionsAt(schema, path);
        if (options.length) {
          text += `\n\n${this.suggestion(node.value, options)}\n\nOptions: ${options.map(markdown).join(', ')}.`;
        }
      }
    }
    if (key && path.at(-2) === 'states') {
      const owner = this.snapshot.extensors.find(e => e.states.includes(String(field)));
      if (owner) text += `\n\n**${markdown(String(field))}** is contributed by **${markdown(owner.name)}**. ${markdown(owner.doc)}`;
    }
    const own = this.snapshot.definitions.find(d => d.path === definitionPath(file));
    if (key && path.at(-2) === 'components') {
      // The service describes a key with its value's schema, so the package and the resolved
      // authored value are read from the snapshot instead of from the composed description.
      const packaged = this.packageOf(String(field));
      if (packaged) text += `\n\n${packaged}`;
      const value = (own?.resolved.components as Record<string, unknown> | undefined)?.[String(field)];
      if (value !== undefined) text += `\n\nResolved authored value:\n\n\`\`\`json\n${JSON.stringify(value, null, 2)}\n\`\`\``;
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
      comments: 'ignore', trailingCommas: 'ignore', schemaValidation: file.startsWith('scenes/') ? 'ignore' : 'error',
    }, this.schema(file));
  }

  reference(file: string, text: string, offset: number): string | undefined {
    const token = tokenAt(text, offset);
    if (!token || token.key || token.node.type !== 'string') return;
    const field = token.path.at(-1);
    if (field === 'descendsFrom' || field === 'definition' || (field === 'to' && token.path.includes('grantsToWards'))) {
      return this.snapshot.definitions.find(d => d.path === token.node.value)?.source ?? undefined;
    }
    if (token.path.at(-2) === 'presets') return `presets/${token.node.value}.jsonc`;
    return;
  }
}
