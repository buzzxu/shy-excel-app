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
        // gid1 携带组头文本「明细」（引出 level 1 的分组），客户端据此渲染分组表头。
        Field::new("__gid1", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "1"), ("group", "明细")])),
        Field::new("c0", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "序号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "10")])),
        Field::new("c1", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "订单编号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "20")])),
        Field::new("c2", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "商品"), ("merge", "false"), ("level", "1"), ("type", "STRING"), ("width", "20")])),
        Field::new("c3", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "金额"), ("merge", "false"), ("level", "1"), ("type", "STRING"), ("width", "15")])),
    ];
    // schema 级标题（整行合并的标题行）。
    let schema = Arc::new(Schema::new(fields).with_metadata(meta(&[("sheet_title", "订单列表")])));

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
    assert_eq!(res.rows, 200, "渲染行数 = 100*2（表头不计入）");
    assert_eq!(res.files.len(), 4, "ceil(100/30)=4 文件");

    // 表头新增合并：标题行整行合并(1) + level0 两个 merge 列纵向跨表头(2) + 「明细」组头横向(1) = 4。
    // 数据合并：每订单 2 个 merge 列。file1=30 订单 → 60；末文件=10 订单 → 20。
    assert_eq!(count_merge_cells(&res.files[0]), 60 + 4, "file1 = 60 数据合并 + 4 表头合并");
    assert_eq!(count_merge_cells(res.files.last().unwrap()), 20 + 4, "file4 = 20 数据合并 + 4 表头合并");

    // 分组表头渲染：标题与组头文本须出现在 sharedStrings（证明已渲染，不再是平铺单行表头）。
    let ss = shared_strings(&res.files[0]);
    assert!(ss.contains("订单列表"), "应有标题行「订单列表」");
    assert!(ss.contains("明细"), "应有「明细」组头");

    std::fs::remove_dir_all(&dir).ok();
}

/// 3 层（订单→子订单→明细）分组表头几何校验：与 shyexcel/报货单 参考结构逐格对齐，杜绝平铺重复列。
#[test]
fn three_level_grouped_header() {
    // 列布局：L0=[序号*,订单编号*] L1=[子单号*,订单金额] L2=[产品,数量]（*=数据合并列）
    let fields = vec![
        Field::new("__gid0", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "0")])),
        Field::new("__gid1", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "1"), ("group", "子订单")])),
        Field::new("__gid2", DataType::Int64, true).with_metadata(meta(&[("role", "gid"), ("level", "2"), ("group", "明细")])),
        Field::new("c0", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "序号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "10")])),
        Field::new("c1", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "订单编号"), ("merge", "true"), ("level", "0"), ("type", "STRING"), ("width", "20")])),
        Field::new("c2", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "子单号"), ("merge", "true"), ("level", "1"), ("type", "STRING"), ("width", "20")])),
        Field::new("c3", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "订单金额"), ("merge", "false"), ("level", "1"), ("type", "STRING"), ("width", "15")])),
        Field::new("c4", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "产品"), ("merge", "false"), ("level", "2"), ("type", "STRING"), ("width", "20")])),
        Field::new("c5", DataType::Utf8, true).with_metadata(meta(&[("role", "col"), ("title", "数量"), ("merge", "false"), ("level", "2"), ("type", "STRING"), ("width", "10")])),
    ];
    let schema = Arc::new(Schema::new(fields).with_metadata(meta(&[("sheet_title", "订单列表")])));

    let (mut g0, mut g1, mut g2) = (Vec::new(), Vec::new(), Vec::new());
    let mut cols: Vec<Vec<String>> = vec![Vec::new(); 6];
    let (mut info, mut it) = (0i64, 0i64);
    for o in 0..2 {
        for _f in 0..1 {
            info += 1;
            for _ in 0..2 {
                it += 1;
                g0.push(o as i64);
                g1.push(info);
                g2.push(it);
                cols[0].push(format!("{}", o + 1));
                cols[1].push(format!("DD{:04}", o));
                cols[2].push(format!("ZD{:04}", info));
                cols[3].push("100.00".into());
                cols[4].push(format!("产品{}", it));
                cols[5].push(format!("{}", it));
            }
        }
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(g0)),
        Arc::new(Int64Array::from(g1)),
        Arc::new(Int64Array::from(g2)),
        Arc::new(StringArray::from(cols[0].clone())),
        Arc::new(StringArray::from(cols[1].clone())),
        Arc::new(StringArray::from(cols[2].clone())),
        Arc::new(StringArray::from(cols[3].clone())),
        Arc::new(StringArray::from(cols[4].clone())),
        Arc::new(StringArray::from(cols[5].clone())),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = StreamWriter::try_new(&mut buf, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }
    let dir = std::env::temp_dir().join(format!("xlsxcore_3l_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = GenConfig { out_dir: dir.clone(), base_name: "t".into(), orders_per_file: 1000 };
    let res = generate_from_arrow(Cursor::new(buf), &cfg).unwrap();

    let xml = sheet_xml(&res.files[0], "xl/worksheets/sheet1.xml");
    // 表头几何（title_offset=1, 3 层 → 表头 4 行，数据从第 5 行）。数据列只有 6 个（c0..c5 → A..F），gid 列不写出。
    assert!(xml.contains("ref=\"A1:F1\""), "标题行整行合并 A1:F1");
    assert!(xml.contains("ref=\"A2:A4\""), "L0 序号 纵向合并 A2:A4");
    assert!(xml.contains("ref=\"B2:B4\""), "L0 订单编号 纵向合并 B2:B4");
    assert!(xml.contains("ref=\"C2:F2\""), "「子订单」组头横跨 C2:F2");
    assert!(xml.contains("ref=\"C3:C4\""), "L1 子单号 纵向合并 C3:C4");
    assert!(xml.contains("ref=\"D3:D4\""), "L1 订单金额 纵向合并 D3:D4");
    assert!(xml.contains("ref=\"E3:F3\""), "「明细」组头横跨 E3:F3");
    // 数据从第 5 行起（叶子层在第 4 行表头，故首条数据行号=5）。
    assert!(xml.contains("<row r=\"5\""), "数据应从第 5 行开始");

    std::fs::remove_dir_all(&dir).ok();
}

fn count_merge_cells(path: &std::path::Path) -> usize {
    sheet_xml(path, "xl/worksheets/sheet1.xml").matches("<mergeCell ").count()
}

fn shared_strings(path: &std::path::Path) -> String {
    sheet_xml(path, "xl/sharedStrings.xml")
}

fn sheet_xml(path: &std::path::Path, name: &str) -> String {
    let f = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(f).unwrap();
    let mut xml = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut xml).unwrap();
    xml
}
