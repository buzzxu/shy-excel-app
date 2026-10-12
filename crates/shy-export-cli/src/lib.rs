//! 导出客户端 headless 核心（M-D2，REQ-2026-06-21-001）。
//! HTTPS chunked 拉 Arrow IPC 流（`ureq`，无 async 运行时）→ 流式喂 `shy-xlsx-core` 生成。
//! Tauri 命令在工作线程调用 [`download_and_generate_cb`]，并传入进度回调。

use std::io::{self, BufReader, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub use shy_xlsx_core::{GenConfig, GenResult};

/// 进度回调参数：`(已完成单数, 已完成行数, 总单数估计)`；总数未知时为 0。
pub type Progress = (u64, u64, u64);

/// 建连超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 读超时：按单次 socket 读计（非整次导出总时长），放宽到 5 分钟，避免服务端首查询慢时被误杀。
const READ_TIMEOUT: Duration = Duration::from_secs(300);
/// Arrow IPC 流结束标记（EOS）：continuation `0xFFFFFFFF` + 长度 0；arrow-java `ArrowStreamWriter.end()/close()` 会写出。
const ARROW_EOS: [u8; 8] = [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0];
const TIMEOUT_MSG: &str = "服务器长时间无响应，导出已中止，请稍后重试。";
const CANCEL_MSG: &str = "已取消导出";

/// 拉取 `url`（含 `?job=&token=`）的 Arrow 流并生成 xlsx（多文件分块，见 GenConfig）。
/// 真流式：响应体边到边解码边写，内存与行数无关。
pub fn download_and_generate(url: &str, cfg: &GenConfig) -> Result<GenResult, Box<dyn std::error::Error>> {
    download_and_generate_cb(url, cfg, &AtomicBool::new(false), |_| {})
}

/// 同 [`download_and_generate`]，但读响应头 `X-Export-Total-Orders` 作总数估计，并在生成过程中
/// 回调进度 `(orders, rows, total)`，供 UI 进度条/ETA。`cancel` 置位后，下一次读响应体即中止并返回「已取消导出」
/// （等待响应头期间无法打断，见函数内注释）。
///
/// 错误处理：服务端业务错误（HTTP 4xx/5xx，或 200 + JSON `Result`）会被识别并把 `message` 作为
/// 错误信息返回，**且任何错误信息都不含请求 URL**（避免泄露 job/token/Authorization 等参数）。
pub fn download_and_generate_cb<F: FnMut(Progress)>(
    url: &str,
    cfg: &GenConfig,
    cancel: &AtomicBool,
    mut on_progress: F,
) -> Result<GenResult, Box<dyn std::error::Error>> {
    // 不跟随重定向：否则 https 可信来源可被 302 带到任意（含 http）地址，绕过前端的来源信任校验。
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .redirects(0)
        .build();
    let result = agent.get(url).call();
    // ureq 2 无法打断「等待响应头」中的请求：此间点取消，要等首字节到达或读超时后才在这里/读响应体时生效。
    if cancel.load(Ordering::Relaxed) {
        return Err(CANCEL_MSG.into());
    }
    let resp = match result {
        // 只接受 200：3xx（不跟随）等一律拒绝，绝不回显 URL / Location。
        Ok(r) if r.status() != 200 => {
            return Err(format!("服务端返回了非预期的响应（HTTP {}），已拒绝导出", r.status()).into());
        }
        Ok(r) => r,
        // 服务端返回错误状态：尽量取 JSON message，绝不回显 URL。
        Err(ureq::Error::Status(code, r)) => {
            // 401/403（登出后令牌/会话失效在「请求建立时」就会命中这里）给明确的「登录失效」
            // 提示，不回显鉴权响应体（常是 HTML/英文 Unauthorized，对用户无意义）。
            let msg = match code {
                401 | 403 => "登录状态已失效，请重新登录后重试导出。".to_string(),
                _ => read_error_message(r).unwrap_or_else(|| format!("服务端返回错误（HTTP {code}）")),
            };
            return Err(msg.into());
        }
        // 传输层错误（DNS/连接等）：给通用提示，不带 URL；建连/等响应头超时单独提示。
        Err(ureq::Error::Transport(t)) => {
            let timed_out = std::error::Error::source(&t)
                .and_then(|s| s.downcast_ref::<io::Error>())
                .is_some_and(|e| matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock));
            return Err(if timed_out { TIMEOUT_MSG } else { "网络连接失败，请检查网络后重试" }.into());
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

    let mut guard = StreamGuard { inner: BufReader::new(resp.into_reader()), cancel, tail: [0; 8], io_err: None, saw_eof: false };
    // 流式过程中连接被断开（如导出途中登出/网关掐断/网络波动；此时 HTTP 状态已是 200，
    // 不会再变成 401）→ 按守卫记录的读取状态映射为友好提示，不回显技术文案；本地错误（如磁盘满）走最后一档。
    let res = match shy_xlsx_core::generate_from_arrow_cb(&mut guard, cfg, |orders, rows| {
        on_progress((orders, rows, total));
    }) {
        Ok(r) => r,
        Err(e) => {
            let msg = if cancel.load(Ordering::Relaxed) {
                CANCEL_MSG.to_string()
            } else if matches!(guard.io_err, Some(io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)) {
                TIMEOUT_MSG.to_string()
            } else if guard.io_err.is_some() || guard.saw_eof {
                "导出中断：与服务器的连接已断开（可能是登录失效或网络波动），请重新登录后重试。".to_string()
            } else {
                format!("导出处理失败，请重试（{e}）")
            };
            return Err(msg.into());
        }
    };

    // 读完 EOS 后 finish() 仍在保存最后一个文件，这期间点的取消也要生效（外层会清理工作目录）。
    if cancel.load(Ordering::Relaxed) {
        return Err(CANCEL_MSG.into());
    }

    // 结束标记校验：连接恰在消息边界断开时 Arrow 当作正常结束（ureq chunked 断流也只返回 Ok(0)），
    // 只有 EOS 能证明服务端写完了整条流。
    if guard.tail != ARROW_EOS {
        return Err("导出未完成：数据流未正常结束（可能是登录失效或网络中断），请重新登录后重试。".into());
    }

    // 完整性校验：已知总数却收到明显偏少（不足 90%）→ 视为中断，杜绝「悄悄交付残缺文件还报成功」。
    // X-Export-Total-Orders 是估计值，用 90% 容差避免服务端略微高估时误报完整导出为未完成。
    if total > 0 && res.orders.saturating_mul(10) < total.saturating_mul(9) {
        return Err(format!(
            "导出未完成：预计约 {total} 单，仅收到 {}，可能因登录失效或网络中断，请重新登录后重试。",
            res.orders
        )
        .into());
    }
    Ok(res)
}

/// 响应体读取守卫：每次 read 先查取消标志；记下最后返回的 8 字节（判 EOS）、底层 IO 错误类型、
/// 是否读到 EOF，供生成结束后判定流是否完整、并把错误映射成准确的提示。
struct StreamGuard<'a, R> {
    inner: R,
    cancel: &'a AtomicBool,
    tail: [u8; 8],
    io_err: Option<io::ErrorKind>,
    saw_eof: bool,
}

impl<R: Read> Read for StreamGuard<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            // 不能用 ErrorKind::Interrupted：std 的 read_exact 视其为可重试，会原地死循环。
            return Err(io::Error::other("cancelled"));
        }
        match self.inner.read(buf) {
            Ok(0) if !buf.is_empty() => {
                self.saw_eof = true;
                Ok(0)
            }
            Ok(n) => {
                // 滚动保留最后 8 字节（Arrow 常按 4 字节小块读）
                if n >= 8 {
                    self.tail.copy_from_slice(&buf[n - 8..n]);
                } else {
                    self.tail.copy_within(n.., 0);
                    self.tail[8 - n..].copy_from_slice(&buf[..n]);
                }
                Ok(n)
            }
            Err(e) => {
                self.io_err = Some(e.kind());
                Err(e)
            }
        }
    }
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
    Ok(shy_xlsx_core::generate_from_arrow(reader, cfg)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 流读取中途取消：错误类型不能是 Interrupted（read_exact 会无限重试 → 取消卡死）。
    #[test]
    fn guard_cancel_is_not_retryable() {
        let c = AtomicBool::new(true);
        let mut g = StreamGuard { inner: &[0u8; 8][..], cancel: &c, tail: [0; 8], io_err: None, saw_eof: false };
        assert_ne!(g.read(&mut [0u8; 4]).unwrap_err().kind(), io::ErrorKind::Interrupted);
    }
}
