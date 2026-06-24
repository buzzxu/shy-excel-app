//! shy-xlsx-core — Arrow IPC 流 → 多层合并 xlsx 生成核心（REQ-2026-06-21-001 M-D1）。
//!
//! 自描述：从 Arrow schema 的 field metadata 读取列定义（title/merge/level/width）与 N 个 `__gid` 层级列，
//! **客户端无需硬编码任何业务列**（"客户端永不随新导出改动"）。
//!
//! 生成：每个 Arrow 行 = 一条最深叶子；按各 `__gidL` 在同层组内对 `merge=true` 列**逐层** `merge_range`。
//! 内存：normal 模式 + **多文件分块**（每 N 个顶层组一个文件，save 后 drop 释放）→ 峰值与总量无关（见 R-E）。
//!
//! PoC 实测（spike）：单文件 50w 7.5GB 不可行；5w/文件 → ~1.9GB。本核心把 spike 的单层合并推广到多层。

use arrow::array::{Array, Int64Array, StringArray};
use arrow::ipc::reader::StreamReader;
use rust_xlsxwriter::{Format, FormatAlign, FormatBorder, Workbook, Worksheet, XlsxError};
use std::io::Read;
use std::path::PathBuf;

/// 生成配置。
pub struct GenConfig {
    /// 输出目录。
    pub out_dir: PathBuf,
    /// 文件基名（不含序号/扩展名），如 `导出订单_2026-06-22`。
    pub base_name: String,
    /// 每文件顶层组（一个「单」：订单/工单等）数 —— 分块粒度，控内存（默认建议 7w）。
    pub orders_per_file: u64,
}

/// 生成结果。
pub struct GenResult {
    pub files: Vec<PathBuf>,
    pub orders: u64,
    pub rows: u64,
}

#[derive(Clone)]
struct ColMeta {
    title: String,
    merge: bool,
    level: usize,
    width: f64,
    batch_idx: usize,
    xlsx_col: u16,
}

/// 主入口：消费 Arrow IPC 流，按自描述 schema 生成（多文件分块）。
pub fn generate_from_arrow<R: Read>(reader: R, cfg: &GenConfig) -> Result<GenResult, Box<dyn std::error::Error>> {
    generate_from_arrow_cb(reader, cfg, |_, _| {})
}

