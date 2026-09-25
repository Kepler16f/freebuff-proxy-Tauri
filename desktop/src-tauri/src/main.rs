#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Tauri 桌面壳：托盘常驻 + 内嵌控制台。
// 决策记录：.agents/notes/implemented/architecture/2026-09-25-tauri-desktop-shell.md
// 关键约束：服务端代码零改动；镜像零改动（desktop/ 已被 .dockerignore 排除）。

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, Wry, WindowEvent};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const DEFAULT_PORT: u16 = 8787;
/// 200ms × 150 = 30s：等 node 监听的预算（服务端在监听后才会碰上游，起得很快）
const HEALTHZ_MAX_TRIES: u32 = 150;

struct AppState {
    child: Mutex<Option<Child>>,
    server_root: PathBuf,
    data_dir: PathBuf,
    settings_path: PathBuf,
    port: u16,
    /// 服务不是本进程拉起的（启动时外部已有健康实例）
    attached: Mutex<bool>,
    ready: Mutex<bool>,
    /// start_server 失败（超时/子进程退出/无法拉起），splash 轮询据此提示
    failed: Mutex<bool>,
    admin_password: Mutex<Option<String>>,
    stderr_tail: Mutex<Vec<String>>,
    /// 壳层设置（shell.json）：记住密码并自动填充的开关与记住的密码
    settings: Mutex<ShellSettings>,
    /// 托盘菜单里需要动态改文案/可用性的项
    ui: Mutex<Option<UiItems>>,
}

/// 壳层设置，持久化在 <app_data_dir>/shell.json（与服务端数据互不干涉）
#[derive(Serialize, Deserialize, Clone)]
struct ShellSettings {
    autofill: bool,
    admin_password: Option<String>,
}

impl Default for ShellSettings {
    fn default() -> Self {
        Self {
            autofill: true,
            admin_password: None,
        }
    }
}

fn load_settings(path: &std::path::Path) -> ShellSettings {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(path: &std::path::Path, s: &ShellSettings) {
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = fs::write(path, json);
    }
}

struct UiItems {
    status: MenuItem<Wry>,
    show_password: MenuItem<Wry>,
    autofill: CheckMenuItem<Wry>,
}

#[derive(Serialize, Clone)]
struct ReadyPayload {
    port: u16,
}

#[derive(Serialize, Clone)]
struct FailedPayload {
    detail: String,
}

#[derive(Serialize)]
struct ServerStatePayload {
    ready: bool,
    attached: bool,
    failed: bool,
    port: u16,
    password: Option<String>,
}

fn healthz_ok(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(900)));
    let req = "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = String::new();
    let _ = BufReader::new(stream).read_to_string(&mut buf);
    buf.contains("200") && buf.contains("ok")
}

fn port_in_use(port: u16) -> bool {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// 首次启动日志形如 `  密码:     IT3Z...`；只认「密码:」后跟无空白 token 的行。
fn extract_password(line: &str) -> Option<String> {
    let idx = line.find("密码:")?;
    let rest = line[idx + "密码:".len()..].trim();
    let token = rest.split_whitespace().next()?;
    if token.len() >= 6 && token.chars().any(|c| c.is_ascii_alphanumeric()) {
        Some(token.to_string())
    } else {
        None
    }
}

fn resolve_layout(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    // 安装版：资源目录 <安装目录>/resources/server；数据放 %APPDATA%（Program Files 不可写）
    if let Ok(rd) = app.path().resource_dir() {
        let root = rd.join("resources").join("server");
        if root.join("bin").join("serve.js").exists() {
            let data = app
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())?
                .join("data");
            return Ok((root, data));
        }
    }
    // 开发态：src-tauri 的上两级 = 仓库根目录，数据就地放仓库 data/（与裸机跑一致）
    let dev_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    if dev_root.join("bin").join("serve.js").exists() {
        let data = dev_root.join("data");
        return Ok((dev_root, data));
    }
    Err("找不到服务端文件（bin/serve.js），安装可能不完整。".to_string())
}

fn resolve_node(server_root: &std::path::Path) -> PathBuf {
    // 安装包内置 node 运行时（随资源分发）；找不到再退回系统 PATH
    let bundled_exe = server_root.join("node.exe");
    if bundled_exe.exists() {
        return bundled_exe;
    }
    let bundled_unix = server_root.join("node");
    if bundled_unix.exists() {
        // tar/deb 打包可能丢掉可执行位，启动前补上（失败无害）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&bundled_unix, std::fs::Permissions::from_mode(0o755));
        }
        return bundled_unix;
    }
    PathBuf::from("node")
}

