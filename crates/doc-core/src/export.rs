//! 把已解析的 [`Document`] 结构化导出成纯文本 / Markdown / HTML。
//!
//! 这是纯函数式的「模型 -> 字符串」序列化:无 IO / zip / XML,只读领域模型。三种形态:
//! - [`to_text`]:全文按块拼成纯文本(段落换行、表格行内单元格按 tab、行间换行)。
//! - [`to_markdown`]:段落按空行分隔;标题样式(`Heading1`/`标题1`)映射成 `#`;
//!   **无合并的表格输出 GFM 管道表;一旦含合并单元格(`gridSpan` 横向 / `vMerge` 纵向)
//!   或嵌套表,则改用 HTML `<table>` 以保真 `rowspan`/`colspan`**(GFM 表无法表达合并)。
//! - [`to_html`]:段落 `<p>`、标题 `<h1>..<h6>`、表格 `<table>`(带 `rowspan`/`colspan`),
//!   文本经 HTML 转义。
//!
//! 容错:空段落跳过、空表跳过、未知样式当普通段落,绝不 panic。

use crate::model::{Block, Cell, Document, Table, VMerge};

// ============================================================ 纯文本

/// 全文按块拼成纯文本:段落各占一行,表格每行的单元格用 `\t` 连接,块/行之间用 `\n`。
pub fn to_text(doc: &Document) -> String {
    let mut out: Vec<String> = Vec::new();
    for b in &doc.body {
        match b {
            Block::Paragraph(p) => out.push(p.text()),
            Block::Table(t) => {
                for row in &t.rows {
                    let cells: Vec<String> = row.cells.iter().map(|c| c.text()).collect();
                    out.push(cells.join("\t"));
                }
            }
        }
    }
    out.join("\n")
}

// ============================================================ Markdown

/// 全文导出为 Markdown。段落以空行分隔;标题样式映射成 `#`;表格无合并时输出 GFM 管道表,
/// 含合并(或嵌套表)时退回 HTML `<table>` 以保真 `rowspan`/`colspan`。
pub fn to_markdown(doc: &Document) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in &doc.body {
        match b {
            Block::Paragraph(p) => {
                let t = p.text();
                if t.is_empty() {
                    continue;
                }
                match heading_level(p.style.as_deref()) {
                    Some(level) => parts.push(format!("{} {t}", "#".repeat(level as usize))),
                    None => parts.push(t),
                }
            }
            Block::Table(t) => {
                let md = markdown_table(t);
                if !md.is_empty() {
                    parts.push(md);
                }
            }
        }
    }
    parts.join("\n\n")
}

