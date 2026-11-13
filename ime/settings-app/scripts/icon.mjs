import { mkdirSync, writeFileSync } from 'node:fs';
const dir = new URL('../src-tauri/icons/', import.meta.url);
mkdirSync(dir, { recursive: true });
// ICO: one 32×32, 32-bit DIB plus an empty AND mask.
const pixels = 32 * 32 * 4, mask = 32 * 4;
const out = Buffer.alloc(22 + 40 + pixels + mask);
out.writeUInt16LE(1, 2); out.writeUInt16LE(1, 4);
out[6] = 32; out[7] = 32; out.writeUInt16LE(1, 10); out.writeUInt16LE(32, 12);
out.writeUInt32LE(40 + pixels + mask, 14); out.writeUInt32LE(22, 18);
out.writeUInt32LE(40, 22); out.writeInt32LE(32, 26); out.writeInt32LE(64, 30);
out.writeUInt16LE(1, 34); out.writeUInt16LE(32, 36); out.writeUInt32LE(pixels, 42);
for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) {
  const p = 62 + (y * 32 + x) * 4;
  const ink = x >= 4 && x < 28 && (y === 9 || y === 10 || (x >= 15 && x <= 17 && y >= 8 && y < 25));
  out[p] = ink ? 20 : 255; out[p + 1] = ink ? 20 : 255; out[p + 2] = ink ? 20 : 255; out[p + 3] = 255;
}
writeFileSync(new URL('icon.ico', dir), out);
