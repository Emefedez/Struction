import { spawn, ChildProcessWithoutNullStreams } from 'node:child_process';
import { createInterface } from 'node:readline';

interface Pending {
  resolve(value: unknown): void;
  reject(error: Error): void;
  timer: NodeJS.Timeout;
}

/** Request IDs keep late responses from becoming the reply to a newer request. */
export class Host {
  private readonly process: ChildProcessWithoutNullStreams;
  private readonly pending = new Map<number, Pending>();
  private nextId = 0;
  private failure?: Error;

  constructor(executable: string, args: string[], cwd: string, log: (text: string) => void) {
    this.process = spawn(executable, args, { cwd, shell: false, windowsHide: true });
    this.process.stderr.on('data', chunk => log(String(chunk)));
    createInterface({ input: this.process.stdout }).on('line', line => {
      try {
        const response = JSON.parse(line);
        const pending = this.pending.get(response.id);
        if (!pending) return;
        this.pending.delete(response.id);
        clearTimeout(pending.timer);
        if (response.ok) pending.resolve(response.result);
        else pending.reject(new Error(response.error?.message ?? 'Language host rejected request'));
      } catch {
        this.fail(new Error('Language host wrote invalid JSON to stdout; send logs to stderr'));
        this.process.kill();
      }
    });
    this.process.on('error', error => this.fail(error));
    this.process.on('exit', (code, signal) => this.fail(new Error(`Language host exited (${signal ?? code})`)));
    this.process.stdin.on('error', error => this.fail(error));
  }

  request<T>(command: object): Promise<T> {
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(() => {
        this.fail(new Error('Language host timed out after 30 seconds'));
        this.process.kill();
      }, 30_000);
      this.pending.set(id, { resolve: value => resolve(value as T), reject, timer });
      this.process.stdin.write(JSON.stringify({ id, command }) + '\n');
    });
  }

  private fail(error: Error): void {
    this.failure ??= error;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(this.failure);
    }
    this.pending.clear();
  }

  dispose(): void {
    this.fail(new Error('Language host stopped'));
    this.process.kill();
  }
}
