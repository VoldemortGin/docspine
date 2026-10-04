# CLAUDE.md — docspine(宪章)

> 家族关系与依赖：先读 [`docs/spine-family.md`](docs/spine-family.md)（每仓副本相同，真源在家族根目录）。

Spine 家族成员之一:**纯 Rust 的 Word(.docx / OOXML)结构化解析器 + 本地图片 OCR**,
**表格解析是重点**。先读家族 `../README.md`,本文件是 docspine 的操作指南,风格对齐
`../corespine/CLAUDE.md` / `../pptspine/CLAUDE.md`。

## 这是什么

`.docx` 本质是 OOXML —— 一个装着 XML 部件的 zip 包。docspine **直接走 `word/document.xml`**,
把它解析成**信息无损**的结构化模型:段落(带样式的 run)、**表格(行/列/单元格 + 横向
`gridSpan` / 纵向 `vMerge` 合并 + 嵌套表 + 单元格段落/填充/宽度,做扎实)**、内嵌图片
(`word/media/`,经 `word/_rels` 关系定位)。嵌入的图片还可以经**离线、确定性**的姊妹 crate
[`ocrspine`](../ocrspine)(PP-OCRv5 / `tract-onnx`)做本地 OCR;若图片本身是一张表格,可从 OCR
词框**几何重建**成网格(思路移植自 pdfspine 的 `image_table`)。**无云端、无网络。**

解析好的文档还能**导出 PDF**(`to_pdf()` / `save_pdf()`):`doc-render` 把有效样式驱动的 IR 喂给
家族共享的纯 Rust 排版引擎 [`pdf-typeset`](../pdfspine)(pdfspine Phase A,git dep + 钉死 rev),
得到流式版面 + 分页 + 逐节页面几何(`sectPr`)、样式/编号/表格保真——无 LibreOffice、无云转换。

docspine 是文档引擎三件套(pdf / ppt / doc)里的 `doc`,与 pdfspine / pptspine 共享 ocrspine。

## 宪章(不可违背)

- **零网络、零云 LLM。** OCR 一律走本地 `ocrspine`(tract-onnx),确定性输出。任何联网/云推理
  的代码**不准进**。
- **容错解析,绝不 panic。** 未知元素跳过、缺失属性 → `None`、畸形输入 → 类型化 `DocError`。
  解析层对脏输入必须健壮。
- **不被恶意输入拖垮。** zip 读取走 `doc_parse::ZipLimits`(缺省:条目数 10 000、单条目 256 MiB、
  累计解压 1 GiB、压缩比 10 000(仅对 > 1 MiB 的条目判定;deflate 上限 ~1032:1 永远触发不了,
  只拦其他压缩方法的炸弹)、条目名 1024 字节),绝不按条目头声明大小预分配(`take(limit + 1)` 流式读,防声明造假);拒绝绝对路径 / `..` 条目名;
  zip 限额触限一律 `DocError::LimitExceeded { kind: LimitKind, limit, actual }`(Python 侧为
  `DocZipError`,信息带限额种类如 `entry-bytes`);自定义限额用 `parse_*_with_limits`(Rust)。
  `w:tbl` / `w:sdt` / `w:customXml` / 行内 run 容器(`w:hyperlink`·`w:ins`·`w:moveTo`·`w:fldSimple`·
  `w:smartTag`)/ `mc:AlternateContent` / 文本框 `w:txbxContent` 递归深度上限 `MAX_NEST_DEPTH = 64`,
  更深的子树静默跳过(不报错)。
  行属性解析 `trHeight` / `tblHeader` / `cantSplit` / `gridBefore` / `gridAfter`(后两者钳到 `MAX_TABLE_COLS`,列号与 `vMerge` 配对 / HTML rowspan / PDF 版面都计入行首空缺;上方不是 restart 的孤立 `vMerge continue` 当普通格,内容保留)。表格网格尺寸也有上限:`gridSpan` 与 `w:tblGrid` 列数解析时钳到 Word 的 `MAX_TABLE_COLS = 63`,`Table::col_count()` 同样封顶(饱和求和);PDF 映射按 `MAX_TABLE_CELLS = 250_000`(列数 × 行数)截行并发 `table-over-budget` 告警(文本 / HTML 导出不受影响)。列表 / 页码编号超过 `MAX_LIST_NUMBER = 32767` 时字母 / 罗马格式回退十进制,计数自增饱和。
