import * as childProcess from 'child_process';
import * as readline from 'readline';
import * as vscode from 'vscode';
import {
  CreateParams,
  JsonRpcNotification,
  JsonRpcRequest,
  PromptParams,
  WorkspaceStateParams,
} from './protocol';
import * as fs from 'fs';
import * as path from 'path';

/**
 * Resolve the host binary path using a three-tier priority:
 *
 *   1. **User setting directory** — `configured` from `arrowCoder.server.path`.
 *      - an existing file (absolute or relative to the extension) is used as-is;
 *      - an existing directory is searched for `arrow-coder-vscode[.exe]`.
 *   2. **Extension install directory** — the bundled build inside the
 *      extension at `<ext>/bin/<platform>-<arch>/` (e.g.
 *      `bin/win32-x64/arrow-coder-vscode.exe`), with fallbacks to the flat
 *      `bin/` and `bin/<platform>/` for dev. This is what a packaged build
 *      (or `vscode:prepublish`) picks up.
 *   3. **PATH** — fall back to the bare command name `arrow-coder-vscode`,
 *      letting the OS resolve it from `PATH`.
 *
 * Tier 1 always wins when it points at something real, so a user can pin an
 * explicit binary. Tier 2 keeps dev/installed builds ahead of any stale
 * `cargo install` copy sitting on PATH.
 */
function resolveHostBinary(configured: string, extensionUri?: vscode.Uri): string {
  const exe = process.platform === 'win32' ? '.exe' : '';
  const name = `arrow-coder-vscode${exe}`;

  // --- Tier 1: user setting (file or directory) ---
  if (configured && configured.trim()) {
    const trimmed = configured.trim();
    // Absolute path to a file.
    if (path.isAbsolute(trimmed) && fs.existsSync(trimmed) && fs.statSync(trimmed).isFile()) {
      return trimmed;
    }
    // Absolute path to a directory → look for the binary inside it.
    if (path.isAbsolute(trimmed) && fs.existsSync(trimmed) && fs.statSync(trimmed).isDirectory()) {
      const inside = path.join(trimmed, name);
      if (fs.existsSync(inside)) {
        return inside;
      }
    }
    // Relative path → resolve against the extension directory.
    if (!path.isAbsolute(trimmed) && (trimmed.includes('/') || trimmed.includes('\\'))) {
      const base = extensionUri ? extensionUri.fsPath : __dirname;
      const resolved = path.resolve(base, trimmed);
      if (fs.existsSync(resolved)) {
        return fs.statSync(resolved).isDirectory()
          ? path.join(resolved, name)
          : resolved;
      }
    }
  }

  // --- Tier 2: bundled binary inside the extension install directory ---
  // When packaged (or after `vscode:prepublish`), the host binary ships inside
  // the extension at `<ext>/bin/`, a single platform-agnostic directory where
  // every platform build lands (no `<platform>-<arch>` subfolder). The legacy
  // `bin/<platform>-<arch>/` layout is kept as a fallback for already-packaged
  // .vsix files that predate this change. We do NOT walk up to a parent cargo
  // workspace — a bundled build is always self-contained under the extension root.
  const extDir = extensionUri ? extensionUri.fsPath : __dirname;
  const platform = process.platform; // win32 | darwin | linux
  const arch = process.arch; // x64 | arm64 | ia32
  const targetDir = `${platform}-${arch}`;
  const extCandidates = [
    path.join(extDir, 'bin', name),
    path.join(extDir, 'bin', `${name}.exe`),
    path.join(extDir, 'bin', targetDir, name),
    path.join(extDir, 'bin', targetDir, `${name}.exe`),
  ];
  for (const c of extCandidates) {
    if (fs.existsSync(c)) {
      return c;
    }
  }

  // --- Tier 3: PATH ---
  return 'arrow-coder-vscode';
}

/**
 * Drives the Rust `arrow-coder-vscode` host binary over stdio.
 *
 * The host speaks newline-delimited JSON-RPC 2.0: we send `Request`s on stdin
 * and receive `JsonRpcNotification`s on stdout (method families `agent/*` and
 * `session/*`). This class owns the child process lifecycle and fans
 * notifications out to subscribers.
 */