/// 同 [`generate_from_arrow`]，但每处理完一个 Arrow batch 回调一次进度 `(已完成单数, 已完成行数)`，
/// 供 UI 实时更新进度条/ETA。回调在生成线程内被调用，应轻量（如发事件）。
pub fn generate_from_arrow_cb<R: Read, F: FnMut(u64, u64)>(
    reader: R,
    cfg: &GenConfig,
    mut on_progress: F,
) -> Result<GenResult, Box<dyn std::error::Error>> {
    let sr = StreamReader::try_new(reader, None)?;
    let schema = sr.schema();

    // 顶部标题（整行合并的标题行，如「订单列表」）；schema 级 metadata，无则空。
    let sheet_title = schema.metadata().get("sheet_title").cloned().unwrap_or_default();

    let mut gid_idx: Vec<usize> = Vec::new();
    // 每层「组头」文本（引出该层的上一层 collection 标题，如 子订单/明细）；第 0 层为空。与 gid_idx 平行。
    let mut group_labels: Vec<String> = Vec::new();
    let mut cols: Vec<ColMeta> = Vec::new();
    for (i, f) in schema.fields().iter().enumerate() {
        let md = f.metadata();
        match md.get("role").map(String::as_str) {
            Some("gid") => {
                let lvl: usize = md.get("level").and_then(|s| s.parse().ok()).unwrap_or(gid_idx.len());
                // 按 level 放置（B4 按 0..N-1 顺序产出，这里仍按 level 排）
                while gid_idx.len() <= lvl {
                    gid_idx.push(usize::MAX);
                    group_labels.push(String::new());
                }
                gid_idx[lvl] = i;
                if let Some(g) = md.get("group") {
                    group_labels[lvl] = g.clone();
                }
            }
            Some("col") => cols.push(ColMeta {
                title: md.get("title").cloned().unwrap_or_default(),
                merge: md.get("merge").map(|s| s == "true").unwrap_or(false),
                level: md.get("level").and_then(|s| s.parse().ok()).unwrap_or(0),
                width: md.get("width").and_then(|s| s.parse().ok()).unwrap_or(12.0),
                batch_idx: i,
                xlsx_col: 0,
            }),
            _ => {}
        }
    }
    for (k, c) in cols.iter_mut().enumerate() {
        c.xlsx_col = k as u16;
    }
    let num_levels = gid_idx.len().max(1);
    while group_labels.len() < num_levels {
        group_labels.push(String::new());
    }

    let mut gen = Generator::new(cfg, cols, num_levels, sheet_title, group_labels)?;
    let mut g = vec![0i64; num_levels];
    // 批次内进度回报粒度：每满这么多行也回调一次，保证 UI「已导出 单/行」持续跳动，
    // 即便服务端把整次导出放进一个大 batch（下游 Tauri 层 120ms 节流防事件风暴）。
    const PROGRESS_EVERY_ROWS: u64 = 2048;

    for batch in sr {
        let batch = batch?;
        let gids: Vec<&Int64Array> = gid_idx
            .iter()
            .map(|&i| batch.column(i).as_any().downcast_ref::<Int64Array>().expect("__gid 非 int64"))
            .collect();
        let strs: Vec<&StringArray> = gen
            .cols
            .iter()
            .map(|c| batch.column(c.batch_idx).as_any().downcast_ref::<StringArray>().expect("列非 utf8"))
            .collect();
        for r in 0..batch.num_rows() {
            for l in 0..num_levels {
                g[l] = gids[l].value(r);
            }
            gen.row(&g, &strs, r)?;
            if gen.total_rows % PROGRESS_EVERY_ROWS == 0 {
                on_progress(gen.total_orders, gen.total_rows);
            }
        }
        on_progress(gen.total_orders, gen.total_rows);
    }
    gen.finish()
}

struct Generator<'a> {
    cfg: &'a GenConfig,
    cols: Vec<ColMeta>,
    num_levels: usize,
    /// 每层级的 merge 列（值为 cols 下标），与 grp_val 平行。
    merge_by_level: Vec<Vec<usize>>,
    /// 非 merge 列（每行写）。
    nonmerge: Vec<usize>,
    fmt: Format,
    hdr: Format,
    title_fmt: Format,

    /// 顶部标题（整行合并）；空则不画标题行。
    sheet_title: String,
    /// 每层组头文本（len==num_levels，第 0 层为空）。
    group_labels: Vec<String>,
    /// 表头总行数（含标题行）；数据从该行开始。
    header_rows: u32,
    /// 每层第一列的 xlsx 列号（用于组头横向合并起点）。
    level_first_col: Vec<u16>,
    /// 最右数据列号（cols.len()-1）。
    last_col: u16,

    wb: Workbook,
    files: Vec<PathBuf>,
    file_no: u32,
    next_row: u32, // 下一个写入的 xlsx 行（0=表头）
    orders_in_file: u64,

    have_prev: bool,
    prev_g: Vec<i64>,
    grp_start: Vec<u32>,
    grp_val: Vec<Vec<String>>,

    total_orders: u64,
    total_rows: u64,
}

