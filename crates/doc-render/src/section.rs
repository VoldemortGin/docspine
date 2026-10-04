//! 节([`Section`])→ 每页几何([`PageGeom`])+ 分页回调([`PageProvider`])。
//!
//! 引擎的流式布局每**起一页**(含首页)调用一次 `next_page`;docspine 一节之内页面尺寸与
//! 左右边距不变,上 / 下边距可能因页眉页脚避让逐页不同(首页 / 奇偶页用不同部件),节界处
//! 换几何。渲染层对每一节各跑一次 `layout_flow`(节界 = 强制起新页 + 换页面几何,语义与
//! 「节界处塞 `Block::PageBreak`」一致——引擎的回调无法区分溢出换页与显式换页,所以节的
//! 推进由渲染层的按节调用驱动)。

use doc_core::geom::twips_to_points;
use doc_core::model::Section;
use pdf_typeset::{PageGeom, PageProvider};

/// 一节的分页回调:按节内页序号(0 起)逐页取几何。
pub(crate) struct SectionPages<F> {
    next: usize,
    geom_for: F,
}

impl<F: FnMut(usize) -> PageGeom> SectionPages<F> {
    /// 由「节内页序号 → 该页几何(已换算成磅)」构造。
    pub(crate) fn new(geom_for: F) -> Self {
        SectionPages { next: 0, geom_for }
    }
}

impl<F: FnMut(usize) -> PageGeom> PageProvider for SectionPages<F> {
    fn next_page(&mut self) -> PageGeom {
        let geom = (self.geom_for)(self.next);
        self.next += 1;
        geom
    }
}

/// 一节的页面几何(twip → 磅)。负的上/下边距(Word 语义:正文可侵入页眉/页脚区,
/// 数值即固定的正文起点)取绝对值;装订线并入左边距(LTR 简化,v1)。
pub(crate) fn page_geom(sect: &Section) -> PageGeom {
    let m = &sect.margins;
    PageGeom {
        width: twips_to_points(sect.page_width),
        height: twips_to_points(sect.page_height),
        margin_top: twips_to_points(m.top).abs(),
        margin_right: twips_to_points(m.right).max(0.0),
        margin_bottom: twips_to_points(m.bottom).abs(),
        margin_left: twips_to_points(m.left + m.gutter).max(0.0),
    }
}
