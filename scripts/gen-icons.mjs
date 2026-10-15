#!/usr/bin/env node
// resources/icon.svg(48px 이상) · icon-small.svg(16~32px)로 플랫폼별 아이콘을 만든다.
//   resources/icon.png        512×512 (Linux·electron-builder 기본)
//   resources/icons/<n>.png   16·24·32·48·64·128·256·512·1024
//   resources/icon.ico        Windows(16~256 묶음 — 작은 크기는 단순판)
//   resources/icon.icns       macOS
// 사용: node scripts/gen-icons.mjs
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { Resvg } from '@resvg/resvg-js'
import png2icons from 'png2icons'

const RES = resolve(import.meta.dirname, '..', 'resources')
const big = readFileSync(resolve(RES, 'icon.svg'))
const small = readFileSync(resolve(RES, 'icon-small.svg'))

/** 작은 크기는 단순판을 쓴다(가지가 뭉개지지 않게) */
const render = (size) =>
  new Resvg(size <= 32 ? small : big, { fitTo: { mode: 'width', value: size }, shapeRendering: 2 }).render().asPng()

mkdirSync(resolve(RES, 'icons'), { recursive: true })
const sizes = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
const pngs = Object.fromEntries(sizes.map((s) => [s, render(s)]))
for (const s of sizes) writeFileSync(resolve(RES, 'icons', `${s}.png`), pngs[s])
writeFileSync(resolve(RES, 'icon.png'), pngs[512])

// ICO: png2icons 는 한 장의 원본에서 크기를 줄여 만든다 — 원본은 256(상세판).
// 작은 크기도 단순판이 들어가도록 직접 ICO 를 조립한다(PNG 압축 항목은 Vista+ 에서 모든 크기 지원).
const icoSizes = [16, 24, 32, 48, 64, 128, 256]
const header = Buffer.alloc(6)
header.writeUInt16LE(0, 0)
header.writeUInt16LE(1, 2)
header.writeUInt16LE(icoSizes.length, 4)
const dir = Buffer.alloc(16 * icoSizes.length)
let offset = 6 + dir.length
const images = []
icoSizes.forEach((s, i) => {
  const png = pngs[s] ?? render(s)
  const e = i * 16
  dir.writeUInt8(s >= 256 ? 0 : s, e)
  dir.writeUInt8(s >= 256 ? 0 : s, e + 1)
  dir.writeUInt8(0, e + 2)
  dir.writeUInt8(0, e + 3)
  dir.writeUInt16LE(1, e + 4)
  dir.writeUInt16LE(32, e + 6)
  dir.writeUInt32LE(png.length, e + 8)
  dir.writeUInt32LE(offset, e + 12)
  offset += png.length
  images.push(png)
})
writeFileSync(resolve(RES, 'icon.ico'), Buffer.concat([header, dir, ...images]))

const icns = png2icons.createICNS(pngs[1024], png2icons.BICUBIC2, 0)
if (!icns) throw new Error('icns 생성 실패')
writeFileSync(resolve(RES, 'icon.icns'), icns)
console.log(`icons written to ${RES}`)
