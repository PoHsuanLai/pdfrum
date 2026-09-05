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
/// [`Diagnostic`](pdfrum_common::Diagnostic)s on the value that came back.
/// `Err` means the operation could not produce an answer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The password does not open the document.
    ///
    /// The one variant worth matching on, because it is the one that means
    /// *ask the user again* rather than *give up*. It is here, rather than
    /// inside [`Error::Open`]'s payload, so that a caller who has never
    /// depended on `pdfrum-parser` can write the match; everything else an
    /// open can go wrong with is [`Error::Open`].
    ///
    /// See [`Document::open_with_password`](crate::Document::open_with_password)
    /// for the worked example.
    #[error("wrong password")]
    WrongPassword,

    /// The file could not be opened: it is not a PDF, its encryption is one
    /// this reader does not implement, or it is damaged past what recovery
    /// could repair.
    ///
    /// A wrong password is [`Error::WrongPassword`] and never arrives here,
    /// even though [`OpenError`](crate::OpenError) — this variant's payload —
    /// still has a variant for it: the conversion routes that one case out.
    #[error("cannot open document: {0}")]
    Open(#[source] pdfrum_parser::LoadError),

    /// The document opened, but something asked of it afterwards failed —
    /// a page index with no page behind it, an object the store cannot
    /// produce, a reference loop.
    #[error("cannot read document: {0}")]
    Read(#[source] pdfrum_parser::Error),

    /// A page would not render. In practice this is a target size that is
    /// zero or larger than the rasterizer's limit; content that will not draw
    /// is a diagnostic, not an error, because the page still has an image.
    #[error("cannot render page: {0}")]
    Render(#[source] pdfrum_render::Error),

    /// Reading a document-level feature failed — an outline, an annotation,
    /// a form field, the structure tree.
    #[error("cannot read document feature: {0}")]
    Doc(#[from] pdfrum_doc::Error),

    /// Writing the document out failed.
    #[error("cannot save document: {0}")]
    #[cfg(feature = "edit")]
    Save(#[from] pdfrum_edit::Error),

    /// Text extraction refused an index.
    #[error("cannot extract text: {0}")]
    Text(#[from] pdfrum_text::Error),

    /// A ceiling the caller set in [`Limits`](crate::Limits) was exceeded:
    /// a render whose target has more pixels than
    /// [`Limits::max_render_pixels`](crate::Limits::max_render_pixels)
    /// allows, or a [`Limits::deadline`](crate::Limits::deadline) that passed
    /// while the document was being opened, a page loaded, or a page
    /// rendered. (Text extraction cannot fail; past the deadline it returns
    /// an empty page and records
    /// [`DiagKind::TimeLimitReached`](crate::DiagKind::TimeLimitReached).)
    ///
    /// The one variant besides [`Error::WrongPassword`] a caller acts on
    /// rather than reports: the payload says which cap and by how much, and
    /// its message says what would satisfy it. Never produced by a default
    /// `Limits`, whose ceilings are all off. The member crates' errors carry
    /// a `Limit` variant of their own for the same value; the conversions
    /// below route every one of them here, so a caller matches one place.
    #[error(transparent)]
    Limit(#[from] pdfrum_common::LimitExceeded),

    /// The filesystem refused a read or a write. Only the path-taking
    /// convenience methods ([`Document::open`](crate::Document::open),
    /// [`Document::save`](crate::Document::save)) can produce this; the
    /// bytes- and writer-taking ones cannot.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// This crate's result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Wrong passwords become [`Error::WrongPassword`]; everything else an open
/// can fail with becomes [`Error::Open`].
///
/// Hand-written rather than `#[error(transparent)]`/`#[from]` because that is
/// the whole content of the conversion: `?` on a `load` is what lifts the
/// variant, so every path that opens a document reports the same way and not
/// only [`Document::open_with_password`](crate::Document::open_with_password).
impl From<pdfrum_parser::LoadError> for Error {
    fn from(e: pdfrum_parser::LoadError) -> Self {
        match e {
            pdfrum_parser::LoadError::WrongPassword => Error::WrongPassword,
            pdfrum_parser::LoadError::Limit(limit) => Error::Limit(limit),
            other => Error::Open(other),
        }
    }
}

/// A ceiling hit while reading becomes [`Error::Limit`]; everything else
/// becomes [`Error::Read`].
impl From<pdfrum_parser::Error> for Error {
    fn from(e: pdfrum_parser::Error) -> Self {
        match e {
            pdfrum_parser::Error::Limit(limit) => Error::Limit(limit),
            other => Error::Read(other),
        }
    }
}

/// A ceiling hit while rendering becomes [`Error::Limit`]; everything else
/// becomes [`Error::Render`].
impl From<pdfrum_render::Error> for Error {
    fn from(e: pdfrum_render::Error) -> Self {
        match e {
            pdfrum_render::Error::Limit(limit) => Error::Limit(limit),
            other => Error::Render(other),
        }
    }
}
