//! 集成：本地 HTTP 服务 Arrow IPC 字节 → ureq 拉流 → 生成。验证客户端数据通道（M-D2）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use shy_export_cli::{download_and_generate, GenConfig};

fn meta(p: &[(&str, &str)]) -> HashMap<String, String> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn arrow_bytes(n_orders: usize, k: usize) -> Vec<u8> {
    let fields = vec![
        Field::new("__gid0", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "0")])),
        Field::new("__gid1", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "1")])),
        Field::new("c0", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "订单编号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "20")])),
        Field::new("c1", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "商品"), ("merge", "false"), ("level", "1"), ("type", "STRING"), ("width", "20")])),
    ];
    let schema = Arc::new(Schema::new(fields));
    let (mut g0, mut g1, mut c0, mut c1) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut item = 0i64;
    for o in 0..n_orders {
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
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let mut buf = Vec::new();
    {
        let mut w = StreamWriter::try_new(&mut buf, &schema).unwrap();
        w.write(&batch).unwrap();
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
