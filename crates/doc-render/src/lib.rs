#![forbid(unsafe_code)]
//! `doc-render` —— docspine 的**布局保真** PDF 渲染层(PRD-PDF-EXPORT C-1)。
//!
//! 把(扩展后的)doc-core IR 映射到家族共享排版引擎
//! [`pdf-typeset`](pdf_typeset)(pdfspine Phase A,git dep + 钉死 rev):
//!
//! - **节 → 页面几何**:每一节对应一次 [`Typesetter::layout_flow`] 调用,分页回调
//!   ([`pdf_typeset::PageProvider`])节内页面尺寸不变、上/下边距按页眉页脚避让逐页取;
//!   节界 = 强制新页 + 换几何。
//! - **页眉页脚**(`header`):先量部件高度再排正文,正文排完(总页数已知)后逐页画,
//!   `PAGE` / `NUMPAGES` 现算。
//! - **有效样式驱动**:每段每 run 经 doc-core C-5 的 `resolve_para` / `resolve_run`
//!   (表格内 `*_in_table`)得到最终值再喂引擎;渲染前 `styles.validate()` 一次,
//!   体检告警并入 [`RenderWarning`]。
//! - **降级绝不 panic**:引擎告警 + docspine 侧降级(多栏压平、段落边框/底纹、
//!   图片待 C-8)合流成 `Vec<RenderWarning>`,由 py-bindings 按类去重浮出。
//!
//! 本 crate 不做 IO(`save_pdf` 的落盘在 py-bindings);与 `doc_core::export`
//! 的字符串导出面无共享(那是内容级,这里是版面级)。

mod header;
mod map;
mod section;
mod table;
pub mod warn;

use std::collections::BTreeMap;
use std::path::Path;

use doc_core::model::Document;
use doc_core::{DocError, PageNumFormat, Result};
use pdf_typeset::{FontResolver, ImageSpec, Op, PageOps, Typesetter};

pub use warn::RenderWarning;

/// 渲染选项。`font_map` 是用户的字体替换覆盖(请求 family → 覆盖值,如
/// `{"宋体": "Songti SC"}` 或 `{"Calibri": "/path/to/Carlito.ttf"}`),喂给引擎
/// 字体解析器。
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// 请求字体族 → 覆盖目标。值为**存在的文件路径**时,把该字体文件注入解析器
    /// (文件须包含所请求的字体族);否则视为**替代字体族名**,叠加进引擎替换表。
    pub font_map: BTreeMap<String, String>,
}

/// 一次渲染的结果:PDF 字节 + 全部降级告警(发生序)。
#[derive(Clone, Debug)]
pub struct RenderResult {
    /// 序列化好的 PDF 字节。
    pub pdf: Vec<u8>,
    /// 渲染期间的全部降级(样式体检 → 映射降级 → 引擎降级)。
    pub warnings: Vec<RenderWarning>,
}

/// 把一份解析好的文档渲染成 PDF(系统字体解析;同机同字体环境 ⇒ 字节确定)。
///
/// `media` 是解析输出的图片字节表(`裸文件名 → 字节`):内嵌光栅图按块级渲染
/// (EMU→pt 显示尺寸,C-8 已落地);EMF/WMF 等矢量格式画浅灰占位框,缺字节/缺
/// 尺寸的图跳过——各自一次性降级告警。
///
/// # Errors
///
/// 引擎序列化失败折成 [`DocError::Render`](绝不 panic)。
pub fn render_pdf(
    doc: &Document,
    media: &BTreeMap<String, Vec<u8>>,
    options: &RenderOptions,
) -> Result<RenderResult> {
    // `DOCSPINE_DETERMINISTIC_FONTS` 置位时**只用内置 Liberation/Noto 兜底字体**
    // (不扫系统字体),使输出跨机器逐字节确定——供提交式 `.ssimref` 基线的 CI 门
    // (C-10 第 4 层)与自证测试用;缺省仍走系统字体解析(同机同字体环境确定)。
    let ts = if std::env::var_os("DOCSPINE_DETERMINISTIC_FONTS").is_some() {
        Typesetter::new(FontResolver::without_system_fonts())
    } else {
        Typesetter::with_system_fonts()
    };
    render_with(ts, doc, media, options)
}

