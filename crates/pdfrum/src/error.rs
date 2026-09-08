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

    /// An SVG would not resolve — malformed XML, or an `<svg>` with no
    /// usable size. Only [`Canvas::draw_svg`](crate::Canvas::draw_svg)
    /// produces it, behind the `svg-import` feature.
    ///
    /// An SVG construct that resolves but has no PDF spelling is **not** an
    /// error: it is an
    /// [`Unsupported`](crate::Unsupported) item on the returned
    /// [`SvgIngestReport`](crate::SvgIngestReport), the same way damage a
    /// document survives is a diagnostic rather than an error.
    #[error("cannot read svg: {0}")]
    #[cfg(feature = "svg-import")]
    Svg(#[source] crate::SvgError),

    /// The filesystem refused a read or a write. Only the path-taking
    /// convenience methods ([`Document::open`](crate::Document::open),
    /// [`Document::save`](crate::Document::save)) can produce this; the
    /// bytes- and writer-taking ones cannot.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// This crate's result type.
pub type Result<T> = std::result::Result<T, Error>;

/// A stable number for each kind of [`Error`], for a caller who cannot
/// match on the enum: a C header, a JSON body, a log line.
///
/// `#[repr(u32)]`, and **every number is fixed for good** — a variant added
/// later takes a new number, and none of these is ever renumbered or
/// reused, which is what lets a `pdfrum.h` carry them as constants. The
/// gaps are deliberate room: `Save` is 7 whether or not the `edit` feature
/// that produces it is on. [`Display`](std::fmt::Display) on [`Error`] is
/// unchanged; the number is beside the message, not instead of it.
///
/// ```
/// use pdfrum::{Document, Error, ErrorCode};
///
/// let err = Document::open_with_password("tests/fixtures/encrypted.pdf", b"nope")
///     .expect_err("the wrong password");
/// assert_eq!(err.code(), ErrorCode::WrongPassword);
/// assert_eq!(u32::from(err.code()), 3);
/// assert_eq!(err.to_string(), "wrong password");
/// # Ok::<(), Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
#[non_exhaustive]
pub enum ErrorCode {
    /// [`Error::Io`]: the filesystem refused a read or a write.
    Io = 1,
    /// [`Error::Open`]: the file could not be opened.
    Open = 2,
    /// [`Error::WrongPassword`]: the password does not open the document.
    WrongPassword = 3,
    /// [`Error::Read`]: the document opened, but a read of it failed.
    Read = 4,
    /// [`Error::Render`]: a page would not render.
    Render = 5,
    /// [`Error::Doc`]: a document-level feature would not read.
    Doc = 6,
    /// [`Error::Save`]: writing the document out failed. Only produced with
    /// the `edit` feature; the number is reserved either way.
    Save = 7,
    /// [`Error::Text`]: text extraction refused an index.
    Text = 8,
    /// [`Error::Limit`]: a ceiling the caller set was exceeded.
    Limit = 9,
    /// `Error::Svg`: an SVG would not resolve. Only produced with the
    /// `svg-import` feature; the number is reserved either way.
    Svg = 10,
}

/// The number, for a header or a wire format.
///
/// ```
/// assert_eq!(u32::from(pdfrum::ErrorCode::Limit), 9);
/// ```
impl From<ErrorCode> for u32 {
    fn from(code: ErrorCode) -> u32 {
        code as u32
    }
}

/// The error [`ErrorCode`]'s [`TryFrom<u32>`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no error code numbered {0}")]
pub struct UnknownErrorCode(pub u32);

impl ErrorCode {
    /// The domain word the code stands for, as [`Display`](std::fmt::Display)
    /// writes it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::Io => "io",
            ErrorCode::Open => "open",
            ErrorCode::WrongPassword => "wrong-password",
            ErrorCode::Read => "read",
            ErrorCode::Render => "render",
            ErrorCode::Doc => "doc",
            ErrorCode::Save => "save",
            ErrorCode::Text => "text",
            ErrorCode::Limit => "limit",
            ErrorCode::Svg => "svg",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    /// The domain word, not the number and not the failing error's own
    /// message — [`Error`]'s [`Display`](std::fmt::Display) says what went
    /// wrong, this says which family it belongs to.
    ///
    /// ```
    /// assert_eq!(pdfrum::ErrorCode::WrongPassword.to_string(), "wrong-password");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The number back, for a header or a wire format that carried one.
///
/// Fallible, and so [`TryFrom`] rather than [`From`]: the enum is
/// `#[non_exhaustive]` and most `u32`s name no code.
///
/// ```
/// use pdfrum::ErrorCode;
///
/// assert_eq!(ErrorCode::try_from(9), Ok(ErrorCode::Limit));
/// assert!(ErrorCode::try_from(0).is_err());
/// ```
impl TryFrom<u32> for ErrorCode {
    type Error = UnknownErrorCode;

    fn try_from(n: u32) -> core::result::Result<ErrorCode, UnknownErrorCode> {
        match n {
            1 => Ok(ErrorCode::Io),
            2 => Ok(ErrorCode::Open),
            3 => Ok(ErrorCode::WrongPassword),
            4 => Ok(ErrorCode::Read),
            5 => Ok(ErrorCode::Render),
            6 => Ok(ErrorCode::Doc),
            7 => Ok(ErrorCode::Save),
            8 => Ok(ErrorCode::Text),
            9 => Ok(ErrorCode::Limit),
            10 => Ok(ErrorCode::Svg),
            other => Err(UnknownErrorCode(other)),
        }
    }
}

impl Error {
    /// The error's stable code — see [`ErrorCode`].
    ///
    /// ```
    /// use pdfrum::{Document, Error, ErrorCode};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let err = doc.page(7).expect_err("there is one page");
    /// assert_eq!(err.code(), ErrorCode::Read);
    /// # Ok::<(), Error>(())
    /// ```
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Error::WrongPassword => ErrorCode::WrongPassword,
            Error::Open(_) => ErrorCode::Open,
            Error::Read(_) => ErrorCode::Read,
            Error::Render(_) => ErrorCode::Render,
            Error::Doc(_) => ErrorCode::Doc,
            #[cfg(feature = "edit")]
            Error::Save(_) => ErrorCode::Save,
            Error::Text(_) => ErrorCode::Text,
            Error::Limit(_) => ErrorCode::Limit,
            #[cfg(feature = "svg-import")]
            Error::Svg(_) => ErrorCode::Svg,
            Error::Io(_) => ErrorCode::Io,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant maps, and the numbers are the ones the header will
    /// carry: change a row here and the header is a different header.
    #[test]
    fn every_error_has_the_code_the_table_says() {
        let table: Vec<(Error, ErrorCode, u32)> = vec![
            (Error::Io(std::io::Error::other("io")), ErrorCode::Io, 1),
            (
                Error::Open(pdfrum_parser::LoadError::NotPdf),
                ErrorCode::Open,
                2,
            ),
            (Error::WrongPassword, ErrorCode::WrongPassword, 3),
            (
                Error::Read(pdfrum_parser::Error::Unresolved(
                    pdfrum_object::ObjRef::new(1, 0),
                )),
                ErrorCode::Read,
                4,
            ),
            (
                Error::Render(pdfrum_render::Error::TargetEmpty {
                    width: 0,
                    height: 0,
                }),
                ErrorCode::Render,
                5,
            ),
            (Error::Doc(pdfrum_doc::Error::NoCatalog), ErrorCode::Doc, 6),
            #[cfg(feature = "edit")]
            (
                Error::Save(pdfrum_edit::Error::BadPageRange),
                ErrorCode::Save,
                7,
            ),
            (
                Error::Text(pdfrum_text::Error::CharIndexOutOfRange {
                    index: pdfrum_text::CharIndex::from(3),
                    len: 1,
                }),
                ErrorCode::Text,
                8,
            ),
            (
                Error::Limit(pdfrum_common::LimitExceeded::Stopped {
                    during: pdfrum_common::Operation::PageLoad,
                    page: None,
                }),
                ErrorCode::Limit,
                9,
            ),
        ];
        for (error, code, number) in table {
            assert_eq!(error.code(), code, "{error}");
            assert_eq!(u32::from(code), number, "{error}");
        }
        // The reserved numbers are reserved with their features off too.
        assert_eq!(u32::from(ErrorCode::Save), 7);
        assert_eq!(u32::from(ErrorCode::Svg), 10);
    }
}
