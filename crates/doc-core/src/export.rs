//! 把已解析的 [`Document`] 结构化导出成纯文本 / Markdown / HTML。
//!
//! 这是纯函数式的「模型 -> 字符串」序列化:无 IO / zip / XML,只读领域模型。三种形态:
//! - [`to_text`]:全文按块拼成纯文本(段落换行、表格行内单元格按 tab、行间换行)。
//! - [`to_markdown`]:段落按空行分隔;标题按样式表识别(`outlineLvl` / 样式名沿 `basedOn` / styleId,见 `style::resolve_heading_level`)映射成 `#`;
//!   **无合并的表格输出 GFM 管道表;一旦含合并单元格(`gridSpan` 横向 / `vMerge` 纵向)
//!   或嵌套表,则改用 HTML `<table>` 以保真 `rowspan`/`colspan`**(GFM 表无法表达合并)。
//! - [`to_html`]:段落 `<p>`、标题 `<h1>..<h6>`、表格 `<table>`(带 `rowspan`/`colspan`),
//!   文本经 HTML 转义。
//!
//! 列表 / 超链接 / 图片(三种导出各有呈现,见各 `Mode`):
//! - 列表标签由 [`crate::numbering::ListCounters`] 现算(与 PDF 映射同一引擎):纯文本 / HTML 作文字前缀;
//!   Markdown 里项目符号 `- `、合法有序标记(`1.` / `2)`)直接作列表项并按 `ilvl` 缩进(每层 4 空格,
//!   连续项紧排),其余标签(`a)` / `1.2.3` / `(a)` / `第一章`)作转义后的文字前缀。计数在正文(含表格
//!   单元格)里连续推进;页眉页脚部件、每条注、每个文本框各自独立计数。
//! - 超链接:Markdown `[文本](url)` / HTML `<a href>`;只放行 `http` / `https` / `mailto`,文档内锚点与
//!   其它 scheme 只出文字;纯文本只出文字。
//! - 图片:Markdown `![alt](媒体名)` / HTML `<img alt src>`(alt = `docPr@descr`,缺则 `@title`);纯文本有
//!   alt 时出 `[图片: alt]`,没有就不出。
//!
//! 浮动文本框([`crate::model::TextBox`])的内容紧随其锚定段落之后,按同样规则输出。
//!
//! 页眉页脚与脚注尾注(`to_text` / `to_markdown` / `to_html`):
//! - 页眉页脚按节引用顺序**去重**后各输出一次,页眉放文首、页脚放文末,带 `[Header: 类型]` /
//!   `[Footer: 类型]`(Markdown 为加粗标题行)标记;内容为空的部件不输出。
//! - 脚注 / 尾注按**在正文中首次出现的顺序**编号:纯文本正文 `[n]` / `[en]`、文末 `[n] 内容`;
//!   Markdown 正文 `[^n]` / `[^en]`、文末 `[^n]: 内容`。引用指向不存在的 id 时不出标记;
//!   定义了却无人引用的注不输出(与 Word 一致)。
//! - HTML:页眉 / 页脚是 `<header data-type="default|first|even">` / `<footer ...>`(同样去重、空部件
//!   不输出);注引用是 `<sup id="fnref-1"><a href="#fn-1">[1]</a></sup>`(尾注 `e1`),重复引用只在首次带
//!   `id`;文末每条注是 `<div class="note" id="fn-1">`,内含内容块与指回 `#fnref-1` 的回链。
//!
//! 容错:空段落跳过、空表跳过、未知样式当普通段落,绝不 panic。

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::model::{
    Block, Cell, Document, HeaderFooterKind, NoteKind, Paragraph, RunSegment, Table, TextRun,
    VMerge,
};
use crate::numbering::{ListCounters, NumFmt};
use crate::style::{NumRef, StyleCache};