/// 在给定引擎实例上渲染(测试注入确定性字体解析器用;一实例一文档)。
fn render_with(
    mut ts: Typesetter,
    doc: &Document,
    media: &BTreeMap<String, Vec<u8>>,
    options: &RenderOptions,
) -> Result<RenderResult> {
    // 缺省制表位间隔(C-9):settings.xml 的 defaultTabStop(twip → 磅)喂给引擎;
    // 部件/属性缺失时不设置,沿用引擎缺省(36pt = 720 twip = Word 缺省 0.5 英寸)。
    if let Some(tw) = doc.default_tab_stop {
        ts.set_tab_interval(doc_core::geom::twips_to_points(tw));
    }

    // 字体替换覆盖要在任何布局之前配置(引擎按样式 memoize 解析结果)。
    for (requested, candidate) in &options.font_map {
        let path = Path::new(candidate);
        if path.is_file() {
            if let Ok(bytes) = std::fs::read(path) {
                ts.resolver_mut().add_font_data(bytes);
            }
        } else {
            ts.resolver_mut()
                .add_substitution(requested, &[candidate.as_str()]);
        }
    }

    let (pages, mut warnings) = layout_document(&mut ts, doc, media);
    let result = ts
        .emit(&pages)
        .map_err(|e| DocError::Render(e.to_string()))?;

    warnings.extend(result.warnings.into_iter().map(RenderWarning::Engine));
    Ok(RenderResult {
        pdf: result.pdf,
        warnings,
    })
}

/// 排版整篇文档:逐节流式排正文(页眉页脚先量、正文逐页避让),正文全部排完(总页数已知)
/// 后逐页画页眉页脚;返回每页 ops + docspine 侧告警(引擎告警在 `emit` 时取)。
fn layout_document(
    ts: &mut Typesetter,
    doc: &Document,
    media: &BTreeMap<String, Vec<u8>>,
) -> (Vec<PageOps>, Vec<RenderWarning>) {
    let mapped = map::map_document_with_media(doc, media);
    let mut hf = header::HeaderFooters::new(doc, media);
    let mut pages = Vec::new();
    // 每页的 (节序号, 节内页序号),页眉页脚选部件用。
    let mut placement = Vec::new();
    // 每页的显示页码(`w:pgNumType@w:start` 重置、缺省接续上一节),`PAGE` 字段与奇偶页判定用。
    let mut numbers: Vec<i64> = Vec::new();
    let mut next_number = 1_i64;
    for (si, plan) in mapped.sections.iter().enumerate() {
        hf.measure_section(ts, si, plan.geom);
        // 每节一个分页回调;引擎每起一页调用一次,含首页。
        let first_number = doc
            .sections
            .get(si)
            .map_or(next_number, |s| s.first_page_number(next_number));
        let mut provider =
            section::SectionPages::new(|k| hf.body_geom(si, k, first_number + k as i64));
        let mut section_pages = ts.layout_flow(&plan.blocks, &mut provider);
        // 锚定浮动图(C-8):画在本节**首页**的绝对位置。behindDoc 衬于正文下方
        // (插到 ops 最前),否则叠加在上层(追加到末尾)。文字不环绕(声明降级)。
        for img in &plan.overlays {
            if let Some(id) = ts.add_image(&ImageSpec::new(img.bytes.clone(), img.w, img.h)) {
                let op = Op::Image {
                    id,
                    x: img.x,
                    y: img.y,
                    w: img.w,
                    h: img.h,
                };
                if let Some(page) = section_pages.first_mut() {
                    if img.behind {
                        page.ops.insert(0, op);
                    } else {
                        page.ops.push(op);
                    }
                }
            }
        }
        placement.extend((0..section_pages.len()).map(|k| (si, k)));
        numbers.extend((0..section_pages.len()).map(|k| first_number + k as i64));
        next_number = first_number + section_pages.len() as i64;
        pages.extend(section_pages);
    }
    hf.draw(ts, &mut pages, &placement, &numbers);
    let mut warnings = mapped.warnings;
    if doc
        .sections
        .iter()
        .any(|s| s.page_number_format == PageNumFormat::Other)
    {
        warnings.push(RenderWarning::PageNumFormatUnsupported);
    }
    warnings.extend(hf.into_warnings());
    (pages, warnings)
}

