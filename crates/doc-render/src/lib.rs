#![forbid(unsafe_code)]
//! `doc-render` —— docspine 的**布局保真** PDF 渲染层(PRD-PDF-EXPORT C-1)。
//!
//! 把(扩展后的)doc-core IR 映射到家族共享排版引擎
//! [`pdf-typeset`](pdf_typeset)(pdfspine Phase A,git dep + 钉死 rev):
//!
//! - **节 → 页面几何**:每一节对应一次 [`Typesetter::layout_flow`] 调用,分页回调
//!   ([`pdf_typeset::PageProvider`])节内页页同几何;节界 = 强制新页 + 换几何。
//! - **有效样式驱动**:每段每 run 经 doc-core C-5 的 `resolve_para` / `resolve_run`
//!   (表格内 `*_in_table`)得到最终值再喂引擎;渲染前 `styles.validate()` 一次,
//!   体检告警并入 [`RenderWarning`]。
//! - **降级绝不 panic**:引擎告警 + docspine 侧降级(多栏压平、段落边框/底纹、
//!   图片待 C-8)合流成 `Vec<RenderWarning>`,由 py-bindings 按类去重浮出。
//!
//! 本 crate 不做 IO(`save_pdf` 的落盘在 py-bindings);与 `doc_core::export`
//! 的字符串导出面无共享(那是内容级,这里是版面级)。

mod map;
mod section;
mod table;
pub mod warn;

use std::collections::BTreeMap;
use std::path::Path;

use doc_core::model::Document;
use doc_core::{DocError, Result};
use pdf_typeset::Typesetter;

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
    render_with(Typesetter::with_system_fonts(), doc, media, options)
}

/// 在给定引擎实例上渲染(测试注入确定性字体解析器用;一实例一文档)。
fn render_with(
    mut ts: Typesetter,
    doc: &Document,
    media: &BTreeMap<String, Vec<u8>>,
    options: &RenderOptions,
) -> Result<RenderResult> {
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

    let mapped = map::map_document_with_media(doc, media);
    let mut pages = Vec::new();
    for plan in &mapped.sections {
        // 每节一个分页回调(节内页页同几何);引擎每起一页调用一次,含首页。
        let mut provider = section::SectionPages::new(plan.geom);
        pages.extend(ts.layout_flow(&plan.blocks, &mut provider));
    }
    let result = ts
        .emit(&pages)
        .map_err(|e| DocError::Render(e.to_string()))?;

    let mut warnings = mapped.warnings;
    warnings.extend(result.warnings.into_iter().map(RenderWarning::Engine));
    Ok(RenderResult {
        pdf: result.pdf,
        warnings,
    })
}

// ============================================================ 冒烟测试(确定性字体)

#[cfg(test)]
mod tests {
    use super::*;
    use doc_core::model::{Block as DocBlock, BreakKind, Paragraph, RunSegment, Section, TextRun};
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
}
