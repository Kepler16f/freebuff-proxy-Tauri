#!/usr/bin/env node
// 组装随桌面客户端分发的服务端运行时（desktop/src-tauri/resources/server/）：
//   源码 + 生产依赖 + 目标平台的 node 运行时。
// 由 tauri.conf.json 的 beforeBuildCommand 调用，本地构建与 CI 共用同一入口。
// 目标平台来自 TAURI_ENV_TARGET_TRIPLE（tauri v2 钩子注入），缺省用宿主机。
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { execSync, spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const DESKTOP = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const ROOT = path.resolve(DESKTOP, '..')
const STAGE = path.join(DESKTOP, 'src-tauri', 'resources', 'server')
const NODE_VERSION = 'v24.19.0'
const NPM_FLAGS = '--omit=dev --no-audit --no-fund --allow-remote=all'

const TRIPLE_MAP = {
  'x86_64-pc-windows-msvc': { dist: 'win-x64', archive: 'zip' },
  'aarch64-pc-windows-msvc': { dist: 'win-arm64', archive: 'zip' },
  'x86_64-unknown-linux-gnu': { dist: 'linux-x64', archive: 'tar.gz' },
  'aarch64-unknown-linux-gnu': { dist: 'linux-arm64', archive: 'tar.gz' },
  'x86_64-apple-darwin': { dist: 'darwin-x64', archive: 'tar.gz' },
  'aarch64-apple-darwin': { dist: 'darwin-arm64', archive: 'tar.gz' },
}

function hostTarget() {
  const a = os.arch() // arm64 / x64
  const arch = a === 'x64' ? 'x64' : a
  if (process.platform === 'win32') return { dist: `win-${arch}`, archive: 'zip' }
  if (process.platform === 'linux') return { dist: `linux-${arch}`, archive: 'tar.gz' }
  if (process.platform === 'darwin') return { dist: `darwin-${arch}`, archive: 'tar.gz' }
  throw new Error(`unsupported host platform: ${process.platform}`)
}

const triple = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.TAURI_PLATFORM_TRIPLE || ''
const target = TRIPLE_MAP[triple] || hostTarget()
console.log(`[runtime] target triple="${triple || '(host)'}" → node ${NODE_VERSION} ${target.dist}`)

// 1) 组装服务端源码 + 生产依赖
fs.rmSync(path.join(DESKTOP, 'src-tauri', 'resources'), { recursive: true, force: true })
fs.mkdirSync(STAGE, { recursive: true })
execSync('node scripts/inject-version.mjs', { cwd: ROOT, stdio: 'inherit' })
for (const item of ['bin', 'src', 'dashboard', 'package.json', 'package-lock.json']) {
  fs.cpSync(path.join(ROOT, item), path.join(STAGE, item), { recursive: true })
}
execSync(`npm ci ${NPM_FLAGS}`, { cwd: STAGE, stdio: 'inherit' })

// 2) 下载目标平台的 node 运行时
const name = `node-${NODE_VERSION}-${target.dist}`
const archive = `${name}.${target.archive}`
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'fbp-node-'))
const urls = [
  `https://nodejs.org/dist/${NODE_VERSION}/${archive}`,
  `https://registry.npmmirror.com/-/binary/node/${NODE_VERSION}/${archive}`,
]
const dest = path.join(tmp, archive)
let downloaded = false
for (const url of urls) {
  try {
    console.log(`[runtime] downloading ${url}`)
    execSync(`curl -fsSL --retry 3 --connect-timeout 20 --max-time 300 -o "${dest}" "${url}"`, { stdio: 'inherit' })
    if (fs.statSync(dest).size > 1024 * 1024) { downloaded = true; break }
  } catch (e) {
    console.warn(`[runtime] failed: ${e.message?.split('\n')[0]}`)
  }
}
if (!downloaded) throw new Error(`node runtime download failed for ${target.dist}`)

// 3) 解包并取出 node 可执行文件
if (target.archive === 'zip') {
  if (process.platform === 'win32') {
    spawnSync('powershell', ['-NoProfile', '-Command',
      `Expand-Archive -Path '${dest}' -DestinationPath '${tmp}/x' -Force`], { stdio: 'inherit' })
  } else {
    execSync(`tar -xf "${dest}" -C "${tmp}/x"`)
  }
  fs.copyFileSync(path.join(tmp, 'x', name, 'node.exe'), path.join(STAGE, 'node.exe'))
} else {
  fs.mkdirSync(path.join(tmp, 'x'), { recursive: true })
  execSync(`tar -xzf "${dest}" -C "${tmp}/x"`)
  fs.copyFileSync(path.join(tmp, 'x', name, 'bin', 'node'), path.join(STAGE, 'node'))
  try { fs.copyFileSync(path.join(tmp, 'x', name, 'LICENSE'), path.join(STAGE, 'node.LICENSE')) } catch {}
  fs.chmodSync(path.join(STAGE, 'node'), 0o755)
}
fs.rmSync(tmp, { recursive: true, force: true })
console.log(`[runtime] staged → ${STAGE} (${(fs.statSync(STAGE).size / 1024 / 1024).toFixed(0)} MB+)`)
