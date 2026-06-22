//! headless CLI（联调/无 GUI；Tauri 壳 M-D3 复用 lib）：
//!   xwjd-export-cli <url> <out_dir> [base_name] [orders_per_file]
//!   xwjd-export-cli --local <arrow_file> <out_dir> [base_name] [orders_per_file]
use std::path::{Path, PathBuf};
use xwjd_export_cli::{download_and_generate, generate_local, GenConfig};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let local = args.get(1).map(|s| s == "--local").unwrap_or(false);
    let base_i = if local { 2 } else { 1 }; // 源参数位置
    if args.len() < base_i + 2 {
        eprintln!("用法: xwjd-export-cli [--local] <url|file> <out_dir> [base_name] [orders_per_file]");
        std::process::exit(2);
    }
    let cfg = GenConfig {
        out_dir: PathBuf::from(&args[base_i + 1]),
        base_name: args.get(base_i + 2).cloned().unwrap_or_else(|| "导出".into()),
        orders_per_file: args.get(base_i + 3).and_then(|s| s.parse().ok()).unwrap_or(70_000),
    };
    let res = if local {
        generate_local(Path::new(&args[base_i]), &cfg)
    } else {
        download_and_generate(&args[base_i], &cfg)
    };
    match res {
        Ok(r) => println!("OK files={} orders={} rows={}", r.files.len(), r.orders, r.rows),
        Err(e) => {
            eprintln!("ERR {e}");
            std::process::exit(1);
        }
    }
}
