//! 合并单元格开销实测（解答「30k 行客户端导出慢，是不是 Excel 生成太慢」）。
//! 用真实列结构（L0=16 合并列、L1=25 合并列、L2=6 叶子列）合成 30k 叶子行，
//! 对比「全合并」vs「不合并（值写首行）」的生成耗时 / 合并区数 / 文件大小。
//! 运行：cargo test -p xwjd-xlsx-core --release --test perf_merge -- --ignored --nocapture

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use xwjd_xlsx_core::{generate_from_arrow, GenConfig};

fn meta(p: &[(&str, &str)]) -> HashMap<String, String> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// 合成 Arrow：orders×subs×items 叶子行；L0/L1 列按 merge 标志、L2 叶子非合并。
fn build(orders: usize, subs: usize, items: usize, l0: usize, l1: usize, l2: usize, merge: bool) -> Vec<u8> {
    let mut fields = vec![
        Field::new("__gid0", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "0")])),
        Field::new("__gid1", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "1"), ("group", "子订单")])),
        Field::new("__gid2", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "2"), ("group", "明细")])),
    ];
    let m = if merge { "true" } else { "false" };
    let mut ci = 0;
    for _ in 0..l0 { fields.push(Field::new(format!("c{ci}"), DataType::Utf8, true).with_metadata(meta(&[("role","col"),("title","主单列"),("merge",m),("level","0"),("type","STRING"),("width","15")]))); ci+=1; }
    for _ in 0..l1 { fields.push(Field::new(format!("c{ci}"), DataType::Utf8, true).with_metadata(meta(&[("role","col"),("title","子单列"),("merge",m),("level","1"),("type","STRING"),("width","15")]))); ci+=1; }
    for _ in 0..l2 { fields.push(Field::new(format!("c{ci}"), DataType::Utf8, true).with_metadata(meta(&[("role","col"),("title","明细列"),("merge","false"),("level","2"),("type","STRING"),("width","15")]))); ci+=1; }
    let schema = Arc::new(Schema::new(fields).with_metadata(meta(&[("sheet_title", "订单列表")])));

    let total = orders * subs * items;
    let ncol = l0 + l1 + l2;
    let (mut g0, mut g1, mut g2) = (Vec::with_capacity(total), Vec::with_capacity(total), Vec::with_capacity(total));
    let mut cols: Vec<Vec<String>> = vec![Vec::with_capacity(total); ncol];
    let (mut sub_id, mut item_id) = (0i64, 0i64);
    for o in 0..orders {
        for _ in 0..subs {
            sub_id += 1;
            for _ in 0..items {
                item_id += 1;
                g0.push(o as i64); g1.push(sub_id); g2.push(item_id);
                for k in 0..ncol { cols[k].push(format!("值{}", k)); }
            }
        }
    }
    let mut arrays: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(g0)), Arc::new(Int64Array::from(g1)), Arc::new(Int64Array::from(g2))];
    for k in 0..ncol { arrays.push(Arc::new(StringArray::from(std::mem::take(&mut cols[k])))); }
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    let mut buf = Vec::new();
    { let mut w = StreamWriter::try_new(&mut buf, &schema).unwrap(); w.write(&batch).unwrap(); w.finish().unwrap(); }
    buf
}

fn run(label: &str, bytes: Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("xlsxperf_{}_{}", label, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = GenConfig { out_dir: dir.clone(), base_name: "t".into(), orders_per_file: 1_000_000 };
    let t = Instant::now();
    let res = generate_from_arrow(Cursor::new(bytes), &cfg).unwrap();
    let dt = t.elapsed();
    let mut merges = 0usize; let mut size = 0u64;
    for f in &res.files {
        size += std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        let file = std::fs::File::open(f).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        zip.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut xml).unwrap();
        merges += xml.matches("<mergeCell ").count();
    }
    println!("[{label}] rows={} files={} 耗时={:?} 合并区={} 文件={}KB", res.rows, res.files.len(), dt, merges, size/1024);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[ignore]
fn perf_30k_merge_vs_nomerge() {
    // 真实列结构：L0=16 合并列、L1=25 合并列、L2=6 叶子列；1万订单×1子单×3明细 = 3万叶子行
    println!("\n== 3万叶子行（1万订单×1×3），41 合并列 ==");
    run("合并", build(10_000, 1, 3, 16, 25, 6, true));
    run("不合并", build(10_000, 1, 3, 16, 25, 6, false));

    // 放大到 9万叶子行（3万订单×1×3）看趋势
    println!("\n== 9万叶子行（3万订单×1×3）==");
    run("合并", build(30_000, 1, 3, 16, 25, 6, true));
    run("不合并", build(30_000, 1, 3, 16, 25, 6, false));
}