fn set_status(app: &AppHandle, text: &str) {
    let state = app.state::<AppState>();
    let ui = state.ui.lock().unwrap();
    if let Some(items) = ui.as_ref() {
        let _ = items.status.set_text(text);
    }
}

fn fail(app: &AppHandle, detail: String) {
    set_status(app, "启动失败");
    {
        let state = app.state::<AppState>();
        *state.failed.lock().unwrap() = true;
    }
    let _ = app.emit("server://failed", FailedPayload { detail: detail.clone() });
    let _ = app
        .dialog()
        .message(detail)
        .title("Freebuff Proxy 启动失败")
        .show(|_| {});
}

/// 向本地服务验证记住的密码是否仍然有效（用户改过密码就不再填充）
enum LoginCheck {
    Ok,
    Invalid,
    Unreachable,
}

fn verify_login(port: u16, password: &str) -> LoginCheck {
    let body = serde_json::json!({ "username": "admin", "password": password }).to_string();
    let Ok(mut stream) = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(1500),
    ) else {
        return LoginCheck::Unreachable;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(3000)));
    let req = format!(
        "POST /api/auth/login HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return LoginCheck::Unreachable;
    }
    let mut buf = String::new();
    let _ = BufReader::new(stream).read_to_string(&mut buf);
    if buf.starts_with("HTTP/1.1 200") {
        LoginCheck::Ok
    } else if buf.contains(" 401 ") || buf.contains(" 403 ") {
        LoginCheck::Invalid
    } else {
        LoginCheck::Unreachable
    }
}

/// 控制台页加载完成后注入：等登录表单出现（app.js 异步渲染）就填一次。
/// 已登录（无表单）时静默超时；只填空字段，不覆盖用户正在输入的内容。
fn autofill_js(password: &str) -> String {
    let pw = serde_json::to_string(password).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(function() {{
  if (window.__fbpAutofill) return;
  window.__fbpAutofill = true;
  var PW = {pw};
  var tries = 0;
  var timer = setInterval(function() {{
    tries++;
    var user = document.getElementById('login-user');
    var pass = document.getElementById('login-pass');
    if (pass) {{
      if (!pass.value) pass.value = PW;
      if (user && !user.value) user.value = 'admin';
      clearInterval(timer);
    }} else if (tries > 40) {{ clearInterval(timer); }}
  }}, 500);
}})();"#
    )
}

/// 控制台页面加载完成：验证记住的密码 → 有效就注入填充，失效（改过密码）就清除记忆
fn handle_console_loaded(webview: &tauri::Webview<Wry>) {
    let state = webview.state::<AppState>();
    let port = state.port;
    let attached = *state.attached.lock().unwrap();
    let (autofill, password) = {
        let s = state.settings.lock().unwrap();
        (s.autofill, s.admin_password.clone())
    };
    if !autofill {
        return;
    }
    let Some(password) = password else { return };
    let webview = webview.clone();
    let handle = webview.app_handle().clone();
    thread::spawn(move || match verify_login(port, &password) {
        LoginCheck::Ok => {
            let _ = webview.eval(&autofill_js(&password));
        }
        LoginCheck::Invalid => {
            // 附着在外部实例上时不动自己的记忆（可能是别的实例的密码）
            if !attached {
                let st = handle.state::<AppState>();
                let mut s = st.settings.lock().unwrap();
                s.admin_password = None;
                save_settings(&st.settings_path, &s);
            }
        }
        LoginCheck::Unreachable => {}
    });
}

fn kill_child(state: &AppState) {
    if let Some(mut c) = state.child.lock().unwrap().take() {
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &c.id().to_string()])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
        #[cfg(not(windows))]
        {
            let _ = c.kill();
        }
        let _ = c.wait();
    }
}

