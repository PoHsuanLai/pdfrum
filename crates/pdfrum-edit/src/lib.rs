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
mod annot_spec;
mod attach;
mod build_graph;
mod canvas;
mod content;
mod dests;
mod doc;
mod encrypt;
mod error;
mod flatten;
mod font;
mod form;
mod form_field;
mod image;
mod import;
mod info;
mod names;
mod outline;
mod page_label;
mod page_objects;
mod pages;
mod pdfa_convert;
mod prefs;
mod stamp;
#[cfg(feature = "svg-import")]
mod svg_ingest;
#[cfg(feature = "svg-text")]
mod svg_text;
mod write;

pub use annot::{
    AnnotBorder, AnnotBorderStyle, AnnotGoToView, AnnotLinkAction, AnnotLinkHighlight, AnnotMeta,
    AnnotRemoteDest, AnnotSpec, AnnotWrite, BorderStyleName, DEFAULT_DA, LineEndingStyle, Quad,
    add_annotation, delete_annotation, delete_annotation_at, delete_named_destination,
    ensure_named_destination, named_destinations, set_named_destination, update_annotation,
    update_annotation_at,
};
pub use annot_spec::{
    CaretSpec, CircleSpec, FreeTextSpec, InkSpec, LineSpec, LinkSpec, MarkupKind, MarkupSpec,
    SquareSpec, TextSpec,
};
pub use attach::{
    AttachmentOptions, AttachmentOptionsBuilder, add_attachment, delete_attachment,
    remove_attachment, set_attachment_description, set_attachment_file, set_attachment_file_with,
    set_attachment_name, set_attachment_param,
};
pub use build_graph::build_graph;
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
pub use flatten::{FlattenMode, Flattened, UnknownFlattenMode, flatten, flatten_document};
pub use font::embed::{EmbeddedFont, FontEncoding, MissingGlyph, string_width};
pub use font::{GidMap, Subsetted, subset};
pub use form::{WidgetAppearance, set_need_appearances, set_widget_appearance};
pub use form_field::{DEFAULT_FIELD_DA, FieldKindSpec, FieldSpec, add_form_field, add_form_font};
pub use image::{EmbeddedImage, PixelFormat};
pub use import::{
    ImportOptions, ImportOptionsBuilder, NUpOptions, NUpOptionsBuilder, PageRange, import_pages,
    n_page_to_one,
};
pub use info::{pdf_date, set_info_entry, set_info_name, set_xmp_metadata};
pub use outline::{BookmarkSpec, BookmarkTarget, set_outline};
pub use page_label::{PageLabelRange, PageLabelStyle, set_page_labels};
pub use page_objects::{ImageBuilder, PathBuilder, TextBuilder};
pub use pages::{
    PageBox, add_blank_page, delete_pages, reorder_pages, set_page_box, set_page_rotation,
    set_page_rotation_to,
};
pub use pdfa_convert::{convert as to_pdfa, pdf_date_to_iso8601};
// `NUpOptionsBuilder::sheet` takes a `Size`, so the type has to be reachable
// from this crate rather than only from whatever else the caller depends on.
pub use kurbo::Size;
pub use pdfrum_font::StandardFont;
pub use prefs::{
    Duplex, ViewerPreferences, clear_open_action, set_open_action, set_viewer_preferences,
};
pub use stamp::{StampOptions, StampOptionsBuilder, StampPosition, UnknownStampPosition};
#[cfg(feature = "svg-import")]
pub use svg_ingest::{SvgFit, SvgIngestReport, Unsupported, UnsupportedItem};
#[cfg(feature = "svg-text")]
pub use svg_text::SvgFonts;
pub use write::id::{FileId, IdSource};
pub use write::{Encryption, EncryptionBuilder, SaveMode, SaveOptions, SaveOptionsBuilder, save};