/// 一张表 -> Markdown。无合并/无嵌套表时用 GFM 管道表(首行作表头);否则退回 HTML 表。
fn markdown_table(table: &Table) -> String {
    if table_needs_html(table) {
        let mut s = String::new();
        push_html_table(table, &mut s);
        return s;
    }
    let ncols = table.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    if ncols == 0 {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for (i, row) in table.rows.iter().enumerate() {
        let mut cells: Vec<String> = row.cells.iter().map(md_cell_text).collect();
        while cells.len() < ncols {
            cells.push(String::new());
        }
        lines.push(format!("| {} |", cells.join(" | ")));
        // GFM 要求首行后紧跟一行分隔符。
        if i == 0 {
            lines.push(format!("| {} |", vec!["---"; ncols].join(" | ")));
        }
    }
    lines.join("\n")
}

/// GFM 单元格文字:换行规整为 `<br>`(否则会撑破表格),竖线转义,避免破坏管道语法。
fn md_cell_text(cell: &Cell) -> String {
    cell.text().replace('\n', "<br>").replace('|', "\\|")
}

/// 表格是否需要退回 HTML:任一单元格横向跨列 / 参与纵向合并 / 含嵌套表(GFM 表无法表达)。
fn table_needs_html(table: &Table) -> bool {
    table.rows.iter().any(|r| {
        r.cells.iter().any(|c| {
            c.grid_span > 1
                || c.v_merge != VMerge::None
                || c.blocks.iter().any(|b| matches!(b, Block::Table(_)))
        })
    })
}

// ============================================================ HTML

/// 全文导出为 HTML 片段:段落 `<p>`、标题 `<h1>..<h6>`、表格 `<table>`(带合并)。文本经转义。
pub fn to_html(doc: &Document) -> String {
    let mut out = String::new();
    for b in &doc.body {
        match b {
            Block::Paragraph(p) => {
                let t = p.text();
                if t.is_empty() {
                    continue;
                }
                match heading_level(p.style.as_deref()) {
                    Some(level) => {
                        out.push_str(&format!("<h{level}>{}</h{level}>\n", escape_html(&t)))
                    }
                    None => out.push_str(&format!("<p>{}</p>\n", escape_html(&t))),
                }
            }
            Block::Table(t) => {
                push_html_table(t, &mut out);
                out.push('\n');
            }
        }
    }
    out.trim_end().to_string()
}

/// 把一张表渲染成 HTML `<table>`,正确还原 `colspan`(`gridSpan`)与 `rowspan`(`vMerge`)。
///
/// 纵向合并语义:`restart` 格起始并向下吞并若干 `continue` 格;`continue` 格被吞掉,**不**
/// 单独输出 `<td>`。合并按**网格列**对齐(用 `gridSpan` 累加出每格的起始网格列号),不是按
/// 单元格序号,这样横向合并与纵向合并叠加时也对得上。
fn push_html_table(table: &Table, out: &mut String) {
    let starts = grid_starts(table);
    out.push_str("<table>\n");
    for (i, row) in table.rows.iter().enumerate() {
        out.push_str("<tr>\n");
        let tag = if row.is_header { "th" } else { "td" };
        for (ci, cell) in row.cells.iter().enumerate() {
            // 被纵向合并吞掉的延续格不单独输出。
            if cell.v_merge == VMerge::Continue {
                continue;
            }
            let colspan = cell.grid_span.max(1) as usize;
            let rowspan = if cell.v_merge == VMerge::Restart {
                vmerge_rowspan(table, &starts, i, starts[i][ci])
            } else {
                1
            };
            out.push('<');
            out.push_str(tag);
            if colspan > 1 {
                out.push_str(&format!(" colspan=\"{colspan}\""));
            }
            if rowspan > 1 {
                out.push_str(&format!(" rowspan=\"{rowspan}\""));
            }
            out.push('>');
            out.push_str(&html_cell_content(cell));
            out.push_str("</");
            out.push_str(tag);
            out.push_str(">\n");
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>");
}

/// 每行各单元格的**起始网格列号**(按 `gridSpan` 累加)。用于把 `vMerge` 延续格对齐到列。
fn grid_starts(table: &Table) -> Vec<Vec<usize>> {
    table
        .rows
        .iter()
        .map(|r| {
            let mut acc = 0usize;
            let mut v = Vec::with_capacity(r.cells.len());
            for c in &r.cells {
                v.push(acc);
                acc += c.grid_span.max(1) as usize;
            }
            v
        })
        .collect()
}

/// 从 `start_row` 的 `restart` 格(起始网格列 `g`)向下数有多少行在同列是 `continue`,得 `rowspan`。
fn vmerge_rowspan(table: &Table, starts: &[Vec<usize>], start_row: usize, g: usize) -> usize {
    let mut span = 1usize;
    for (row_starts, row) in starts.iter().zip(&table.rows).skip(start_row + 1) {
        let mut continues = false;
        for (cell, &start) in row.cells.iter().zip(row_starts) {
            match start.cmp(&g) {
                std::cmp::Ordering::Equal => {
                    continues = cell.v_merge == VMerge::Continue;
                    break;
                }
                std::cmp::Ordering::Greater => break, // 该列没有对应格。
                std::cmp::Ordering::Less => {}
            }
        }
        if continues {
            span += 1;
        } else {
            break;
        }
    }
    span
}

/// 单元格内容 -> HTML:段落文字转义后以 `<br>` 连接,嵌套表递归成内层 `<table>`。
fn html_cell_content(cell: &Cell) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in &cell.blocks {
        match b {
            Block::Paragraph(p) => {
                let t = p.text();
                if !t.is_empty() {
                    parts.push(escape_html(&t));
                }
            }
            Block::Table(t) => {
                let mut s = String::new();
                push_html_table(t, &mut s);
                parts.push(s);
            }
        }
    }
    parts.join("<br>")
}

/// HTML 文本转义(`& < > " '`);run 内换行规整为 `<br>`。
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' => out.push_str("<br>"),
            _ => out.push(c),
        }
    }
    out
}

// ============================================================ 标题映射

