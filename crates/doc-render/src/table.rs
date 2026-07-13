//! 表格映射(C-7):doc-core [`Table`](DocTable) → 引擎 [`TableSpec`]。
//!
//! 引擎的表格原语只会「量格子、画格子」(每格四边独立线 op + 填充 + 标量 padding,
//! 行不跨页整行挪、行高 = 内容与 `min_height` 取大)。所有 OOXML 语义在这里预解干净:
//!
//! - **span map**:`gridSpan`/`vMerge` 压平成 `行 × 列` 占位网格——每个网格位恰好
//!   一个引擎格;合并区的内容/底纹落在锚格(vMerge restart 格),被吞并的位置发空格
//!   延续底纹;合并区**内部**不画线。锚格内容排在锚行(行高随内容长,合并区总高
//!   ≥ 内容,v1 近似)。
//! - **边框冲突消解**:逐条物理边线解一次—— 每边先按「`tcBorders` > 表级」得出两侧
//!   各自的候选(表级 = 表样式链与直接 `tblBorders` 级联后的六槽:周边四槽 +
//!   insideH/insideV),再按**线重**归并共享边(`val="none"/"nil"` 重 0,可见边按
//!   `sz` 比大,平手取上/左侧)。归并后的线**只指派给一个格**(上边给下格的 top、
//!   左边给右格的 left;末行/末列的 bottom/right 兜底),每条物理边恰画一次,
//!   `get_drawings` 的线数确定。
//! - **单元格边距**:Word 缺省(左右 108 twip、上下 0)← 表级 `tblCellMar`(含表
//!   样式)← `tcMar`,逐边级联。引擎 padding 是四边同一标量:取上下边距均值
//!   (Word 缺省 0,排版高度保真);左右边距折成格内**顶层段落**的缩进(文本 x
//!   精确;嵌套表不吃左右边距,v1 近似)。
//! - **行高**:`trHeight` 按 `hRule` 语义——`auto` 忽略数值、`atLeast`/`exact` →
//!   `min_height`(引擎无“精确封顶”,exact 按下限近似,内容不截断)。行高超过
//!   一页正文高度 → [`RowTooTall`](crate::warn::RenderWarning::RowTooTall) 一次性
//!   告警(行不跨页语义下整行溢出);`cantSplit` 引擎内建(所有行整行挪页)。
//! - **vAlign**:center/bottom 映射到引擎单元格垂直锚定(`VAnchor`,TS-11)。
//! - **列宽**:`tblGrid` 的 dxa → `Fixed`;无 grid 时从首行 `tcW` 推导(dxa 绝对宽、
//!   pct 对当前节正文宽解析),都无 → `Auto`。`tblInd`/表级 `jc` 解析保真、v1 不
//!   参与布局(引擎表格自正文左缘起排)。嵌套表经块映射天然递归。
use doc_core::geom::{twips_to_points, Twips};
use doc_core::model::{Cell as DocCell, CellVAlign, HeightRule, Table as DocTable, VMerge};
use doc_core::style::{resolve_table, Border, CellMargins, ColorRef, EffectiveTableProps};
use doc_core::Document;
use pdf_typeset::{
    Block, BorderEdge, CellBorders, ColumnWidth, Rgb, TableCell, TableRow, TableSpec, VAnchor,
};

use crate::map::{map_blocks, rgb, MapCtx};

/// Word 缺省单元格边距(twip):左右 108(5.4pt)、上下 0(Word 缺省 tblCellMar)。
const DEFAULT_MARGINS: CellMargins = CellMargins {
    top: Some(0),
    left: Some(108),
    bottom: Some(0),
    right: Some(108),
};

/// span map 的一个合并区:锚格(承载内容/底纹/tcBorders)+ 锚位。
struct Region<'a> {
    row: usize,
    col: usize,
    cell: &'a DocCell,
}

/// 占位网格:`grid[r][c]` = 该网格位所属合并区的下标(缺格的参差位为 `None`)。
type Grid = Vec<Vec<Option<usize>>>;