- **正文不静默丢失。** 透明容器(`w:sdt` / `w:customXml` / `w:smartTag` / `w:hyperlink` / `w:fldSimple`)
  展开;修订按“接受全部”:`w:ins`·`w:moveTo` 保留、`w:del`·`w:moveFrom` 丢弃,`w:pPrChange`·`w:rPrChange`·`w:tcPrChange`·`w:tblPrChange`·`w:trPrChange`·`w:sectPrChange`·`w:tblGridChange` 里的修订前旧属性整体跳过;复杂字段只留缓存结果
  (`w:instrText` 不进正文);`mc:AlternateContent` 取第一个产出非空内容的 `mc:Choice`,否则 `mc:Fallback`
  (绝不两份都出;落选 Choice 推进过的复杂字段栈会回滚,字段状态只认选中分支);浮动文本框只抽取(`TextRun.text_boxes`,导出紧随锚定段落),PDF 不画 + 告警;
  行级·单元格级 `w:sdt`/`w:customXml` 透明展开;`m:oMath`/`m:oMathPara` 只抽 `m:t` 文本(分式 / 上下标 / 根号用 `1/2`·`x^2`·`x_i`·`sqrt(x)` 线性记法消歧,`m:oMathPara > m:oMath` / `m:d` / `m:nary` / `m:func` 等外壳透明,其内结构同样线性化,`TextRun.is_math`,PDF 按普通文字出 + `math-flattened` 告警)。
  页眉页脚(`Section.headers/footers` 引用 + `Document.header_footers` 部件表,按部件去重导出)与脚注尾注(`Document.footnotes/endnotes` + `RunSegment::NoteRef` 引用)只抽取进 `to_text`/`to_markdown`/`to_html`(HTML 用 `<header>`/`<footer>` + `<sup>` 锚点与回链),页眉页脚 PDF 逐页照画(`doc-render/src/header.rs`:生效部件走 `Document::header_footer_for_page`——`w:titlePg` 首页 / `w:evenAndOddHeaders` 偶数页 / 缺类型逐节向前继承;先量后排、正文避让,避不开时 `header-footer-overflow`;`PAGE`/`NUMPAGES` 现算——`PAGE` 取显示页码(`w:pgNumType@w:start` 重置 / 缺省接续上一节,按节 `w:fmt` 或字段 `\* roman` 等开关格式化,奇偶页眉也看显示页码;未知 `w:fmt` 按阿拉伯数字 + `page-number-format-unsupported` 告警),`NUMPAGES` 恒为物理总页数,其余字段用缓存结果,`TextRun.field` 记字段指令:`Arc<str>`,同一字段结果区的 run 共享一份,单条指令封顶 `MAX_FIELD_INSTR = 4096` 字节);脚注尾注 PDF 不画 + `notes-not-rendered` 告警。
  `w:ruby` 只取基字 `w:rubyBase` 进正文(注音 `w:rt` 不进正文、不存);`w:dir` / `w:bdo` 与 `w:hyperlink` 等同为 run 容器;`w:altChunk`(外部内容块)不解析,只计数进 `Document.alt_chunk_count`(Python `alt_chunk_count`)+ PDF 导出 `alt-chunk-skipped` 告警。解析诊断通道 `Document.diagnostics`(Python `doc.diagnostics()`):`Diagnostic{kind,part,count}`(`DiagnosticKind` `#[non_exhaustive]`:xml-truncated / nesting-depth-exceeded / table-columns-clamped / grid-span-clamped / numbering-value-clamped / missing-part / alt-chunk-not-imported / style-chain-truncated / notes-truncated / field-instr-truncated),只含种类 / 部件路径 / 计数、绝不含正文;walker 只在 `Ctx.stats` 累加计数,`lib.rs::parse_part` 统一转诊断并做每部件一次良构检查。文档属性 `docProps/core.xml` → `Document.core_properties`(`xml/core_props.rs`;经包根 rels 的 core-properties 关系定位,回退 `docProps/core.xml`;Python `core_properties()` 固定键 dict,缺失 `None`);属性值是隐私字段,绝不进告警 / 诊断 / trace。Python 的 `text` / `to_text` / `to_markdown` / `to_html` 在 `py.detach` 下释放 GIL。
  批注部件(`word/comments.xml` → `Document.comments` + 正文 `RunSegment::CommentRef` 引用点;作者 / 内容是文档内容可进模型,但**不得**进 trace / 日志 / 告警)是审阅元数据,默认**不进**任何导出,PDF 不画。
  仍未覆盖:批注范围(`commentRangeStart/End`)、`commentsExtended` 回复链。
