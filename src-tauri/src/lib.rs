//! 本地导出（SwiftExport）— 通用高速数据导出客户端 · Tauri v2 GUI 壳（REQ-2026-06-21-001）。
//! 浏览器 `swiftexport://` 深链唤起 → 解析 job/token/url → 调 `download_and_generate_cb`
//! （Arrow 流式拉取 + 本地多层合并生成 + 多文件分块）→ 事件回传任务进度/结果。
//! 托盘常驻 + 单实例路由 + 开机自启 + 设置持久化（下载目录/每文件订单数）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;

/// 唤起协议前缀（改名时只改这一处 + tauri.conf.json + 前端）。
const DEEP_LINK_SCHEME: &str = "swiftexport://";

/// 深链收件箱：前端 webview 就绪前到达的深链先缓冲，待前端 `frontend_ready` 时一次性领取，
/// 解决「客户端未开时第一次点下载，任务收不到」的冷启动竞态。就绪后到达的深链直接发事件。
#[derive(Default)]
struct DeepLinkInbox {
    inner: Mutex<Inbox>,
}
#[derive(Default)]
struct Inbox {
    ready: bool,
    pending: Vec<String>,
}

/// 投递一条深链：就绪→发事件；未就绪→缓冲。锁内判定，避免就绪切换瞬间丢链接。
fn deliver(app: &AppHandle, url: String) {
    if !url.starts_with(DEEP_LINK_SCHEME) {
        return;
    }
    let inbox = app.state::<DeepLinkInbox>();
    let mut g = inbox.inner.lock().unwrap();
    if g.ready {
        drop(g);
        let _ = app.emit("deep-link", url);
    } else {
        g.pending.push(url);
    }
}

/// 前端就绪：标记 ready 并领走在它就绪前缓冲的深链（冷启动那一条就在这里被取到）。
#[tauri::command]
fn frontend_ready(state: State<DeepLinkInbox>) -> Vec<String> {
    let mut g = state.inner.lock().unwrap();
    g.ready = true;
    std::mem::take(&mut g.pending)
}

/// 一次导出的最终结果（返回前端）。
#[derive(serde::Serialize, Clone)]
struct Outcome {
    files: usize,
    orders: u64,
    rows: u64,
    /// 单文件=文件完整路径；多文件=输出目录路径。
    path: String,
    /// path 是否为目录（多文件）。
    is_dir: bool,
    /// 总耗时（毫秒）。
    elapsed_ms: u64,
}

/// 进度事件载荷（生成过程中持续发往前端，按 task_id 关联）。
#[derive(serde::Serialize, Clone)]
struct ProgressEvent {
    task_id: String,
    orders: u64,
    rows: u64,
    /// 总单数估计（来自服务端响应头）；0 表示未知 → 前端不显示 ETA、只显示吞吐。
    total: u64,
    elapsed_ms: u64,
}

/// 执行一次导出。阻塞逻辑放工作线程，避免卡 UI；过程中按 task_id 发 `export-progress` 事件。
#[tauri::command]
async fn run_export(
    app: AppHandle,
    task_id: String,
    job_id: String,
    stream_url: String,
    download_dir: String,
    base_name: String,
    orders_per_file: u64,
) -> Result<Outcome, String> {
    let start = Instant::now();
    tauri::async_runtime::spawn_blocking(move || {
        // 工作目录：下载目录/<jobid 前8位>/。生成完成后再决定单文件归位 / 多文件保留。
        let dl = PathBuf::from(&download_dir);
        let sub: String = job_id.chars().take(8).collect();
        let work = dl.join(if sub.is_empty() { "export" } else { &sub });
        std::fs::create_dir_all(&work).map_err(|e| format!("创建导出目录失败: {e}"))?;

        let cfg = xwjd_export_cli::GenConfig {
            out_dir: work.clone(),
            base_name: base_name.clone(),
            orders_per_file: orders_per_file.max(1),
        };

        // 进度回调：节流 ~120ms 发一次，避免事件风暴。
        let mut last = Instant::now();
        let mut first = true;
        let res = xwjd_export_cli::download_and_generate_cb(&stream_url, &cfg, |(orders, rows, total)| {
            if first || last.elapsed().as_millis() >= 120 {
                first = false;
                last = Instant::now();
                let _ = app.emit(
                    "export-progress",
                    ProgressEvent {
                        task_id: task_id.clone(),
                        orders,
                        rows,
                        total,
                        elapsed_ms: start.elapsed().as_millis() as u64,
                    },
                );
            }
        })
        .map_err(|e| e.to_string())?;

        // 归位：单文件 → 移回下载目录根（重名自动加序号）并删子目录；多文件 → 保留 jobid 子目录。
        let (path, is_dir) = if res.files.len() == 1 {
            let src = &res.files[0];
            let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("xlsx");
            let target = unique_path(&dl, &base_name, ext);
            move_file(src, &target).map_err(|e| format!("移动文件失败: {e}"))?;
            let _ = std::fs::remove_dir_all(&work);
            (target.to_string_lossy().into_owned(), false)
        } else {
            (work.to_string_lossy().into_owned(), true)
        };

        Ok(Outcome {
            files: res.files.len(),
            orders: res.orders,
            rows: res.rows,
            path,
            is_dir,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 系统默认下载目录（设置项默认值；取不到则退回 home）。
#[tauri::command]
fn default_download_dir(app: AppHandle) -> String {
    app.path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 在系统文件管理器中打开：is_dir=true 打开目录；否则在所在目录中定位并选中该文件。
#[tauri::command]
fn reveal_path(path: String, is_dir: bool) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let r = if is_dir { open_dir(&p) } else { reveal_file(&p) };
    r.map_err(|e| e.to_string())
}

/// 重名自动加序号：dir/base.ext → dir/base (2).ext → …
fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    p
}

/// 跨设备/普通移动：先 rename，失败则 copy+删源。
fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    std::fs::remove_file(src)
}

fn reveal_file(file: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg("-R").arg(file).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", file.display()))
            .spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = file.parent() {
            std::process::Command::new("xdg-open").arg(dir).spawn()?;
        }
    }
    Ok(())
}

fn open_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(dir).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(dir).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(dir).spawn()?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DeepLinkInbox::default())
        // 单实例：已运行时第二次唤起把深链从 argv 转发给现有实例并聚焦
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            for u in argv {
                deliver(app, u);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 系统托盘 + 菜单：显示窗口 / 设置 / 退出
            let show = tauri::menu::MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let settings = tauri::menu::MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &settings, &quit])?;
            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("本地导出")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "settings" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                            let _ = w.emit("open-settings", ());
                        }
                    }
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            // 冷启动唤起：捕获「启动本进程的那条深链」（macOS 经 Opened 事件 / Win·Linux 经 argv）。
            // 此时前端多半未就绪 → deliver 会缓冲，待 frontend_ready 领走，解决首次点击丢任务。
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for u in urls {
                    deliver(app.handle(), u.to_string());
                }
            }
            // 运行期深链回调（运行中再次唤起）
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for u in event.urls() {
                    deliver(&handle, u.to_string());
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            run_export,
            default_download_dir,
            reveal_path,
            frontend_ready
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
