//! ShyExcel · 数据导出 — 通用高速数据导出客户端 · Tauri v2 GUI 壳。
//! 浏览器 `shyexport://` 深链唤起 → 解析 job/token/url → 调 `download_and_generate_cb`
//! （Arrow 流式拉取 + 本地多层合并生成 + 多文件分块）→ 事件回传任务进度/结果。
//! 托盘常驻 + 单实例路由 + 开机自启 + 设置持久化（下载目录/每文件单数）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;

/// 唤起协议前缀（改名时只改这一处 + tauri.conf.json + 前端）。
const DEEP_LINK_SCHEME: &str = "shyexport://";

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
    // 置前主窗口：信任确认框挂在主窗口上，窗口最小化/隐藏时用户看不到，后续确认也会排队卡住。
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize(); // 最小化时 set_focus 不生效，须先还原
        let _ = w.show();
        let _ = w.set_focus();
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

/// 进行中的导出：task_id → 取消标志（`cancel_export` 置位，拉流读取时检查）。
#[derive(Default)]
struct Running(Mutex<HashMap<String, Arc<AtomicBool>>>);

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
#[allow(clippy::too_many_arguments)]
async fn run_export(
    app: AppHandle,
    running: State<'_, Running>,
    task_id: String,
    job_id: String,
    stream_url: String,
    download_dir: String,
    base_name: String,
    orders_per_file: u64,
) -> Result<Outcome, String> {
    let start = Instant::now();
    // 先登记取消标志（早于任何文件操作），之后任何时刻点「取消」都不会丢；下面每条返回路径都要移除它。
    let cancel = Arc::new(AtomicBool::new(false));
    running.0.lock().unwrap().insert(task_id.clone(), cancel.clone());
    let id = task_id.clone();

    // 工作目录：下载目录/<jobid 前8位安全字符>/（本次新建、本任务独占）。生成完成后再决定单文件归位 / 多文件保留。
    let dl = PathBuf::from(&download_dir);
    let work = match std::fs::create_dir_all(&dl).and_then(|_| create_work_dir(&dl, &job_id)) {
        Ok(w) => w,
        Err(e) => {
            running.0.lock().unwrap().remove(&id);
            return Err(format!("创建导出目录失败: {e}"));
        }
    };
    let work_dir = work.clone();

    let out = tauri::async_runtime::spawn_blocking(move || {
        let cfg = shy_export_cli::GenConfig {
            out_dir: work.clone(),
            base_name: base_name.clone(),
            orders_per_file: orders_per_file.max(1),
        };

        // 进度回调：节流 ~120ms 发一次，避免事件风暴。
        let mut last = Instant::now();
        let mut first = true;
        let res = shy_export_cli::download_and_generate_cb(&stream_url, &cfg, &cancel, |(orders, rows, total)| {
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
        // 移动失败（极少见）就原地交付工作目录里的文件：返回 Err 会触发失败清理，删掉唯一的完整产物。
        let (path, is_dir) = if res.files.len() == 1 {
            let src = &res.files[0];
            let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("xlsx");
            let target = unique_path(&dl, &base_name, ext);
            if move_file(src, &target).is_ok() {
                let _ = std::fs::remove_dir(&work);
                (target.to_string_lossy().into_owned(), false)
            } else {
                (src.to_string_lossy().into_owned(), false)
            }
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
    .map_err(|e| e.to_string())
    .and_then(|r| r);

    running.0.lock().unwrap().remove(&id);
    if out.is_err() {
        // 失败/取消/校验不通过：删掉残缺产物。work_dir 是本次 create_dir 新建、只含本任务文件，整删安全。
        let _ = std::fs::remove_dir_all(&work_dir);
    }
    out
}

/// 取消进行中的导出：置位取消标志，拉流在下一次读取时中止（任务已结束/不存在则忽略）。
#[tauri::command]
fn cancel_export(running: State<Running>, task_id: String) {
    if let Some(f) = running.0.lock().unwrap().get(&task_id) {
        f.store(true, Ordering::Relaxed);
    }
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

/// 在系统文件管理器中打开：普通目录直接打开；否则在所在目录中定位并选中（见 [`reveal_target`]）。
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    let (p, is_dir) = reveal_target(Path::new(&path));
    let r = if is_dir { open_dir(&p) } else { reveal_file(&p) };
    r.map_err(|e| e.to_string())
}

/// 先规范化（解开符号链接与 `..`，否则 `X.app/Contents/..`、指向 `.app` 的链接会被当成普通目录），再判定：
/// 只有普通目录（无扩展名）才直接打开；文件与 `.app` 等 bundle 目录一律「定位并选中」，绝不执行目标。
/// 多文件输出目录名只含 `[A-Za-z0-9_-]` 和 " (n)"，没有扩展名，故不受影响。返回（规范化路径, 是否直接打开）。
fn reveal_target(path: &Path) -> (PathBuf, bool) {
    // dunce：Windows 上 std::fs::canonicalize 返回 `\\?\C:\…` 形式，explorer 不认，dunce 会还原成普通路径
    let p = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let open = p.is_dir() && p.extension().is_none();
    (p, open)
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

/// 新建本任务独占的工作目录：dl/<job 前8位>，重名加序号 dl/<name> (2) → …
/// 只保留 `[A-Za-z0-9_-]`，`..`/绝对路径/盘符都逃不出 dl；用 `create_dir`（非 `_all`）确保目录是本次新建的，
/// 失败时整删才不会误删用户已有目录或其它任务的文件。
fn create_work_dir(dl: &Path, job_id: &str) -> std::io::Result<PathBuf> {
    let mut name: String = job_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(8)
        .collect();
    if name.is_empty() {
        name = "export".into();
    }
    let mut p = dl.join(&name);
    let mut n = 2;
    loop {
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                p = dl.join(format!("{name} ({n})"));
                n += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// 跨设备/普通移动：先 rename，失败则 copy+删源。copy 失败时删掉写了一半的 dst；
/// copy 成功后删源失败仍算成功（dst 已完整）。
fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    if let Err(e) = std::fs::copy(src, dst) {
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    let _ = std::fs::remove_file(src);
    Ok(())
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
        .manage(Running::default())
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
        .setup(|app| {
            // 系统托盘 + 菜单：显示窗口 / 设置 / 退出
            let show = tauri::menu::MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let settings = tauri::menu::MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &settings, &quit])?;
            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("ShyExcel")
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
            cancel_export,
            default_download_dir,
            reveal_path,
            frontend_ready
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 恶意/异常 job 不能让工作目录逃出下载目录；同 job 两次拿到两个不同目录；空 job 用 export。
    #[test]
    fn work_dir_is_new_child_of_download_dir() {
        let dl = std::env::temp_dir().join(format!("shyexport_workdir_{}", std::process::id()));
        std::fs::remove_dir_all(&dl).ok();
        std::fs::create_dir_all(&dl).unwrap();
        let canon_dl = dl.canonicalize().unwrap();

        assert_eq!(create_work_dir(&dl, "").unwrap().file_name().unwrap(), "export");
        for job in ["..", "/etc/x", "C:\\Users"] {
            let w = create_work_dir(&dl, job).unwrap();
            assert_eq!(w.canonicalize().unwrap().parent(), Some(canon_dl.as_path()), "job={job:?} 逃出了下载目录: {w:?}");
        }
        let (a, b) = (create_work_dir(&dl, "samejob1").unwrap(), create_work_dir(&dl, "samejob1").unwrap());
        assert_ne!(a, b, "同 job 两次应得到两个不同目录");
        assert!(a.is_dir() && b.is_dir());

        std::fs::remove_dir_all(&dl).ok();
    }

    /// 只直接打开普通目录；`.app` 这类 bundle 目录和文件走「定位并选中」，不会被启动/执行。
    #[test]
    fn only_plain_dirs_are_opened() {
        let base = std::env::temp_dir().join(format!("shyexport_reveal_{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(base.join("abcd1234 (2)")).unwrap();
        std::fs::create_dir_all(base.join("X.app/Contents")).unwrap();
        std::fs::write(base.join("a.xlsx"), b"x").unwrap();
        let opens = |rel: &str| reveal_target(&base.join(rel)).1;

        assert!(opens("abcd1234 (2)"));
        assert!(!opens("X.app"));
        assert!(!opens("a.xlsx"));
        assert!(!opens("X.app/Contents/.."), "`..` 回到 .app 也不能直接打开");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("X.app"), base.join("link")).unwrap();
            assert!(!opens("link"), "指向 .app 的符号链接也不能直接打开");
        }

        std::fs::remove_dir_all(&base).ok();
    }
}