- **缝的元模式(家族统一)。** 唯一外部能力(OCR)经 Protocol seam 接入:`OcrEngine`(来自
  `ocrspine`)是协议,`PaddleOcr` 是确定性默认实现;core 只依赖协议,**绝不**直接 import 任何
  推理 SDK。
- **docx 优先,旧 .doc 后续。** 现代 `.docx` 是主目标。旧二进制 `.doc`(OLE/CFB,`[MS-DOC]`)
  只做**探测 + 类型化降级**(`legacy-doc` 特性下用 `cfb` crate 列流);完整正文重建后续,别卡主线。
- **最小、优雅,不过度设计。** 文本 + 表格是**必须项**且要扎实;样式/颜色尽力而为。痛了再抽,
  带证据抽。

## 铁律(本仓特有)

- **`../pdfspine/` 与 `../pptspine/` 只读。** 它们正在发布 CI。可读它们学模式(PyO3 chokepoint /
  工作区布局 / image_table 几何 / release.yml 的 git dep 写法),但**绝不**写入或修改它们的
  **任何**文件。
- **依赖 `../ocrspine`(git dep,**不是** path)。** 在 `[workspace.dependencies]` 里一次性声明
  `ocrspine = { git = "...", rev = "041958a…" }`(同 pptspine 现在的写法),`doc-ocr` 用
  `ocrspine.workspace = true`。家族发布统一走 git dep —— CI 的 `maturin build` 会自己
  `cargo fetch` ocrspine,runner 上不需要 sibling checkout。

## 模块地图(按 crate 定位)

