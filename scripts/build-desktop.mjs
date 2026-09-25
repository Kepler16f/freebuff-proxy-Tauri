#!/usr/bin/env node
// 一键构建桌面客户端安装包：注入版本号 → npx tauri build
// （资源组装与目标平台 node 运行时下载已统一收敛到
//   tauri.conf.json 的 beforeBuildCommand → scripts/prepare-desktop-runtime.mjs）
// 用法：node scripts/build-desktop.mjs   （在仓库根目录执行）
import fs from 'node:fs'
import path from 'node:path'
import { execSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), '..')
const DESKTOP = path.join(ROOT, 'desktop')

const pkg = JSON.parse(fs.readFileSync(path.join(ROOT, 'package.json'), 'utf8'))
const version = pkg.version
console.log(`[build-desktop] version ${version}`)

const run = (cmd, opts = {}) =>
  execSync(cmd, { stdio: 'inherit', cwd: ROOT, ...opts })

// 1) 注入前端版本徽章（与发版流水线同一脚本）
run('node scripts/inject-version.mjs')

// 2) 同步版本号到 Tauri 配置与桌面 package.json
const confPath = path.join(DESKTOP, 'src-tauri', 'tauri.conf.json')
const conf = JSON.parse(fs.readFileSync(confPath, 'utf8'))
conf.version = version
fs.writeFileSync(confPath, JSON.stringify(conf, null, 2) + '\n')
const dPkgPath = path.join(DESKTOP, 'package.json')
const dPkg = JSON.parse(fs.readFileSync(dPkgPath, 'utf8'))
dPkg.version = version
fs.writeFileSync(dPkgPath, JSON.stringify(dPkg, null, 2) + '\n')

// 3) 编译安装包（bundle 目标按平台自动选择：nsis/deb/appimage/dmg）
run('npx tauri build', { cwd: DESKTOP })
console.log('[build-desktop] done → desktop/src-tauri/target/release/bundle/')
