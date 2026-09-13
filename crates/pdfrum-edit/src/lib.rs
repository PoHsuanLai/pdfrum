#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
// Everything here is written *from* untrusted input: index with `get()`.
#![warn(clippy::indexing_slicing)]

// Every module is private and the `pub use` block below is the whole surface,
// so an item is reachable exactly one way and reading that block is reading
// the API. Siblings still reach
// across — `write` names `encrypt`, `content::marks` names `write::object` —
// which is what the `pub(crate)` on a few *sub*modules is for; at this level
// `mod` already means crate-visible.
mod annot;
mod attach;
mod build_graph;
mod builders;
mod canvas;
mod content;
mod doc;
mod encrypt;
mod error;
mod flatten;
mod font;
mod image;
mod import;
mod info;
mod names;
mod pages;
mod pdfa_convert;
mod stamp;
#[cfg(feature = "svg-import")]
mod svg_ingest;
#[cfg(feature = "svg-text")]
mod svg_text;
mod write;

pub use annot::{
    AnnotBorder, AnnotBorderStyle, AnnotMeta, AnnotSpec, AnnotWrite, DEFAULT_DA, Quad,
    add_annotation,
};
pub use attach::{
    AttachmentOptions, AttachmentOptionsBuilder, add_attachment, delete_attachment,
    remove_attachment, set_attachment_description, set_attachment_file, set_attachment_param,
};
pub use build_graph::build_graph;
pub use builders::{ImageBuilder, PathBuilder, TextBuilder};
#[cfg(feature = "svg-import")]
pub use canvas::SvgForm;
pub use canvas::{Canvas, Dash, Fill, LineCap, LineJoin, MiterLimit, Paint, Stroke};
pub use content::{
    ContentsShape, PageRewrite, Regenerated, ResourceTable, ShareCounts, apply_rewrite, regenerate,
    shared_objects, write_float, write_matrix, write_point, write_rect,
};
pub use doc::EditDoc;
pub use encrypt::{Encryptor, IvSource};
pub use error::Error;
pub use flatten::{FlattenMode, Flattened, UnknownFlattenMode, flatten};
pub use font::embed::{EmbeddedFont, FontEncoding, MissingGlyph, string_width};
pub use font::{GidMap, Subsetted, subset};
pub use image::{EmbeddedImage, PixelFormat};
pub use import::{ImportOptions, NUpOptions, PageRange, import_pages, n_page_to_one};
pub use info::{pdf_date, set_info_entry};
pub use pages::{PageBox, add_blank_page, delete_pages, set_page_box, set_page_rotation};
pub use pdfa_convert::{convert as to_pdfa, pdf_date_to_iso8601};
pub use pdfrum_font::StandardFont;
pub use stamp::{StampOptions, StampOptionsBuilder, StampPosition, UnknownStampPosition};
#[cfg(feature = "svg-import")]
pub use svg_ingest::{SvgFit, SvgIngestReport, Unsupported, UnsupportedItem};
#[cfg(feature = "svg-text")]
pub use svg_text::SvgFonts;
pub use write::id::{FileId, IdSource};
pub use write::{Encryption, SaveMode, SaveOptions, save};
