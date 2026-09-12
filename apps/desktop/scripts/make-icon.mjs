// Generates the default monochrome brand mark as a 1024x1024 PNG with no
// dependencies (Node's zlib only). The owner will supply the final logo;
// until then this is the mark: a white "hash" glyph on black, rounded.
//
//   node scripts/make-icon.mjs  ->  src-tauri/app-icon.png
//   pnpm tauri icon src-tauri/app-icon.png
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(here, "../src-tauri/app-icon.png");
const S = 1024;

const crcTable = new Int32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  crcTable[n] = c;
}
const crc32 = (buf) => {
  let c = -1;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
};

// Geometry, in a 1024 grid.
const radius = 224; // rounded square corner
const inset = 0; // the mark fills the tile; Windows adds its own padding
const bar = 96; // stroke width
const v1 = 352, v2 = 576; // vertical bar x positions (left edges)
const h1 = 352, h2 = 576; // horizontal bar y positions (top edges)
const span0 = 224, span1 = 800; // bar extent

const insideRounded = (x, y) => {
  const l = inset, t = inset, r = S - inset, b = S - inset;
  if (x < l || y < t || x >= r || y >= b) return false;
  const cx = x < l + radius ? l + radius : x >= r - radius ? r - radius - 1 : x;
  const cy = y < t + radius ? t + radius : y >= b - radius ? b - radius - 1 : y;
  const dx = x - cx, dy = y - cy;
  return dx * dx + dy * dy <= radius * radius;
};
const insideMark = (x, y) => {
  const inV = (x >= v1 && x < v1 + bar) || (x >= v2 && x < v2 + bar);
  const inH = (y >= h1 && y < h1 + bar) || (y >= h2 && y < h2 + bar);
  const inSpanY = y >= span0 && y < span1;
  const inSpanX = x >= span0 && x < span1;
  return (inV && inSpanY) || (inH && inSpanX);
};

const raw = Buffer.alloc((S * 4 + 1) * S);
for (let y = 0; y < S; y++) {
  raw[y * (S * 4 + 1)] = 0; // filter: none
  for (let x = 0; x < S; x++) {
    const o = y * (S * 4 + 1) + 1 + x * 4;
    if (!insideRounded(x, y)) {
      raw[o] = 0; raw[o + 1] = 0; raw[o + 2] = 0; raw[o + 3] = 0;
    } else if (insideMark(x, y)) {
      raw[o] = 255; raw[o + 1] = 255; raw[o + 2] = 255; raw[o + 3] = 255;
    } else {
      raw[o] = 0; raw[o + 1] = 0; raw[o + 2] = 0; raw[o + 3] = 255;
    }
  }
}
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, png);
console.log(`wrote ${out} (${png.length} bytes)`);