export class ArrowCoderHost {
  private proc: childProcess.ChildProcess | undefined;
  private rl: readline.Interface | undefined;
  private readonly listeners = new Set<(n: JsonRpcNotification) => void>();
  private readonly statusListeners = new Set<(ready: boolean, error?: string) => void>();
  private readonly workspaceListeners = new Set<(state: WorkspaceStateParams) => void>();
  private starting = false;
  private started = false;
  private nextId = 1;
  private extensionUri?: vscode.Uri;

  /** Provide the extension's install location so binary resolution can fall
   *  back to the bundled/debug build (tier 2 of the lookup). */
  setExtensionUri(uri: vscode.Uri): void {
    this.extensionUri = uri;
  }

  get isRunning(): boolean {
    return this.started && this.proc !== undefined;
  }

  /** Subscribe to host JSON-RPC notifications. Returns an unsubscribe function. */
  onNotification(cb: (n: JsonRpcNotification) => void): () => void {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  onStatus(cb: (ready: boolean, error?: string) => void): () => void {
    this.statusListeners.add(cb);
    return () => this.statusListeners.delete(cb);
  }

  /**
   * Subscribe to the `session/workspace_state` notification. The sidebar tree
   * view uses this to render the workspace / session list. Returns an
   * unsubscribe function.
   */
  onWorkspaceState(cb: (state: WorkspaceStateParams) => void): () => void {
    this.workspaceListeners.add(cb);
    return () => this.workspaceListeners.delete(cb);
  }

  /**
   * Start the host and create a session. Resolves once `session/create`
   * acknowledges with an `agent/done` notification; rejects on
   * spawn/initialization failure.
   */
  async start(params: CreateParams): Promise<void> {
    if (this.started || this.starting) {
      return;
    }
    this.starting = true;

    const configured = vscode.workspace
      .getConfiguration('arrowCoder')
      .get<string>('server.path', 'arrow-coder-vscode');

    // Resolve the host binary using the three-tier lookup: user setting
    // directory → extension install directory → PATH.
    const binary = resolveHostBinary(configured, this.extensionUri);

    const cwd =
      params.cwd ??
      vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ??
      process.cwd();

    const merged: CreateParams = {
      cwd,
      agent: params.agent,
      autoApprove: params.autoApprove,
      resume: params.resume ?? null,
    };

    ArrowCoderHost.outputChannel().appendLine(
      `[host] spawning "${binary}" (cwd=${cwd}, agent=${merged.agent}, autoApprove=${merged.autoApprove})`
    );

    await new Promise<void>((resolve, reject) => {
      try {
        this.proc = childProcess.spawn(binary, [], {
          cwd,
          env: { ...process.env },
          stdio: ['pipe', 'pipe', 'pipe'],
        });
      } catch (err) {
        this.starting = false;
        reject(new Error(`Failed to spawn ${binary}: ${(err as Error).message}`));
        return;
      }

      this.proc.on('error', (err) => {
        this.starting = false;
        this.emitStatus(false, `Process error: ${err.message}`);
        reject(new Error(`Host process error: ${err.message}`));
      });

      this.proc.on('exit', (code, signal) => {
        this.started = false;
        this.proc = undefined;
        this.emitStatus(false, signal ? `Host terminated (${signal})` : `Host exited (${code})`);
      });

      if (!this.proc.stdout) {
        this.starting = false;
        reject(new Error('Host stdout unavailable'));
        return;
      }

      // Parse NDJSON lines from stdout.
      this.rl = readline.createInterface({ input: this.proc.stdout });
      this.rl.on('line', (line) => this.handleLine(line));

      // Forward stderr to the output channel for debugging.
      if (this.proc.stderr) {
        this.proc.stderr.on('data', (d) => {
          const text = d.toString();
          ArrowCoderHost.outputChannel().append(text);
        });
      }

      // Send session/create and wait for the ack.
      const off = this.onNotification((n) => {
        if (n.method === 'agent/done') {
          off();
          this.started = true;
          this.starting = false;
          // Defer the status broadcast one tick so the webview's message
          // listener has time to register before we post.
          setTimeout(() => this.emitStatus(true), 0);
          resolve();
        } else if (n.method === 'agent/error') {
          off();
          this.starting = false;
          const err = (n.params as { error?: string })?.error ?? 'unknown error';
          this.emitStatus(false, err);
          reject(new Error(`session/create failed: ${err}`));
        }
      });

      this.send(this.req('session/create', merged));
    });
  }

  /** Send a chat prompt. Requires the host to be running. */
  sendPrompt(content: string, references: string[] = []): void {
    if (!this.isRunning) {
      throw new Error('Host not running');
    }
    const params: PromptParams = {
      input: { type: 'message', content, references },
    };
    this.send(this.req('session/prompt', params));
  }

  /** Send a slash command (recorded + executed by the core). */
  runCommand(name: string, args: string[] = []): void {
    if (!this.isRunning) {
      throw new Error('Host not running');
    }
    const params: PromptParams = { input: { type: 'command', name, args } };
    this.send(this.req('session/prompt', params));
  }

  undo(): void {
    this.send(this.req('session/undo'));
  }

  /**
   * Send a JSON-RPC request (used for workspace/session control). Accepts a
   * full `JsonRpcRequest` or a shorthand `{ method, params? }` (jsonrpc/id are
   * filled in automatically).
   */
  sendRaw(req: Partial<JsonRpcRequest> & { method: string }): void {
    if (!this.isRunning) {
      ArrowCoderHost.outputChannel().appendLine(
        `[host] sendRaw ignored, host not running: ${req.method}`
      );
      return;
    }
    this.send(this.req(req.method, req.params));
  }

  cancel(): void {
    this.send(this.req('session/cancel'));
  }

  /**
   * Reconfigure the active model / reasoning effort. Takes effect on the next
   * prompt only (the host stashes it and applies it before the next turn).
   */
  reconfigure(model: string | null, reasoningEffort: string | null): void {
    const params: Record<string, unknown> = {};
    if (model !== null) {
      params.model = model;
    }
    if (reasoningEffort !== null) {
      params.reasoning_effort = reasoningEffort;
    }
    this.send(this.req('session/reconfigure', params));
  }

  /** Build a fully-formed JSON-RPC 2.0 request with a unique id. */
  private req(method: string, params?: unknown): JsonRpcRequest {
    return { jsonrpc: '2.0', id: this.nextId++, method, params };
  }

  /** Terminate the host process. */
  stop(): void {
    if (this.proc) {
      this.proc.kill();
      this.proc = undefined;
    }
    this.rl?.close();
    this.rl = undefined;
    this.started = false;
    this.starting = false;
  }

  private send(req: JsonRpcRequest): void {
    if (!this.proc?.stdin) {
      throw new Error('Host stdin unavailable');
    }
    ArrowCoderHost.outputChannel().appendLine(`[host] -> ${req.method}`);
    this.proc.stdin.write(JSON.stringify(req) + '\n');
  }

  private handleLine(line: string): void {
    const trimmed = line.trim();
    if (!trimmed) {
      return;
    }
    let n: JsonRpcNotification;
    try {
      n = JSON.parse(trimmed) as JsonRpcNotification;
    } catch {
      // Non-JSON output (e.g. a stray log) — surface to the output channel.
      ArrowCoderHost.outputChannel().appendLine(trimmed);
      return;
    }
    ArrowCoderHost.outputChannel().appendLine(`[host] <- ${n.method}`);
    if (n.method === 'session/workspace_state') {
      const p = n.params as WorkspaceStateParams;
      for (const cb of this.workspaceListeners) {
        cb(p);
      }
    }
    for (const cb of this.listeners) {
      cb(n);
    }
  }

  private emitStatus(ready: boolean, error?: string): void {
    for (const cb of this.statusListeners) {
      cb(ready, error);
    }
  }

  /**
   * Terminate the child process and reset running state. Event listeners
   * registered by the sidebar providers are intentionally preserved across a
   * `dispose()` so a subsequent `start()` (e.g. "New Session") keeps delivering
   * events to the same views.
   */
  dispose(): void {
    this.started = false;
    this.starting = false;
    if (this.proc && !this.proc.killed) {
      this.proc.kill('SIGTERM');
    }
    this.proc = undefined;
    this.rl?.close();
    this.rl = undefined;
  }

  private static _channel: vscode.OutputChannel | undefined;
  private static outputChannel(): vscode.OutputChannel {
    if (!ArrowCoderHost._channel) {
      ArrowCoderHost._channel = vscode.window.createOutputChannel('Arrow Coder');
    }
    return ArrowCoderHost._channel;
  }
}
