//! 导出客户端 headless 核心（M-D2，REQ-2026-06-21-001）。
//! HTTPS chunked 拉 Arrow IPC 流（`ureq`，无 async 运行时）→ 流式喂 `xwjd-xlsx-core` 生成。
//! Tauri 命令在工作线程调用 [`download_and_generate`]，并可传入进度回调。

use std::io::BufReader;

pub use xwjd_xlsx_core::{GenConfig, GenResult};

/// 拉取 `url`（含 `?job=&token=`）的 Arrow 流并生成 xlsx（多文件分块，见 GenConfig）。
/// 真流式：响应体边到边解码边写，内存与行数无关。
pub fn download_and_generate(url: &str, cfg: &GenConfig) -> Result<GenResult, Box<dyn std::error::Error>> {
    let resp = ureq::get(url).call()?;
    let reader = BufReader::new(resp.into_reader());
    Ok(xwjd_xlsx_core::generate_from_arrow(reader, cfg)?)
}
