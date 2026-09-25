# Freebuff Proxy Desktop v1.14.7

freebuff-proxy 的桌面客户端发布（上游 v1.14.7，版本号保持不变）。Tauri 壳 + 内嵌本地服务，
**安装即用、离线可用**（每个安装包内置对应平台的 node 运行时与服务端源码）。

## 下载

| 平台 | 文件 |
| --- | --- |
| Windows x64 | `FreebuffProxy_1.14.7_x64-setup.exe` |
| Windows ARM64 | `FreebuffProxy_1.14.7_arm64-setup.exe` |
| Linux x64 | `freebuff-proxy-desktop_1.14.7_amd64.deb` / `FreebuffProxy_1.14.7_amd64.AppImage` |
| Linux ARM64 | `freebuff-proxy-desktop_1.14.7_arm64.deb` / `FreebuffProxy_1.14.7_aarch64.AppImage` |
| macOS (Intel) | `FreebuffProxy_1.14.7_x64.dmg` |
| macOS (Apple Silicon) | `FreebuffProxy_1.14.7_aarch64.dmg` |

## 亮点

- 双击即用：托盘常驻，关窗最小化到托盘，打开即是 Web 控制台
- 首次启动的管理员密码自动复制到剪贴板并弹窗告知，登录框自动填充（托盘菜单可关）
- 端口被占时自动附着已有实例，可与 zip/Docker 部署共存
- 数据目录：Windows/macOS 在用户应用数据目录，Linux 在 `~/.local/share`；与服务端数据互不干涉

## 安装说明

- **Windows**：直接运行 setup.exe。SmartScreen 首次可能提示"未签名"——点「更多信息 → 仍要运行」。
- **macOS**：未做公证（无开发者证书），首次打开若被拦，执行
  `xattr -cr /Applications/FreebuffProxy.app` 或右键 → 打开。
- **Linux**：deb 直接安装；AppImage 需 `chmod +x` 后运行。

## 与上游的关系

服务端代码与上游 [HengXin666/freebuff-proxy](https://github.com/HengXin666/freebuff-proxy) v1.14.7 完全一致，
本仓库只增加了 `desktop/` 桌面壳与跨平台发布流水线。Docker 部署请用上游仓库。
