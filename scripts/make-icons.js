'use strict';
// Draws the app icon (cell bars on an orange tile) as PNGs. Run: npm run icons
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

function png(size, draw) {
  const raw = Buffer.alloc((size * 4 + 1) * size);
  const ss = 4; // supersampling for smooth edges
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < ss; sy++) for (let sx = 0; sx < ss; sx++) {
        const c = draw((x + (sx + 0.5) / ss) / size, (y + (sy + 0.5) / ss) / size);
        if (c) { r += c[0] * c[3]; g += c[1] * c[3]; b += c[2] * c[3]; a += c[3]; }
      }
      const o = y * (size * 4 + 1) + 1 + x * 4;
      const n = ss * ss;
      raw[o] = a ? Math.round(r / a) : 0;
      raw[o + 1] = a ? Math.round(g / a) : 0;
      raw[o + 2] = a ? Math.round(b / a) : 0;
      raw[o + 3] = Math.round((a / n) * 255);
    }
  }
  const chunk = (type, data) => {
    const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4); crc.writeUInt32BE(zlib.crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
}

function inRoundRect(x, y, x0, y0, x1, y1, r) {
  if (x < x0 || x > x1 || y < y0 || y > y1) return false;
  const cx = Math.min(Math.max(x, x0 + r), x1 - r);
  const cy = Math.min(Math.max(y, y0 + r), y1 - r);
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
}

function bars(x, y) {
  const top = 0.24, bottom = 0.76, rail = 0.065;
  if (x >= 0.22 && x <= 0.78 && ((y >= top && y <= top + rail) || (y >= bottom - rail && y <= bottom))) return true;
  if (y < top || y > bottom) return false;
  for (const cx of [0.305, 0.435, 0.565, 0.695]) if (Math.abs(x - cx) <= 0.032) return true;
  return false;
}

const tile = (x, y) => {
  if (!inRoundRect(x, y, 0.04, 0.04, 0.96, 0.96, 0.2)) return null;
  if (bars(x, y)) return [255, 247, 238, 1];
  const t = y; // gentle vertical gradient
  return [Math.round(226 - 30 * t), Math.round(108 - 28 * t), Math.round(46 - 12 * t), 1];
};
// Tray: bars on a dark rounded tile so it reads on light and dark taskbars.
const tray = (x, y) => {
  if (!inRoundRect(x, y, 0.02, 0.02, 0.98, 0.98, 0.22)) return null;
  if (bars(x, y)) return [255, 255, 255, 1];
  return [214, 96, 40, 1];
};
// macOS menu bar template: black bars only, the system tints them.
const template = (x, y) => (bars(x, y) || (inRoundRect(x, y, 0.1, 0.1, 0.9, 0.9, 0.18) && !inRoundRect(x, y, 0.17, 0.17, 0.83, 0.83, 0.12)) ? [0, 0, 0, 1] : null);

const out = path.join(__dirname, '..', 'assets');
fs.mkdirSync(out, { recursive: true });
for (const s of [64, 256, 512]) fs.writeFileSync(path.join(out, `icon-${s}.png`), png(s, tile));
fs.writeFileSync(path.join(out, 'tray.png'), png(32, tray));
fs.writeFileSync(path.join(out, 'tray-mac.png'), png(22, template));
fs.writeFileSync(path.join(out, 'tray-mac@2x.png'), png(44, template));
fs.mkdirSync(path.join(__dirname, '..', 'build'), { recursive: true });
fs.copyFileSync(path.join(out, 'icon-512.png'), path.join(__dirname, '..', 'build', 'icon.png'));
console.log('icons written to', out);