/// 映射一张表格(嵌套表经 [`map_blocks`] 递归,表格样式上下文取最内层表)。
pub(crate) fn map_table(
    doc: &Document,
    table: &DocTable,
    ctx: &mut MapCtx,
    media: &std::collections::BTreeMap<String, Vec<u8>>,
) -> TableSpec {
    let eff = resolve_table(doc, table);
    let columns = column_policies(table, ctx);
    let ncols = columns.len();
    let nrows = table.rows.len();
    let (regions, grid) = build_span_map(table, ncols);
    let (h_edges, v_edges) = resolve_edges(doc, &eff, &regions, &grid, nrows, ncols);

    let mut rows = Vec::with_capacity(nrows);
    for (r, row) in table.rows.iter().enumerate() {
        let mut cells = Vec::with_capacity(ncols);
        for c in 0..ncols {
            let region = grid[r][c].map(|ai| &regions[ai]);
            let mut tc = match region {
                // 锚位:内容 + 底纹 + 边距落这里。
                Some(rg) if (rg.row, rg.col) == (r, c) => {
                    anchor_cell(doc, table, rg.cell, &eff, ctx, media)
                }
                // 被合并吞并的占位:底纹延续,无内容。
                Some(rg) => {
                    let mut t = TableCell::new(Vec::new());
                    t.fill = rg.cell.fill.map(rgb);
                    t
                }
                // 参差行的缺格:空补齐(周边线仍按表级画)。
                None => TableCell::new(Vec::new()),
            };
            // 每条物理边指派给一个格:上边归下格 top、左边归右格 left;
            // 末行/末列的 bottom/right 兜底。合并区内线在消解处已是 None。
            tc.borders = CellBorders {
                top: h_edges[r][c],
                bottom: if r + 1 == nrows {
                    h_edges[r + 1][c]
                } else {
                    None
                },
                left: v_edges[r][c],
                right: if c + 1 == ncols {
                    v_edges[r][c + 1]
                } else {
                    None
                },
            };
            cells.push(tc);
        }
        let mut tr = TableRow::new(cells);
        // trHeight 按 hRule:auto 忽略数值;atLeast(缺省)/exact → min_height
        // (引擎无“精确封顶”,exact 按下限近似,内容不截断)。
        tr.min_height = match row.height_rule {
            HeightRule::Auto => None,
            HeightRule::AtLeast | HeightRule::Exact => {
                row.height.filter(|h| *h > 0).map(twips_to_points)
            }
        };
        if tr.min_height.is_some_and(|h| h > ctx.body_height()) {
            ctx.row_too_tall(); // 行不跨页:超页高的行整行溢出(一次性告警)。
        }
        rows.push(tr);
    }
    TableSpec::new(columns, rows)
}

/// 列宽策略:`tblGrid` 的 dxa → `Fixed`(0/负值容错 `Auto`);无 grid 时从首行
/// `tcW` 推导(dxa 绝对宽;pct 对当前节正文宽解析;跨列格均摊到各列),都无 → `Auto`。
fn column_policies(table: &DocTable, ctx: &MapCtx) -> Vec<ColumnWidth> {
    if !table.grid_cols.is_empty() {
        return table
            .grid_cols
            .iter()
            .map(|&t| {
                if t > 0 {
                    ColumnWidth::Fixed(twips_to_points(t))
                } else {
                    ColumnWidth::Auto
                }
            })
            .collect();
    }
    let mut cols = vec![ColumnWidth::Auto; table.col_count().max(1)];
    if let Some(row) = table.rows.first() {
        let mut c = 0usize;
        for cell in &row.cells {
            let span = (cell.grid_span.max(1)) as usize;
            let width = match (cell.width, cell.width_pct) {
                (Some(t), _) if t > 0 => Some(twips_to_points(t)),
                (_, Some(p)) if p > 0.0 => Some(ctx.body_width() * f64::from(p) / 100.0),
                _ => None,
            };
            if let Some(w) = width {
                let per_col = w / span as f64;
                for slot in cols.iter_mut().skip(c).take(span) {
                    *slot = ColumnWidth::Fixed(per_col);
                }
            }
            c += span;
            if c >= cols.len() {
                break;
            }
        }
    }
    cols
}

