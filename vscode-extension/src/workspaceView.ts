// Activity-Bar sidebar tree view for Arrow Coder workspaces and sessions.
//
// Mirrors how mature AI-coding extensions (Continue, Cline) surface history:
// a dedicated view container in the Activity Bar whose TreeView lists the
// workspaces (by cwd) with their sessions nested underneath. Clicking a
// session opens/resumes it in the chat panel; the tree is kept in sync with
// the host's `workspace_state` snapshot.

import * as vscode from 'vscode';
import { ArrowCoderHost } from './host';
import { Workspace, WorkspaceSession } from './protocol';

/** A node in the workspace tree. */
export type WorkspaceNode =
  | {
      kind: 'workspace';
      path: string;
      title: string;
      sessions: WorkspaceSession[];
    }
  | {
      kind: 'session';
      workspacePath: string;
      session: WorkspaceSession;
    };

export class WorkspaceViewProvider
  implements vscode.TreeDataProvider<WorkspaceNode>, vscode.Disposable
{
  private emitter = new vscode.EventEmitter<WorkspaceNode | undefined | null | void>();
  readonly onDidChangeTreeData: vscode.Event<WorkspaceNode | undefined | null | void> =
    this.emitter.event;

  private workspaces: Workspace[] = [];
  private activePath: string | undefined;
  private activeSession: string | undefined;
  private unsub: (() => void) | undefined;

  constructor(
    private readonly host: ArrowCoderHost,
    /** Open/resume a session in the chat panel. Supplied by extension.ts. */
    private readonly onOpenSession: (
      workspacePath: string,
      sessionId: string
    ) => void
  ) {
    this.unsub = this.host.onWorkspaceState((state) => {
      this.workspaces = state.workspaces;
      this.activePath = state.active_path;
      this.activeSession = state.active_session;
      this.emitter.fire();
    });
  }

  /** Ask the host for a fresh snapshot (e.g. after a manual refresh). */
  refresh(): void {
    this.host.sendRaw({ method: 'workspace/list' });
  }

  getTreeItem(element: WorkspaceNode): vscode.TreeItem {
    if (element.kind === 'workspace') {
      const item = new vscode.TreeItem(
        element.title,
        element.sessions.length > 0
          ? vscode.TreeItemCollapsibleState.Expanded
          : vscode.TreeItemCollapsibleState.Collapsed
      );
      item.contextValue = 'workspace';
      item.iconPath = new vscode.ThemeIcon('folder');
      item.description = element.path;
      item.tooltip = element.path;
      return item;
    }
    // session node
    const isActive =
      element.workspacePath === this.activePath &&
      element.session.id === this.activeSession;
    const label =
      element.session.title || `(untitled ${element.session.id.slice(0, 8)})`;
    const item = new vscode.TreeItem(
      label,
      vscode.TreeItemCollapsibleState.None
    );
    item.contextValue = 'session';
    item.iconPath = new vscode.ThemeIcon(
      isActive ? 'circle-filled' : 'circle-outline'
    );
    item.command = {
      command: 'arrowCoder.workspace.openSession',
      title: 'Open Session',
      arguments: [element.workspacePath, element.session.id],
    };
    if (isActive) {
      item.description = 'active';
    } else if (element.session.created_at) {
      item.description = new Date(
        element.session.created_at
      ).toLocaleDateString();
    }
    return item;
  }

  getChildren(element?: WorkspaceNode): WorkspaceNode[] {
    if (!element) {
      // Top level: workspace roots, sorted by most recently seen first.
      return [...this.workspaces]
        .sort((a, b) => (b.last_seen ?? 0) - (a.last_seen ?? 0))
        .map((w) => ({
          kind: 'workspace',
          path: w.path,
          title: w.title,
          sessions: w.sessions,
        }));
    }
    if (element.kind === 'workspace') {
      // Nested: the workspace's sessions, most recent first.
      return element.sessions.map((s) => ({
        kind: 'session',
        workspacePath: element.path,
        session: s,
      }));
    }
    return [];
  }

  dispose(): void {
    this.unsub?.();
    this.emitter.dispose();
  }
}
