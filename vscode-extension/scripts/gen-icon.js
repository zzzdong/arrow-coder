// Generates a minimal 128x128 PNG icon at assets/icon.png.
// Pure Node (no deps) using zlib for the IDAT deflate. Run once:
//   node scripts/gen-icon.js
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

const W = 128;
const H = 128;

// Build raw RGBA pixels: dark blue background with a lighter arrow (chevron).
function px(x, y) {
  // Arrow: a right-pointing chevron centered.
  const cx = 64;
  const cy = 64;
  // simple ">" shape made of two thick strokes
  const inArrow =
    // upper stroke
    (y >= cy - 30 && y <= cy - 10 && x >= cx - 30 + (y - (cy - 30)) && x <= cx + 10) ||
    // lower stroke
    (y >= cy + 10 && y <= cy + 30 && x >= cx - 30 + (cy + 30 - y) && x <= cx + 10) ||
    // shaft
    (y >= cy - 6 && y <= cy + 6 && x >= cx - 35 && x <= cx + 5);
  if (inArrow) return [120, 200, 255, 255]; // light blue
  return [30, 41, 59, 255]; // slate background
}

const raw = Buffer.alloc(H * (1 + W * 4));
let o = 0;
for (let y = 0; y < H; y++) {
  raw[o++] = 0; // filter type 0
  for (let x = 0; x < W; x++) {
    const [r, g, b, a] = px(x, y);
    raw[o++] = r; raw[o++] = g; raw[o++] = b; raw[o++] = a;
  }
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, 'ascii');
  const crcBuf = Buffer.alloc(4);
  // CRC32 of type + data
  const crc = crc32(Buffer.concat([typeBuf, data]));
  crcBuf.writeUInt32BE(crc >>> 0, 0);
  return Buffer.concat([len, typeBuf, data, crcBuf]);
}

function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c;
}

const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;

const idat = zlib.deflateSync(raw);
const png = Buffer.concat([
  sig,
  chunk('IHDR', ihdr),
  chunk('IDAT', idat),
  chunk('IEND', Buffer.alloc(0)),
]);

const outDir = path.join(__dirname, '..', 'assets');
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, 'icon.png'), png);
console.log(`[gen-icon] wrote ${path.join(outDir, 'icon.png')} (${png.length} bytes)`);