impl<'a> Generator<'a> {
    fn new(cfg: &'a GenConfig, cols: Vec<ColMeta>, num_levels: usize, sheet_title: String, group_labels: Vec<String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut merge_by_level = vec![Vec::new(); num_levels];
        let mut nonmerge = Vec::new();
        for (k, c) in cols.iter().enumerate() {
            if c.merge {
                merge_by_level[c.level.min(num_levels - 1)].push(k);
            } else {
                nonmerge.push(k);
            }
        }
        // 每层组头横向合并的起点列 = 该层及更深列里最小的列号（列按层级有序，故即第一个 level>=l 的列）。
        let mut level_first_col = vec![0u16; num_levels];
        for l in 0..num_levels {
            level_first_col[l] = cols.iter().find(|c| c.level >= l).map(|c| c.xlsx_col).unwrap_or(0);
        }
        let last_col = cols.len().saturating_sub(1) as u16;
        let title_offset: u32 = if sheet_title.is_empty() { 0 } else { 1 };
        let header_rows = title_offset + num_levels as u32; // 标题行 + 每层一行表头；数据从此行起
        let grp_val = merge_by_level.iter().map(|v| vec![String::new(); v.len()]).collect();
        let mut g = Generator {
            cfg,
            cols,
            num_levels,
            merge_by_level,
            nonmerge,
            fmt: Format::new().set_border(FormatBorder::Thin),
            hdr: Format::new().set_bold().set_align(FormatAlign::Center).set_align(FormatAlign::VerticalCenter).set_border(FormatBorder::Thin),
            title_fmt: Format::new().set_bold().set_align(FormatAlign::Center),
            sheet_title,
            group_labels,
            header_rows,
            level_first_col,
            last_col,
            wb: Workbook::new(),
            files: Vec::new(),
            file_no: 0,
            next_row: 0,
            orders_in_file: 0,
            have_prev: false,
            prev_g: vec![i64::MIN; num_levels],
            grp_start: vec![header_rows; num_levels],
            grp_val,
            total_orders: 0,
            total_rows: 0,
        };
        g.start_sheet()?;
        Ok(g)
    }

    /// 渲染表头：标题行(整行合并) + 每层一行表头（本层列纵向合并到最深表头行；上一层 collection 标题作组头横跨后代列）。
    /// 与 shyexcel 嵌套渲染一致：主单列 + 「子订单」组 + 「明细」组，重复列各归其组、不再平铺。
    fn start_sheet(&mut self) -> Result<(), XlsxError> {
        let title_offset: u32 = if self.sheet_title.is_empty() { 0 } else { 1 };
        let last_header_row = title_offset + self.num_levels as u32 - 1;
        let ws = self.wb.add_worksheet();

        // 列宽（按数据列设置一次）
        for c in &self.cols {
            if c.width > 0.0 {
                ws.set_column_width(c.xlsx_col, c.width)?;
            }
        }

        // 标题行（整行合并）
        if title_offset == 1 {
            if self.last_col > 0 {
                ws.merge_range(0, 0, 0, self.last_col, self.sheet_title.as_str(), &self.title_fmt)?;
            } else {
                ws.write_with_format(0, 0, self.sheet_title.as_str(), &self.title_fmt)?;
            }
        }

        // 分层表头
        for l in 0..self.num_levels {
            let hrow = title_offset + l as u32;
            // 本层各列：写在 hrow，纵向合并到最深表头行
            for c in self.cols.iter().filter(|c| c.level == l) {
                if last_header_row > hrow {
                    ws.merge_range(hrow, c.xlsx_col, last_header_row, c.xlsx_col, c.title.as_str(), &self.hdr)?;
                } else {
                    ws.write_with_format(hrow, c.xlsx_col, c.title.as_str(), &self.hdr)?;
                }
            }
            // 组头：引出本层的 collection 标题，写在上一层那一行(hrow-1)，横跨本层及更深列
            if l >= 1 && !self.group_labels[l].is_empty() {
                let grow = hrow - 1;
                let start_col = self.level_first_col[l];
                if self.last_col > start_col {
                    ws.merge_range(grow, start_col, grow, self.last_col, self.group_labels[l].as_str(), &self.hdr)?;
                } else {
                    ws.write_with_format(grow, start_col, self.group_labels[l].as_str(), &self.hdr)?;
                }
            }
        }

        self.next_row = self.header_rows;
        Ok(())
    }

