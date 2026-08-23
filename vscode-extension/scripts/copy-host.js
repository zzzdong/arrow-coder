// Copies a freshly-built arrow-coder-vscode host binary into the extension's
// `bin/` directory (a single, platform-agnostic location) so that Tier 2 of
// the host resolution (bundled binary) works both during local F5 development
// and in the packaged .vsix.
//
// All platform builds land in the SAME `bin/` directory — there is no
// `bin/<platform>-<arch>/` split anymore. The `--target` option only controls
// WHICH cargo build output to copy *from* (the source cargo target dir); the
// destination is always `bin/<exe>`.
//
// Usage:
//   node scripts/copy-host.js [--target win32-x64|linux-x64|darwin-arm64]
//                             [--release] [--profile <cargo-profile>]
//
// When --target is omitted, it falls back to the current Node platform/arch.
// When --release is given, the binary is read from target/<rust-target>/release
// instead of target/debug. The Rust target triple is derived from the
// vsce --target tuple (win32-x64 -> x86_64-pc-windows-msvc, etc.).

const fs = require('fs');
const path = require('path');

function parseArgs(argv) {
  const args = { target: undefined, release: false, profile: undefined };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--target') args.target = argv[++i];
    else if (a === '--release') args.release = true;
    else if (a === '--profile') args.profile = argv[++i];
  }
  return args;
}

// vsce --target tuple -> Rust cargo target triple
const rustTargetMap = {
  'win32-x64': 'x86_64-pc-windows-msvc',
  'win32-ia32': 'i686-pc-windows-msvc',
  'win32-arm64': 'aarch64-pc-windows-msvc',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'linux-arm64': 'aarch64-unknown-linux-gnu',
  'darwin-x64': 'x86_64-apple-darwin',
  'darwin-arm64': 'aarch64-apple-darwin',
};

// Node platform/arch -> vsce --target tuple
const platformMap = { win32: 'win32', darwin: 'darwin', linux: 'linux' };
const archMap = { x64: 'x64', arm64: 'arm64', ia32: 'ia32' };

const args = parseArgs(process.argv.slice(2));

const targetTuple =
  args.target ||
  `${platformMap[process.platform] || process.platform}-${archMap[process.arch] || process.arch}`;

const rustTarget = rustTargetMap[targetTuple];
if (!rustTarget) {
  console.error(`[copy-host] unknown target tuple: ${targetTuple}`);
  process.exit(1);
}

const extDir = __dirname.replace(/[\\/]scripts$/, '');
const wsRoot = path.dirname(extDir); // cargo workspace root

const exe = targetTuple.startsWith('win32') ? 'arrow-coder-vscode.exe' : 'arrow-coder-vscode';

// Profile resolution: explicit --profile wins, otherwise --release -> "release".
const cargoProfile = args.profile || (args.release ? 'release' : 'debug');
const srcDir = path.join(wsRoot, 'target', rustTarget, cargoProfile);
const src = path.join(srcDir, exe);

// All platform builds share a single `bin/` directory (no `<platform>-<arch>`
// subfolder). `--target` only selects the source cargo output dir above.
const destDir = path.join(extDir, 'bin');
const dest = path.join(destDir, exe);

if (!fs.existsSync(src)) {
  console.error(
    `[copy-host] source binary not found: ${src}\n` +
      `Run \`cargo build -p arrow-coder-vscode --target ${rustTarget}${cargoProfile === 'release' ? ' --release' : ''}\` first.`
  );
  process.exit(1);
}

fs.mkdirSync(destDir, { recursive: true });
fs.copyFileSync(src, dest);
console.log(`[copy-host] copied ${src} -> ${dest}`);
