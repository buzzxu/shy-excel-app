//! 集成：本地 HTTP 服务 Arrow IPC 字节 → ureq 拉流 → 生成。验证客户端数据通道（M-D2）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use shy_export_cli::{download_and_generate, download_and_generate_bg, download_and_generate_cb, GenConfig};

fn meta(p: &[(&str, &str)]) -> HashMap<String, String> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn arrow_bytes(n_orders: usize, k: usize) -> Vec<u8> {
    arrow_bytes_batches(n_orders, k, 1)
}

/// 同 [`arrow_bytes`]，但写 `batches` 个 record batch、每个 `n_orders` 单（gid 接续编号）；`batches = 0` 即空导出（schema + EOS）。
fn arrow_bytes_batches(n_orders: usize, k: usize, batches: usize) -> Vec<u8> {
    let fields = vec![
        Field::new("__gid0", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "0")])),
        Field::new("__gid1", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "1")])),
        Field::new("c0", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "订单编号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "20")])),
        Field::new("c1", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "商品"), ("merge", "false"), ("level", "1"), ("type", "STRING"), ("width", "20")])),
    ];
    let schema = Arc::new(Schema::new(fields));
    let mut buf = Vec::new();
    {
        let mut w = StreamWriter::try_new(&mut buf, &schema).unwrap();
        let mut item = 0i64;
        for b in 0..batches {
            let (mut g0, mut g1, mut c0, mut c1) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for o in b * n_orders..(b + 1) * n_orders {
                for _ in 0..k {
                    g0.push(o as i64);
                    g1.push(item);
                    item += 1;
                    c0.push(format!("DD{:06}", o));
                    c1.push(format!("商品{}", item));
                }
            }
            let cols: Vec<ArrayRef> = vec![
                Arc::new(Int64Array::from(g0)),
                Arc::new(Int64Array::from(g1)),
                Arc::new(StringArray::from(c0)),
                Arc::new(StringArray::from(c1)),
            ];
            w.write(&RecordBatch::try_new(schema.clone(), cols).unwrap()).unwrap();
        }
        w.finish().unwrap();
    }
    buf
}

#[test]
fn http_stream_to_xlsx() {
    let body = arrow_bytes(10, 2);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf); // 读取请求行/头（GET 无体，一次足够）
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apache.arrow.stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
            stream.flush().unwrap();
        }
    });

    let dir = std::env::temp_dir().join(format!("xlsxcli_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = GenConfig { out_dir: dir.clone(), base_name: "t".into(), orders_per_file: 4 };
    let url = format!("http://127.0.0.1:{}/mall/order/export/stream?job=j&token=t", port);
    let res = download_and_generate(&url, &cfg).unwrap();
    handle.join().ok();

    assert_eq!(res.orders, 10, "订单数");
    assert_eq!(res.rows, 20, "渲染行数");
    assert_eq!(res.files.len(), 3, "ceil(10/4)=3 文件");
    for f in &res.files {
        assert!(f.exists() && std::fs::metadata(f).unwrap().len() > 0, "文件非空");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// 起一个一次性 mock HTTP server，对首个连接回写 `response_head`（含状态行+头+CRLF）后再写 `body`。
/// 若 `body_fraction < 1.0`，只发送部分 body 然后关闭连接，模拟「流式中途断开」。
fn serve_once(response_head: String, body: Vec<u8>, body_fraction: f64) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response_head.as_bytes());
            let n = ((body.len() as f64) * body_fraction) as usize;
            let _ = stream.write_all(&body[..n]);
            let _ = stream.flush();
            // 函数返回 → stream drop → 连接关闭（部分 body 时即为「中途断开」）
        }
    });
    port
}

fn expect_err<T>(r: Result<T, Box<dyn std::error::Error>>) -> String {
    match r {
        Ok(_) => panic!("expected an error, got Ok"),
        Err(e) => e.to_string(),
    }
}