    fn row(&mut self, g: &[i64], strs: &[&StringArray], r: usize) -> Result<(), Box<dyn std::error::Error>> {
        // 1) 最浅变化层级
        let chg = if !self.have_prev {
            0
        } else {
            (0..self.num_levels).find(|&l| g[l] != self.prev_g[l]).unwrap_or(self.num_levels)
        };

        // 2) 顶层边界 + 文件满 → 轮转（先关闭挂起组、save、新文件）
        if self.have_prev && chg == 0 && self.orders_in_file >= self.cfg.orders_per_file {
            self.rotate_file()?;
        }

        let cur = self.next_row;

        // 3) 关闭 chg..N 层级的上一组（merge_range 到上一行 cur-1）
        if self.have_prev && chg < self.num_levels {
            let ws = self.wb.worksheet_from_index(0)?;
            for l in (chg..self.num_levels).rev() {
                close_level(ws, &self.fmt, &self.cols, &self.merge_by_level[l], self.grp_start[l], cur - 1, &self.grp_val[l])?;
            }
        }

        // 4) 打开新组（capture 本行值）；首行打开所有层级
        let open_from = if self.have_prev { chg } else { 0 };
        for l in open_from..self.num_levels {
            self.grp_start[l] = cur;
            for (k, &ci) in self.merge_by_level[l].iter().enumerate() {
                self.grp_val[l][k] = cell(strs, ci, r);
            }
        }

        // 5) 写非 merge 列（每行）
        {
            let ws = self.wb.worksheet_from_index(0)?;
            for &ci in &self.nonmerge {
                ws.write_with_format(cur, self.cols[ci].xlsx_col, cell(strs, ci, r).as_str(), &self.fmt)?;
            }
        }

        // 6) 计数：新顶层组 = 新订单
        if chg == 0 {
            self.orders_in_file += 1;
            self.total_orders += 1;
        }
        self.next_row += 1;
        self.total_rows += 1;
        self.have_prev = true;
        self.prev_g.copy_from_slice(g);
        Ok(())
    }

    /// 关闭当前所有挂起组（到最后写入行），保存当前文件并开启新文件。
    fn rotate_file(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.close_all()?;
        self.save_current()?;
        self.wb = Workbook::new();
        self.start_sheet()?;
        self.have_prev = false;
        self.orders_in_file = 0;
        Ok(())
    }

    fn close_all(&mut self) -> Result<(), XlsxError> {
        if !self.have_prev {
            return Ok(());
        }
        let end = self.next_row - 1;
        let ws = self.wb.worksheet_from_index(0)?;
        for l in (0..self.num_levels).rev() {
            close_level(ws, &self.fmt, &self.cols, &self.merge_by_level[l], self.grp_start[l], end, &self.grp_val[l])?;
        }
        Ok(())
    }

    fn save_current(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.file_no += 1;
        let path = self.cfg.out_dir.join(format!("{}_part{:02}.xlsx", self.cfg.base_name, self.file_no));
        let mut old = std::mem::replace(&mut self.wb, Workbook::new());
        old.save(&path)?; // old drop -> 释放内存
        self.files.push(path);
        Ok(())
    }

    fn finish(mut self) -> Result<GenResult, Box<dyn std::error::Error>> {
        self.close_all()?;
        self.save_current()?;
        Ok(GenResult { files: self.files, orders: self.total_orders, rows: self.total_rows })
    }
}

/// 关闭一个层级的所有 merge 列：跨 [start,end] 合并；单行则退化为写单元格（merge_range 不接受 1x1）。
fn close_level(ws: &mut Worksheet, fmt: &Format, cols: &[ColMeta], merge_cols: &[usize], start: u32, end: u32, vals: &[String]) -> Result<(), XlsxError> {
    for (k, &ci) in merge_cols.iter().enumerate() {
        let col = cols[ci].xlsx_col;
        if end > start {
            ws.merge_range(start, col, end, col, vals[k].as_str(), fmt)?;
        } else {
            ws.write_with_format(start, col, vals[k].as_str(), fmt)?;
        }
    }
    Ok(())
}

#[inline]
fn cell(strs: &[&StringArray], k: usize, r: usize) -> String {
    if strs[k].is_null(r) {
        String::new()
    } else {
        strs[k].value(r).to_string()
    }
}
