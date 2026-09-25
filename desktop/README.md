# desktop/ — Freebuff Proxy Windows 桌面客户端（Tauri）

给 Web 控制台套的桌面壳：托盘常驻 + 内嵌 WebView2 窗口，安装包自带服务端运行时
（源码 + 生产依赖 + node.exe），装完即用、离线可用。服务端代码零改动，镜像零改动。

- 决策记录：`.agents/notes/implemented/architecture/2026-09-25-tauri-desktop-shell.md`
- 产物：`src-tauri/target/release/bundle/nsis/FreebuffProxy_<版本>_aarch64-setup.exe`

## 构建（本机需要 Node ≥ 20 + rustup + MSVC Build Tools）

```sh
node scripts/build-desktop.mjs    # 仓库根目录执行，一键出安装包
```

脚本做四件事：注入版本号 → 组装 `src-tauri/resources/server/`（源码 + 生产依赖 +
node.exe，gitignored）→ 同步版本到 tauri.conf.json → `npx tauri build`。

## 结构

- `src/index.html` — 启动 splash（等 healthz → 跳转到控制台 URL）
- `src-tauri/src/main.rs` — 服务生命周期管理 / 密码捕获 / 托盘 / 单实例
- `src-tauri/resources/server/` — 构建时生成的服务端运行时（不提交）
- `build/make-icon.mjs` — 生成图标源图（纯几何 PNG）

## 行为要点

- 数据目录：安装版在 `%APPDATA%\com.freebuff.proxy.desktop\data`，开发态就地用仓库 `data/`
- 关窗 = 隐藏到托盘；托盘「退出」才会真正结束服务进程
- 端口默认 8787（`FREEBUFF_PROXY_PORT` 环境变量可覆盖，壳与子进程共用该值）
- 启动时若探测到已有健康实例（healthz OK），直接附着，不重复拉起
