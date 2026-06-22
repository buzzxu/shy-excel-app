//! 雪王高速导出客户端 — Tauri v2 GUI 壳（M-D3，REQ-2026-06-21-001）。
//! 浏览器 `xwjdexport://` 深链唤起 → 解析 job/token/url → 调用已验证的 `download_and_generate`
//! （Arrow 流式拉取 + 本地多层合并生成 + 多文件分块）→ 事件回传前端任务进度/结果。
//! 托盘常驻 + 单实例路由 + 开机自启 + 设置持久化（导出目录/每文件订单数）。

use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

#[derive(serde::Serialize, Clone)]
struct Outcome {
    files: usize,
    orders: u64,
    rows: u64,
}

/// 执行一次导出（前端在收到 deep-link 后调用）。阻塞逻辑放工作线程，避免卡 UI。
#[tauri::command]
async fn run_export(
    stream_url: String,
    out_dir: String,
    base_name: String,
    orders_per_file: u64,
) -> Result<Outcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = xwjd_export_cli::GenConfig {
            out_dir: PathBuf::from(out_dir),
            base_name,
            orders_per_file: orders_per_file.max(1),
        };
        xwjd_export_cli::download_and_generate(&stream_url, &cfg)
            .map(|r| Outcome { files: r.files.len(), orders: r.orders, rows: r.rows })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 把深链 URL 转发给前端（前端解析 job/token/host → 入队 → 调 run_export）。
fn emit_deep_links(app: &AppHandle, urls: Vec<String>) {
    for u in urls {
        if u.starts_with("xwjdexport://") {
            let _ = app.emit("deep-link", u);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例：已运行时第二次唤起把深链从 argv 转发给现有实例并聚焦
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            emit_deep_links(app, argv);
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
            // 系统托盘 + 菜单
            let show = tauri::menu::MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &quit])?;
            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("雪王高速导出")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            // 运行期深链回调（冷启动唤起 / 运行中再次唤起）
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                let urls: Vec<String> = event.urls().iter().map(|u| u.to_string()).collect();
                emit_deep_links(&handle, urls);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![run_export])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
