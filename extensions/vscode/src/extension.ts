import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { Range } from 'vscode-json-languageservice';
import { describeDefinition, engineRange, Features, sourceFile } from './features';
import { Host } from './host';
import { Snapshot } from './types';

const PROTOCOL_VERSION = 1;
const SELECTOR: vscode.DocumentSelector = [
  { language: 'jsonc', scheme: 'file' },
  { language: 'json', scheme: 'file' },
];

function configured(folder: vscode.WorkspaceFolder, id: string): string | undefined {
  return vscode.workspace.getConfiguration('struction', folder.uri).get<string>(id)?.trim() || undefined;
}

/** The workspace's own layouts, used when `struction.projectRoot` is empty. */
function projectRoot(folder: vscode.WorkspaceFolder): string | undefined {
  const setting = configured(folder, 'projectRoot');
  if (setting) return path.isAbsolute(setting) ? setting : path.resolve(folder.uri.fsPath, setting);
  for (const candidate of ['project', 'apps/playground/project']) {
    const resolved = path.resolve(folder.uri.fsPath, candidate);
    if (fs.existsSync(resolved)) return resolved;
  }
  return undefined;
}

/** Absolute paths stand; a bare name is looked up on PATH; a relative path resolves from the folder. */
function hostExecutable(folder: vscode.WorkspaceFolder): string {
  const setting = configured(folder, 'hostPath') ?? 'struction-language';
  return path.isAbsolute(setting) || !setting.includes(path.sep) ? setting : path.resolve(folder.uri.fsPath, setting);
}

function projectRelative(root: string | undefined, uri: vscode.Uri): string | undefined {
  if (!root || uri.scheme !== 'file') return undefined;
  const relative = path.relative(root, uri.fsPath);
  if (!relative || relative.startsWith('..') || path.isAbsolute(relative)) return undefined;
  return relative.split(path.sep).join('/');
}

function toRange(range: Range): vscode.Range {
  return new vscode.Range(range.start.line, range.start.character, range.end.line, range.end.character);
}

/** One workspace folder's language host and the snapshot it last produced. */
class Session implements vscode.Disposable {
  private readonly diagnostics = vscode.languages.createDiagnosticCollection('struction');
  private readonly status: vscode.StatusBarItem;
  private readonly reported = new Set<string>();
  private host?: Host;
  private features?: Features;
  private queue: Promise<unknown> = Promise.resolve();
  private timer?: NodeJS.Timeout;
  private root?: string;
  private problems = 0;

  constructor(private readonly folder: vscode.WorkspaceFolder, private readonly log: vscode.OutputChannel) {
    this.status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    this.status.command = 'struction.restart';
  }

  get count(): number {
    return this.problems;
  }

  get name(): string {
    return this.folder.name;
  }

  get definitions(): string[] {
    return [...(this.features?.snapshot.definitions ?? [])].map(d => d.path).sort();
  }

  snapshot(): Snapshot | undefined {
    return this.features?.snapshot;
  }

  start(): void {
    this.status.show();
    void this.restart();
  }

  dispose(): void {
    clearTimeout(this.timer);
    this.host?.dispose();
    this.diagnostics.dispose();
    this.status.dispose();
  }