```
crates/
  doc-core/    领域模型 + 几何(twip/EMU) + 类型化 DocError。无 IO / zip / XML。#![forbid(unsafe_code)]
    src/error.rs   DocError(thiserror):Zip/Xml/Unsupported/InvalidArgument/Io/Ocr + kind() + Result<T>
    src/geom.rs    Twips(1440/inch) + Emu(914400/inch) + *_to_points
    src/model.rs   Document/Block(Paragraph|Table)/Paragraph/TextRun/Table/Row/Cell/VMerge/Picture/Color
    src/style.rs   有效样式 resolver:docDefaults → 表格样式 → pStyle basedOn 链 → 直接格式(级联 + theme 解引 + 防环)
    src/numbering.rs 列表模型 + 计数引擎:numId/ilvl → 标签串(起值/编号格式/层级重置);段落有效编号(含样式级 numPr + `lvl@pStyle`)由 `style::resolve_numbering` 统一解析
    src/export.rs  Document → 纯文本 / Markdown / HTML(纯序列化;含合并单元格转 HTML `<table>`);标题识别走样式表(`style::resolve_heading_level`:段落 / 样式 `outlineLvl` → 样式名沿 basedOn → styleId 字面;Markdown / HTML 7–9 级按 6 级);列表标签(`ListCounters` 现算,正文含单元格连续、页眉页脚 / 注 / 文本框各自独立)/ 超链接(仅 `http`·`https`·`mailto`)/ 图片(`Picture.alt` ← `docPr@descr|title`)进三种导出
  doc-parse/   OOXML 读取:zip 解包 + quick-xml 遍历 -> Document。本轮核心。#![forbid(unsafe_code)]
    src/lib.rs     parse_path / parse_bytes -> ParsedDoc { document, media };CFB 早判降级
    src/zip_pkg.rs zip 读 API:主部件经 `_rels/.rels` 的 officeDocument 关系定位(回退 word/document.xml),附属部件(styles/numbering/settings/footnotes/endnotes/comments/theme)经主部件 rels 按类型定位(回退固定路径;`..` 逃出包根拒绝)+ media + ZipLimits 解压限额
    src/xml/document.rs  quick-xml walker:w:body -> blocks;段落/run/样式 + **表格(合并/嵌套/填充)** + 图片
    src/xml/props.rs     共享 rPr/pPr 属性片段解析(document.xml 与 styles.xml 同构,只写一份)
    src/xml/styles.rs    styles.xml → StyleTable:docDefaults + 样式定义(id/basedOn/type/default)
    src/xml/theme.rs     theme1.xml → Theme:clrScheme 颜色槽 + fontScheme 主/次字体
    src/xml/numbering.rs numbering.xml → NumberingTable:num → abstractNum + 每层 lvl
    src/xml/settings.rs  settings.xml → defaultTabStop(C-9 制表位间隔;缺失落 720 twip 缺省)+ evenAndOddHeaders
    src/legacy.rs  旧二进制 .doc(OLE/CFB)探测:probe_doc(legacy-doc 特性) + CFB_MAGIC 早判
  doc-ocr/     图片 OCR 桥 + 图像表格几何重建。#![forbid(unsafe_code)]
    src/lib.rs     ocr_image_bytes / DocOcr{engine};把 OcrWord 映射成 OcrItem
    src/table.rs   reconstruct_from_words / reconstruct_table_from_image:行列带状聚类 -> 网格(移植自 pdfspine image_table)
  doc-render/  docx IR → PDF 布局保真渲染(PRD-PDF-EXPORT):over 家族共享 pdf-typeset 引擎(git dep)。#![forbid(unsafe_code)]
    src/lib.rs     render_pdf / RenderOptions{font_map} / RenderResult{pdf, warnings};按节 layout_flow
    src/map.rs     doc-core IR → 引擎 Block:有效样式驱动 + run 分段 + 列表标签 + 图片/EMF·WMF 占位
    src/section.rs 节 → PageGeom + 分页回调(逐页取几何:页眉页脚避让改上/下边距,节界换几何)
    src/header.rs  页眉页脚:按页选部件 + 先量后排正文避让 + 页眉/页脚区排版 + PAGE/NUMPAGES 现算
    src/table.rs   表格映射:span map 压平 + 边框冲突消解 + 单元格边距/行高 + vAlign 引擎锚定
    src/warn.rs    RenderWarning 枚举(引擎侧 + docspine 侧降级)+ kind() 去重标签
  py-bindings/ PyO3 _core 扩展。唯一用 unsafe(经 PyO3)的 crate。#![deny(unsafe_op_in_unsafe_fn)]
    src/lib.rs     open -> Document handle;body()/paragraphs()/tables()/text() -> list[dict];ocr_image / reconstruct_image_table(ocr 特性);probe_doc;异常层级
```

特性:`py-bindings` 的 `ocr` 特性(默认在 `[tool.maturin] features` 里开)编入 doc-ocr/ocrspine;
`legacy-doc` 透传给 doc-parse 的同名特性。精简结构解析(无 OCR)默认零重依赖。

## 跑(始终从包根)

```bash
uv venv .venv
VIRTUAL_ENV="$(pwd)/.venv" uv pip install maturin pytest "pdfspine>=0.8,<0.13"   # pdfspine = test extra,缺它 PDF 测试静默 skip
cargo test -p doc-core -p doc-parse                 # 纯解析单测,秒级
cargo test -p doc-ocr -p doc-render                 # OCR 几何重建 + PDF 渲染单测(首次编译 ocrspine/pdf-typeset 较慢)
OCRSPINE_MODELS="$(cd ../ocrspine && pwd)/models" \
  VIRTUAL_ENV="$(pwd)/.venv" .venv/bin/maturin develop --release
.venv/bin/python -c "import docspine"               # 期望 import-clean
OCRSPINE_MODELS="$(cd ../ocrspine && pwd)/models" \
  .venv/bin/python -m pytest python/tests -q         # 解析测试必过;OCR 测试需 models env
```