fn tmp_cfg(tag: &str) -> (std::path::PathBuf, GenConfig) {
    let dir = std::env::temp_dir().join(format!("xlsxcli_{}_{}", tag, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = GenConfig { out_dir: dir.clone(), base_name: "t".into(), orders_per_file: 4 };
    (dir, cfg)
}

/// 登出后令牌/会话失效 → 请求建立时 401 → 友好「登录失效」提示，且不泄露 URL/参数。
#[test]
fn http_401_friendly_login_expired() {
    let head = "HTTP/1.1 401 Unauthorized\r\nContent-Type: text/html\r\nContent-Length: 12\r\nConnection: close\r\n\r\nUnauthorized".to_string();
    let port = serve_once(head, Vec::new(), 1.0);
    let (dir, cfg) = tmp_cfg("401");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=secret", port);
    let err = expect_err(download_and_generate(&url, &cfg));
    std::fs::remove_dir_all(&dir).ok();
    assert!(err.contains("登录"), "401 应给登录失效提示，实际: {err}");
    assert!(!err.contains("secret") && !err.contains("Unauthorized"), "不应回显 URL/鉴权响应体: {err}");
}

/// 导出途中连接被断开（HTTP 已 200）→ 友好中断提示，不回显底层 Arrow/IO 文案。
#[test]
fn http_midstream_drop_friendly() {
    let body = arrow_bytes(50, 4);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apache.arrow.stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let port = serve_once(head, body, 0.5); // 只发一半就断
    let (dir, cfg) = tmp_cfg("drop");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=secret", port);
    let err = expect_err(download_and_generate(&url, &cfg));
    std::fs::remove_dir_all(&dir).ok();
    assert!(err.contains("导出"), "中途断开应走友好映射（导出中断/未完成/处理失败），实际: {err}");
    assert!(!err.contains("secret"), "错误信息不应泄露 URL/参数: {err}");
}

/// 完整性校验：声明总数远大于实收 → 判为未完成（杜绝静默残缺文件）。
#[test]
fn http_incomplete_guard() {
    let body = arrow_bytes(10, 2);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apache.arrow.stream\r\nX-Export-Total-Orders: 100\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let port = serve_once(head, body, 1.0); // 完整发送，但仅 10 单 vs 声明 100
    let (dir, cfg) = tmp_cfg("incomplete");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=t", port);
    let err = expect_err(download_and_generate(&url, &cfg));
    std::fs::remove_dir_all(&dir).ok();
    assert!(err.contains("导出未完成"), "总数100实收10应判未完成，实际: {err}");
}

fn arrow_head(len: usize) -> String {
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apache.arrow.stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n")
}

/// 流在消息边界被截断（缺 EOS 结束标记）：Arrow 会当作正常结束，必须靠 EOS 校验判为未完成。
#[test]
fn http_missing_eos_is_incomplete() {
    let mut body = arrow_bytes(10, 2);
    body.truncate(body.len() - 8); // 去掉 EOS：0xFFFFFFFF 00000000
    let port = serve_once(arrow_head(body.len()), body, 1.0);
    let (dir, cfg) = tmp_cfg("noeos");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=t", port);
    let err = expect_err(download_and_generate(&url, &cfg));
    std::fs::remove_dir_all(&dir).ok();
    assert!(err.contains("未完成"), "缺 EOS 应判未完成，实际: {err}");
}

/// 取消标志已置位 → 请求返回后、读响应体前即中止，提示「已取消导出」。
#[test]
fn http_cancelled() {
    let body = arrow_bytes(10, 2);
    let port = serve_once(arrow_head(body.len()), body, 1.0);
    let (dir, cfg) = tmp_cfg("cancel");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=t", port);
    let err = expect_err(download_and_generate_cb(&url, &cfg, &AtomicBool::new(true), |_| {}));
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(err, "已取消导出");
}

/// 不跟随重定向：302 指向一个能正常出流的地址，也必须拒绝（防可信来源被带到任意地址），且不回显 URL。
#[test]
fn http_redirect_rejected() {
    let body = arrow_bytes(10, 2);
    let target = serve_once(arrow_head(body.len()), body, 1.0);
    let head = format!(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target}/evil\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let port = serve_once(head, Vec::new(), 1.0);
    let (dir, cfg) = tmp_cfg("redirect");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=secret", port);
    let err = expect_err(download_and_generate(&url, &cfg));
    std::fs::remove_dir_all(&dir).ok();
    assert!(err.contains("302") && err.contains("非预期"), "302 应被拒绝，实际: {err}");
    assert!(!err.contains("secret") && !err.contains("evil"), "不应回显 URL/Location: {err}");
}

/// 服务端「卡住」：接受连接、发完 `prefix`（可为空）后既不再发也不关连接，客户端读会一直阻塞。
/// 300ms 后另一线程置取消标志 → `download_and_generate_bg` 须不等阻塞中的读、立即返回「已取消导出」。
fn cancel_while_stalled(prefix: Vec<u8>, tag: &str) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(&prefix);
            let _ = stream.flush();
            thread::sleep(Duration::from_secs(60)); // 持有连接不关
        }
    });
    let (dir, cfg) = tmp_cfg(tag);
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        c.store(true, Ordering::Relaxed);
    });
    let t0 = Instant::now();
    let (res, _worker) = download_and_generate_bg(format!("http://127.0.0.1:{port}/s?job=j&token=t"), cfg, cancel, |_| {});
    let elapsed = t0.elapsed();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(expect_err(res), "已取消导出");
    assert!(elapsed < Duration::from_secs(3), "取消应立即返回，实际耗时 {elapsed:?}");
}