// ============================================================ 冒烟测试(确定性字体)

#[cfg(test)]
mod tests {
    use super::*;
    use doc_core::model::{
        Block as DocBlock, BreakKind, Cell, Paragraph, Row, RunSegment, Section, Table as DocTable,
        TextBox, TextRun,
    };
    use pdf_typeset::FontResolver;

    /// 确定性引擎(仅内置 Liberation/Noto 兜底字体,不扫系统字体)。
    fn deterministic() -> Typesetter {
        Typesetter::new(FontResolver::without_system_fonts())
    }

    fn para(text: &str) -> DocBlock {
        DocBlock::Paragraph(Paragraph {
            runs: vec![TextRun::from_text(text)],
            ..Paragraph::default()
        })
    }

    fn doc_of(body: Vec<DocBlock>) -> Document {
        let end = body.len();
        Document {
            body,
            sections: vec![Section {
                end_block: end,
                ..Section::default()
            }],
            ..Document::default()
        }
    }

    /// 数 PDF 里的页对象(`/Type /Page`,排除 `/Pages`)。pdf-typeset 的页对象字典
    /// 不压缩,可直接按字节数。
    fn count_pages(pdf: &[u8]) -> usize {
        let needle = b"/Type /Page";
        pdf.windows(needle.len())
            .enumerate()
            .filter(|(i, w)| *w == needle && pdf.get(i + needle.len()) != Some(&b's'))
            .count()
    }

    /// C-1 绿条(Rust 侧):两段直格文档 → 合法 PDF、1 页、无告警噪声。
    #[test]
    fn two_paragraph_direct_format_renders_one_page() {
        let mut styled = TextRun::from_text("Hello bold world");
        styled.rpr.b = Some(true);
        let doc = doc_of(vec![
            para("Plain paragraph one."),
            DocBlock::Paragraph(Paragraph {
                runs: vec![styled],
                ..Paragraph::default()
            }),
        ]);
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert!(res.pdf.starts_with(b"%PDF-"));
        assert_eq!(count_pages(&res.pdf), 1);
    }

    fn render_fast(doc: &Document) -> RenderResult {
        let t0 = std::time::Instant::now();
        let res = render_with(
            deterministic(),
            doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(20),
            "应很快返回"
        );
        res
    }

