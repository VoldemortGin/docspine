//! 类型化错误 [`DocError`] 与 crate 级 [`Result`] 别名。
//!
//! 解析层对脏输入必须健壮:任何失败都收敛成一个 `DocError` 变体,**绝不 panic**。
//! `kind()` 返回稳定的字符串标签,供 FFI 层(py-bindings)映射到 Python 异常层级。

/// docspine 的统一错误类型。
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum DocError {
    /// zip 容器层面的错误(打不开、坏条目、缺失部件)。
    #[error("zip error: {0}")]
    Zip(String),

    /// XML 部件解析错误(quick-xml 报错、结构非法)。
    #[error("xml error: {0}")]
    Xml(String),

    /// 命中了尚未实现 / 不支持的特性(如旧二进制 `.doc`)。
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// 调用方传入的参数非法(静态信息即可)。
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),

    /// 底层 IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 图片 OCR 失败(由 `doc-ocr` 把 `ocrspine::OcrError` 映射过来)。
    #[error("ocr error: {0}")]
    Ocr(String),

    /// PDF 渲染/序列化失败(由 `doc-render` 把 pdf-typeset 引擎错误映射过来)。
    #[error("render error: {0}")]
    Render(String),

    /// 输入触达了 zip 读取资源限额(zip 炸弹 / 超多条目 / 超长名等恶意或畸形输入的护栏)。
    /// `actual` 是触发时观测到的值(流式读取被截断时为下界 `cap + 1`)。
    #[error("limit exceeded: {kind} (limit {limit}, actual {actual})")]
    LimitExceeded {
        kind: LimitKind,
        limit: u64,
        actual: u64,
    },
}

/// [`DocError::LimitExceeded`] 命中的是哪一种限额。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitKind {
    /// zip 条目总数。
    Entries,
    /// 单个条目的(声明或实际)解压字节数。
    EntryBytes,
    /// 全包累计实际解压字节数。
    TotalBytes,
    /// 单个条目的压缩比(未压缩 / 压缩;声明值与实际读出值都检查)。
    CompressionRatio,
    /// 条目名字节长度。
    NameLength,
}

impl LimitKind {
    /// 稳定的字符串标签(出现在错误信息里)。
    pub fn as_str(self) -> &'static str {
        match self {
            LimitKind::Entries => "entries",
            LimitKind::EntryBytes => "entry-bytes",
            LimitKind::TotalBytes => "total-bytes",
            LimitKind::CompressionRatio => "compression-ratio",
            LimitKind::NameLength => "name-length",
        }
    }
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl DocError {
    /// 稳定的错误类别标签,供 FFI 层映射到具体 Python 异常。
    pub fn kind(&self) -> &'static str {
        match self {
            DocError::Zip(_) => "zip",
            DocError::Xml(_) => "xml",
            DocError::Unsupported(_) => "unsupported",
            DocError::InvalidArgument(_) => "invalid-argument",
            DocError::Io(_) => "io",
            DocError::Ocr(_) => "ocr",
            DocError::Render(_) => "render",
            DocError::LimitExceeded { .. } => "limit-exceeded",
        }
    }
}

/// crate 级 `Result` 别名。
pub type Result<T> = std::result::Result<T, DocError>;
