//! 验证：单个大 batch 内进度也会多次回报（让用户感知「一直在导出」），且末次为最终值。

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use shy_xlsx_core::{generate_from_arrow_cb, GenConfig};

fn meta(p: &[(&str, &str)]) -> HashMap<String, String> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn progress_fires_multiple_times_within_one_batch() {
    // 把 5000 单放进**一个** Arrow batch（模拟服务端发大批次）。
    let fields = vec![
        Field::new("__gid0", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "0")])),
        Field::new("c0", DataType::Utf8, true)
            .with_metadata(meta(&[("role", "col"), ("title", "名称"), ("merge", "false"), ("level", "0"), ("width", "20")])),
    ];
    let schema = Arc::new(Schema::new(fields));
    let n = 5000i64;
    let gid0: Vec<i64> = (0..n).collect(); // 每行一个独立顶层组 → 5000 单 / 5000 行
    let c0: Vec<String> = (0..n).map(|i| format!("r{i}")).collect();
    let columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(gid0)), Arc::new(StringArray::from(c0))];
    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = StreamWriter::try_new(&mut buf, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    let dir = std::env::temp_dir().join(format!("xlsxcore_prog_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = GenConfig { out_dir: dir.clone(), base_name: "p".into(), orders_per_file: 1_000_000 };

    let mut calls = 0u32;
    let mut last = (0u64, 0u64);
    let res = generate_from_arrow_cb(Cursor::new(buf), &cfg, |orders, rows| {
        calls += 1;
        last = (orders, rows);
    })
    .unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(res.orders, 5000, "5000 单");
    assert_eq!(res.rows, 5000, "5000 行");
    // PROGRESS_EVERY_ROWS=2048 → 单个 batch 内应在 2048/4096 处 + 批末多次回报，> 1 次。
    assert!(calls > 1, "批次内应多次回报进度（让用户感知一直在导出），实际仅 {calls} 次");
    assert_eq!(last, (5000, 5000), "末次进度应为最终精确值");
}