注:`cargo build -p py-bindings`(独立 link)会因 `extension-module` 缺 libpython 符号而**预期失败**;
请用 `cargo check`(类型检查)或 `maturin develop`(真正构建扩展),与 pptspine 一致。

## Fuzzing(cargo-fuzz,把"绝不 panic"变成可证明)

`fuzz/` 是**独立 package + 独立 workspace**(根 `Cargo.toml` 里 `exclude = ["fuzz"]`),需要 nightly +
`cargo install cargo-fuzz`;不进 `ci.yml`,由 `.github/workflows/fuzz.yml` 每日跑(也可手动触发)。
只有 panic / abort / OOM(`-rss_limit_mb=2048`)算失败,任何 `Err` 都可接受。

```bash
cargo run --manifest-path fuzz/Cargo.toml --bin make_seeds       # 现场生成种子到 fuzz/corpus/(已 .gitignore,不落二进制 fixture)
cargo +nightly fuzz run parse_document_xml -- -max_total_time=120 -rss_limit_mb=2048
cargo +nightly fuzz run parse_docx         -- -max_total_time=120 -rss_limit_mb=2048
cargo +nightly fuzz run parse_parts        -- -max_total_time=120 -rss_limit_mb=2048
cargo +nightly fuzz run render_pdf         -- -max_total_time=120 -rss_limit_mb=2048
```

- target:`parse_docx`(任意字节 → `parse_bytes`)、`parse_document_xml`(字节当 `word/document.xml`,
  现场打成最小 zip,直达 XML 层,收益最大)、`render_pdf`(`PK` 开头按 docx,否则当 document.xml;解析成功再
  `render_pdf`,`DOCSPINE_DETERMINISTIC_FONTS=1` 不扫系统字体)。`parse_parts`(首字节选部件种类 document / styles / numbering / header / footnotes / comments / settings,其余字节当该部件 XML,其它部件取最小合法内容;解析后跑导出器 + `render_pdf`)。各解析 target 末尾都跑 `to_text` / `to_markdown` / `to_html`(`fuzz/src/lib.rs::exercise_exports`,另推进编号计数引擎)。
- 复现 crash:`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<crash-file>`;最小化:
  `cargo +nightly fuzz tmin <target> <crash-file>`;`RUST_BACKTRACE=1` 看 panic 位置。
- 修复流程:在对应 crate 最小改动修掉,并往 `crates/doc-parse/tests/fuzz_regressions.rs`(或所属 crate 的
  单测)加**现场构造**的回归测试(不提交 crash 二进制)。
- 新增 target:`fuzz/fuzz_targets/<name>.rs` + `fuzz/Cargo.toml` 加 `[[bin]]` + `fuzz.yml` 的 matrix 加名字
  + `fuzz/seed.rs` 补种子;共用的打包帮助函数放 `fuzz/src/lib.rs`。
- `fuzz/Cargo.lock` 由根 `Cargo.lock` 拷贝而来以钉住依赖版本;根 workspace 依赖(git rev 等)变更后重新拷贝。

## 约定

- Python **3.12+**;Rust **2021** 边缘;import 顺序 **stdlib > 三方 > 本地**;简体中文 docstring/注释,
  匹配家族风格。
- **TDD**——测试即规格(Rust:`crates/*/tests/*.rs` 用 `zip`/合成词框现造 fixture;Python:
  `python/tests/conftest.py` 用纯 `zipfile` 合成含**重型表格 + 内嵌图片**的最小 .docx,不落二进制 fixture)。
- **最小改动**——只改需求要求的部分。
- **深层、按职责分组**的布局:crate / 文件路径先定位职责,再读文件名。
- 每个 crate `#![forbid(unsafe_code)]`,**唯独** `py-bindings` 用 `#![deny(unsafe_op_in_unsafe_fn)]`
  (PyO3 需要 unsafe FFI glue)。