/// 等响应头期间取消（服务端首批数据迟迟没凑齐，一个字节都没发）。
#[test]
fn http_cancel_while_waiting_headers() {
    cancel_while_stalled(Vec::new(), "stall_head");
}

/// 已收到响应头和半条流后服务端停顿，此时取消。
#[test]
fn http_cancel_while_body_stalled() {
    let body = arrow_bytes(10, 2);
    let mut prefix = arrow_head(body.len()).into_bytes();
    prefix.extend_from_slice(&body[..body.len() / 2]);
    cancel_while_stalled(prefix, "stall_body");
}

/// 服务端已发出响应头和 schema、首批数据前出错断开（DB 异常/超时）→ 提示服务端出错，不引导用户去重新登录。
#[test]
fn http_drop_before_first_batch() {
    let body = arrow_bytes(10, 2);
    // schema 消息 = continuation(4) + 元数据长度(4) + 元数据；响应头仍声明完整长度，发到 cut 就断开。
    // 断在 schema 之后（消息边界，走 EOS 校验）和断在首批数据中间（走读错误分支）都应给同一提示。
    let schema_end = 8 + i32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
    for cut in [schema_end, schema_end + 16] {
        let port = serve_once(arrow_head(body.len()), body[..cut].to_vec(), 1.0);
        let (dir, cfg) = tmp_cfg(&format!("nobatch{cut}"));
        let url = format!("http://127.0.0.1:{}/s?job=j&token=t", port);
        let err = expect_err(download_and_generate(&url, &cfg));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(err, "导出中断：服务端在返回数据前出错或连接已断开，请稍后在页面重新发起导出。", "cut={cut}");
    }
}

/// 工作线程版本的成功路径：结果与直接调用一致，工作线程正常结束。
#[test]
fn http_bg_ok_path() {
    let body = arrow_bytes(10, 2);
    let port = serve_once(arrow_head(body.len()), body, 1.0);
    let (dir, cfg) = tmp_cfg("bgok");
    let (res, worker) = download_and_generate_bg(format!("http://127.0.0.1:{port}/s?job=j&token=t"), cfg, Arc::new(AtomicBool::new(false)), |_| {});
    let res = res.map(|r| (r.orders, r.rows, r.files.len()));
    let joined = worker.join();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(res.unwrap(), (10, 20, 3));
    assert!(joined.is_ok(), "工作线程应正常结束");
}

/// 工作线程 panic（如进度回调出错）→ 映射为「内部错误」，调用方不会挂住。
#[test]
fn http_bg_panic_maps_to_internal_error() {
    let body = arrow_bytes(10, 2);
    let port = serve_once(arrow_head(body.len()), body, 1.0);
    let (dir, cfg) = tmp_cfg("bgpanic");
    let (res, worker) = download_and_generate_bg(format!("http://127.0.0.1:{port}/s?job=j&token=t"), cfg, Arc::new(AtomicBool::new(false)), |_| panic!("emit boom"));
    let err = expect_err(res);
    let joined = worker.join();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(err, "导出处理失败，请重试（内部错误）");
    assert!(joined.is_err(), "工作线程应已 panic");
}

/// 数据已开始（第 1 批已解出）后在第 2 批中途断开 → 仍是「连接已断开」提示（区别于首批前断开）。
#[test]
fn http_drop_after_data_started() {
    let one = arrow_bytes(10, 2); // schema + 第 1 批 + EOS
    let body = arrow_bytes_batches(10, 2, 2); // schema + 第 1 批 + 第 2 批 + EOS（前两段与 one 相同）
    let batch2_start = one.len() - 8;
    let cut = batch2_start + (body.len() - one.len()) / 2; // 第 2 批中间
    let port = serve_once(arrow_head(body.len()), body[..cut].to_vec(), 1.0);
    let (dir, cfg) = tmp_cfg("drop2");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=t", port);
    let err = expect_err(download_and_generate(&url, &cfg));
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(err, "导出中断：与服务器的连接已断开（可能是登录失效或网络波动），请重新登录后在页面重新发起导出。");
}

/// 空导出（只有 schema + EOS、0 行、无总数头）→ 正常完成，0 单 0 行。
#[test]
fn http_empty_export_ok() {
    let body = arrow_bytes_batches(10, 2, 0);
    let port = serve_once(arrow_head(body.len()), body, 1.0);
    let (dir, cfg) = tmp_cfg("empty");
    let url = format!("http://127.0.0.1:{}/s?job=j&token=t", port);
    let res = download_and_generate(&url, &cfg).map(|r| (r.orders, r.rows));
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(res.unwrap(), (0, 0));
}