  /** Re-analyzes changed buffers once typing settles; the oldest pending request is dropped. */
  schedule(): void {
    const delay = vscode.workspace.getConfiguration('struction', this.folder.uri).get<number>('validationDelay') ?? 300;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      void this.analyze();
    }, Math.max(0, delay));
  }

  async restart(): Promise<void> {
    clearTimeout(this.timer);
    this.host?.dispose();
    this.host = undefined;
    this.features = undefined;
    this.problems = 0;
    this.diagnostics.clear();
    this.root = projectRoot(this.folder);
    if (!this.root) {
      this.status.text = '$(question) Struction';
      this.status.tooltip = `No project directory in ${this.folder.name}. Set struction.projectRoot.`;
      this.log.appendLine(`[${this.folder.name}] no project directory; set struction.projectRoot`);
      return;
    }
    const root = this.root;
    this.status.text = '$(sync~spin) Struction';
    this.status.tooltip = `Analyzing ${root}`;
    try {
      const host = new Host(hostExecutable(this.folder),
        [...(vscode.workspace.getConfiguration('struction', this.folder.uri).get<string[]>('hostArgs') ?? []), root],
        root, text => this.log.append(text));
      this.host = host;
      const described = await host.request<{ protocol_version: number }>({ op: 'describe' });
      if (described.protocol_version !== PROTOCOL_VERSION) {
        throw new Error(`language host speaks protocol ${described.protocol_version}, this extension speaks ${PROTOCOL_VERSION}`);
      }
      await this.analyze();
    } catch (error) {
      this.report(error instanceof Error ? error.message : String(error));
    }
  }

  async validate(): Promise<void> {
    await this.analyze();
  }

  /** The feature set and project-relative file for a document, when this session owns it. */
  resolve(uri: vscode.Uri): { features: Features; file: string } | undefined {
    const file = projectRelative(this.root, uri);
    return this.features && file ? { features: this.features, file } : undefined;
  }

  private analyze(): Promise<void> {
    const run = this.queue.then(() => this.request());
    // A failed host leaves later buffers unanalyzed, so the queue only carries failures forward.
    this.queue = run.catch(() => {});
    return run;
  }

  private async request(): Promise<void> {
    const host = this.host;
    const root = this.root;
    if (!host || !root) return;
    const sources: Record<string, string> = {};
    for (const document of vscode.workspace.textDocuments) {
      const file = projectRelative(root, document.uri);
      if (file && sourceFile(file)) sources[file] = document.getText();
    }
    this.status.text = '$(sync~spin) Struction';
    const snapshot = await host.request<Snapshot>({ op: 'analyze', sources });
    this.features = new Features(snapshot);
    this.publish(snapshot);
    this.status.text = this.problems ? '$(warning) Struction' : '$(check) Struction';
    this.status.tooltip = `${this.folder.name}: ${snapshot.definitions.length} definitions, ` +
      `${snapshot.extensors.length} extensors, ${this.problems} problems in ${root}`;
  }

  private publish(snapshot: Snapshot): void {
    this.diagnostics.clear();
    this.problems = 0;
    for (const engine of snapshot.diagnostics) {
      const uri = engine.file ? this.uriOf(engine.file) : undefined;
      if (!uri) {
        // Library and configuration problems have no file to open; the channel keeps them reachable.
        this.log.appendLine(`[${this.folder.name}] ${engine.file ?? 'project'}: ${engine.message}`);
        this.problems += 1;
        continue;
      }
      const text = this.textOf(uri);
      // Without the file's text the reported token is unknown, so a single character stands in for it.
      const range = text
        ? toRange(engineRange(text, engine.line, engine.column))
        : new vscode.Range(Math.max((engine.line ?? 1) - 1, 0), 0, Math.max((engine.line ?? 1) - 1, 0), 1);
      const diagnostic = new vscode.Diagnostic(range, engine.message, vscode.DiagnosticSeverity.Error);
      diagnostic.source = 'Struction';
      this.diagnostics.set(uri, [...(this.diagnostics.get(uri) ?? []), diagnostic]);
      this.problems += 1;
    }
  }

  /** The host reports project files relative to the root it was given and library files absolute. */
  private uriOf(file: string): vscode.Uri | undefined {
    const root = this.root;
    if (!root) return undefined;
    return vscode.Uri.file(path.isAbsolute(file) ? file : path.resolve(root, file));
  }

  /** Unedited diagnostics need their file's text to span the reported token. */
  private textOf(uri: vscode.Uri): string | undefined {
    const open = vscode.workspace.textDocuments.find(document => document.uri.toString() === uri.toString());
    if (open) return open.getText();
    try {
      return fs.readFileSync(uri.fsPath, 'utf8');
    } catch {
      return undefined;
    }
  }

  private report(message: string): void {
    const missing = message.includes('ENOENT');
    const full = `${message}${missing ? ' Build it with `cargo install --path crates/struction_language` or set struction.hostPath.' : ''}`;
    this.log.appendLine(`[${this.folder.name}] ${full}`);
    this.status.text = '$(error) Struction';
    this.status.tooltip = full;
    if (this.reported.has(full)) return;
    this.reported.add(full);
    void vscode.window.showErrorMessage(`Struction: ${full}`);
  }
}

