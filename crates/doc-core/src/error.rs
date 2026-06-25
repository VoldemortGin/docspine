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
        }
    }
}

/// crate 级 `Result` 别名。
pub type Result<T> = std::result::Result<T, DocError>;
