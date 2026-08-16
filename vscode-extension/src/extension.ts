import * as vscode from 'vscode';
import { HostController } from './host/HostController';
import { ChatViewProvider } from './chatPanel';

let sharedHost: HostController | undefined;
let chatProvider: ChatViewProvider | undefined;

/** Lazily create the single shared host controller used by both sidebar views. */
function getHost(extensionUri: vscode.Uri): HostController {
  if (!sharedHost) {
    sharedHost = new HostController(extensionUri);
  }
  return sharedHost;
}

export function activate(context: vscode.ExtensionContext): void {
  const host = getHost(context.extensionUri);
  const provider = new ChatViewProvider(host, context.extensionUri);
  chatProvider = provider;
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(
      ChatViewProvider.viewType,
      provider,
      // Keep the webview's JS context alive when the user switches to another
      // sidebar view (e.g. git), so it does NOT reload and lose state (scroll,
      // open message blocks, draft, expanded panels) nor re-show "Resumed session".
      { webviewOptions: { retainContextWhenHidden: true } }
    )
  );

  // Native panel title-bar actions (render to the right of the view title).
  context.subscriptions.push(
    vscode.commands.registerCommand('arrowCoder.newSession', () => provider.newSession()),
    vscode.commands.registerCommand('arrowCoder.toggleHistory', () => provider.toggleHistory()),
    // Right-click "Add to Arrow Coder Chat": from the editor (selection or file)
    // or the explorer (file/folder). Forwards the reference to the webview draft.
    vscode.commands.registerCommand('arrowCoder.addToChat', (uri?: vscode.Uri) =>
      provider.addToChat(uri)
    )
  );

  // Start the agent automatically so the sidebar has live data immediately.
  const cfg = vscode.workspace.getConfiguration('arrowCoder');
  host
    .start({
      agent: cfg.get<string>('server.agent', 'default'),
      autoApprove: cfg.get<boolean>('server.autoApprove', false),
      resume: null,
      fresh: false,
    })
    .catch((err) => {
      vscode.window.showErrorMessage(`Arrow Coder failed to start: ${err.message}`);
    });
}

export function deactivate(): void {
  sharedHost?.stop();
  sharedHost = undefined;
  chatProvider = undefined;
}
