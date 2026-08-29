//! The one error type the facade hands out.

/// Anything that can go wrong through this crate's API.
///
/// The variants name **domains**, not the crates behind them: a caller who
/// has never heard of `pdfrum-parser` still knows what
/// [`Error::Open`] means. Each wraps the member crate's own error so the
/// detail survives — [`source`](std::error::Error::source) reaches it, and so
/// does a `match` on the payload.
///
/// Note what is *not* here. Damage a document survives is not an error at
/// all: a rebuilt cross-reference table, a page whose content stream ends
/// mid-operator, a font that had to be substituted are all recorded as
/// [`Diagnostic`](pdfrum_common::Diagnostic)s on the value that came back
/// (STYLE.md §3). `Err` means the operation could not produce an answer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file could not be opened: it is not a PDF, its password is wrong,
    /// its encryption is one this reader does not implement, or it is damaged
    /// past what recovery could repair.
    ///
    /// [`LoadError::WrongPassword`](pdfrum_parser::LoadError::WrongPassword)
    /// is the one worth matching on: it means *ask the user again* rather
    /// than *give up*, which is why the parser keeps it distinct.
    #[error("cannot open document: {0}")]
    Open(#[from] pdfrum_parser::LoadError),

    /// The document opened, but something asked of it afterwards failed —
    /// a page index with no page behind it, an object the store cannot
    /// produce, a reference loop.
    #[error("cannot read document: {0}")]
    Read(#[from] pdfrum_parser::Error),

    /// A page would not render. In practice this is a target size that is
    /// zero or larger than the rasterizer's limit; content that will not draw
    /// is a diagnostic, not an error, because the page still has an image.
    #[error("cannot render page: {0}")]
    Render(#[from] pdfrum_render::Error),

    /// Reading a document-level feature failed — an outline, an annotation,
    /// a form field, the structure tree.
    #[error("cannot read document feature: {0}")]
    Doc(#[from] pdfrum_doc::Error),

    /// Writing the document out failed.
    #[error("cannot save document: {0}")]
    Save(#[from] pdfrum_edit::Error),

    /// Text extraction refused an index.
    #[error("cannot extract text: {0}")]
    Text(#[from] pdfrum_text::Error),

    /// The filesystem refused a read or a write. Only the path-taking
    /// convenience methods ([`Document::open`](crate::Document::open),
    /// [`Document::save`](crate::Document::save)) can produce this; the
    /// bytes- and writer-taking ones cannot.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// This crate's result type.
pub type Result<T> = std::result::Result<T, Error>;