    fn kinds(res: &RenderResult) -> Vec<&'static str> {
        res.warnings.iter().map(RenderWarning::kind).collect()
    }

    /// 无 `tblGrid`、首格 `gridSpan` 巨大(手工构造的 IR 绕过解析钳制):列数钳到
    /// Word 上限,不按声明值分配,渲染不 abort。
    #[test]
    fn huge_grid_span_without_grid_renders_quickly() {
        let table = DocTable {
            rows: vec![Row {
                cells: vec![Cell {
                    grid_span: u32::MAX,
                    blocks: vec![para("wide")],
                    ..Cell::default()
                }],
                ..Row::default()
            }],
            ..DocTable::default()
        };
        let res = render_fast(&doc_of(vec![DocBlock::Table(table)]));
        assert!(res.pdf.starts_with(b"%PDF-"));
        assert!(!kinds(&res).contains(&"table-over-budget"));
    }

    /// 上万 `gridCol` × 上万空行:列数钳到 63、行数按总格预算截断并告警一次,不 abort。
    #[test]
    fn oversized_grid_times_rows_is_truncated_with_warning() {
        let table = DocTable {
            grid_cols: vec![100; 10_000],
            rows: vec![Row::default(); 10_000],
            ..DocTable::default()
        };
        let res = render_fast(&doc_of(vec![DocBlock::Table(table)]));
        assert!(res.pdf.starts_with(b"%PDF-"));
        let n = kinds(&res)
            .iter()
            .filter(|k| **k == "table-over-budget")
            .count();
        assert_eq!(n, 1, "超预算只告警一次");
    }

    /// 正常的 63 列表不受限、无告警。
    #[test]
    fn normal_63_column_table_renders_without_budget_warning() {
        let cell = |t: &str| Cell {
            grid_span: 1,
            blocks: vec![para(t)],
            ..Cell::default()
        };
        let table = DocTable {
            grid_cols: vec![100; 63],
            rows: vec![
                Row {
                    cells: (0..63).map(|_| cell("x")).collect(),
                    ..Row::default()
                };
                20
            ],
            ..DocTable::default()
        };
        let res = render_fast(&doc_of(vec![DocBlock::Table(table)]));
        assert!(!kinds(&res).contains(&"table-over-budget"));
    }

    /// 浮动文本框只抽取不绘制:渲染不 panic、照常出页,`text-box-not-rendered` 只报一次。
    #[test]
    fn text_boxes_warn_once_and_are_not_drawn() {
        let mut anchor = TextRun::from_text("anchor");
        for _ in 0..2 {
            anchor.text_boxes.push(TextBox {
                blocks: vec![para("inside box")],
            });
        }
        let doc = doc_of(vec![
            DocBlock::Paragraph(Paragraph {
                runs: vec![anchor.clone()],
                ..Paragraph::default()
            }),
            DocBlock::Paragraph(Paragraph {
                runs: vec![anchor],
                ..Paragraph::default()
            }),
        ]);
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert_eq!(count_pages(&res.pdf), 1);
        let n = res
            .warnings
            .iter()
            .filter(|w| w.kind() == "text-box-not-rendered")
            .count();
        assert_eq!(n, 1);
    }

    /// 公式 run(`is_math`)按普通文字渲染不 panic,`math-flattened` 只报一次。
    #[test]
    fn math_runs_warn_once() {
        let mut m = TextRun::from_text("x12");
        m.is_math = true;
        let doc = doc_of(vec![
            DocBlock::Paragraph(Paragraph {
                runs: vec![m.clone()],
                ..Paragraph::default()
            }),
            DocBlock::Paragraph(Paragraph {
                runs: vec![m],
                ..Paragraph::default()
            }),
        ]);
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert_eq!(count_pages(&res.pdf), 1);
        let n = res
            .warnings
            .iter()
            .filter(|w| w.kind() == "math-flattened")
            .count();
        assert_eq!(n, 1);
    }

    /// 页眉页脚现在照画(原 `header_footer_warns_once_and_is_not_drawn` 断言“告警一次且
    /// 不画”,行为改变后改写):渲染不 panic、页数不变,页眉 / 页脚文字进了页面 ops,
    /// 不再发任何 `header-footer-*` 告警;只有空页眉同样不告警。
    #[test]
    fn header_footer_is_drawn_without_warning() {
        use doc_core::model::{HeaderFooterKind, HeaderFooterRef};
        let r = |id: &str| HeaderFooterRef {
            kind: HeaderFooterKind::Default,
            rel_id: id.to_string(),
        };
        let mut doc = doc_of(vec![para("body")]);
        doc.sections[0].headers = vec![r("h1")];
        doc.sections[0].footers = vec![r("f1")];
        doc.header_footers
            .insert("h1".into(), vec![para("page header")]);
        doc.header_footers
            .insert("f1".into(), vec![para("page footer")]);
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert_eq!(count_pages(&res.pdf), 1);
        let n = |res: &RenderResult| {
            res.warnings
                .iter()
                .filter(|w| w.kind().starts_with("header-footer"))
                .count()
        };
        assert_eq!(n(&res), 0);
        let (pages, _) = layout_document(&mut deterministic(), &doc, &BTreeMap::new());
        let drawn: String = pages[0]
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            drawn.contains("header") && drawn.contains("footer"),
            "{drawn}"
        );

        // 只有空页眉:不告警。
        let mut empty = doc_of(vec![para("body")]);
        empty.sections[0].headers = vec![r("h1")];
        empty
            .header_footers
            .insert("h1".into(), vec![DocBlock::Paragraph(Paragraph::default())]);
        let res = render_with(
            deterministic(),
            &empty,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert_eq!(n(&res), 0);
    }

    /// 脚注 / 尾注引用不画不 panic,`notes-not-rendered` 只报一次(多引用、脚注 + 尾注并存)。
    #[test]
    fn note_refs_warn_once_and_are_not_drawn() {
        use doc_core::model::{NoteKind, RunSegment};
        let mut run = TextRun::from_text("text");
        run.segments.push(RunSegment::NoteRef {
            kind: NoteKind::Footnote,
            id: 1,
        });
        run.segments.push(RunSegment::NoteRef {
            kind: NoteKind::Endnote,
            id: 1,
        });
        let doc = doc_of(vec![
            DocBlock::Paragraph(Paragraph {
                runs: vec![run.clone()],
                ..Paragraph::default()
            }),
            DocBlock::Paragraph(Paragraph {
                runs: vec![run],
                ..Paragraph::default()
            }),
        ]);
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert_eq!(count_pages(&res.pdf), 1);
        let n = res
            .warnings
            .iter()
            .filter(|w| w.kind() == "notes-not-rendered")
            .count();
        assert_eq!(n, 1);
    }

    /// 节界起新页 + 段内 `w:br@page` 起新页:1 + 1 + 1 = 3 页。
    #[test]
    fn sections_and_explicit_page_breaks_paginate() {
        let mut breaking = TextRun::from_text("before");
        breaking.segments.push(RunSegment::Break(BreakKind::Page));
        breaking.segments.push(RunSegment::Text("after".into()));
        let mut doc = doc_of(vec![
            para("section one"),
            DocBlock::Paragraph(Paragraph {
                runs: vec![breaking],
                ..Paragraph::default()
            }),
        ]);
        doc.sections = vec![
            Section {
                end_block: 1,
                ..Section::default()
            },
            Section {
                page_width: 16_838, // A4 横向
                page_height: 11_906,
                end_block: 2,
                ..Section::default()
            },
        ];
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert_eq!(count_pages(&res.pdf), 3, "节界 1 次 + 段内换页 1 次");
    }

    /// font_map 替换覆盖:未安装的请求名经候选解析,引擎报 FontSubstituted。
    #[test]
    fn font_map_feeds_engine_substitutions() {
        let mut run = TextRun::from_text("mapped");
        run.rpr.fonts.ascii = Some(doc_core::style::FontRef::Named("NoSuchFamily".into()));
        let doc = doc_of(vec![DocBlock::Paragraph(Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        })]);
        let mut options = RenderOptions::default();
        options
            .font_map
            .insert("NoSuchFamily".into(), "Liberation Serif".into());
        let res = render_with(deterministic(), &doc, &BTreeMap::new(), &options).expect("render");
        assert!(res.warnings.iter().any(|w| w.kind() == "font-substituted"));
    }

    /// UTF-16BE 字节(name 表 Windows 平台字符串编码)。
    fn utf16be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }

    /// 等长字节替换(name 表改族名不动表长,校验和不重算,ttf-parser/fontdb 不校验)。
    fn replace_bytes(haystack: &mut [u8], needle: &[u8], repl: &[u8]) {
        assert_eq!(needle.len(), repl.len());
        let mut i = 0;
        while i + needle.len() <= haystack.len() {
            if haystack[i..i + needle.len()] == *needle {
                haystack[i..i + needle.len()].copy_from_slice(repl);
                i += needle.len();
            } else {
                i += 1;
            }
        }
    }

    /// 改名 name 表里的字体族(同 pdf-typeset 自己的 fontres 测试写法):等长
    /// 同时替换 Latin-1/Mac-Roman 与 UTF-16BE 两种编码,产出一个不会与内置/系统
    /// 字体撞名的合成字体。
    fn rename_family(font: &[u8], from: &str, to: &str) -> Vec<u8> {
        assert_eq!(from.len(), to.len());
        let mut out = font.to_vec();
        replace_bytes(&mut out, from.as_bytes(), to.as_bytes());
        replace_bytes(&mut out, &utf16be(from), &utf16be(to));
        out
    }

    /// font_map 的文件路径分支:值是磁盘上存在的字体文件 → 直接把字节注入解析器
    /// (`add_font_data`),而非仅仅注册一条替换族名。用改名后的内嵌 Liberation Sans
    /// 写临时文件,请求族名与注入字体的真实族名一致;零 `font-substituted` 告警才
    /// 说明确实是这份文件的字节被解析并命中,不是巧合撞上了内置替换表。
    #[test]
    fn font_map_file_path_injects_font_bytes() {
        use pdf_fonts::liberation::{liberation_face, LiberationFamily};

        let renamed = rename_family(
            liberation_face(LiberationFamily::Sans, false, false),
            "Liberation Sans",
            "DocspineTestFnt",
        );
        let path =
            std::env::temp_dir().join(format!("docspine-font-map-test-{}.ttf", std::process::id()));
        std::fs::write(&path, &renamed).expect("write temp font fixture");

        let mut run = TextRun::from_text("embedded");
        run.rpr.fonts.ascii = Some(doc_core::style::FontRef::Named("DocspineTestFnt".into()));
        let doc = doc_of(vec![DocBlock::Paragraph(Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        })]);
        let mut options = RenderOptions::default();
        options.font_map.insert(
            "DocspineTestFnt".into(),
            path.to_str().expect("utf8 temp path").into(),
        );
        let res = render_with(deterministic(), &doc, &BTreeMap::new(), &options).expect("render");
        let _ = std::fs::remove_file(&path);

        assert!(
            !res.warnings.iter().any(|w| w.kind() == "font-substituted"),
            "font_map 文件路径注入的字体应直接命中,不应触发替换告警: {:?}",
            res.warnings
        );
    }

    /// 样式表 basedOn 环:渲染不悬挂,体检告警浮出。
    #[test]
    fn style_cycle_surfaces_as_warning_and_terminates() {
        let mut doc = doc_of(vec![para("cyclic")]);
        for (id, base) in [("A", "B"), ("B", "A")] {
            doc.styles.styles.insert(
                id.into(),
                doc_core::style::Style {
                    based_on: Some(base.into()),
                    ..Default::default()
                },
            );
        }
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render must terminate");
        assert!(res
            .warnings
            .iter()
            .any(|w| w.kind() == "style-based-on-cycle"));
    }

    /// 负字符间距:可前进的紧缩照常排、无回退告警;紧缩过度(簇不前进)时引擎只把
    /// 该段负间距归零 + `signed-spacing-fallback` 告警,文本照常渲染、不 panic。
    #[test]
    fn negative_char_spacing_condenses_or_falls_back_with_warning() {
        let render = |twips: i64| {
            let mut run = TextRun::from_text("Condensed text");
            run.rpr.spacing = Some(twips);
            let doc = doc_of(vec![DocBlock::Paragraph(Paragraph {
                runs: vec![run],
                ..Paragraph::default()
            })]);
            render_with(
                deterministic(),
                &doc,
                &BTreeMap::new(),
                &RenderOptions::default(),
            )
            .expect("render")
        };
        let mild = render(-20); // -1pt
        assert!(mild.pdf.starts_with(b"%PDF"));
        assert!(
            !mild
                .warnings
                .iter()
                .any(|w| w.kind() == "signed-spacing-fallback"),
            "-1pt 可前进,不应回退: {:?}",
            mild.warnings
        );
        let extreme = render(-2000); // -100pt:簇前进量为负
        assert!(extreme.pdf.starts_with(b"%PDF"));
        assert!(extreme
            .warnings
            .iter()
            .any(|w| w.kind() == "signed-spacing-fallback"));
    }
}