#[cfg(test)]
thread_local! {
    /// 仅测试编译:注编号表的查找探测次数(Vec 线性扫描每比较一个元素计 1,索引查找计 1)。
    static NOTE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn count_note_probe(n: usize) {
    NOTE_PROBES.with(|c| c.set(c.get() + n));
}

#[cfg(test)]
thread_local! {
    /// 仅测试编译:行内拼接 / Markdown 转义各趟扫描检查的字节数(线性性断言用)。
    static SCAN_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

// ============================================================ 纯文本

/// 全文按块拼成纯文本:段落各占一行,表格每行的单元格用 `\t` 连接,块/行之间用 `\n`。
pub fn to_text(doc: &Document) -> String {
    let notes = Notes::new(doc, NoteStyle::Text);
    let mut out: Vec<String> = Vec::new();
    for (kind, blocks) in header_footer_parts(doc, false) {
        let lines = notes.scope(|| non_empty_lines(blocks, &notes));
        if !lines.is_empty() {
            out.push(format!("[Header: {}]", kind.as_str()));
            out.extend(lines.into_iter().map(|b| b.s));
        }
    }
    let mut body = Vec::new();
    text_blocks(&doc.body, &mut body, &notes);
    out.extend(body.into_iter().map(|b| b.s));
    for (label, blocks) in notes.referenced() {
        let lines = notes.scope(|| non_empty_lines(blocks, &notes));
        out.push(format!("{label} {}", MdBuf::join(lines, " ").s));
    }
    for (kind, blocks) in header_footer_parts(doc, true) {
        let lines = notes.scope(|| non_empty_lines(blocks, &notes));
        if !lines.is_empty() {
            out.push(format!("[Footer: {}]", kind.as_str()));
            out.extend(lines.into_iter().map(|b| b.s));
        }
    }
    out.join("\n")
}

/// 一串块的纯文本行,去掉空行(页眉页脚 / 注内容用:Word 里空段落很常见,别灌噪声)。
/// Markdown 导出的脚注内容也走这里(行内容未转义,由调用方整条转义)。
fn non_empty_lines(blocks: &[Block], notes: &Notes) -> Vec<MdBuf> {
    let mut lines = Vec::new();
    text_blocks(blocks, &mut lines, notes);
    lines.retain(|l| !l.is_empty());
    lines
}

fn text_blocks(blocks: &[Block], out: &mut Vec<MdBuf>, notes: &Notes) {
    let mode = notes.plain_mode();
    for b in blocks {
        match b {
            Block::Paragraph(p) => {
                let item = notes.list_item(p);
                let t = para_inline(p, notes, mode);
                out.push(match item {
                    Some(it) if !t.is_empty() => prefixed(&it, t),
                    _ => t,
                });
                for tb in p.text_boxes() {
                    notes.scope(|| text_blocks(&tb.blocks, out, notes));
                }
            }
            Block::Table(t) => out.extend(table_lines(t, notes, mode)),
        }
    }
}

/// 一张表的纯文本行:每行的单元格用 `\t` 连接(单元格内多行用 `\n` 连成一格,嵌套表的行同理压平)。
fn table_lines(t: &Table, notes: &Notes, mode: Mode) -> Vec<MdBuf> {
    t.rows
        .iter()
        .map(|row| {
            // 行首跳过的网格列(`gridBefore`)补空字段,首个单元格才落在正确的列。
            let mut cells: Vec<MdBuf> = (0..row.grid_before).map(|_| MdBuf::new(mode)).collect();
            cells.extend(
                row.cells
                    .iter()
                    .map(|c| MdBuf::join(cell_lines(&c.blocks, notes, mode), "\n")),
            );
            MdBuf::join(cells, "\t")
        })
        .collect()
}

/// 单元格内的行(纯文本 / GFM 单元格用):每段一行(空段保留空行),列表标签作前缀,
/// 段落锚定的文本框内容紧随其后;嵌套表按 [`table_lines`] 压平成多行(行内单元格 `\t`),不丢字。
fn cell_lines(blocks: &[Block], notes: &Notes, mode: Mode) -> Vec<MdBuf> {
    let mut lines = Vec::new();
    for b in blocks {
        match b {
            Block::Paragraph(p) => {
                let item = notes.list_item(p);
                let t = para_inline(p, notes, mode);
                lines.push(match item {
                    Some(it) if !t.is_empty() => prefixed(&it, t),
                    _ => t,
                });
                for tb in p.text_boxes() {
                    let inner =
                        notes.scope(|| MdBuf::join(cell_lines(&tb.blocks, notes, mode), "\n"));
                    if !inner.is_empty() {
                        lines.push(inner);
                    }
                }
            }
            Block::Table(t) => lines.extend(table_lines(t, notes, mode)),
        }
    }
    lines
}

// ============================================================ Markdown

/// 全文导出为 Markdown。段落以空行分隔;标题样式映射成 `#`;表格无合并时输出 GFM 管道表,
/// 含合并(或嵌套表)时退回 HTML `<table>` 以保真 `rowspan`/`colspan`。
pub fn to_markdown(doc: &Document) -> String {
    let notes = Notes::new(doc, NoteStyle::Markdown);
    let mut parts: Vec<String> = Vec::new();
    for (kind, blocks) in header_footer_parts(doc, false) {
        push_md_header_footer("Header", kind, blocks, &notes, &mut parts);
    }
    markdown_blocks(&doc.body, &mut parts, &notes);
    for (label, blocks) in notes.referenced() {
        let lines = notes.scope(|| non_empty_lines(blocks, &notes));
        let content = md_escape(&MdBuf::join(lines, " "), MdCtx::NoteDef);
        parts.push(format!("{label}: {content}"));
    }
    for (kind, blocks) in header_footer_parts(doc, true) {
        push_md_header_footer("Footer", kind, blocks, &notes, &mut parts);
    }
    parts.join("\n\n")
}

/// 一个页眉 / 页脚部件 -> Markdown:加粗标题行 + 内容块;内容为空的部件整体跳过。
fn push_md_header_footer(
    role: &str,
    kind: HeaderFooterKind,
    blocks: &[Block],
    notes: &Notes,
    parts: &mut Vec<String>,
) {
    let mut inner = Vec::new();
    notes.scope(|| markdown_blocks(blocks, &mut inner, notes));
    if !inner.is_empty() {
        parts.push(format!("**{role} ({})**", kind.as_str()));
        parts.extend(inner);
    }
}

fn markdown_blocks(blocks: &[Block], parts: &mut Vec<String>, notes: &Notes) {
    // 上一个输出块是否是真列表项,及其(有效)层级:连续真列表项紧排成一个块,
    // 缩进按层级(每层 4 空格),且不超过「上一项层级 + 1」,免得被当成缩进代码块。
    let mut prev_list_level: Option<u32> = None;
    for b in blocks {
        match b {
            Block::Paragraph(p) => {
                let item = notes.list_item(p);
                let t = para_inline(p, notes, Mode::Markdown);
                if !t.is_empty() {
                    match (heading_level(notes, p), item) {
                        (Some(level), item) => {
                            let content = match item {
                                Some(it) if !it.bullet => prefixed(&it, t),
                                _ => t,
                            };
                            let content = md_escape(&content, MdCtx::Heading);
                            parts.push(format!("{} {content}", "#".repeat(level as usize)));
                            prev_list_level = None;
                        }
                        (None, Some(it)) => match md_list_marker(&it) {
                            Some(marker) => {
                                let level = it.level.min(prev_list_level.map_or(0, |l| l + 1));
                                let t = md_escape(&t, MdCtx::Block);
                                let line = format!("{}{marker} {t}", "    ".repeat(level as usize));
                                match (prev_list_level, parts.last_mut()) {
                                    (Some(_), Some(last)) => {
                                        last.push('\n');
                                        last.push_str(&line);
                                    }
                                    _ => parts.push(line),
                                }
                                prev_list_level = Some(level);
                            }
                            None => {
                                parts.push(md_escape(&prefixed(&it, t), MdCtx::Block));
                                prev_list_level = None;
                            }
                        },
                        (None, None) => {
                            parts.push(md_escape(&t, MdCtx::Block));
                            prev_list_level = None;
                        }
                    }
                }
                for tb in p.text_boxes() {
                    notes.scope(|| markdown_blocks(&tb.blocks, parts, notes));
                    prev_list_level = None;
                }
            }
            Block::Table(t) => {
                let md = markdown_table(t, notes);
                if !md.is_empty() {
                    parts.push(md);
                }
                prev_list_level = None;
            }
        }
    }
}

/// 一张表 -> Markdown。无合并/无嵌套表时用 GFM 管道表(首行作表头);否则退回 HTML 表。
fn markdown_table(table: &Table, notes: &Notes) -> String {
    if table_needs_html(table) {
        let mut s = String::new();
        push_html_table(table, &mut s, notes);
        return s;
    }
    let ncols = table
        .rows
        .iter()
        .map(|r| r.grid_before as usize + r.cells.len())
        .max()
        .unwrap_or(0);
    if ncols == 0 {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for (i, row) in table.rows.iter().enumerate() {
        // 行首跳过的网格列(`gridBefore`)补空单元格;行尾不足补齐到 ncols。
        let mut cells: Vec<String> = vec![String::new(); row.grid_before as usize];
        cells.extend(row.cells.iter().map(|c| md_cell_text(c, notes)));
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

/// GFM 单元格文字:各行(及行内换行)规整为导出器生成的 `<br>`(否则会撑破表格),按单元格规则转义
/// (竖线一律转义,不做行首规则)。
fn md_cell_text(cell: &Cell, notes: &Notes) -> String {
    let lines = cell_lines(&cell.blocks, notes, Mode::Markdown);
    md_escape(&MdBuf::join(lines, "\n"), MdCtx::Cell)
}

/// 表格是否需要退回 HTML:任一单元格横向跨列 / 参与纵向合并 / 含嵌套表(GFM 表无法表达)。
fn table_needs_html(table: &Table) -> bool {
    table.rows.iter().any(|r| {
        r.cells
            .iter()
            .any(|c| c.grid_span > 1 || c.v_merge != VMerge::None || has_nested_table(&c.blocks))
    })
}

/// 单元格内容里(含段落锚定的文本框)是否有表:GFM 单元格放不下,须退回 HTML 才不丢字。
fn has_nested_table(blocks: &[Block]) -> bool {
    blocks.iter().any(|b| match b {
        Block::Table(_) => true,
        Block::Paragraph(p) => p.text_boxes().any(|tb| has_nested_table(&tb.blocks)),
    })
}

// ============================================================ HTML

/// 全文导出为 HTML 片段:段落 `<p>`、标题 `<h1>..<h6>`、表格 `<table>`(带合并)。文本经转义。
pub fn to_html(doc: &Document) -> String {
    let notes = Notes::new(doc, NoteStyle::Html);
    let mut out = String::new();
    for (kind, blocks) in header_footer_parts(doc, false) {
        push_html_header_footer("header", kind, blocks, &notes, &mut out);
    }
    html_blocks(&doc.body, &mut out, &notes);
    for (label, blocks) in notes.referenced() {
        out.push_str(&format!("<div class=\"note\" id=\"fn-{label}\">\n"));
        out.push_str(&format!("<sup>[{label}]</sup>\n"));
        notes.scope(|| html_blocks(blocks, &mut out, &notes));
        out.push_str(&format!("<a href=\"#fnref-{label}\">&#8617;</a>\n</div>\n"));
    }
    for (kind, blocks) in header_footer_parts(doc, true) {
        push_html_header_footer("footer", kind, blocks, &notes, &mut out);
    }
    out.trim_end().to_string()
}

/// 一个页眉 / 页脚部件 -> `<header>` / `<footer>` 容器(`data-type` 标 default/first/even);
/// 内容为空的部件整体跳过。
fn push_html_header_footer(
    tag: &str,
    kind: HeaderFooterKind,
    blocks: &[Block],
    notes: &Notes,
    out: &mut String,
) {
    let mut inner = String::new();
    notes.scope(|| html_blocks(blocks, &mut inner, notes));
    if !inner.is_empty() {
        out.push_str(&format!(
            "<{tag} data-type=\"{}\">\n{inner}</{tag}>\n",
            kind.as_str()
        ));
    }
}

fn html_blocks(blocks: &[Block], out: &mut String, notes: &Notes) {
    for b in blocks {
        match b {
            Block::Paragraph(p) => {
                let item = notes.list_item(p);
                let t = para_inline(p, notes, Mode::Html);
                if !t.is_empty() {
                    let t = match item {
                        Some(it) => prefixed(&it, t).s,
                        None => t.s,
                    };
                    match heading_level(notes, p) {
                        Some(level) => out.push_str(&format!("<h{level}>{t}</h{level}>\n")),
                        None => out.push_str(&format!("<p>{t}</p>\n")),
                    }
                }
                for tb in p.text_boxes() {
                    notes.scope(|| html_blocks(&tb.blocks, out, notes));
                }
            }
            Block::Table(t) => {
                push_html_table(t, out, notes);
                out.push('\n');
            }
        }
    }
}

/// 把一张表渲染成 HTML `<table>`,正确还原 `colspan`(`gridSpan`)与 `rowspan`(`vMerge`)。
///
/// 纵向合并语义:`restart` 格起始并向下吞并若干**真正的延续格**;延续格被吞掉,**不**单独输出
/// `<td>`。上方不是 `restart`(或其延续链)的孤立 `continue` 格按普通单元格输出,内容不丢。
/// 合并按**网格列**对齐(用 `gridBefore` + `gridSpan` 累加出每格的起始网格列号),不是按
/// 单元格序号,这样横向合并、行首空缺与纵向合并叠加时也对得上;行首空缺(`gridBefore`)用一个
/// 空单元格(多列用 `colspan`)占位。
fn push_html_table(table: &Table, out: &mut String, notes: &Notes) {
    let starts = grid_starts(table);
    let cont = continuations(table, &starts);
    out.push_str("<table>\n");
    for (i, row) in table.rows.iter().enumerate() {
        out.push_str("<tr>\n");
        let tag = if row.is_header { "th" } else { "td" };
        if row.grid_before > 0 {
            out.push_str(&format!("<{tag}"));
            if row.grid_before > 1 {
                out.push_str(&format!(" colspan=\"{}\"", row.grid_before));
            }
            out.push_str(&format!("></{tag}>\n"));
        }
        for (ci, cell) in row.cells.iter().enumerate() {
            // 被纵向合并吞掉的延续格不单独输出。
            if cont[i][ci] {
                continue;
            }
            let colspan = cell.grid_span.max(1) as usize;
            let rowspan = if cell.v_merge == VMerge::Restart {
                vmerge_rowspan(&starts, &cont, i, starts[i][ci])
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
            out.push_str(&html_cell_content(&cell.blocks, notes));
            out.push_str("</");
            out.push_str(tag);
            out.push_str(">\n");
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>");
}

/// 每行各单元格的**起始网格列号**(从 `gridBefore` 起按 `gridSpan` 累加)。用于把 `vMerge`
/// 延续格对齐到列。
fn grid_starts(table: &Table) -> Vec<Vec<usize>> {
    table
        .rows
        .iter()
        .map(|r| {
            let mut acc = r.grid_before as usize;
            let mut v = Vec::with_capacity(r.cells.len());
            for c in &r.cells {
                v.push(acc);
                acc += c.grid_span.max(1) as usize;
            }
            v
        })
        .collect()
}

/// 每个单元格是否是**真正的** `vMerge` 延续格:标了 `continue`,且正上方(同起始网格列)的格是
/// `restart` 或本身已是真延续格。其余 `continue` 格(首行 / 上方是普通格 / 上方是孤立 continue)
/// 是孤立延续,当普通单元格处理。
fn continuations(table: &Table, starts: &[Vec<usize>]) -> Vec<Vec<bool>> {
    let mut cont: Vec<Vec<bool>> = Vec::with_capacity(table.rows.len());
    for (i, (row, row_starts)) in table.rows.iter().zip(starts).enumerate() {
        let flags = row
            .cells
            .iter()
            .zip(row_starts)
            .map(|(cell, &g)| {
                if cell.v_merge != VMerge::Continue || i == 0 {
                    return false;
                }
                let above = table.rows[i - 1]
                    .cells
                    .iter()
                    .zip(&starts[i - 1])
                    .position(|(_, &s)| s == g);
                above.is_some_and(|ai| {
                    table.rows[i - 1].cells[ai].v_merge == VMerge::Restart || cont[i - 1][ai]
                })
            })
            .collect();
        cont.push(flags);
    }
    cont
}

/// 从 `start_row` 的 `restart` 格(起始网格列 `g`)向下数有多少行在同列是真延续格,得 `rowspan`。
fn vmerge_rowspan(starts: &[Vec<usize>], cont: &[Vec<bool>], start_row: usize, g: usize) -> usize {
    let mut span = 1usize;
    for (row_starts, row_cont) in starts.iter().zip(cont).skip(start_row + 1) {
        let continues = row_starts
            .iter()
            .position(|&s| s == g)
            .is_some_and(|ci| row_cont[ci]);
        if continues {
            span += 1;
        } else {
            break;
        }
    }
    span
}

/// 单元格内容 -> HTML:段落文字转义后以 `<br>` 连接,嵌套表递归成内层 `<table>`;
/// 段落锚定的文本框内容紧随该段之后。
fn html_cell_content(blocks: &[Block], notes: &Notes) -> String {
    let mut parts: Vec<String> = Vec::new();
    for b in blocks {
        match b {
            Block::Paragraph(p) => {
                let item = notes.list_item(p);
                let t = para_inline(p, notes, Mode::Html);
                if !t.is_empty() {
                    parts.push(match item {
                        Some(it) => prefixed(&it, t).s,
                        None => t.s,
                    });
                }
                for tb in p.text_boxes() {
                    let inner = notes.scope(|| html_cell_content(&tb.blocks, notes));
                    if !inner.is_empty() {
                        parts.push(inner);
                    }
                }
            }
            Block::Table(t) => {
                let mut s = String::new();
                push_html_table(t, &mut s, notes);
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

// ============================================================ 行内:列表标签 / 超链接 / 图片

/// 行内文本的呈现方式。
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// 纯文本:只出文字,图片带 alt 时出 `[图片: alt]`。
    Text,
    /// Markdown:`[文本](url)`、`![alt](媒体名)`。
    Markdown,
    /// HTML:文字转义,`<a href>` / `<img>`。
    Html,
}

/// 一个段落的列表项信息。
struct ListItem {
    /// 计数引擎产出的最终标签串(`1.` / `a)` / `1.2.3` / 圆点字面)。
    label: String,
    /// 该层是项目符号(`numFmt = bullet`)。
    bullet: bool,
    /// `ilvl`。
    level: u32,
}

/// Markdown 真列表项的标记:项目符号 `-`;标签恰是合法有序列表标记(1–9 位数字 + `.` / `)`)
/// 时直接用;其它标签没有对应的列表语法,返回 `None`(改作文字前缀,见 [`prefixed`])。
fn md_list_marker(item: &ListItem) -> Option<String> {
    if item.bullet {
        return Some("-".to_string());
    }
    let digits = item.label.trim_end_matches(['.', ')']);
    let punct = &item.label[digits.len()..];
    let ok = (1..=9).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (punct == "." || punct == ")");
    ok.then(|| item.label.clone())
}

/// 把标签当文字前缀(含尾随空格)接在行内内容之前。标签是文档内容:纯文本原样、HTML 转义、
/// Markdown 与后面的文字一起整块转义(见 [`md_escape`])。
fn prefixed(item: &ListItem, t: MdBuf) -> MdBuf {
    let mut out = MdBuf::new(t.mode);
    out.text(&format!("{} ", item.label));
    out.append(t);
    out
}

/// 段落的行内内容:run 文字 + 注标记 + 图片;连续同目标的超链接 run 合成一个链接。
/// 链接按需打开:注标记(HTML 自带 `<a>`,嵌套非法;Markdown 的 `[^n]` 在链接文字里会让 CommonMark 的
/// 外层链接失效)出现时先闭合链接、输出标记,有后续内容再重新打开,首尾的标记因此不留空链接。
/// Markdown 不在这里转义:返回带来源掩码的缓冲,由块的组装方拼成整块后一趟转义(上下文会跨 run)。
fn para_inline(p: &Paragraph, notes: &Notes, mode: Mode) -> MdBuf {
    let mut out = MdBuf::new(mode);
    let mut i = 0;
    while i < p.runs.len() {
        let target = p.runs[i].link_target.as_deref();
        let end = i + p.runs[i..]
            .iter()
            .take_while(|r| r.link_target.as_deref() == target)
            .count();
        out.link = target.and_then(safe_url).and_then(|url| match mode {
            Mode::Text => None,
            Mode::Markdown => Some(LinkState::new("[".into(), format!("]({})", md_url(&url)))),
            Mode::Html => Some(LinkState::new(
                format!("<a href=\"{}\">", escape_html(&url)),
                "</a>".into(),
            )),
        });
        for r in &p.runs[i..end] {
            run_inline(r, notes, &mut out);
        }
        out.close_link();
        out.link = None;
        i = end;
    }
    out
}

/// 一个 run 的行内内容(文字分段 + 图片),写进 `out`(可能位于一个按需打开的链接里)。
fn run_inline(run: &TextRun, notes: &Notes, out: &mut MdBuf) {
    let mode = out.mode;
    for seg in &run.segments {
        match seg {
            RunSegment::Text(s) => out.text(s),
            RunSegment::Tab => out.text("\t"),
            RunSegment::Break(_) => match mode {
                Mode::Html => out.content_gen("<br>"),
                // 链接文字里的换行折成空格:连续两个换行会产生空行、把链接截断。
                Mode::Markdown if out.link.is_some() => out.text(" "),
                _ => out.text("\n"),
            },
            RunSegment::NoteRef { kind, id } => {
                if let Some(mark) = notes.mark(*kind, *id) {
                    out.close_link();
                    out.gen(&mark);
                }
            }
            RunSegment::CommentRef { .. } => {}
        }
    }
    for pic in &run.pictures {
        let alt = pic.alt.as_deref().unwrap_or("");
        match (mode, pic.media_name.as_deref()) {
            (Mode::Markdown, Some(src)) => {
                out.content_gen("![");
                out.alt_text(alt);
                out.gen(&format!("]({})", md_url(src)));
            }
            (Mode::Html, Some(src)) => out.content_gen(&format!(
                "<img alt=\"{}\" src=\"{}\">",
                escape_html(alt),
                escape_html(src)
            )),
            // 纯文本,或没有可引用的媒体名:有 alt 才出(alt 是文档内容,按模式转义),没有就不出。
            _ if !alt.is_empty() => out.text(&format!("[图片: {alt}]")),
            _ => {}
        }
    }
}

/// 行内缓冲里每个字节的来源(只在 Markdown 下记录)。
#[derive(Clone, Copy, PartialEq)]
enum Span {
    /// 导出器生成的 Markdown 语法(链接括号与目的地、脚注标记、图片语法、分隔符),不转义。
    Gen,
    /// 文档内容,按块的上下文转义。
    Text,
    /// 链接文字 / 图片 alt 里的文档内容:另外转义 `]`。
    Link,
}

/// 一个按需打开的超链接:有内容要写时才输出开标记;闭合时若没写进任何内容就撤掉开标记。
struct LinkState {
    open: String,
    close: String,
    /// 开标记在缓冲里的起点;`None` = 当前未打开。
    opened_at: Option<usize>,
}

impl LinkState {
    fn new(open: String, close: String) -> Self {
        LinkState {
            open,
            close,
            opened_at: None,
        }
    }
}

/// 行内输出缓冲。纯文本 / HTML 直接拼好(HTML 文字写入时即转义);Markdown 额外记**逐字节来源**
/// (与 `s` 等长),由块的组装方把整块拼完后一趟 [`md_escape`]——转义规则依赖跨 run 的上下文
/// (`&` 与后一个 run 的 `#x3C;`、导出器生成的 `]` 与后一个 run 的 `(`),不能逐片段转义。
struct MdBuf {
    mode: Mode,
    s: String,
    spans: Vec<Span>,
    link: Option<LinkState>,
}

impl MdBuf {
    fn new(mode: Mode) -> Self {
        MdBuf {
            mode,
            s: String::new(),
            spans: Vec::new(),
            link: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.s.is_empty()
    }

    fn push(&mut self, t: &str, span: Span) {
        self.s.push_str(t);
        if self.mode == Mode::Markdown {
            self.spans.extend(std::iter::repeat_n(span, t.len()));
        }
    }

    /// 导出器生成的语法(不转义;不打开链接)。
    fn gen(&mut self, t: &str) {
        self.push(t, Span::Gen);
    }

    /// 导出器生成、但属于链接内容的语法(图片、HTML `<br>`):先按需打开链接。
    fn content_gen(&mut self, t: &str) {
        self.open_link();
        self.gen(t);
    }

    /// 文档内容。HTML 写入时转义;Markdown 记来源,链接文字里的换行折成空格。
    fn text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        self.open_link();
        match self.mode {
            Mode::Text => self.s.push_str(t),
            Mode::Html => self.s.push_str(&escape_html(t)),
            Mode::Markdown if self.link.is_some() => self.push(&t.replace('\n', " "), Span::Link),
            Mode::Markdown => self.push(t, Span::Text),
        }
    }

    /// 图片 alt(Markdown 专用):与链接文字同样转义 `]`,换行折成空格。
    fn alt_text(&mut self, t: &str) {
        self.push(&t.replace('\n', " "), Span::Link);
    }

    fn open_link(&mut self) {
        let Some(l) = self.link.as_mut() else { return };
        if l.opened_at.is_some() {
            return;
        }
        l.opened_at = Some(self.s.len());
        let open = l.open.clone();
        self.gen(&open);
    }

    fn close_link(&mut self) {
        let Some(l) = self.link.as_mut() else { return };
        let Some(at) = l.opened_at.take() else { return };
        if self.s.len() == at + l.open.len() {
            self.s.truncate(at);
            self.spans.truncate(at);
        } else {
            let close = l.close.clone();
            self.gen(&close);
        }
    }

    fn append(&mut self, other: MdBuf) {
        self.s.push_str(&other.s);
        self.spans.extend(other.spans);
    }

    /// 用导出器生成的分隔符连接若干缓冲。
    fn join(parts: Vec<MdBuf>, sep: &str) -> MdBuf {
        let mode = parts.first().map_or(Mode::Text, |p| p.mode);
        let mut out = MdBuf::new(mode);
        for (i, p) in parts.into_iter().enumerate() {
            if i > 0 {
                out.gen(sep);
            }
            out.append(p);
        }
        out
    }
}

/// Markdown 块的转义上下文。
#[derive(Clone, Copy, PartialEq)]
enum MdCtx {
    /// 段落 / 列表项内容 / 列表标签作前缀的段落:每行行首做块结构转义;块的首行(含空行之后的行)
    /// 去掉 >= 4 列的前导空白。
    Block,
    /// ATX 标题内容:标题行只做行内转义 + 结尾 `#` 序列转义;标题里的换行之后按 `Block` 处理。
    Heading,
    /// GFM 表格单元格:只做行内转义,`|` 一律转义,换行写成 `<br>`,**不做任何行首规则**。
    Cell,
    /// 脚注定义 `[^n]: 内容`:只做行内转义,换行折成空格,并保证纯 CommonMark 下不构成链接引用定义。
    NoteDef,
}

#[cfg(test)]
fn count_scan(n: usize) {
    SCAN_BYTES.with(|c| c.set(c.get() + n));
}

/// Markdown 转义:**在文档内容不能注入任何 Markdown 结构的前提下,转义越少越好**(输出给 RAG)。
/// 只转义文档内容(`Span::Text` / `Span::Link`),导出器生成的语法原样。规则(最小必要集,复审用
/// markdown-it 逐条验证过):
/// - `[` 一律(链接 / 图片 / 引用定义 / 脚注都要它开头);`]` 只在链接文字与图片 alt 里。
/// - `(` 与 `:` 只在紧跟**导出器生成的** `]` 时(脚注标记 `[^1]` 后接文档的 `(javascript:…)` 会成内联链接,
///   接 `: x` 会成引用定义);`)` 不转义。
/// - `<` 只在后跟 ASCII 字母、`/`、`!`、`?` 时(HTML 标签 / 注释 / 处理指令 / 自动链接都以此开头)。
/// - `>` 只在行首(引用块)。`&` 只在构成实体引用时(`&名字;`、`&#数字;`、`&#x十六进制;`)。
/// - `_` 只在词边界(两侧都是字母数字的词内 `_` 不能开闭强调);`*` 两侧都是空白时不转义。
/// - 反引号一律;`~` 同一块里出现两个及以上时(删除线成对)。
/// - `\` 只在后跟 ASCII 标点、换行或位于块尾时(否则它本就是字面反斜杠)。
/// - 行首(`Block`,及 `Heading` 的续行):`#{1,6}` 后跟空白 / 行尾、`- + *` 后跟空白 / 行尾、只由
///   `- = * _` 与空白组成的行(setext / 分隔线)、1–9 位数字 + `.`/`)` + 空白 / 行尾、`>`、像表格分隔行的行,
///   各转义其首个(或数字后的)标点。三个以上的反引号 / `~` 已由上面的规则覆盖。
/// - 块的首行(段落首行、列表项内容首行、空行之后的行)前导空白 >= 4 列会成缩进代码块:**去掉这段前导
///   空白**——CommonMark 渲染段落时本来就会去掉首行前导空白,可见内容不变,只是原始 Markdown 里该行
///   失去缩进;续行(段落内换行)不会成代码块,缩进保留。
/// - `|`:单元格里一律;`Block` / `Heading` 里只要有一行像表格分隔行(只含 `|` `-` `:` 与空白、含 `-`
///   且含 `|` 或 `:`),整块的 `|` 都转义(否则可拼出管道表)。
/// - 标题:结尾的 `#` 序列(前面是空白或整行都是 `#`)会被当成结束序列吃掉,转义其首个 `#`。
/// - 脚注定义:见 [`note_def_guard`]。
///
/// 一趟线性扫描(另有两趟线性预扫:`~` 计数、分隔行检测);前瞻都有常数上界。
fn md_escape(buf: &MdBuf, ctx: MdCtx) -> String {
    let s = buf.s.as_str();
    let bytes = s.as_bytes();
    let is_content = |i: usize| buf.spans.get(i).is_some_and(|&k| k != Span::Gen);
    let line_ctx = matches!(ctx, MdCtx::Block | MdCtx::Heading);
    #[cfg(test)]
    count_scan(if line_ctx { 2 * s.len() } else { s.len() });
    let tildes = bytes
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b == b'~' && is_content(i))
        .count();
    let pipes = ctx == MdCtx::Cell || (line_ctx && s.split('\n').any(is_delimiter_row));
    let heading_hash = match ctx {
        MdCtx::Heading => closing_hash(&s[..s.find('\n').unwrap_or(s.len())]),
        _ => None,
    };
    let guard = match ctx {
        MdCtx::NoteDef => note_def_guard(s),
        _ => NoteGuard::default(),
    };
    let mut out = String::with_capacity(s.len() + s.len() / 8 + 8);
    // 行状态(只在 Block / Heading 下用):本行是否做行首规则、是否块的首行、是否在行首、本行是否空白。
    let mut line_rules = ctx == MdCtx::Block;
    let mut block_first = true;
    let mut at_line_begin = line_rules;
    let mut line_blank = false;
    // 行首规则要求转义的位置。
    let mut force: Option<usize> = None;
    let mut prev: Option<char> = None;
    let mut i = 0;
    while i < s.len() {
        if at_line_begin {
            at_line_begin = false;
            let line_end = s[i..].find('\n').map_or(s.len(), |k| i + k);
            #[cfg(test)]
            count_scan(line_end - i);
            let line = &s[i..line_end];
            let rest = line.trim_start_matches([' ', '\t']);
            let ws = &line[..line.len() - rest.len()];
            line_blank = rest.is_empty();
            if !(block_first && !line_blank && indent_cols(ws) >= 4) {
                out.push_str(ws);
                prev = ws.chars().next_back().or(prev);
            }
            i += ws.len();
            force = line_start_escape(rest).map(|off| i + off);
            continue;
        }
        let Some(c) = s[i..].chars().next() else {
            break;
        };
        let len = c.len_utf8();
        if c == '\n' {
            match ctx {
                MdCtx::Cell => {
                    out.push_str("<br>");
                    prev = Some('>');
                    i += len;
                    continue;
                }
                MdCtx::NoteDef => out.push(' '),
                MdCtx::Block | MdCtx::Heading => {
                    out.push('\n');
                    // 标题行结束后是新块;否则空行之后才是新块的首行。
                    block_first = !line_rules || line_blank;
                    line_rules = true;
                    at_line_begin = true;
                }
            }
            prev = Some('\n');
            i += len;
            continue;
        }
        // 下一个字符按它最终的样子算:单元格里的换行写成 `<br>`(不是空白)。
        let next = match s[i + len..].chars().next() {
            Some('\n') if ctx == MdCtx::Cell => Some('<'),
            n => n,
        };
        let after_gen_bracket = i > 0 && bytes[i - 1] == b']' && !is_content(i - 1);
        let escape = is_content(i)
            && (force == Some(i)
                || heading_hash == Some(i)
                || guard.escape.contains(&Some(i))
                || match c {
                    '\\' => next.is_none_or(|n| n == '\n' || n.is_ascii_punctuation()),
                    '[' | '`' => true,
                    ']' => buf.spans.get(i) == Some(&Span::Link),
                    '(' | ':' => after_gen_bracket,
                    '<' => next
                        .is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?')),
                    '&' => entity_follows(&s[i..]),
                    '_' => {
                        !(prev.is_some_and(char::is_alphanumeric)
                            && next.is_some_and(char::is_alphanumeric))
                    }
                    '*' => {
                        !(prev.is_none_or(char::is_whitespace)
                            && next.is_none_or(char::is_whitespace))
                    }
                    '~' => tildes >= 2,
                    '|' => pipes,
                    _ => false,
                });
        if escape {
            out.push('\\');
        }
        out.push(c);
        prev = Some(c);
        i += len;
    }
    if guard.zwsp {
        out.push_str(" \u{200B}");
    }
    out
}

/// 前导空白的列数(制表符推进到下一个 4 的倍数)。
fn indent_cols(ws: &str) -> usize {
    ws.chars().fold(
        0,
        |col, c| if c == '\t' { col / 4 * 4 + 4 } else { col + 1 },
    )
}

/// 行首(已去掉前导空白的本行 `rest`)需要转义的字节偏移;不需要时 `None`。
fn line_start_escape(rest: &str) -> Option<usize> {
    let first = rest.chars().next()?;
    let ws_or_end = |at: usize| {
        rest[at..]
            .chars()
            .next()
            .is_none_or(|c| c == ' ' || c == '\t')
    };
    let only_marks = rest
        .chars()
        .all(|c| matches!(c, '-' | '=' | '*' | '_' | ' ' | '\t'));
    if only_marks || is_delimiter_row(rest) {
        return Some(0);
    }
    match first {
        '#' => {
            let run = rest.bytes().take_while(|&b| b == b'#').count();
            (run <= 6 && ws_or_end(run)).then_some(0)
        }
        '-' | '+' | '*' => ws_or_end(1).then_some(0),
        '>' => Some(0),
        '0'..='9' => {
            let d = rest.bytes().take_while(u8::is_ascii_digit).count();
            let marker = d <= 9 && matches!(rest.as_bytes().get(d), Some(b'.' | b')'));
            (marker && ws_or_end(d + 1)).then_some(d)
        }
        _ => None,
    }
}

/// 一行是否像 GFM 表格分隔行:只含 `|` `-` `:` 与空白,含 `-`,且含 `|` 或 `:`。
fn is_delimiter_row(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && t.contains('-')
        && (t.contains('|') || t.contains(':'))
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// ATX 标题行结尾的 `#` 结束序列(前面是空白,或整行都是 `#`)的起点;没有则 `None`。
fn closing_hash(line: &str) -> Option<usize> {
    let t = line.trim_end_matches([' ', '\t']);
    let start = t.trim_end_matches('#').len();
    if start == t.len() {
        return None;
    }
    let before = &t[..start];
    (before.trim().is_empty() || before.ends_with([' ', '\t'])).then_some(start)
}

/// `&` 开头的串是否构成实体引用(`&名字;` / `&#1–7 位数字;` / `&#x1–6 位十六进制;`);前瞻有上界。
fn entity_follows(t: &str) -> bool {
    let b = t.as_bytes();
    let run = |from: usize, max: usize, ok: fn(&u8) -> bool| {
        let n = b[from.min(b.len())..]
            .iter()
            .take(max + 1)
            .take_while(|c| ok(c))
            .count();
        (n >= 1 && n <= max && b.get(from + n) == Some(&b';')).then_some(())
    };
    match b.get(1) {
        Some(b'#') => match b.get(2) {
            Some(b'x' | b'X') => run(3, 6, u8::is_ascii_hexdigit).is_some(),
            _ => run(2, 7, u8::is_ascii_digit).is_some(),
        },
        Some(c) if c.is_ascii_alphabetic() => run(1, 32, u8::is_ascii_alphanumeric).is_some(),
        _ => false,
    }
}

/// 脚注定义的防护:`[^1]: 内容` 在不开脚注扩展的 CommonMark 里是**链接引用定义**——内容的第一个词
/// 成为链接目的地(`javascript:` / `data:` 也行),正文的 `[^1]` 就渲染成指向它的链接。定义只在
/// 「目的地 + 可选标题 + 行尾」时成立,所以:
/// - 内容以 `<` 开头时转义它(否则可成尖括号目的地,里面还能有空格);
/// - 第二个词以 `"` `'` `(` 开头时转义它(不再能作标题,定义因多余内容不成立);
/// - 只有一个词时在末尾补「空格 + U+200B」:U+200B 不是 CommonMark 空白、也不能开始标题,定义不成立;
///   渲染不可见,在支持脚注扩展的解析器(GFM)里仍是同一条脚注。
#[derive(Default)]
struct NoteGuard {
    escape: [Option<usize>; 2],
    zwsp: bool,
}

fn note_def_guard(s: &str) -> NoteGuard {
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\n');
    let lead = s.len() - s.trim_start_matches(is_ws).len();
    if lead == s.len() {
        return NoteGuard::default();
    }
    let first_end = s[lead..].find(is_ws).map_or(s.len(), |k| lead + k);
    let second = s[first_end..].find(|c| !is_ws(c)).map(|k| first_end + k);
    let lt = (s.as_bytes()[lead] == b'<').then_some(lead);
    match second {
        None => NoteGuard {
            escape: [lt, None],
            zwsp: true,
        },
        Some(k) => NoteGuard {
            escape: [
                lt,
                matches!(s.as_bytes()[k], b'"' | b'\'' | b'(').then_some(k),
            ],
            zwsp: false,
        },
    }
}

/// 超链接目标白名单:只放行 `http` / `https` / `mailto`(scheme 大小写不敏感,先去掉控制字符与
/// 空白——浏览器同样忽略它们,`java\tscript:` 之类不能绕过);相对路径 / 书签 / 其它 scheme 一律
/// `None`(只出文字)。返回去掉控制字符后的 URL。
fn safe_url(target: &str) -> Option<String> {
    let url: String = target.chars().filter(|c| !c.is_control()).collect();
    let url = url.trim().to_string();
    let scheme: String = url
        .chars()
        .filter(|c| !c.is_whitespace())
        .take_while(|&c| c != ':')
        .collect::<String>()
        .to_ascii_lowercase();
    let has_colon = url.contains(':');
    (has_colon && matches!(scheme.as_str(), "http" | "https" | "mailto")).then_some(url)
}

/// Markdown 链接 / 图片目的地:空白、反斜杠与括号 / 尖括号百分号编码(其余原样),保证目的地不会被
/// `)` / 空白提前终止、也不会被 `\` 转义吞掉;构成实体引用的 `&` 反斜杠转义(目的地里的实体会被解码,
/// `?q=&copy;` 会变成 `?q=©`;用 `\&` 而不是 `%26`,不改变查询串的含义)。
fn md_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for (i, c) in url.char_indices() {
        match c {
            ' ' => out.push_str("%20"),
            '\\' => out.push_str("%5C"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '&' if entity_follows(&url[i..]) => out.push_str("\\&"),
            _ => out.push(c),
        }
    }
    out
}

// ============================================================ 页眉页脚 / 脚注尾注

/// 按节引用顺序去重后的页眉(`footer = false`)或页脚部件;同一部件只出一次,
/// 标签取其最先出现的类型。
fn header_footer_parts(doc: &Document, footer: bool) -> Vec<(HeaderFooterKind, &[Block])> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for sect in &doc.sections {
        let refs = if footer { &sect.footers } else { &sect.headers };
        for r in refs {
            if let Some(blocks) = doc.header_footers.get(&r.rel_id) {
                if seen.insert(r.rel_id.as_str()) {
                    out.push((r.kind, blocks.as_slice()));
                }
            }
        }
    }
    out
}

/// 注标记的写法。
#[derive(Clone, Copy, PartialEq)]
enum NoteStyle {
    /// HTML:`<sup id=\"fnref-1\"><a href=\"#fn-1\">[1]</a></sup>`(只在首次引用带 `id`)。
    Html,
    /// 纯文本:`[1]` / `[e1]`。
    Text,
    /// Markdown 脚注:`[^1]` / `[^e1]`。
    Markdown,
}

/// 一类注(脚注 / 尾注)的首次引用顺序:`ids` 按编号排,`index` 是 `id -> 编号(1 起)` 的有序索引,
/// 登记与取号都是 O(log n)(不再对 `ids` 线性扫描)。
#[derive(Default)]
struct NoteOrder {
    ids: Vec<i64>,
    index: BTreeMap<i64, usize>,
}

impl NoteOrder {
    /// 登记一次引用:首次出现才分配下一个编号。
    fn insert(&mut self, id: i64) {
        #[cfg(test)]
        count_note_probe(1);
        if let std::collections::btree_map::Entry::Vacant(e) = self.index.entry(id) {
            self.ids.push(id);
            e.insert(self.ids.len());
        }
    }

    /// 该 id 的编号(1 起);未登记 `None`。
    fn number(&self, id: i64) -> Option<usize> {
        #[cfg(test)]
        count_note_probe(1);
        self.index.get(&id).copied()
    }
}

/// 导出期的注编号表:脚注 / 尾注各按正文中**首次引用的顺序**从 1 编号(重复引用沿用
/// 同一号);只收有定义的注,悬空引用不编号。
struct Notes<'a> {
    doc: &'a Document,
    style: NoteStyle,
    footnotes: NoteOrder,
    endnotes: NoteOrder,
    /// HTML 已输出过 `id` 的引用标签(同一注多次引用时只首次带 `id`,避免重复 id)。
    emitted: RefCell<BTreeSet<String>>,
    /// 当前作用域的列表计数。正文(含表格单元格)共用一份、按文档顺序连续推进;页眉页脚部件、
    /// 每条脚注尾注、每个文本框各开独立作用域([`Notes::scope`]),与 PDF 映射的划分一致。
    counters: RefCell<ListCounters>,
    /// 本次导出内的样式解析缓存(局部值,不进 `Document`):同一样式的编号 / 标题级别只解析一次。
    styles: RefCell<StyleCache>,
}

impl<'a> Notes<'a> {
    fn new(doc: &'a Document, style: NoteStyle) -> Self {
        let mut notes = Notes {
            doc,
            style,
            footnotes: NoteOrder::default(),
            endnotes: NoteOrder::default(),
            emitted: RefCell::new(BTreeSet::new()),
            counters: RefCell::new(ListCounters::new()),
            styles: RefCell::new(StyleCache::new()),
        };
        notes.collect(&doc.body);
        notes
    }

    /// 按文档顺序扫引用(含表格单元格与文本框)。
    fn collect(&mut self, blocks: &[Block]) {
        for b in blocks {
            match b {
                Block::Paragraph(p) => {
                    for seg in p.runs.iter().flat_map(|r| &r.segments) {
                        if let RunSegment::NoteRef { kind, id } = seg {
                            self.register(*kind, *id);
                        }
                    }
                    for tb in p.text_boxes() {
                        self.collect(&tb.blocks);
                    }
                }
                Block::Table(t) => {
                    for cell in t.rows.iter().flat_map(|r| &r.cells) {
                        self.collect(&cell.blocks);
                    }
                }
            }
        }
    }

    fn register(&mut self, kind: NoteKind, id: i64) {
        let (defined, order) = match kind {
            NoteKind::Footnote => (&self.doc.footnotes, &mut self.footnotes),
            NoteKind::Endnote => (&self.doc.endnotes, &mut self.endnotes),
        };
        if defined.contains_key(&id) {
            order.insert(id);
        }
    }

    /// 在一份全新的列表计数里跑 `f`,结束后还原外层计数(页眉页脚 / 注 / 文本框各自独立)。
    fn scope<T>(&self, f: impl FnOnce() -> T) -> T {
        let outer = self.counters.replace(ListCounters::new());
        let out = f();
        self.counters.replace(outer);
        out
    }

    /// 本导出里内联文本的呈现方式(HTML 块自行传 [`Mode::Html`])。
    fn plain_mode(&self) -> Mode {
        match self.style {
            NoteStyle::Markdown => Mode::Markdown,
            _ => Mode::Text,
        }
    }

    /// 推进并取该段落的列表标签(必须每个段落恰好调用一次,空段也要推进计数);非列表段 `None`。
    fn list_item(&self, p: &Paragraph) -> Option<ListItem> {
        let NumRef {
            num_id,
            ilvl: level,
        } = self.styles.borrow_mut().numbering(self.doc, p)?;
        let label = self
            .counters
            .borrow_mut()
            .advance(&self.doc.numbering, num_id, level)?;
        let bullet = self
            .doc
            .numbering
            .level(num_id, level)
            .is_some_and(|l| l.fmt == NumFmt::Bullet);
        Some(ListItem {
            label,
            bullet,
            level,
        })
    }

    /// 标签正文(不含方括号):脚注 `1`,尾注 `e1`。
    fn label(kind: NoteKind, n: usize) -> String {
        match kind {
            NoteKind::Footnote => n.to_string(),
            NoteKind::Endnote => format!("e{n}"),
        }
    }

    /// 正文里该引用的标记串(HTML 为原样标记,调用方不得再转义);悬空引用时 `None`。
    fn mark(&self, kind: NoteKind, id: i64) -> Option<String> {
        let order = match kind {
            NoteKind::Footnote => &self.footnotes,
            NoteKind::Endnote => &self.endnotes,
        };
        let n = order.number(id)?;
        let label = Self::label(kind, n);
        match self.style {
            NoteStyle::Html => {
                let id = if self.emitted.borrow_mut().insert(label.clone()) {
                    format!(" id=\"fnref-{label}\"")
                } else {
                    String::new()
                };
                Some(format!(
                    "<sup{id}><a href=\"#fn-{label}\">[{label}]</a></sup>"
                ))
            }
            NoteStyle::Text => Some(format!("[{label}]")),
            NoteStyle::Markdown => Some(format!("[^{label}]")),
        }
    }

    /// 文末的注定义:`(行首标签, 内容块)`,脚注在前、尾注在后,各按编号顺序。
    /// 标签:纯文本 `[1]`,Markdown `[^1]`(调用方按需加 `:`)。
    fn referenced(&self) -> Vec<(String, &'a [Block])> {
        let mut out = Vec::new();
        for (kind, order, defined) in [
            (NoteKind::Footnote, &self.footnotes, &self.doc.footnotes),
            (NoteKind::Endnote, &self.endnotes, &self.doc.endnotes),
        ] {
            for (i, id) in order.ids.iter().enumerate() {
                let label = Self::label(kind, i + 1);
                let head = match self.style {
                    NoteStyle::Markdown => format!("[^{label}]"),
                    NoteStyle::Html => label,
                    _ => format!("[{label}]"),
                };
                if let Some(blocks) = defined.get(id) {
                    out.push((head, blocks.as_slice()));
                }
            }
        }
        out
    }
}

// ============================================================ 标题映射

/// 段落的标题层级(1..=6;`None` = 普通段落):走样式表(段落 / 样式 `outlineLvl`、样式名沿
/// `basedOn` 链、styleId 字面匹配,见 [`crate::style::resolve_heading_level`]);Markdown / HTML 只有
/// 6 级,7–9 级按 6 级输出。样式部分经 [`Notes`] 的缓存,同一样式只解析一次。
fn heading_level(notes: &Notes, p: &Paragraph) -> Option<u8> {
    notes
        .styles
        .borrow_mut()
        .heading_level(notes.doc, p)
        .map(|l| l.min(6))
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

    /// F 个不同脚注引用:编号登记与取号的查找总探测次数与 F 同阶(线性),而不是 F²。
    #[test]
    fn note_numbering_lookups_are_linear() {
        let f = 2000usize;
        let mut doc = Document::default();
        let mut p = Paragraph::default();
        for i in 1..=f as i64 {
            p.runs.push(TextRun {
                segments: vec![crate::model::RunSegment::NoteRef {
                    kind: NoteKind::Footnote,
                    id: i,
                }],
                ..Default::default()
            });
            doc.footnotes.insert(i, vec![]);
        }
        doc.body.push(Block::Paragraph(p));
        NOTE_PROBES.with(|c| c.set(0));
        let text = to_text(&doc);
        let probes = NOTE_PROBES.with(|c| c.get());
        assert!(text.starts_with("[1][2][3]"), "编号按首次引用顺序");
        assert!(probes <= 4 * f, "probes {probes} 应 <= 4 * F = {}", 4 * f);
    }

    /// 修复 1:同一段里 N 个 run(普通文字 / 同一尾注的 N 次引用 / 满是 Markdown 特殊字符)时,
    /// 三种导出为判定行首 / 做 Markdown 转义而检查的总字节数与输出同阶(线性),不随 run 数平方增长。
    #[test]
    fn inline_line_state_is_linear_in_run_count() {
        let n = 4_000usize;
        let plain = || TextRun::from_text("a");
        let evil = || TextRun::from_text("[](<>*_`&~\\#-");
        let note = || TextRun {
            segments: vec![crate::model::RunSegment::NoteRef {
                kind: NoteKind::Endnote,
                id: 1,
            }],
            ..Default::default()
        };
        let link = |t: &str| TextRun {
            link_target: Some("http://x.example/".into()),
            ..TextRun::from_text(t)
        };
        let cases: Vec<(&str, Vec<TextRun>)> = vec![
            ("plain", (0..n).map(|_| plain()).collect()),
            ("note", (0..n).map(|_| note()).collect()),
            ("evil", (0..n).map(|_| evil()).collect()),
            ("link", (0..n).map(|_| link("a")).collect()),
        ];
        for (name, runs) in cases {
            let mut doc = Document {
                body: vec![Block::Paragraph(Paragraph {
                    runs,
                    ..Default::default()
                })],
                ..Default::default()
            };
            doc.endnotes
                .insert(1, vec![Block::Paragraph(para("e", None))]);
            for (fmt, f) in [
                ("text", to_text as fn(&Document) -> String),
                ("markdown", to_markdown),
                ("html", to_html),
            ] {
                SCAN_BYTES.with(|c| c.set(0));
                let out = f(&doc);
                let scanned = SCAN_BYTES.with(|c| c.get());
                assert!(
                    scanned <= 4 * out.len() + 64,
                    "{name}/{fmt}: 扫描检查了 {scanned} 字节,输出仅 {} 字节",
                    out.len()
                );
            }
        }
    }

    /// HTML:超链接内的脚注引用不能生成嵌套 `<a>`(非法):在该位置先闭合外层链接、输出注标记、
    /// 再重新打开链接,文字顺序不变;位于链接首 / 尾的注标记不留空 `<a>`。
    #[test]
    fn html_note_ref_inside_hyperlink_does_not_nest_anchors() {
        let url = Some("http://x.example/".to_string());
        let text = |t: &str| TextRun {
            link_target: url.clone(),
            ..TextRun::from_text(t)
        };
        let note = |id: i64| TextRun {
            segments: vec![crate::model::RunSegment::NoteRef {
                kind: NoteKind::Footnote,
                id,
            }],
            link_target: url.clone(),
            ..Default::default()
        };
        let mut doc = Document {
            body: vec![Block::Paragraph(Paragraph {
                runs: vec![note(1), text("a"), note(2), text("b"), note(3)],
                ..Default::default()
            })],
            ..Default::default()
        };
        for id in 1..=3 {
            doc.footnotes
                .insert(id, vec![Block::Paragraph(para("n", None))]);
        }
        let html = to_html(&doc);
        let body = html.split("<div class=\"note\"").next().unwrap().trim_end();
        let sup = |n: u32| format!("<sup id=\"fnref-{n}\"><a href=\"#fn-{n}\">[{n}]</a></sup>");
        let a = |t: &str| format!("<a href=\"http://x.example/\">{t}</a>");
        assert_eq!(
            body,
            format!("<p>{}{}{}{}{}</p>", sup(1), a("a"), sup(2), a("b"), sup(3))
        );
        // 任何时刻最多打开一个 `<a`。
        let mut open = 0i32;
        for tok in body.split('<').skip(1) {
            if tok.starts_with("a ") {
                open += 1;
                assert!(open <= 1, "嵌套 <a>:{body}");
            } else if tok.starts_with("/a>") {
                open -= 1;
            }
        }
    }

    #[test]
    fn heading_level_maps_variants() {
        let level = |style: Option<&str>| {
            let p = para("x", style);
            heading_level(&Notes::new(&Document::default(), NoteStyle::Text), &p)
        };
        assert_eq!(level(Some("Heading1")), Some(1));
        assert_eq!(level(Some("heading 3")), Some(3));
        assert_eq!(level(Some("标题2")), Some(2));
        assert_eq!(level(Some("Title")), Some(1));
        assert_eq!(level(Some("Heading9")), Some(6));
        assert_eq!(level(Some("Normal")), None);
        assert_eq!(level(None), None);
    }
}