/// 拉起服务（若外部已有健康实例则直接进入「附着」模式）。
fn start_server(app: &AppHandle) {
    {
        let state = app.state::<AppState>();
        if healthz_ok(state.port) {
            *state.attached.lock().unwrap() = true;
            *state.ready.lock().unwrap() = true;
            let _ = app.emit("server://ready", ReadyPayload { port: state.port });
            set_status(app, &format!("运行中（外部实例）: {}", state.port));
            return;
        }
    }
    set_status(app, "启动中…");
    let (root, data, port, node) = {
        let state = app.state::<AppState>();
        (
            state.server_root.clone(),
            state.data_dir.clone(),
            state.port,
            resolve_node(&state.server_root),
        )
    };
    let _ = std::fs::create_dir_all(&data);

    let mut cmd = Command::new(&node);
    cmd.arg("bin/serve.js")
        .current_dir(&root)
        .env("FREEBUFF_PROXY_DATA_DIR", &data)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            fail(
                app,
                format!(
                    "无法启动 node（{e}）。\n查找路径: {}\n请确认本机已安装 Node.js ≥ 20。",
                    node.display()
                ),
            );
            return;
        }
    };
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    {
        let state = app.state::<AppState>();
        *state.child.lock().unwrap() = Some(child);
        *state.attached.lock().unwrap() = false;
        *state.ready.lock().unwrap() = false;
    }

    // stdout：捕获首次启动的管理员密码 → 缓存 + 剪贴板 + 原生弹窗
    let h = app.clone();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().flatten() {
            if let Some(pw) = extract_password(&line) {
                let st = h.state::<AppState>();
                *st.admin_password.lock().unwrap() = Some(pw.clone());
                {
                    // 首启密码落盘记忆（开关开启时），下次启动仍可自动填充
                    let mut s = st.settings.lock().unwrap();
                    if s.autofill {
                        s.admin_password = Some(pw.clone());
                        save_settings(&st.settings_path, &s);
                    }
                }
                if let Some(ui) = st.ui.lock().unwrap().as_ref() {
                    let _ = ui.show_password.set_enabled(true);
                }
                let _ = h.clipboard().write_text(&pw);
                let _ = h
                    .dialog()
                    .message(format!(
                        "首次启动已创建管理员账号：\n\n用户名  admin\n密码    {pw}\n\n密码已复制到剪贴板，登录后请立即修改。"
                    ))
                    .title("Freebuff Proxy 管理员密码")
                    .show(|_| {});
            }
        }
    });

    // stderr：只做排空 + 留尾部，供失败时展示（不排空会撑爆管道导致子进程卡死）
    let h = app.clone();
    thread::spawn(move || {
        let st = h.state::<AppState>();
        for line in BufReader::new(stderr).lines().flatten() {
            let mut tail = st.stderr_tail.lock().unwrap();
            if tail.len() >= 40 {
                tail.remove(0);
            }
            tail.push(line);
        }
    });

    // 就绪监听：healthz 通了 → 通知前端跳控制台；子进程中途退出 → 展示 stderr 尾部
    let h = app.clone();
    thread::spawn(move || {
        for _ in 0..HEALTHZ_MAX_TRIES {
            thread::sleep(Duration::from_millis(200));
            {
                let st = h.state::<AppState>();
                let exited = st
                    .child
                    .lock()
                    .unwrap()
                    .as_mut()
                    .and_then(|c| c.try_wait().ok())
                    .flatten()
                    .is_some();
                if exited {
                    let tail = st.stderr_tail.lock().unwrap().join("\n");
                    fail(&h, format!("本地服务进程已退出。\n\n{tail}"));
                    return;
                }
            }
            if healthz_ok(port) {
                let st = h.state::<AppState>();
                *st.ready.lock().unwrap() = true;
                let _ = h.emit("server://ready", ReadyPayload { port });
                set_status(&h, &format!("运行中: {port}"));
                return;
            }
        }
        fail(&h, format!("等待服务就绪超时（30s），端口 {port} 一直无响应。"));
    });
}

