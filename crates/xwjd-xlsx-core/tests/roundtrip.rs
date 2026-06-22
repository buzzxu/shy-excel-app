//! 端到端：合成多层 Arrow IPC（含 field metadata）→ 生成核心 → 校验文件数 + 多层合并区数。
//! 模型：2 层（订单→明细），订单级 2 个 merge 列；每订单 2 明细 → 合并均为真 merge_range。

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use xwjd_xlsx_core::{generate_from_arrow, GenConfig};

fn meta(p: &[(&str, &str)]) -> HashMap<String, String> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn multi_level_merge_chunking() {
    let n_orders = 100usize;
    let k = 2usize;

    let fields = vec![
        Field::new("__gid0", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "0")])),
        Field::new("__gid1", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "1")])),
        Field::new("c0", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "序号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "10")])),
        Field::new("c1", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "订单编号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "20")])),
        Field::new("c2", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "商品"), ("merge", "false"), ("level", "1"), ("type", "STRING"), ("width", "20")])),
        Field::new("c3", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "金额"), ("merge", "false"), ("level", "1"), ("type", "STRING"), ("width", "15")])),
    ];
    let schema = Arc::new(Schema::new(fields));

    let (mut gid0, mut gid1) = (Vec::new(), Vec::new());
    let (mut c0, mut c1, mut c2, mut c3) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut item = 0i64;
    for o in 0..n_orders {
        for _ in 0..k {
            gid0.push(o as i64);
            gid1.push(item);
            item += 1;
            c0.push(format!("{}", o + 1));
            c1.push(format!("DD{:06}", o));
            c2.push(format!("商品{}", item));
            c3.push(format!("{}.00", item));
        }
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(gid0)),
        Arc::new(Int64Array::from(gid1)),
        Arc::new(StringArray::from(c0)),
        Arc::new(StringArray::from(c1)),
        Arc::new(StringArray::from(c2)),
        Arc::new(StringArray::from(c3)),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = StreamWriter::try_new(&mut buf, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    let dir = std::env::temp_dir().join(format!("xlsxcore_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = GenConfig { out_dir: dir.clone(), base_name: "t".into(), orders_per_file: 30 };
    let res = generate_from_arrow(Cursor::new(buf), &cfg).unwrap();

    assert_eq!(res.orders, 100, "订单数");
    assert_eq!(res.rows, 200, "渲染行数 = 100*2");
    assert_eq!(res.files.len(), 4, "ceil(100/30)=4 文件");

    assert_eq!(count_merge_cells(&res.files[0]), 60, "file1 = 30 订单 × 2 合并列");
    assert_eq!(count_merge_cells(res.files.last().unwrap()), 20, "file4 = 10 订单 × 2 合并列");

    std::fs::remove_dir_all(&dir).ok();
}

fn count_merge_cells(path: &std::path::Path) -> usize {
    let f = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(f).unwrap();
    let mut xml = String::new();
    zip.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut xml).unwrap();
    xml.matches("<mergeCell ").count()
}