export function activate(context: vscode.ExtensionContext): void {
  const log = vscode.window.createOutputChannel('Struction');
  const sessions = new Map<string, Session>();
  const sessionFor = (uri: vscode.Uri): Session | undefined => {
    const folder = vscode.workspace.getWorkspaceFolder(uri) ?? vscode.workspace.workspaceFolders?.[0];
    return folder && sessions.get(folder.uri.toString());
  };
  const sessionsFor = (): Session[] => {
    const active = vscode.window.activeTextEditor && sessionFor(vscode.window.activeTextEditor.document.uri);
    return active ? [active] : [...sessions.values()];
  };

  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    const session = new Session(folder, log);
    sessions.set(folder.uri.toString(), session);
    context.subscriptions.push(session);
    session.start();
  }

  context.subscriptions.push(
    log,
    vscode.workspace.onDidChangeTextDocument(event => sessionFor(event.document.uri)?.schedule()),
    vscode.workspace.onDidSaveTextDocument(document => sessionFor(document.uri)?.schedule()),
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      for (const session of sessions.values()) void session.restart();
    }),
    vscode.workspace.onDidChangeConfiguration(event => {
      if (event.affectsConfiguration('struction')) {
        for (const session of sessions.values()) void session.restart();
      }
    }),
    vscode.languages.registerHoverProvider(SELECTOR, {
      provideHover: async (document, position) => {
        const resolved = sessionFor(document.uri)?.resolve(document.uri);
        if (!resolved) return undefined;
        const hover = await resolved.features.hover(resolved.file,
          resolved.features.document(document.uri.toString(), document.getText()), position);
        return hover && new vscode.Hover(new vscode.MarkdownString(hover.text),
          hover.range && toRange(hover.range));
      },
    }),
    vscode.languages.registerCompletionItemProvider(SELECTOR, {
      provideCompletionItems: async (document, position) => {
        const resolved = sessionFor(document.uri)?.resolve(document.uri);
        if (!resolved) return undefined;
        const list = await resolved.features.complete(resolved.file,
          resolved.features.document(document.uri.toString(), document.getText()), position);
        // Completion items are plain data; the service's enums line up with the editor's.
        return list && list as unknown as vscode.CompletionList;
      },
    }),
    vscode.languages.registerDefinitionProvider(SELECTOR, {
      provideDefinition: async (document, position) => {
        const resolved = sessionFor(document.uri)?.resolve(document.uri);
        if (!resolved) return undefined;
        const source = resolved.features.reference(resolved.file, document.getText(), document.offsetAt(position));
        return source === undefined ? undefined : new vscode.Location(vscode.Uri.file(source), new vscode.Position(0, 0));
      },
    }),
    vscode.commands.registerCommand('struction.restart', () => {
      for (const session of sessions.values()) void session.restart();
    }),
    vscode.commands.registerCommand('struction.validate', async () => {
      const targets = sessionsFor();
      if (!targets.length) {
        void vscode.window.showWarningMessage('Struction: no workspace folder with a project directory.');
        return;
      }
      await Promise.all(targets.map(session => session.validate()));
      const problems = targets.reduce((total, session) => total + session.count, 0);
      void vscode.window.showInformationMessage(problems
        ? `Struction: ${problems} problem(s) in ${targets.length === 1 ? targets[0].name : 'the workspace'}.`
        : 'Struction: no problems.');
    }),
    vscode.commands.registerCommand('struction.inspect', async () => {
      const session = sessionsFor()[0];
      const paths = session?.definitions ?? [];
      if (!paths.length) {
        void vscode.window.showWarningMessage('Struction: no definitions loaded; check the Struction output channel.');
        return;
      }
      const picked = await vscode.window.showQuickPick(paths, { placeHolder: 'Inspect a definition' });
      const definition = session?.snapshot()?.definitions.find(d => d.path === picked);
      if (definition) await vscode.commands.executeCommand('markdown.showPreview', describeDefinition(definition));
    }),
  );
}

export function deactivate(): void {}