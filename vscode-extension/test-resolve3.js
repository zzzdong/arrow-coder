const fs = require('fs');
const path = require('path');
const vscode = { Uri: { file: (p) => ({ fsPath: p }) } };

function resolveHostBinary(configured, extensionUri) {
  const exe = process.platform === 'win32' ? '.exe' : '';
  const name = `arrow-coder-vscode${exe}`;

  if (configured && configured.trim()) {
    const trimmed = configured.trim();
    if (path.isAbsolute(trimmed) && fs.existsSync(trimmed) && fs.statSync(trimmed).isFile()) return trimmed;
    if (path.isAbsolute(trimmed) && fs.existsSync(trimmed) && fs.statSync(trimmed).isDirectory()) {
      const inside = path.join(trimmed, name);
      if (fs.existsSync(inside)) return inside;
    }
    if (!path.isAbsolute(trimmed) && (trimmed.includes('/') || trimmed.includes('\\'))) {
      const base = extensionUri ? extensionUri.fsPath : __dirname;
      const resolved = path.resolve(base, trimmed);
      if (fs.existsSync(resolved)) return fs.statSync(resolved).isDirectory() ? path.join(resolved, name) : resolved;
    }
  }

  const extDir = extensionUri ? extensionUri.fsPath : __dirname;
  const platform = process.platform;
  const extCandidates = [
    path.join(extDir, 'bin', name),
    path.join(extDir, 'bin', platform, name),
  ];
  for (const c of extCandidates) {
    if (fs.existsSync(c)) return c;
  }
  return 'arrow-coder-vscode';
}

console.log('Tier1 abs file :', resolveHostBinary('D:\\code\\rust\\arrow-coder\\target\\debug\\arrow-coder-vscode.exe', undefined));
console.log('Tier1 abs dir  :', resolveHostBinary('D:\\code\\rust\\arrow-coder\\target\\debug', undefined));
console.log('Tier2 bin/     :', resolveHostBinary('arrow-coder-vscode', vscode.Uri.file(path.resolve('.'))));
console.log('Tier2 platform :', resolveHostBinary('arrow-coder-vscode', vscode.Uri.file(path.resolve('.'))));
