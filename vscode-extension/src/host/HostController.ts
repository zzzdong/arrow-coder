// HostController — state-machine wrapper around the Rust host process.
//
// Responsibilities (post-refactor):
//   - own the ArrowCoderHost lifecycle: spawning -> ready -> running -> stopped
//   - expose a single typed `onNotification` callback (JSON-RPC notifications)
//   - expose a single `sendRequest` for forwarding webview JSON-RPC requests
//
// The controller is a THIN bridge: it does NOT translate protocol content. The
// method/params shapes come straight from src/protocol.ts, which mirrors
// crates/arrow-coder-vscode/src/jsonrpc.rs.

import * as vscode from 'vscode';
import { ArrowCoderHost } from '../host';
import { CreateParams, JsonRpcNotification, JsonRpcRequest } from '../protocol';

export type HostState = 'idle' | 'spawning' | 'ready' | 'running' | 'stopped' | 'error';

export type NotificationHandler = (n: JsonRpcNotification) => void;
export type StatusHandler = (ready: boolean, error?: string) => void;

/**
 * Wrap the legacy ArrowCoderHost with an explicit state machine and a unified
 * JSON-RPC notification stream. The webview bridge (ChatPanel) talks only to
 * this controller, never to ArrowCoderHost directly.
 */
export class HostController {
  private host: ArrowCoderHost;
  private _state: HostState = 'idle';
  private readonly notifHandlers = new Set<NotificationHandler>();
  private readonly statusHandlers = new Set<StatusHandler>();

  constructor(extensionUri?: vscode.Uri) {
    this.host = new ArrowCoderHost();
    if (extensionUri) {
      this.host.setExtensionUri(extensionUri);
    }
  }

  get state(): HostState {
    return this._state;
  }

  get isRunning(): boolean {
    return this.host.isRunning;
  }

  /** Subscribe to parsed JSON-RPC notifications from the host. */
  onNotification(cb: NotificationHandler): () => void {
    this.notifHandlers.add(cb);
    return () => this.notifHandlers.delete(cb);
  }

  /** Subscribe to host lifecycle status (ready / error). */
  onStatus(cb: StatusHandler): () => void {
    this.statusHandlers.add(cb);
    return () => this.statusHandlers.delete(cb);
  }

  /**
   * Start the host and create a session. Resolves once the host reports ready;
   * rejects on spawn / initialization failure.
   */
  async start(params: CreateParams): Promise<void> {
    if (this._state === 'spawning' || this._state === 'running') {
      return;
    }
    this._state = 'spawning';

    this.host.onNotification((n) => {
      for (const cb of this.notifHandlers) {
        cb(n);
      }
    });
    this.host.onStatus((ready, error) => {
      if (ready) {
        this._state = 'running';
      } else {
        this._state = error ? 'error' : 'stopped';
      }
      for (const cb of this.statusHandlers) {
        cb(ready, error);
      }
    });

    try {
      await this.host.start(params);
      this._state = 'running';
    } catch (err) {
      this._state = 'error';
      throw err;
    }
  }

  /**
   * Forward a webview JSON-RPC request to the Rust host (transparent bridge).
   */
  sendRequest(req: JsonRpcRequest): void {
    this.host.sendRaw(req);
  }

  /** High-level control helpers (kept for non-webview callers). */
  sendPrompt(content: string): void {
    this.host.sendPrompt(content);
  }
  undo(): void {
    this.host.undo();
  }
  cancel(): void {
    this.host.cancel();
  }
  reconfigure(model: string | null, effort: string | null): void {
    this.host.reconfigure(model, effort);
  }
  stop(): void {
    this.host.stop();
    this._state = 'stopped';
  }
  dispose(): void {
    this.host.dispose();
    this._state = 'stopped';
  }
}