/// 压平 `gridSpan`/`vMerge` 成占位网格。vMerge 延续格并入正上方网格位的合并区
/// (上方无可并区的畸形延续格容错为自立锚格);声明超出列数的格截断。
fn build_span_map(table: &DocTable, ncols: usize) -> (Vec<Region<'_>>, Grid) {
    let nrows = table.rows.len();
    let mut regions: Vec<Region<'_>> = Vec::new();
    let mut grid: Grid = vec![vec![None; ncols]; nrows];
    for (r, row) in table.rows.iter().enumerate() {
        let mut c = 0usize;
        for cell in &row.cells {
            if c >= ncols {
                break; // 声明列数以 grid 为准(与旧行为一致)。
            }
            let span = (cell.grid_span.max(1)) as usize;
            let owner = if cell.v_merge == VMerge::Continue {
                r.checked_sub(1).and_then(|pr| grid[pr][c])
            } else {
                None
            };
            let idx = owner.unwrap_or_else(|| {
                regions.push(Region {
                    row: r,
                    col: c,
                    cell,
                });
                regions.len() - 1
            });
            for slot in grid[r].iter_mut().skip(c).take(span) {
                *slot = Some(idx);
            }
            c += span;
        }
    }
    (regions, grid)
}

/// 逐条物理边消解最终线:横边表 `(nrows+1) × ncols`(`h[r][c]` = 第 `r` 行上方那段),
/// 竖边表 `nrows × (ncols+1)`(`v[r][c]` = 第 `c` 列左侧那段)。合并区内线 → `None`;
/// 两侧候选各按「tcBorders > 表级(周边槽/inside 槽)」取得,再按线重归并。
#[allow(clippy::type_complexity)]
fn resolve_edges(
    doc: &Document,
    eff: &EffectiveTableProps,
    regions: &[Region<'_>],
    grid: &Grid,
    nrows: usize,
    ncols: usize,
) -> (Vec<Vec<Option<BorderEdge>>>, Vec<Vec<Option<BorderEdge>>>) {
    let mut h = vec![vec![None; ncols]; nrows + 1];
    for (r, h_row) in h.iter_mut().enumerate() {
        for (c, slot) in h_row.iter_mut().enumerate() {
            let above = r.checked_sub(1).and_then(|pr| grid[pr][c]);
            let below = if r < nrows { grid[r][c] } else { None };
            if above.is_some() && above == below {
                continue; // 合并区内线:不画。
            }
            let table_layer = if r == 0 {
                eff.borders.top.as_ref()
            } else if r == nrows {
                eff.borders.bottom.as_ref()
            } else {
                eff.borders.inside_h.as_ref()
            };
            let side_a = above.map(|ai| regions[ai].cell.borders.bottom.as_ref().or(table_layer));
            let side_b = below.map(|ai| regions[ai].cell.borders.top.as_ref().or(table_layer));
            *slot = merge_sides(doc, side_a, side_b);
        }
    }
    let mut v = vec![vec![None; ncols + 1]; nrows];
    for (r, v_row) in v.iter_mut().enumerate() {
        for (c, slot) in v_row.iter_mut().enumerate() {
            let left = c.checked_sub(1).and_then(|pc| grid[r][pc]);
            let right = if c < ncols { grid[r][c] } else { None };
            if left.is_some() && left == right {
                continue; // 合并区内线:不画。
            }
            let table_layer = if c == 0 {
                eff.borders.left.as_ref()
            } else if c == ncols {
                eff.borders.right.as_ref()
            } else {
                eff.borders.inside_v.as_ref()
            };
            let side_a = left.map(|ai| regions[ai].cell.borders.right.as_ref().or(table_layer));
            let side_b = right.map(|ai| regions[ai].cell.borders.left.as_ref().or(table_layer));
            *slot = merge_sides(doc, side_a, side_b);
        }
    }
    (h, v)
}

/// 归并一条共享边两侧的候选:线重大者胜(平手取 `a` 侧——上/左,确定性);
/// 单侧存在(表格周边)即取该侧。候选是「tcBorders > 表级」已就位的每侧最终值。
fn merge_sides(
    doc: &Document,
    side_a: Option<Option<&Border>>,
    side_b: Option<Option<&Border>>,
) -> Option<BorderEdge> {
    let winner = match (side_a.flatten(), side_b.flatten()) {
        (Some(a), Some(b)) => Some(if weight(b) > weight(a) { b } else { a }),
        (a, b) => a.or(b),
    };
    winner.and_then(|b| stroke(doc, b))
}

/// 一条边的「线重」:`none`/`nil` 重 0(显式无线,能在共享边上输给可见线——Word
/// §17.4.39 的实践序);可见线按 `sz`(1/8 磅)比大,0 宽可见线保底重 2(hairline)。
fn weight(b: &Border) -> u32 {
    if is_visible(b) {
        b.sz_eighth_pt.max(2)
    } else {
        0
    }
}

/// 该线型是否可见(`none`/`nil` 之外都画;未知线型按单线近似,与解析容错一致)。
pub(crate) fn is_visible(b: &Border) -> bool {
    !matches!(b.val.as_str(), "none" | "nil")
}

/// 把一条(可见的)边折成引擎线:宽 = `sz`/8 磅(保底 0.25 hairline);色经 theme
/// 解引,`auto`/缺失按黑。不可见 → `None`。
pub(crate) fn stroke(doc: &Document, b: &Border) -> Option<BorderEdge> {
    if !is_visible(b) {
        return None;
    }
    let color = match b.color {
        Some(ColorRef::Rgb(c)) => rgb(c),
        Some(ColorRef::Theme(slot)) => doc.theme.colors.get(slot).map(rgb).unwrap_or(Rgb::BLACK),
        Some(ColorRef::Auto) | None => Rgb::BLACK,
    };
    Some(BorderEdge {
        width: (f64::from(b.sz_eighth_pt) / 8.0).max(0.25),
        color,
    })
}

/// 造一个锚格:内容块递归映射;有效边距(缺省 ← 表级 ← tcMar)折成
/// 「上下均值标量 padding + 顶层段落左右缩进」;vAlign 映射到引擎垂直锚定。
fn anchor_cell(
    doc: &Document,
    table: &DocTable,
    cell: &DocCell,
    eff: &EffectiveTableProps,
    ctx: &mut MapCtx,
    media: &std::collections::BTreeMap<String, Vec<u8>>,
) -> TableCell {
    let mut margins = DEFAULT_MARGINS;
    margins.overlay(&eff.cell_margins);
    margins.overlay(&cell.margins);
    let pt = |t: Option<Twips>| t.map(twips_to_points).unwrap_or(0.0).max(0.0);
    // 引擎 padding 是四边同一标量:取上下边距均值(Word 缺省 0——排版高度保真);
    // 左右边距的剩余量折成顶层段落缩进(文本 x 精确;嵌套表不吃左右边距,v1 近似)。
    let pad = (pt(margins.top) + pt(margins.bottom)) / 2.0;
    let (extra_left, extra_right) = (
        (pt(margins.left) - pad).max(0.0),
        (pt(margins.right) - pad).max(0.0),
    );
    let mut blocks = map_blocks(doc, &cell.blocks, Some(table), ctx, media);
    for block in &mut blocks {
        if let Block::Paragraph(props, _) = block {
            props.indent_left += extra_left;
            props.indent_right += extra_right;
        }
    }
    let mut tc = TableCell::new(blocks);
    tc.fill = cell.fill.map(rgb);
    tc.padding = pad;
    // vAlign(TS-11):center/bottom 映射到引擎的单元格垂直锚定(行高定型后偏移内容)。
    tc.v_align = match cell.v_align {
        Some(CellVAlign::Center) => VAnchor::Middle,
        Some(CellVAlign::Bottom) => VAnchor::Bottom,
        _ => VAnchor::Top,
    };
    tc
}

// ============================================================ 单测:span map 与边框消解的每条语义

#[cfg(test)]
mod tests {
    use doc_core::model::{
        Block as DocBlock, Cell, CellVAlign, Color, HeightRule, Paragraph, Row, Section,
        Table as DocTable, TextRun, VMerge,
    };
    use doc_core::style::{Border, ColorRef, TableBorders};
    use doc_core::Document;
    use pdf_typeset::{Block, ColumnWidth, Rgb, TableSpec, VAnchor};

    use crate::map::map_document;
    use crate::warn::RenderWarning;

    fn para_cell(text: &str) -> Cell {
        Cell {
            blocks: vec![DocBlock::Paragraph(Paragraph {
                runs: vec![TextRun::from_text(text)],
                ..Paragraph::default()
            })],
            grid_span: 1,
            ..Cell::default()
        }
    }

    fn border(sz: u32) -> Border {
        Border {
            val: "single".into(),
            sz_eighth_pt: sz,
            space_pt: 0,
            color: None,
        }
    }

    fn none_border() -> Border {
        Border {
            val: "none".into(),
            sz_eighth_pt: 0,
            space_pt: 0,
            color: None,
        }
    }

    /// 六槽表级边框:周边 sz(8)、insideH sz(4)、insideV sz(2)。
    fn full_borders() -> TableBorders {
        TableBorders {
            top: Some(border(8)),
            bottom: Some(border(8)),
            left: Some(border(8)),
            right: Some(border(8)),
            inside_h: Some(border(4)),
            inside_v: Some(border(2)),
        }
    }

    /// 映射一张表,取引擎 TableSpec 与告警。
    fn map(table: DocTable) -> (TableSpec, Vec<RenderWarning>) {
        let doc = Document {
            body: vec![DocBlock::Table(table)],
            sections: vec![Section {
                end_block: 1,
                ..Section::default()
            }],
            ..Document::default()
        };
        let mapped = map_document(&doc);
        let Block::Table(spec) = mapped.sections[0].blocks[0].clone() else {
            panic!("expected a table block");
        };
        (spec, mapped.warnings)
    }

    /// 全边框 2×2:每条物理边恰画一次(周边归边缘格、内线归下/右格),
    /// insideH/insideV 各归其位,共 12 段。
    #[test]
    fn full_borders_paint_each_physical_edge_once() {
        let table = DocTable {
            grid_cols: vec![2400, 2400],
            borders: full_borders(),
            rows: vec![
                Row {
                    cells: vec![para_cell("a"), para_cell("b")],
                    ..Row::default()
                },
                Row {
                    cells: vec![para_cell("c"), para_cell("d")],
                    ..Row::default()
                },
            ],
            ..DocTable::default()
        };
        let (spec, _) = map(table);
        let b = |r: usize, c: usize| spec.rows[r].cells[c].borders;
        // (0,0):周边 top/left;bottom/right 是内线,归 (1,0).top / (0,1).left。
        assert_eq!(b(0, 0).top.map(|e| e.width), Some(1.0));
        assert_eq!(b(0, 0).left.map(|e| e.width), Some(1.0));
        assert_eq!(b(0, 0).bottom, None);
        assert_eq!(b(0, 0).right, None);
        assert_eq!(
            b(0, 1).left.map(|e| e.width),
            Some(0.25),
            "insideV 归右格 left"
        );
        assert_eq!(
            b(1, 0).top.map(|e| e.width),
            Some(0.5),
            "insideH 归下格 top"
        );
        assert_eq!(b(1, 1).bottom.map(|e| e.width), Some(1.0), "末行周边兜底");
        assert_eq!(b(1, 1).right.map(|e| e.width), Some(1.0), "末列周边兜底");
        let painted: usize = spec
            .rows
            .iter()
            .flat_map(|r| r.cells.iter())
            .map(|c| {
                [
                    c.borders.top,
                    c.borders.bottom,
                    c.borders.left,
                    c.borders.right,
                ]
                .iter()
                .filter(|e| e.is_some())
                .count()
            })
            .sum();
        assert_eq!(painted, 12, "2×2 全边框 = 6 横段 + 6 竖段,每段一次");
    }

    /// tcBorders > tblBorders:显式边(含色)盖表级;显式 none 压掉周边线;
    /// 共享边上 none 输给邻格可见线(线重归并)。
    #[test]
    fn tc_borders_override_and_none_semantics() {
        let mut red = border(16);
        red.color = Some(ColorRef::Rgb(Color::new([0xFF, 0, 0])));
        let mut a = para_cell("a");
        a.borders.top = Some(red);
        a.borders.bottom = Some(none_border()); // 对 insideH:输给可见内线。
        let mut b_cell = para_cell("b");
        b_cell.borders.top = Some(none_border()); // 对周边 top:无对侧,压掉。
        let table = DocTable {
            grid_cols: vec![2400, 2400],
            borders: full_borders(),
            rows: vec![
                Row {
                    cells: vec![a, b_cell],
                    ..Row::default()
                },
                Row {
                    cells: vec![para_cell("c"), para_cell("d")],
                    ..Row::default()
                },
            ],
            ..DocTable::default()
        };
        let (spec, _) = map(table);
        let top_a = spec.rows[0].cells[0].borders.top.expect("tc top");
        assert_eq!(top_a.width, 2.0, "tcBorders sz16 盖过表级 sz8");
        assert_eq!(top_a.color, Rgb::new(1.0, 0.0, 0.0));
        assert_eq!(
            spec.rows[0].cells[1].borders.top, None,
            "显式 none 压掉周边线"
        );
        assert_eq!(
            spec.rows[1].cells[0].borders.top.map(|e| e.width),
            Some(0.5),
            "共享边:none(重 0)输给可见 insideH"
        );
    }

    /// gridSpan + vMerge 合并区:内部无线、底纹延续、内容只落锚格。
    #[test]
    fn merged_region_suppresses_inner_lines_and_extends_fill() {
        let mut anchor = para_cell("wide");
        anchor.grid_span = 2;
        anchor.v_merge = VMerge::Restart;
        anchor.fill = Some(Color::new([0xFF, 0xCC, 0x00]));
        let mut cont = Cell {
            grid_span: 2,
            v_merge: VMerge::Continue,
            ..Cell::default()
        };
        cont.fill = None; // 延续格自身无底纹:占位应从锚格继承。
        let table = DocTable {
            grid_cols: vec![2400, 2400, 2400],
            borders: full_borders(),
            rows: vec![
                Row {
                    cells: vec![anchor, para_cell("b")],
                    ..Row::default()
                },
                Row {
                    cells: vec![cont, para_cell("d")],
                    ..Row::default()
                },
            ],
            ..DocTable::default()
        };
        let (spec, _) = map(table);
        let cell = |r: usize, c: usize| &spec.rows[r].cells[c];
        let gold = Some(Rgb::new(1.0, 0.8, 0.0));
        assert!(!cell(0, 0).blocks.is_empty(), "内容落锚格");
        for (r, c) in [(0, 1), (1, 0), (1, 1)] {
            assert!(cell(r, c).blocks.is_empty(), "({r},{c}) 是被吞并的占位");
            assert_eq!(cell(r, c).fill, gold, "底纹延续到 ({r},{c})");
        }
        // 合并区内线全部压掉:横向 (0,0)-(0,1)、纵向 (0,*)-(1,*)、行 1 的 (1,0)-(1,1)。
        assert_eq!(cell(0, 1).borders.left, None);
        assert_eq!(cell(1, 0).borders.top, None);
        assert_eq!(cell(1, 1).borders.top, None);
        assert_eq!(cell(1, 1).borders.left, None);
        // 合并区与右列之间的竖线照画(insideV)。
        assert_eq!(cell(0, 2).borders.left.map(|e| e.width), Some(0.25));
        assert_eq!(cell(1, 2).borders.left.map(|e| e.width), Some(0.25));
    }

    /// 单元格边距:上下均值折进标量 padding,左右余量折进顶层段落缩进;
    /// Word 缺省(左右 108 twip)给 5.4pt 缩进、零 padding。
    #[test]
    fn cell_margins_fold_into_padding_and_indents() {
        let mut custom = para_cell("m");
        custom.margins.left = Some(288); // 14.4pt
        custom.margins.top = Some(120); // 6pt
        custom.margins.bottom = Some(120); // 6pt
        let table = DocTable {
            grid_cols: vec![2400, 2400],
            rows: vec![Row {
                cells: vec![custom, para_cell("n")],
                ..Row::default()
            }],
            ..DocTable::default()
        };
        let (spec, _) = map(table);
        let cell = &spec.rows[0].cells[0];
        assert!((cell.padding - 6.0).abs() < 1e-9, "padding = (6+6)/2");
        let Block::Paragraph(props, _) = &cell.blocks[0] else {
            panic!("expected a paragraph");
        };
        assert!((props.indent_left - 8.4).abs() < 1e-9, "14.4 − 6 = 8.4");
        assert_eq!(props.indent_right, 0.0, "5.4 − 6 → 0(不倒扣)");
        let plain = &spec.rows[0].cells[1];
        assert_eq!(plain.padding, 0.0, "Word 缺省上下边距 0");
        let Block::Paragraph(props, _) = &plain.blocks[0] else {
            panic!("expected a paragraph");
        };
        assert!(
            (props.indent_left - 5.4).abs() < 1e-9,
            "缺省左边距 108 twip"
        );
        assert!((props.indent_right - 5.4).abs() < 1e-9);
    }

    /// vAlign(center/bottom)与超页高行:各一次性告警;hRule 语义落 min_height。
    #[test]
    fn valign_tall_row_warn_and_hrule_semantics() {
        let mut centered = para_cell("v");
        centered.v_align = Some(CellVAlign::Center);
        let mut bottom = para_cell("w");
        bottom.v_align = Some(CellVAlign::Bottom);
        let table = DocTable {
            grid_cols: vec![2400, 2400],
            rows: vec![
                Row {
                    cells: vec![centered, bottom],
                    height: Some(20_000), // 1000pt > Letter 正文高 648pt。
                    height_rule: HeightRule::Exact,
                    ..Row::default()
                },
                Row {
                    cells: vec![para_cell("a"), para_cell("b")],
                    height: Some(400),
                    height_rule: HeightRule::Auto,
                    ..Row::default()
                },
                Row {
                    cells: vec![para_cell("c"), para_cell("d")],
                    height: Some(400),
                    ..Row::default()
                },
            ],
            ..DocTable::default()
        };
        let (spec, warnings) = map(table);
        assert_eq!(spec.rows[0].min_height, Some(1000.0), "exact 按下限近似");
        assert_eq!(spec.rows[1].min_height, None, "auto 忽略数值");
        assert_eq!(spec.rows[2].min_height, Some(20.0), "缺省 atLeast");
        // vAlign(TS-11):center/bottom 映射到引擎垂直锚定,不再降级;缺省顶对齐。
        assert_eq!(spec.rows[0].cells[0].v_align, VAnchor::Middle);
        assert_eq!(spec.rows[0].cells[1].v_align, VAnchor::Bottom);
        assert_eq!(spec.rows[2].cells[0].v_align, VAnchor::Top, "缺省顶对齐");
        let kinds: Vec<&str> = warnings.iter().map(RenderWarning::kind).collect();
        assert!(
            !kinds.contains(&"cell-valign-ignored"),
            "vAlign 已真渲染,不再有降级告警"
        );
        assert_eq!(kinds.iter().filter(|k| **k == "row-too-tall").count(), 1);
    }

    /// 无 tblGrid 的列宽推导:pct-tcW 对当前节正文宽解析(Letter 正文 468pt)。
    #[test]
    fn pct_cell_widths_resolve_against_body_width() {
        let mut left = para_cell("l");
        left.width_pct = Some(50.0);
        let mut right = para_cell("r");
        right.width = Some(2880); // dxa:144pt。
        let table = DocTable {
            rows: vec![Row {
                cells: vec![left, right],
                ..Row::default()
            }],
            ..DocTable::default()
        };
        let (spec, _) = map(table);
        assert_eq!(spec.columns.len(), 2);
        let ColumnWidth::Fixed(w0) = spec.columns[0] else {
            panic!("pct 列应是 Fixed");
        };
        assert!((w0 - 234.0).abs() < 1e-6, "50% × 468pt 正文宽");
        assert_eq!(spec.columns[1], ColumnWidth::Fixed(144.0));
    }
}
