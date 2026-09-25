// 生成 1024×1024 应用图标源图：深蓝圆角底 + 青色闪电（纯像素几何，无外部依赖）
// 产物：build/icon-src.png，之后用 `npx tauri icon` 派生各平台尺寸。
import fs from 'node:fs'
import path from 'node:path'
import zlib from 'node:zlib'
import { fileURLToPath } from 'node:url'

const SIZE = 1024
const RADIUS = 200
const BG = [15, 23, 42] // #0F172A
const BOLT = [34, 211, 238] // #22D3EE
// 闪电多边形（1024 网格内取点）
const POLY = [
  [640, 120],
  [330, 580],
  [505, 580],
  [415, 905],
  [710, 445],
  [520, 445],
]

function inRoundedRect(x, y) {
  const cx = Math.min(Math.max(x, RADIUS), SIZE - RADIUS)
  const cy = Math.min(Math.max(y, RADIUS), SIZE - RADIUS)
  const dx = x - cx
  const dy = y - cy
  return dx * dx + dy * dy <= RADIUS * RADIUS
}

function inPolygon(x, y) {
  let inside = false
  for (let i = 0, j = POLY.length - 1; i < POLY.length; j = i++) {
    const [xi, yi] = POLY[i]
    const [xj, yj] = POLY[j]
    const intersect =
      yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi
    if (intersect) inside = !inside
  }
  return inside
}

const raw = Buffer.alloc(SIZE * (1 + SIZE * 4))
let off = 0
for (let y = 0; y < SIZE; y++) {
  raw[off++] = 0 // filter: none
  for (let x = 0; x < SIZE; x++) {
    let [r, g, b, a] = [0, 0, 0, 0]
    if (inRoundedRect(x + 0.5, y + 0.5)) {
      ;[r, g, b, a] = [...BG, 255]
      if (inPolygon(x + 0.5, y + 0.5)) [r, g, b] = BOLT
    }
    raw[off++] = r
    raw[off++] = g
    raw[off++] = b
    raw[off++] = a
  }
}

const CRC_TABLE = new Int32Array(256).map((_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c
})
function crc32(buf) {
  let c = -1
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8)
  return (c ^ -1) >>> 0
}
function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body))
  return Buffer.concat([len, body, crc])
}

const ihdr = Buffer.alloc(13)
ihdr.writeUInt32BE(SIZE, 0)
ihdr.writeUInt32BE(SIZE, 4)
ihdr[8] = 8 // bit depth
ihdr[9] = 6 // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
])

const out = path.join(path.dirname(fileURLToPath(import.meta.url)), 'icon-src.png')
fs.writeFileSync(out, png)
console.log(`[icon] wrote ${out} (${(png.length / 1024).toFixed(0)} KB)`)
