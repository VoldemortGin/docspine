//! 表格映射:doc-core [`Table`](DocTable) → 引擎 [`TableSpec`]。
//!
//! C-1 的最小保真面:`w:tblGrid` 的 dxa 列宽 → [`ColumnWidth::Fixed`](dxa→pt,
//! C-2 的换算);`gridSpan` 跨列格后补空占位格保持列对齐(真正的跨列渲染与
//! `vMerge` 合并单元格是 C-7 的 span map);单元格底纹照画;行高 `trHeight` →
//! `min_height`(Word 缺省 hRule=atLeast 语义);嵌套表经块映射天然递归。
//! 表格/单元格边框(`tblBorders`/`tcBorders`)解析尚缺(C-7),本批不画线。

use doc_core::geom::twips_to_points;
use doc_core::model::Table as DocTable;
use doc_core::Document;
use pdf_typeset::{ColumnWidth, TableCell, TableRow, TableSpec};

use crate::map::{map_blocks, rgb, Warnings};

/// 单元格内边距(磅)。Word 缺省 tblCellMar 左右 108 twip = 5.4 pt、上下 0;引擎
/// 是四边统一的标量,v1 折中取 3 pt(tcMar 保真是 C-7)。
const CELL_PADDING_PT: f64 = 3.0;

/// 映射一张表格(嵌套表经 [`map_blocks`] 递归,表格样式上下文取最内层表)。
pub(crate) fn map_table(doc: &Document, table: &DocTable, w: &mut Warnings) -> TableSpec {
    let columns: Vec<ColumnWidth> = if table.grid_cols.is_empty() {
        vec![ColumnWidth::Auto; table.col_count()]
    } else {
        table
            .grid_cols
            .iter()
            .map(|&t| {
                if t > 0 {
                    ColumnWidth::Fixed(twips_to_points(t))
                } else {
                    ColumnWidth::Auto
                }
            })
            .collect()
    };
    let ncols = columns.len();

    let mut rows = Vec::with_capacity(table.rows.len());
    for row in &table.rows {
        let mut cells: Vec<TableCell> = Vec::with_capacity(ncols);
        for cell in &row.cells {
            let mut tc = TableCell::new(map_blocks(doc, &cell.blocks, Some(table), w));
            tc.fill = cell.fill.map(rgb);
            tc.padding = CELL_PADDING_PT;
            cells.push(tc);
            // gridSpan > 1:补空占位格保持后续列对齐(内容只落在首列;底纹跨列延续)。
            for _ in 1..cell.grid_span.max(1) {
                let mut filler = TableCell::new(Vec::new());
                filler.fill = cell.fill.map(rgb);
                filler.padding = CELL_PADDING_PT;
                cells.push(filler);
            }
        }
        cells.truncate(ncols.max(1)); // 防御:声明列数以 grid 为准。
        let mut tr = TableRow::new(cells);
        // trHeight 缺省规则 atLeast:内容可长高 → 引擎的 min_height 语义吻合。
        tr.min_height = row.height.map(twips_to_points);
        rows.push(tr);
    }
    TableSpec::new(columns, rows)
}
