export interface EngineDiagnostic {
  message: string;
  file: string | null;
  line: number | null;
  column: number | null;
}

/** What the engine reads a project-relative file as, reported for every buffer it was sent. */
export interface SourceFile {
  path: string;
  kind: 'definition' | 'preset' | 'scene';
  /** The definition's path or the preset's name; a scene places definitions and names none. */
  name: string | null;
}

export interface Definition {
  path: string;
  source: string | null;
  library: string | null;
  /** The prose the definition's own file starts with. */
  doc: string | null;
  lineage: string[];
  resolved: Record<string, unknown>;
  components: string[];
  extensors: { name: string; reason: Record<string, string>; supplied: string[]; components: string[] }[];
}

export interface Preset {
  name: string;
  source: string | null;
  library: string | null;
}

export interface Extensor {
  name: string;
  doc: string;
  opt_in: boolean;
  requires: string[];
  /** States the package contributes, each with what holds while it does. */
  states: { name: string; doc: string }[];
  components: { name: string; type_path: string; supplied: boolean }[];
}

export interface Action {
  name: string;
  doc: string;
  params: { name: string; type: string; required: boolean; default: unknown }[];
  requires: string[];
}

export interface Snapshot {
  schema: import('vscode-json-languageservice').JSONSchema;
  scene_schema: import('vscode-json-languageservice').JSONSchema;
  files: SourceFile[];
  diagnostics: EngineDiagnostic[];
  definitions: Definition[];
  presets: Preset[];
  extensors: Extensor[];
  actions: Action[];
}