fn do_restart(app: &AppHandle) {
    let port = {
        let state = app.state::<AppState>();
        *state.ready.lock().unwrap() = false;
        set_status(app, "重启中…");
        kill_child(&state);
        state.port
    };
    // 等旧进程真正释放端口（最多 5s），避免新进程撞 EADDRINUSE
    for _ in 0..25 {
        if !port_in_use(port) {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    start_server(app);
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn kill_state_child(app: &AppHandle) {
    let state = app.state::<AppState>();
    kill_child(&state);
}

fn tray_menu_event(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main_window(app),
        "restart" => {
            let h = app.clone();
            thread::spawn(move || do_restart(&h));
        }
        "show_password" => {
            let state = app.state::<AppState>();
            let pw = state.admin_password.lock().unwrap().clone();
            if let Some(pw) = pw {
                let _ = app
                    .dialog()
                    .message(format!(
                        "本次进程捕获到的首次启动密码：\n\n用户名  admin\n密码    {pw}\n\n若你已登录并改过密码，以改后为准。"
                    ))
                    .title("Freebuff Proxy 管理员密码")
                    .show(|_| {});
            }
        }
        "autofill" => {
            // CheckMenuItem 点击后框架已自动翻转勾选态，读到的就是新状态
            let state = app.state::<AppState>();
            let (now_checked, settings_path) = {
                let ui = state.ui.lock().unwrap();
                match ui.as_ref() {
                    Some(items) => (items.autofill.is_checked().unwrap_or(false), state.settings_path.clone()),
                    None => return,
                }
            };
            let mut s = state.settings.lock().unwrap();
            s.autofill = now_checked;
            if !now_checked {
                s.admin_password = None; // 关掉即忘记
            }
            save_settings(&settings_path, &s);
        }
        "quit" => {
            kill_state_child(app);
            app.exit(0);
        }
        _ => {}
    }
}

#[tauri::command]
fn get_server_state(state: State<'_, AppState>) -> ServerStatePayload {
    ServerStatePayload {
        ready: *state.ready.lock().unwrap(),
        attached: *state.attached.lock().unwrap(),
        failed: *state.failed.lock().unwrap(),
        port: state.port,
        password: state.admin_password.lock().unwrap().clone(),
    }
}

#[tauri::command]
fn copy_admin_password(app: AppHandle) -> bool {
    let state = app.state::<AppState>();
    let pw = state.admin_password.lock().unwrap().clone();
    match pw {
        Some(pw) => app.clipboard().write_text(&pw).is_ok(),
        None => false,
    }
}

fn main() {
    tauri::Builder::default()
        // 必须最先注册：二次启动 → 把已有窗口带到前台
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let (server_root, data_dir) = resolve_layout(app.handle())?;
            let settings_path = app
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())?
                .join("shell.json");
            if let Some(parent) = settings_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let settings = load_settings(&settings_path);
            let port = std::env::var("FREEBUFF_PROXY_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_PORT);
            app.manage(AppState {
                child: Mutex::new(None),
                server_root,
                data_dir,
                settings_path,
                port,
                attached: Mutex::new(false),
                ready: Mutex::new(false),
                failed: Mutex::new(false),
                admin_password: Mutex::new(None),
                stderr_tail: Mutex::new(Vec::new()),
                settings: Mutex::new(settings),
                ui: Mutex::new(None),
            });

            let status = MenuItem::with_id(app, "status", "状态：启动中…", false, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "打开控制台", true, None::<&str>)?;
            let restart = MenuItem::with_id(app, "restart", "重启服务", true, None::<&str>)?;
            let show_password =
                MenuItem::with_id(app, "show_password", "显示管理员密码", false, None::<&str>)?;
            let autofill_checked = app.state::<AppState>().settings.lock().unwrap().autofill;
            let autofill = CheckMenuItem::with_id(
                app,
                "autofill",
                "记住密码并自动填充",
                true,
                autofill_checked,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            {
                let state = app.state::<AppState>();
                *state.ui.lock().unwrap() = Some(UiItems {
                    status: status.clone(),
                    show_password: show_password.clone(),
                    autofill: autofill.clone(),
                });
            }
            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().expect("window icon").clone())
                .tooltip("Freebuff Proxy")
                .show_menu_on_left_click(true)
                .menu(
                    &MenuBuilder::new(app)
                        .item(&status)
                        .separator()
                        .item(&open)
                        .item(&restart)
                        .item(&show_password)
                        .item(&autofill)
                        .separator()
                        .item(&quit)
                        .build()?,
                )
                .on_menu_event(|app, ev| tray_menu_event(app, ev.id().as_ref()))
                .build(app)?;

            // 稍等托盘就绪再拉服务，避免状态文案竞态；起服务的活不占主线程
            let handle = app.handle().clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                start_server(&handle);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗 = 隐藏到托盘继续跑；真正退出走托盘「退出」
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .on_page_load(|webview, payload| {
            // 控制台页面加载完成 → 尝试自动填充记住的登录密码
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
                && payload
                    .url()
                    .as_str()
                    .starts_with(&format!(
                        "http://127.0.0.1:{}",
                        webview.state::<AppState>().port
                    ))
            {
                handle_console_loaded(webview);
            }
        })
        .invoke_handler(tauri::generate_handler![get_server_state, copy_admin_password])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 任何退出路径（托盘退出 / 系统关机）都收掉 node 子进程树；
            // 未及释放的上游会话由服务端下次启动的孤儿扫尾兜底（cleanupOrphanSessions）。
            if let tauri::RunEvent::Exit = event {
                kill_state_child(app);
            }
        });
}
