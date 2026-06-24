//! 导出客户端 headless 核心（M-D2，REQ-2026-06-21-001）。
//! HTTPS chunked 拉 Arrow IPC 流（`ureq`，无 async 运行时）→ 流式喂 `xwjd-xlsx-core` 生成。
//! Tauri 命令在工作线程调用 [`download_and_generate_cb`]，并传入进度回调。

use std::io::BufReader;
use std::path::Path;

pub use xwjd_xlsx_core::{GenConfig, GenResult};

/// 进度回调参数：`(已完成单数, 已完成行数, 总单数估计)`；总数未知时为 0。
pub type Progress = (u64, u64, u64);

/// 拉取 `url`（含 `?job=&token=`）的 Arrow 流并生成 xlsx（多文件分块，见 GenConfig）。
/// 真流式：响应体边到边解码边写，内存与行数无关。
pub fn download_and_generate(url: &str, cfg: &GenConfig) -> Result<GenResult, Box<dyn std::error::Error>> {
    download_and_generate_cb(url, cfg, |_| {})
}

/// 同 [`download_and_generate`]，但读响应头 `X-Export-Total-Orders` 作总数估计，并在生成过程中
/// 回调进度 `(orders, rows, total)`，供 UI 进度条/ETA。
///
/// 错误处理：服务端业务错误（HTTP 4xx/5xx，或 200 + JSON `Result`）会被识别并把 `message` 作为
/// 错误信息返回，**且任何错误信息都不含请求 URL**（避免泄露 job/token/Authorization 等参数）。
pub fn download_and_generate_cb<F: FnMut(Progress)>(
    url: &str,
    cfg: &GenConfig,
    mut on_progress: F,
) -> Result<GenResult, Box<dyn std::error::Error>> {
    let resp = match ureq::get(url).call() {
        Ok(r) => r,
        // 服务端返回错误状态：尽量取 JSON message，绝不回显 URL。
        Err(ureq::Error::Status(code, r)) => {
            let msg = read_error_message(r).unwrap_or_else(|| format!("服务端返回错误（HTTP {code}）"));
            return Err(msg.into());
        }
        // 传输层错误（DNS/连接/超时等）：给通用提示，不带 URL。
        Err(ureq::Error::Transport(_)) => {
            return Err("网络连接失败，请检查网络后重试".into());
        }
    };

    // 业务错误体：服务端 ApplicationException → 全局处理器返回 HTTP 200 + JSON Result（非 arrow 流）。
    let ctype = resp.header("Content-Type").unwrap_or_default().to_ascii_lowercase();
    if ctype.contains("json") {
        let msg = read_error_message(resp).unwrap_or_else(|| "导出失败".to_string());
        return Err(msg.into());
    }

    let total: u64 = resp
        .header("X-Export-Total-Orders")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);

    let reader = BufReader::new(resp.into_reader());
    Ok(xwjd_xlsx_core::generate_from_arrow_cb(reader, cfg, |orders, rows| {
        on_progress((orders, rows, total));
    })?)
}

/// 从响应体读取 JSON 中的 `message`/`error` 字段（服务端 `Result` 错误体）。
fn read_error_message(resp: ureq::Response) -> Option<String> {
    let body = resp.into_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    for key in ["message", "error", "msg"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// 从本地 Arrow IPC 文件生成（联调/离线：消费 Java 端 ArrowExportWriter 产物，验证跨语言互通）。
pub fn generate_local(path: &Path, cfg: &GenConfig) -> Result<GenResult, Box<dyn std::error::Error>> {
    let reader = BufReader::new(std::fs::File::open(path)?);
    Ok(xwjd_xlsx_core::generate_from_arrow(reader, cfg)?)
}
