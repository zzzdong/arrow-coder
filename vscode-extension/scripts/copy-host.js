// Copies the freshly-built arrow-coder-vscode host binary into the extension's
// `bin/<platform>-<arch>/` directory so that Tier 2 of the host resolution
// (bundled binary) works both during local F5 development and in the packaged
// .vsix.
//
// The `bin/<platform>-<arch>/` layout matches the platform-specific package
// convention (vsce --target), e.g. `bin/win32-x64/arrow-coder-vscode.exe`.
// `host.ts` probes this exact path during Tier 2 resolution.
//
// Usage: node scripts/copy-host.js
// Expects the cargo workspace to live one level above this extension dir.
const fs = require('fs');
const path = require('path');

// Map Node's process.platform/arch to the VS Code --target tuple form
// (e.g. win32-x64, darwin-arm64, linux-x64).
const platformMap = { win32: 'win32', darwin: 'darwin', linux: 'linux' };
const archMap = { x64: 'x64', arm64: 'arm64', ia32: 'ia32' };
const targetPlatform = platformMap[process.platform] || process.platform;
const targetArch = archMap[process.arch] || process.arch;
const targetDir = `${targetPlatform}-${targetArch}`;

const extDir = __dirname.replace(/[\\/]scripts$/, '');
const wsRoot = path.dirname(extDir); // cargo workspace root
const exe = process.platform === 'win32' ? 'arrow-coder-vscode.exe' : 'arrow-coder-vscode';
const src = path.join(wsRoot, 'target', 'debug', exe);
const destDir = path.join(extDir, 'bin', targetDir);
const dest = path.join(destDir, exe);

if (!fs.existsSync(src)) {
  console.error(`[copy-host] source binary not found: ${src}\nRun \`cargo build -p arrow-coder-vscode\` first.`);
  process.exit(1);
}

fs.mkdirSync(destDir, { recursive: true });
fs.copyFileSync(src, dest);
console.log(`[copy-host] copied ${src} -> ${dest}`);
