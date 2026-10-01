export interface EngineDiagnostic {
  message: string;
  file: string | null;
  line: number | null;
  column: number | null;
}

export interface Definition {
  path: string;
  source: string | null;
  library: string | null;
  lineage: string[];
  resolved: Record<string, unknown>;
  components: string[];
  extensors: { name: string; reason: Record<string, string>; supplied: string[]; components: string[] }[];
}

export interface Extensor {
  name: string;
  doc: string;
  opt_in: boolean;
  requires: string[];
  states: string[];
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
  diagnostics: EngineDiagnostic[];
  definitions: Definition[];
  extensors: Extensor[];
  actions: Action[];
}
