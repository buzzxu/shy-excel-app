//! headless CLI：xwjd-export-cli <url> <out_dir> [base_name] [orders_per_file]
//! 用于联调/无 GUI 场景；Tauri 壳（M-D3）复用 lib 的 download_and_generate。
use std::path::PathBuf;
use xwjd_export_cli::{download_and_generate, GenConfig};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: xwjd-export-cli <url> <out_dir> [base_name] [orders_per_file]");
        std::process::exit(2);
    }
    let cfg = GenConfig {
        out_dir: PathBuf::from(&args[2]),
        base_name: args.get(3).cloned().unwrap_or_else(|| "导出".into()),
        orders_per_file: args.get(4).and_then(|s| s.parse().ok()).unwrap_or(70_000),
    };
    match download_and_generate(&args[1], &cfg) {
        Ok(r) => println!("OK files={} orders={} rows={}", r.files.len(), r.orders, r.rows),
        Err(e) => {
            eprintln!("ERR {e}");
            std::process::exit(1);
        }
    }
}
