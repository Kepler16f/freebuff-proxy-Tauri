# Agent Note: Tauri 桌面客户端（desktop/）——给本地裸机部署一个 GUI 入口

Status: implemented

## Problem

Web 控制台已是全功能 GUI，但裸机部署的**启动体验**是命令行仪式：解压 → 跑
`start.bat` → 黑窗口常驻 → 在滚动日志里翻首次生成的管理员密码 → 手动开浏览器。
用户明确反馈「每次这样用好麻烦」，并在托盘壳 / Electron / Tauri 三个候选里**主动选择了
Tauri 客户端**。

## Decision

新增 `desktop/`：Tauri 2 桌面壳，产物为 NSIS 安装包（`scripts/build-desktop.mjs` 一键重建）。

- **服务端代码零改动**：壳只是 `spawn node bin/serve.js` 的宿主。数据目录安装版走
  `%APPDATA%/<identifier>/data`（安装目录不可写），开发态就地用仓库 `data/`。
- **UI 复用 Web 控制台**：窗口先加载本地 splash（TCP 轮询 healthz），就绪后
  `location.replace(http://127.0.0.1:<port>/)`。远程源不注入 `window.__TAURI__`，
  控制台页面拿不到任何壳 API，攻击面与普通浏览器一致。
- **运行时随包分发**：`src-tauri/resources/server/`（构建期生成，gitignore）内置源码 +
  生产依赖 + `node.exe`，安装后完全自包含、离线可用。
- **首次密码引导**：捕获 stdout 中 `密码: <token>` 行（仅首次启动打印）→ 缓存 + 写剪贴板
  + 原生弹窗；托盘菜单可再次查看（仅本进程内有效，改密后以控制台为准）。
- **生命周期**：关窗 = 隐藏到托盘；托盘「退出」/ `RunEvent::Exit` = `taskkill /F /T` 收掉
  node 子树。强杀安全：服务端启动时的 `cleanupOrphanSessions` 会清扫遗留会话句柄。
- **端口与单实例**：启动前探 `/healthz`，已有健康实例 → 附着模式（不重复 spawn、不换端口），
  所以客户端与既有 zip/Docker 部署可共存；`tauri-plugin-single-instance` 保证二次启动只
  把已有窗口带到前台。端口可用 `FREEBUFF_PROXY_PORT` 覆盖（壳与子进程共用同一值）。
- **镜像零改动**：`.dockerignore` 排除 `desktop/`；运行时依赖、compose、Dockerfile 不动。
- **Tauri 2 ACL 陷阱（实测踩坑）**：手工搭工程漏掉 `src-tauri/capabilities/default.json`
  时，前端 `event.listen` 等核心 IPC 会被**静默拒绝**（reject 被 catch 吞掉），splash 永远
  转圈。必须带 `"permissions": ["core:default"]`。别用窗口标题判断"是否进入控制台"——
  配置的静态标题和页面 `<title>` 恰好相同，会造出假证据；要用 UIA 读页面内容验证。
- **splash 前进保证**：先注册事件监听再做初始查询 + 500ms 轮询 `get_server_state` 兜底
  （WebView 冷启动可能慢于服务就绪，只依赖事件会错过）；`failed` 状态位让轮询也能感知
  启动失败；`additionalBrowserArgs` 加 `--proxy-server=direct://`（webview 只访问
  127.0.0.1，强制直连避免系统代理拦截）。
- **记住密码并自动填充（用户需求，默认开启）**：首启密码捕获时持久化到壳层
  `<app_data_dir>/shell.json`（与服务端数据分离）；控制台页面 `on_page_load(Finished)`
  且 URL 命中本机端口时，先 `POST /api/auth/login` 验证记住的密码——**有效才注入填充**
  （改过密码就静默不再填，并清除记忆），填充用 `webview.eval` 注入自守卫脚本等
  `#login-user/#login-pass` 出现（app.js 异步渲染，跨域页面无 `__TAURI__`，只能走 eval）。
  托盘 `CheckMenuItem`「记住密码并自动填充」可关闭（关=忘记已存密码）。**附着外部实例
  时验证失败不清除记忆**（可能是别的实例的密码，不能误删自己实例的记忆）。
  验证手段：UIA ValuePattern 读字段值 + InvokePattern 真点「登 录」确认登录成功
  （密码框 UIA 返回掩码，但长度可判；窗口标题会骗人，页面内容不会）。

## Alternatives considered

- **什么都不做（复用 start.bat）**：零成本零维护，但启动仪式本身就是用户抱怨的对象，
  「不作为」直接违背本次需求来源 → 否。
- **C# 系统托盘小壳（csc.exe 编译，~250 行 50KB）**：最轻、零新工具链、托盘 + 密码气泡
  都能做；但无内嵌窗口（仍要跳浏览器），交互上限低，且用户在被告知 Rust 工具链成本后
  **明确选择**了 Tauri → 否（保留为最小依赖备选）。
- **Electron 壳**：生态最成熟、UI 控制力强；但要多分发 150MB+ Chromium 与完整 Node 双
  运行时，与「超级轻量」铁律冲突 → 否。
- **Tauri（采纳）**：产物 ~7MB、复用系统 WebView2（Win11 自带）、托盘/单实例/剪贴板/
  对话框插件齐全、NSIS 直接产出标准安装包。代价：引入 Rust 构建链（仅构建期依赖，
  运行期零新增）→ 采纳。

## Consequences

- 安装包 `FreebuffProxy_<version>_arm64-setup.exe` 自包含，用户「双击安装 → 开箱即用」，
  无需 Node/终端知识；升级 = 装新版本安装包（数据在 %APPDATA% 不受影响）。
- 本机构建需要 rustup + MSVC Build Tools（ARM64 机器要含 `VC.Tools.ARM64` 组件），
  构建入口统一收敛到 `scripts/build-desktop.mjs`。
- 壳与服务的契约只有三个：`bin/serve.js` 路径、healthz 端口、stdout 的密码行格式。
  服务端若改动这三者之一，必须同步 `desktop/src-tauri/src/main.rs`。
- 壳内 node 随包版本可能落后于系统 Node；升级客户端 = 重跑打包脚本重新分发。
