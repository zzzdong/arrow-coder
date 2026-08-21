import * as vscode from 'vscode';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { HostController } from './host/HostController';
import { JsonRpcNotification, JsonRpcRequest, JsonRpcResponse } from './protocol';

/**
 * A chat view embedded directly in the Activity Bar sidebar (as a `webview`
 * view). It owns a single shared {@link HostController} and acts as a
 * TRANSPARENT bridge between the host (Rust stdio, JSON-RPC) and the webview UI
 * (JSON-RPC over postMessage).
 *
 * The bridge does NOT translate protocol content: host notifications are
 * forwarded verbatim to the webview, and webview requests are forwarded verbatim
 * to the host. The only messages the bridge originates locally are lifecycle
 * `status` signals.
 *
 * The view merges two regions into one sidebar pane:
 *   - top:    workspace / session tree (rendered inside the webview)
 *   - bottom: the streaming conversation timeline + input bar
 */
export class ChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'arrowCoder.chatView';
  private view: vscode.WebviewView | undefined;
  private readonly host: HostController;
  private readonly extensionUri: vscode.Uri;
  private disposables: vscode.Disposable[] = [];
  /** Unsubscribers for host listeners registered per resolved view. Re-registering
   *  on every `resolveWebviewView` would accumulate duplicate handlers and cause
   *  each host notification to be forwarded multiple times, so we detach the old
   *  ones before wiring up a freshly re-created webview. */
  private notifOff?: () => void;
  private statusOff?: () => void;

  constructor(host: HostController, extensionUri: vscode.Uri) {
    this.host = host;
    this.extensionUri = extensionUri;
  }

  public resolveWebviewView(
    view: vscode.WebviewView,
    _ctx: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
    this.view = view;
    view.webview.options = {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, 'out', 'webview')],
    };
    view.webview.html = this.render();

    // Detach listeners from any previously resolved (now re-created) webview so
    // we never forward a host notification more than once.
    this.notifOff?.();
    this.statusOff?.();

    // Host JSON-RPC notifications -> webview (transparent forward).
    this.notifOff = this.host.onNotification((n: JsonRpcNotification) => this.post(n));
    this.statusOff = this.host.onStatus((ready, error) => {
      if (ready) {
        this.post({ jsonrpc: '2.0', method: 'host/status', params: { ready: true } });
        // Pull the workspace registry so the tree + switcher can render.
        this.host.sendRequest({ jsonrpc: '2.0', id: 0, method: 'workspace/list' });
      } else if (error) {
        this.post({ jsonrpc: '2.0', method: 'host/status', params: { ready: false, error } });
      }
    });

    // Webview JSON-RPC requests -> host (transparent forward).
    view.webview.onDidReceiveMessage(
      (msg: JsonRpcRequest | { jsonrpc: '2.0'; method: 'host/ready' }) => this.handleUiMessage(msg),
      null,
      this.disposables
    );
  }

  /** Reveal the sidebar view (best-effort; the view is always present). */
  public reveal(): void {
    void vscode.commands.executeCommand('workbench.view.extension.arrowCoder');
  }

  /** Start a new session (invoked from the native panel title-bar action). */
  public newSession(): void {
    this.host.sendRequest({ jsonrpc: '2.0', id: 0, method: 'session/new' });
  }

  /** Toggle the in-webview Session History drawer. */
  public toggleHistory(): void {
    this.post({ jsonrpc: '2.0', method: 'ui/toggleHistory', params: {} });
  }

  /**
   * "Add to Arrow Coder Chat" — invoked from an editor or explorer right-click.
   * Forwards a reference to the webview composer draft:
   *  - editor + non-empty selection  → the selected lines (inlined snippet)
   *  - file / folder / whole document → the path (expanded by core as `@path`)
   * The webview appends it to the draft so the user can add an instruction
   * before sending, mirroring the harness `@`-reference flow.
   */
  /**
   * "Add to Arrow Coder Chat" from the editor/explorer context menu.
   *
   * VS Code passes an array of URIs when multiple resources are selected in the
   * Explorer (and a single `vscode.Uri` when invoked from the editor or with one
   * selection). We inject every target as a structured reference so the user can
   * reference several files/folders at once; an active editor selection is
   * captured as a line-range reference.
   */
  public async addToChat(uris?: vscode.Uri[] | vscode.Uri): Promise<void> {
    const list: vscode.Uri[] = Array.isArray(uris)
      ? uris
      : uris
        ? [uris]
        : [];
    if (list.length === 0) return;

    const editor = vscode.window.activeTextEditor;

    for (const uri of list) {
      const path = uri.fsPath;
      // Capture an active editor selection when the focused document matches.
      const selection =
        editor && editor.document.uri.toString() === uri.toString() && !editor.selection.isEmpty
          ? editor.selection
          : undefined;

      let payload: Record<string, unknown>;
      if (selection) {
        const startLine = selection.start.line + 1;
        const endLine = selection.end.line + 1;
        const snippet = editor!.document.getText(selection);
        payload = { kind: 'selection', path, startLine, endLine, snippet };
      } else {
        // Determine file vs directory without a blocking fs.stat when possible.
        let isDir = false;
        try {
          const stat = await vscode.workspace.fs.stat(uri);
          isDir = (stat.type & vscode.FileType.Directory) !== 0;
        } catch {
          isDir = false;
        }
        payload = { kind: 'path', path, isDir };
      }
      this.post({ jsonrpc: '2.0', method: 'ui/injectReference', params: payload });
    }
    this.reveal();
  }

  private handleUiMessage(msg: JsonRpcRequest | { jsonrpc: '2.0'; method: string }): void {
    // The webview speaks JSON-RPC: every outbound message is a request with an
    // `id` (or a host-scoped notification). Forward it verbatim to the host.
    if ('id' in msg) {
      const req = msg as JsonRpcRequest;
      // `vscode/executeCommand` invokes the VS Code editor API, which only
      // exists in the extension host (not the Rust process). Execute it here and
      // reply directly to the webview instead of forwarding to the host.
      if (req.method === 'vscode/executeCommand') {
        void this.execVscodeCommand(req);
        return;
      }
      // `view/diffFile` opens a native VS Code Diff Editor comparing the agent's
      // checkpoint snapshot against the file's current on-disk state. It needs
      // the editor API, so it runs here instead of in the Rust host.
      if (req.method === 'view/diffFile') {
        void this.execDiffFile(req);
        return;
      }
      // `workspace/openFile` opens a file in the VS Code editor. It runs in the
      // extension host (needs the editor API). A file that was deleted after the
      // checkpoint is gracefully reported as `ok:false` instead of an error.
      if (req.method === 'workspace/openFile') {
        void this.execOpenFile(req);
        return;
      }
      // `workspace/readFile` reads a file's content or lists a directory, used by
      // the `@` mention completion + reference injection in the composer.
      if (req.method === 'workspace/readFile') {
        void this.execReadFile(req);
        return;
      }
      this.host.sendRequest(req);
      return;
    }
    // Host-scoped notifications (no id) the bridge handles locally.
    if (msg.method === 'host/ready') {
      // Acknowledge readiness so the webview flips `ready` (this unblocks
      // `maybeAutoOpen`, which then loads the active session's messages).
      this.post({
        jsonrpc: '2.0',
        method: 'host/status',
        params: { ready: this.host.isRunning },
      });
      // Re-pull the workspace registry. This matters after the webview is
      // re-created (e.g. when the user switches to the Source Control view and
      // back): the host is already running, so its `onStatus` callback will NOT
      // fire again, and without this the fresh webview never receives
      // `workspace_state` and stays blank. The host replies with a
      // `session/workspace_state` notification, which restores the tab strip and
      // (via `maybeAutoOpen`) the active session's message history.
      this.host.sendRequest({ jsonrpc: '2.0', id: 0, method: 'workspace/list' });
    }
  }

  /** Execute a VS Code command from the webview and reply with its result. */
  private async execVscodeCommand(req: JsonRpcRequest): Promise<void> {
    try {
      const params = (req.params ?? {}) as { command?: string; args?: unknown[] };
      const command = params.command ?? '';
      const args = Array.isArray(params.args) ? params.args : [];
      const result = await vscode.commands.executeCommand(command, ...args);
      this.post({ jsonrpc: '2.0', id: req.id, result });
    } catch (err) {
      this.post({
        jsonrpc: '2.0',
        id: req.id,
        error: { code: -32000, message: (err as Error).message },
      });
    }
  }

  /**
   * Open a native VS Code Diff Editor comparing the agent's checkpoint snapshot
   * (`originalContent`) against the file's current on-disk state.
   *
   * The snapshot is written to a temporary file so it can be used as the "left"
   * side of `vscode.diff`. The request carries `{ path, originalContent }`;
   * `originalContent` is `undefined`/null when the file was created during the
   * turn (in that case we diff an empty document against the file).
   */
  private async execDiffFile(req: JsonRpcRequest): Promise<void> {
    const params = (req.params ?? {}) as { path?: string; originalContent?: string | null };
    const filePath = params.path ?? '';
    const original = params.originalContent ?? '';
    try {
      if (!filePath) {
        throw new Error('diffFile: missing path');
      }
      const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      const rel =
        workspaceRoot && path.resolve(filePath).startsWith(path.resolve(workspaceRoot))
          ? path.relative(workspaceRoot, filePath).replace(/\\/g, '/')
          : path.basename(filePath);

      const currentUri = vscode.Uri.file(filePath);
      // If the file was deleted after the checkpoint (e.g. a temp helper file),
      // there is nothing to diff against — report it gracefully so the webview
      // can drop it from the change list.
      const stat = await vscode.workspace.fs.stat(currentUri);
      if (stat.type === vscode.FileType.Unknown) {
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: false, reason: 'not_found' } });
        return;
      }
      const originalUri = await this.writeSnapshotTemp(filePath, original);
      await vscode.commands.executeCommand('vscode.diff', originalUri, currentUri, `${rel} (checkpoint → 当前)`);
      this.post({ jsonrpc: '2.0', id: req.id, result: { ok: true } });
    } catch (err) {
      const e = err as Error;
      if (e.message && /ENOENT|no such file|not found/i.test(e.message)) {
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: false, reason: 'not_found' } });
        return;
      }
      this.post({
        jsonrpc: '2.0',
        id: req.id,
        error: { code: -32000, message: e.message },
      });
    }
  }

  /**
   * Read a file's content or list a directory. Used by the `@` mention feature:
   * `{ path, mode:'content' }` returns the file text, `{ path, mode:'list' }`
   * returns directory entries (name, path, isDir). Missing paths are reported
   * gracefully as `{ ok:false, reason:'not_found' }`.
   */
  private async execReadFile(req: JsonRpcRequest): Promise<void> {
    const params = (req.params ?? {}) as { path?: string; mode?: 'content' | 'list' };
    const filePath = params.path ?? '';
    const mode = params.mode ?? 'content';
    try {
      if (!filePath) throw new Error('readFile: missing path');
      const uri = vscode.Uri.file(filePath);
      const stat = await vscode.workspace.fs.stat(uri);
      if (stat.type === vscode.FileType.Unknown) {
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: false, reason: 'not_found' } });
        return;
      }
      if (stat.type === vscode.FileType.Directory) {
        const entries = await vscode.workspace.fs.readDirectory(uri);
        const list = entries.map(([name, type]) => ({
          name,
          path: path.join(filePath, name).replace(/\\/g, '/'),
          isDir: (type & vscode.FileType.Directory) !== 0,
        }));
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: true, kind: 'dir', entries: list } });
        return;
      }
      if (mode === 'list') {
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: true, kind: 'file', entries: [] } });
        return;
      }
      const bytes = await vscode.workspace.fs.readFile(uri);
      const content = Buffer.from(bytes).toString('utf8');
      this.post({ jsonrpc: '2.0', id: req.id, result: { ok: true, kind: 'file', content } });
    } catch (err) {
      const e = err as Error;
      if (e.message && /ENOENT|no such file|not found/i.test(e.message)) {
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: false, reason: 'not_found' } });
        return;
      }
      this.post({
        jsonrpc: '2.0',
        id: req.id,
        error: { code: -32000, message: e.message },
      });
    }
  }

  /** Persist the checkpoint snapshot to a temp file, returning its URI. */
  private async writeSnapshotTemp(filePath: string, original: string): Promise<vscode.Uri> {
    const tmpDir = path.join(os.tmpdir(), 'arrow-coder-diff');
    await fs.promises.mkdir(tmpDir, { recursive: true });
    const tmpFile = path.join(tmpDir, path.basename(filePath) + '.checkpoint');
    await fs.promises.writeFile(tmpFile, original, 'utf8');
    return vscode.Uri.file(tmpFile);
  }

  /**
   * Open a file in the VS Code editor. A file that no longer exists on disk
   * (e.g. a temp file created during the turn and later deleted) is reported
   * gracefully as `{ ok:false, reason:'not_found' }` rather than a hard error,
   * so the webview can drop it from the change list.
   */
  private async execOpenFile(req: JsonRpcRequest): Promise<void> {
    const params = (req.params ?? {}) as { path?: string };
    const filePath = params.path ?? '';
    try {
      if (!filePath) throw new Error('openFile: missing path');
      const uri = vscode.Uri.file(filePath);
      const stat = await vscode.workspace.fs.stat(uri);
      if (stat.type === vscode.FileType.Unknown) {
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: false, reason: 'not_found' } });
        return;
      }
      await vscode.window.showTextDocument(uri, { preview: false });
      this.post({ jsonrpc: '2.0', id: req.id, result: { ok: true } });
    } catch (err) {
      const e = err as Error;
      // File gone between stat and open: treat as not found rather than error.
      if (e.message && /ENOENT|no such file|not found/i.test(e.message)) {
        this.post({ jsonrpc: '2.0', id: req.id, result: { ok: false, reason: 'not_found' } });
        return;
      }
      this.post({
        jsonrpc: '2.0',
        id: req.id,
        error: { code: -32000, message: e.message },
      });
    }
  }

  /**
   * Tear down the current agent host and spin up a fresh one rooted at `cwd`
   * (the selected workspace). The host records the new session in the workspace
   * registry on `session/create`, so the tree gains a new entry.
   */
  private async restart(cwd?: string): Promise<void> {
    this.host.dispose();
    const cfg = vscode.workspace.getConfiguration('arrowCoder');
    try {
      await this.host.start({
        agent: cfg.get<string>('server.agent', 'default'),
        autoApprove: cfg.get<boolean>('server.autoApprove', false),
        resume: null,
        fresh: true,
        cwd: cwd ?? vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? undefined,
      });
    } catch (err) {
      vscode.window.showErrorMessage(`Arrow Coder restart failed: ${(err as Error).message}`);
    }
  }

  /** Start a brand-new session in the current workspace. */
  public newSessionInCurrentWorkspace(): void {
    const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    void this.restart(cwd);
  }

  public openSession(workspacePath: string, sessionId: string): void {
    this.host.sendRequest({
      jsonrpc: '2.0',
      id: 0,
      method: 'workspace/openSession',
      params: { path: workspacePath, session_id: sessionId },
    });
  }

  private post(
    msg: JsonRpcNotification | JsonRpcResponse | { jsonrpc: '2.0'; method: string; params?: unknown }
  ): void {
    this.view?.webview.postMessage(msg);
  }

  private render(): string {
    // Vite emits an ES module bundle plus a CSS file; both must be loaded via
    // asWebviewUri (the webview can't resolve the bundle's own absolute
    // `/assets/...` paths). The @vscode-elements components are bundled into
    // the module, no extra <script> needed.
    const scriptUri = this.view!.webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'out', 'webview', 'assets', 'index.js')
    );
    const styleUri = this.view!.webview.asWebviewUri(
      vscode.Uri.joinPath(this.extensionUri, 'out', 'webview', 'assets', 'index.css')
    );
    const nonce = ChatViewProvider.getNonce();
    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy"
        content="default-src 'none'; style-src ${this.view!.webview.cspSource} 'unsafe-inline'; img-src ${this.view!.webview.cspSource} https: data:; script-src 'nonce-${nonce}';" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Arrow Coder</title>
  <link rel="stylesheet" href="${styleUri}" />
  <style>
    :root { color-scheme: light dark; }
    html, body { height: 100%; margin: 0; }
    #app { height: 100%; }
  </style>
</head>
<body>
  <div id="app"></div>
  <script type="module" nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
  }

  private static getNonce(): string {
    let text = '';
    const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    for (let i = 0; i < 32; i++) {
      text += chars.charAt(Math.floor(Math.random() * chars.length));
    }
    return text;
  }

  public dispose(): void {
    for (const d of this.disposables) {
      d.dispose();
    }
    this.disposables = [];
  }
}
