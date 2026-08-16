// JSON-RPC 2.0 client for the webview <-> extension host bridge.
//
// The extension (ChatPanel) forwards our requests verbatim to the Rust host and
// forwards the host's notifications verbatim back to us. So this client only
// needs to:
//   - assign an `id` to each outgoing request and resolve its promise on the
//     matching response
//   - dispatch inbound notifications (no id) to registered subscribers

import type {
  JsonRpcNotification,
  JsonRpcRequest,
  JsonRpcResponse,
} from '../../src/protocol';

type NotificationHandler = (n: JsonRpcNotification) => void;

export interface VscodeApi {
  postMessage(msg: unknown): void;
  getState<T>(): T | undefined;
  setState<T>(state: T): void;
}

declare function acquireVsCodeApi(): VscodeApi;

class RpcClient {
  private readonly vscode: VscodeApi;
  private nextId = 1;
  private readonly pending = new Map<number | string, (r: JsonRpcResponse) => void>();
  private readonly notifHandlers = new Set<NotificationHandler>();

  constructor() {
    this.vscode = acquireVsCodeApi();
    window.addEventListener('message', (e: MessageEvent) => this.onMessage(e.data));
  }

  /**
   * Send a request and await the host's response (matched by id).
   *
   * NOTE: the Rust host replies to most requests with a JSON-RPC *notification*
   * (no `id`) rather than a response, so `await` would hang forever. To keep
   * callers responsive we reject after `timeoutMs` (default 5s). Callers that
   * must not block on a reply (e.g. undo / restoreFile) should treat rejection
   * as "the request was sent" and optimistically update local state.
   */
  request<T = unknown>(
    method: string,
    params?: unknown,
    timeoutMs = 5000
  ): Promise<T> {
    const id = this.nextId++;
    const req: JsonRpcRequest = { jsonrpc: '2.0', id, method, params };
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`timeout waiting for response to "${method}"`));
      }, timeoutMs);
      this.pending.set(id, (resp) => {
        clearTimeout(timer);
        if (resp.error) {
          reject(new Error(resp.error.message));
        } else {
          resolve(resp.result as T);
        }
      });
      this.vscode.postMessage(req);
    });
  }

  /** Subscribe to host notifications (method families agent/*, session/*). */
  onNotification(cb: NotificationHandler): () => void {
    this.notifHandlers.add(cb);
    return () => this.notifHandlers.delete(cb);
  }

  /** Tell the host we're ready (it will reply with a host/status notification). */
  ready(): void {
    this.vscode.postMessage({ jsonrpc: '2.0', method: 'host/ready' });
  }

  private onMessage(msg: unknown): void {
    if (!msg || typeof msg !== 'object') {
      return;
    }
    const m = msg as Partial<JsonRpcResponse> & Partial<JsonRpcNotification>;
    // Response: has an id and either result or error.
    if ('id' in m && m.id !== undefined && ('result' in m || 'error' in m)) {
      const cb = this.pending.get(m.id!);
      if (cb) {
        this.pending.delete(m.id!);
        cb(m as JsonRpcResponse);
      }
      return;
    }
    // Notification: has a method and no id.
    if ('method' in m && m.method) {
      const notif = m as JsonRpcNotification;
      for (const cb of this.notifHandlers) {
        cb(notif);
      }
    }
  }
}

export const rpc = new RpcClient();
