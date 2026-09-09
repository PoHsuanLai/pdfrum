#![doc = include_str!("../README.md")]
// The two stages are a pipeline of pure functions:
//
//     bytes ──parse_content──▶ Vec<Op> ──build_page──▶ Page { objects, boxes }
//
// `parse_content` tokenizes, fills a sixteen-slot operand ring, and emits one
// `Op` per recognised operator. `build_page` is the fold that turns those
// operators into page objects, and it is where resources, the graphics-state
// stack and form recursion live. Separating them means a content stream can be
// inspected, diffed and fuzzed without a document, and a page can be built from
// synthesized operators without bytes.
//
// Damage is data, not failure: the best-effort results match what PDFium
// produces, because that behaviour is what makes broken PDFs render.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
// Every byte reaching this crate came from an untrusted file: index with
// `get()` and do arithmetic with `checked_*`.
#![warn(clippy::indexing_slicing)]

mod build;
mod color;
mod content;
mod error;
mod function;
mod image;
mod inline_image;
mod mutate;
mod names;
mod ops;
mod optional;
mod page;
mod page_edit;
mod pattern;
// The whole-render stage timers. Public *with the default-off `profiling`
// feature and only then*: the module always exists, because the render calls
// its entry points unconditionally and they compile to empty inline functions
// with the feature off, but its reporting items are part of the instrument
// rather than of the crate a `cargo add pdfrum-page` reaches. Same argument
// and same shape as `pdfrum_render::walkprofile`: the committed API
// snapshots deliberately do not cover the `profiling` feature, because its
// items are not part of the published surface.
//
// The two crates above this one time their own stages, so they forward a
// `profiling` of their own and gate their call sites on it: a caller cannot
// `#[cfg]` on another crate's feature, and leaving this module unconditionally
// public so they need not is a surface with no reader in the default build.
#[cfg(feature = "profiling")]
pub mod renderprofile;
#[cfg(not(feature = "profiling"))]
mod renderprofile;
mod resources;
mod shading;
mod state;
mod tokenize;
mod transfer;
mod transparency;
mod type3;

pub use build::{
    BuildContext, FormFontsKey, FoundPattern, MAX_FORM_LEVEL, StreamBounds, build_form_object,
    build_form_object_with, build_page, build_page_from_dict, build_page_streams,
    eliminate_redundant_clips, load_pattern,
};
pub use color::{
    ColorSpace, ColorValue, Family, PatternSpace, PatternValue, Rgb, Separation,
    SetComponentsError, adobe_cmyk_to_srgb, cmyk_profile_bytes, load_colorspace,
    srgb_profile_bytes,
};
pub use content::parse_content;
pub use error::Error;
pub use function::{Function, FunctionCache, PostScript, parse_program};
#[cfg(feature = "jbig2")]
pub use image::decode_jbig2;
pub use image::{
    BitImage, Converted, Depth, ImageCache, ImageData, ImageMask, MAX_BYTES, MAX_IMAGE_PIXELS,
    Packed, Palette, Pixels, RequestedSize, Rgb8, Rgba8, Row, Rows, Samples, Source, Unpacked,
    decode_image, image_area_is_workable,
};
#[cfg(feature = "jpeg2000")]
pub use image::{JpxImage, decode_jpx};
pub use mutate::IndexOutOfRange;
pub use ops::{
    FillRule, InlineImage, LineCap, LineJoin, MarkProperties, Op, TextItem, TextRenderMode,
};
pub use optional::{OcContext, UsageType, Visibility, page_visibility};
pub use page::{
    Content, DEFAULT_MEDIA_BOX, FormObject, ImageObject, NotAQuarterTurn, Page, PageObject,
    PathObject, Rotation, ShadingObject, TextObject, TextSegment, derive_boxes,
    display_size_from_dict,
};
pub use page_edit::{PageEdit, transform_object};
pub use pattern::{Pattern, ShadingPattern, TileRange, TilingPattern, uncolored_pattern_rgb};
pub use resources::Resources;
pub use shading::{
    Axial, FunctionBased, Geometry, Mesh, MeshParams, MeshReader, Patch, Radial, Shading,
    ShadingKind, ShadingSource, Triangle, Vertex, coons_interior,
};
pub use state::{
    BlendMode, ClipEntry, ClipRule, ClipStack, ContentMarks, GeneralState, GraphicsState,
    MAX_TEXT_OBJECTS, Mark, StateStack, StrokeParams, TextClipLimit, TextClipRun, TextState,
    apply_ext_gstate,
};
pub use transfer::{CHANNEL_SAMPLES, TransferFunc};
pub use transparency::{SoftMask, SoftMaskKind, Transparency};
pub use type3::Type3Metrics;

#[cfg(test)]
mod send_sync {
    // Rendering pages in parallel with rayon must Just Work.
    const fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_types_are_send_and_sync() {
        assert_send_sync::<crate::Op>();
        assert_send_sync::<crate::ColorSpace>();
        assert_send_sync::<crate::Function>();
        assert_send_sync::<crate::Shading>();
        assert_send_sync::<crate::Pattern>();
        assert_send_sync::<crate::ImageData>();
        assert_send_sync::<crate::Error>();
    }
}