/// 把段落样式名映射成标题层级(1..=6):`Heading1`/`heading 2`/`标题3` -> 数字;`Title` -> 1;
/// 其它返回 `None`(当普通段落)。层级钳到 `1..=6`。
fn heading_level(style: Option<&str>) -> Option<u8> {
    let raw = style?.trim();
    if raw.eq_ignore_ascii_case("title") {
        return Some(1);
    }
    let lower = raw.to_lowercase(); // 英文小写化;CJK 与 ASCII 数字不受影响。
    for prefix in ["heading", "标题"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if let Ok(n) = rest.trim().parse::<u8>() {
                return Some(n.clamp(1, 6));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Paragraph, Row, TextRun};

    fn para(text: &str, style: Option<&str>) -> Paragraph {
        Paragraph {
            runs: vec![TextRun::from_text(text)],
            style: style.map(str::to_string),
            ..Paragraph::default()
        }
    }

    fn cell(text: &str, grid_span: u32, v: VMerge) -> Cell {
        Cell {
            blocks: vec![Block::Paragraph(para(text, None))],
            grid_span,
            v_merge: v,
            ..Cell::default()
        }
    }

    fn row(cells: Vec<Cell>, is_header: bool) -> Row {
        Row {
            cells,
            is_header,
            ..Row::default()
        }
    }

    /// 含横向 gridSpan + 纵向 vMerge 的合并表:row0 = [跨2列, 纵向起始];row1 = [a, b, 纵向延续]。
    fn merged_doc() -> Document {
        let t = Table {
            grid_cols: vec![],
            rows: vec![
                row(
                    vec![
                        cell("Merged Header", 2, VMerge::None),
                        cell("Spanning Down", 1, VMerge::Restart),
                    ],
                    true,
                ),
                row(
                    vec![
                        cell("a", 1, VMerge::None),
                        cell("b", 1, VMerge::None),
                        cell("", 1, VMerge::Continue),
                    ],
                    false,
                ),
            ],
            ..Table::default()
        };
        Document {
            body: vec![
                Block::Paragraph(para("Title", Some("Heading1"))),
                Block::Table(t),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn text_joins_paragraphs_and_table_rows() {
        let doc = merged_doc();
        let txt = to_text(&doc);
        assert!(txt.starts_with("Title\n"));
        assert!(txt.contains("Merged Header\tSpanning Down"));
        assert!(txt.contains("a\tb\t"));
    }

    #[test]
    fn markdown_heading_and_merged_table_uses_html() {
        let md = to_markdown(&merged_doc());
        assert!(md.contains("# Title"));
        // 含合并 -> 退回 HTML 表,保真 colspan/rowspan。
        assert!(md.contains("colspan=\"2\""));
        assert!(md.contains("rowspan=\"2\""));
    }

    #[test]
    fn markdown_simple_table_is_gfm() {
        let t = Table {
            grid_cols: vec![],
            rows: vec![
                row(
                    vec![cell("H1", 1, VMerge::None), cell("H2", 1, VMerge::None)],
                    true,
                ),
                row(
                    vec![cell("x", 1, VMerge::None), cell("y", 1, VMerge::None)],
                    false,
                ),
            ],
            ..Table::default()
        };
        let doc = Document {
            body: vec![Block::Table(t)],
            ..Default::default()
        };
        let md = to_markdown(&doc);
        assert!(md.contains("| H1 | H2 |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| x | y |"));
    }

    #[test]
    fn html_renders_heading_and_spans() {
        let html = to_html(&merged_doc());
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<th colspan=\"2\">Merged Header</th>"));
        assert!(html.contains("rowspan=\"2\""));
        assert!(html.contains("<td>a</td>"));
    }

    #[test]
    fn html_escapes_special_chars() {
        let doc = Document {
            body: vec![Block::Paragraph(para("a < b & \"c\"", None))],
            ..Default::default()
        };
        let html = to_html(&doc);
        assert!(html.contains("&lt;"));
        assert!(html.contains("&amp;"));
        assert!(html.contains("&quot;"));
    }

    /// run 分段(Text/Tab/Break)在导出侧折叠回 `\t` / `\n`,与历史 text 字段逐字节一致。
    #[test]
    fn run_segments_fold_to_text_contract() {
        use crate::model::{BreakKind, RunSegment};
        let run = TextRun {
            segments: vec![
                RunSegment::Text("a".into()),
                RunSegment::Tab,
                RunSegment::Text("b".into()),
                RunSegment::Break(BreakKind::Page),
                RunSegment::Text("c".into()),
                RunSegment::Break(BreakKind::Line),
            ],
            ..Default::default()
        };
        let doc = Document {
            body: vec![Block::Paragraph(Paragraph {
                runs: vec![run],
                ..Default::default()
            })],
            ..Default::default()
        };
        assert_eq!(to_text(&doc), "a\tb\nc\n");
        // HTML 侧换行(不分种类)规整为 <br>。
        assert!(to_html(&doc).contains("a\tb<br>c<br>"));
    }

    #[test]
    fn heading_level_maps_variants() {
        assert_eq!(heading_level(Some("Heading1")), Some(1));
        assert_eq!(heading_level(Some("heading 3")), Some(3));
        assert_eq!(heading_level(Some("标题2")), Some(2));
        assert_eq!(heading_level(Some("Title")), Some(1));
        assert_eq!(heading_level(Some("Normal")), None);
        assert_eq!(heading_level(None), None);
    }
}
