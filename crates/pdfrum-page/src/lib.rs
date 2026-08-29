//! Page content semantics (ISO 32000 §8): content-stream operators parsed to
//! a typed `Op` list, the interpreter folding ops into a typed page-object
//! graph, graphics state, colorspaces (device/ICC/Indexed/Separation/Lab),
//! PDF functions (types 0/2/3/4), patterns and shadings (types 1–7), and the
//! transparency model — groups, soft masks, blend modes (SPEC.md §7).
//!
//! # The two stages
//!
//! Content interpretation is a pipeline of two pure functions, which is the
//! whole design:
//!
//! ```text
//! bytes ──parse_content──▶ Vec<Op> ──build_page──▶ Page { objects, boxes }
//! ```
//!
//! [`parse_content`] is infallible and needs nothing but bytes: it tokenizes,
//! fills a sixteen-slot operand ring, and emits one [`Op`] per recognised
//! operator. [`build_page`] is the fold that turns those operators into page
//! objects, and it is where resources, the graphics-state stack and form
//! recursion live. Separating them means a content stream can be inspected,
//! diffed and fuzzed without a document, and a page can be built from
//! synthesized operators without bytes.
//!
//! ```
//! use pdfrum_common::{Diagnostics, Limits};
//! use pdfrum_page::{Op, parse_content};
//!
//! let mut diags = Diagnostics::default();
//! let ops = parse_content(b"0 0 100 50 re f", &Limits::default(), &mut diags);
//! assert_eq!(ops, vec![Op::Rectangle(0.0, 0.0, 100.0, 50.0), Op::Fill()]);
//! ```
//!
//! # Damage is data, not failure
//!
//! Nothing here refuses a file. An unknown operator, a colorspace that will
//! not load, a function whose `/Domain` is missing, an image whose bit depth
//! is nonsense — each records a
//! [`Diagnostic`](pdfrum_common::Diagnostic) and yields the same best-effort
//! result PDFium produces, because that behaviour is what makes broken PDFs
//! render. The recoveries are inventoried in `docs/design/pdfrum-page.md`.

#![forbid(unsafe_code)]
// Every byte reaching this crate came from an untrusted file: index with
// `get()` and do arithmetic with `checked_*`.
#![warn(clippy::indexing_slicing)]

mod build;
pub mod color;
mod content;
mod error;
pub mod function;
pub mod image;
pub mod inline_image;
pub mod mutate;
mod ops;
pub mod optional;
mod page;
pub mod pattern;
mod resources;
pub mod shading;
pub mod state;
mod tokenize;
pub mod transfer;
pub mod transparency;
pub mod type3;

pub use build::{
    BuildContext, FoundPattern, MAX_FORM_LEVEL, StreamBounds, build_form_object, build_page,
    build_page_from_dict, build_page_streams, eliminate_redundant_clips, load_pattern,
};
pub use color::{ColorSpace, ColorValue, PatternValue, Rgb};
pub use content::parse_content;
pub use error::Error;
pub use function::Function;
pub use image::{
    BitImage, ImageCache, ImageData, ImageMask, JpxImage, Pixels, RequestedSize, decode_image,
    decode_jbig2, decode_jpx,
};
pub use mutate::NO_CONTENT_STREAM;
pub use ops::{
    FillRule, InlineImage, LineCap, LineJoin, MarkProperties, Op, TextItem, TextRenderMode,
};
pub use optional::{OcContext, UsageType, Visibility, page_visibility};
pub use page::{
    Content, DEFAULT_MEDIA_BOX, FormObject, ImageObject, Page, PageObject, PathObject, Rotation,
    ShadingObject, TextObject, TextSegment, derive_boxes, is_valid_page_dict,
};
pub use pattern::{Pattern, ShadingPattern, TilingPattern};
pub use resources::Resources;
pub use shading::{Mesh, Shading, ShadingKind};
pub use state::{
    BlendMode, ClipStack, GraphicsState, StateStack, StrokeParams, TextState, apply_ext_gstate,
};
pub use transfer::TransferFunc;
pub use transparency::{SoftMask, SoftMaskKind, Transparency};
pub use type3::Type3Metrics;

/// The `/ExtGState`, colorspace, shading, pattern and image dictionary keys
/// this crate reads.
///
/// Declared here rather than in `pdfrum-object`'s shared table because they
/// are page-content-specific and no other crate spells them.
pub(crate) mod names {
    pub(crate) use pdfrum_object::names::{
        BITS_PER_COMPONENT, CROP_BOX, DECODE_PARMS, FILTER, MEDIA_BOX, OC, RESOURCES, ROTATE,
        SUBTYPE, TR, TYPE,
    };
    pdfrum_object::names! {
        /// An image's or shading's colour space (`/ColorSpace`).
        COLOR_SPACE = "ColorSpace";
        /// The colorspace resource category, also a shading's `/CS`.
        CS = "CS";
        /// Image width in samples (`/Width`).
        WIDTH = "Width";
        /// Image height in samples (`/Height`).
        HEIGHT = "Height";
        /// Sample-value remapping (`/Decode`).
        DECODE = "Decode";
        /// Whether the image is a stencil mask (`/ImageMask`).
        IMAGE_MASK = "ImageMask";
        /// Whether to smooth the image when scaling (`/Interpolate`).
        INTERPOLATE = "Interpolate";
        /// A stencil or colour-key mask (`/Mask`).
        MASK = "Mask";
        /// A soft mask, either an image's or an `/ExtGState`'s (`/SMask`).
        SMASK = "SMask";
        /// The pre-blended background colour behind a soft-masked image
        /// (`/Matte`).
        MATTE = "Matte";
        /// How a JPEG 2000 codestream carries its alpha (`/SMaskInData`).
        SMASK_IN_DATA = "SMaskInData";
        /// A JBIG2 stream's shared segment dictionary (`/JBIG2Globals`).
        JBIG2_GLOBALS = "JBIG2Globals";
        /// Whether a DCT stream's components were transformed
        /// (`/ColorTransform`).
        COLOR_TRANSFORM = "ColorTransform";

        /// The named-resource categories (`/XObject`, `/Font`, …).
        XOBJECT = "XObject";
        /// The font resource category (`/Font`).
        FONT = "Font";
        /// The graphics-state parameter resource category (`/ExtGState`).
        EXT_G_STATE = "ExtGState";
        /// The pattern resource category (`/Pattern`).
        PATTERN = "Pattern";
        /// The shading resource category (`/Shading`).
        SHADING = "Shading";
        /// The marked-content property resource category (`/Properties`).
        PROPERTIES = "Properties";

        /// A form XObject's coordinate mapping, also a pattern's or a
        /// shading's (`/Matrix`).
        MATRIX = "Matrix";
        /// A form XObject's or pattern's clipping rectangle (`/BBox`).
        BBOX = "BBox";
        /// A form XObject's transparency group attributes (`/Group`).
        GROUP = "Group";
        /// A group's or soft mask's subtype (`/S`).
        S = "S";
        /// Whether a transparency group is isolated (`/I`).
        I = "I";
        /// Whether a transparency group is a knockout group (`/K`).
        K = "K";
        /// A soft mask's group XObject (`/G`).
        G = "G";
        /// A soft mask's backdrop colour (`/BC`).
        BC = "BC";
        /// The default colorspace substitutes (`/DefaultGray` and friends).
        DEFAULT_GRAY = "DefaultGray";
        /// The `/DefaultRGB` colorspace substitute.
        DEFAULT_RGB = "DefaultRGB";
        /// The `/DefaultCMYK` colorspace substitute.
        DEFAULT_CMYK = "DefaultCMYK";

        /// A `CalGray`/`CalRGB`/`Lab` diffuse white point (`/WhitePoint`).
        WHITE_POINT = "WhitePoint";
        /// A `CalGray`/`CalRGB`/`Lab` black point (`/BlackPoint`).
        BLACK_POINT = "BlackPoint";
        /// A `CalGray` or `CalRGB` gamma (`/Gamma`).
        GAMMA = "Gamma";
        /// A `Lab` space's component ranges (`/Range`).
        RANGE = "Range";
        /// An `ICCBased` stream's component count (`/N`).
        N = "N";
        /// An `ICCBased` stream's fallback space (`/Alternate`).
        ALTERNATE = "Alternate";
        /// A `DeviceN` space's colorant names (`/Names`).
        NAMES = "Names";
        /// A `DeviceN` space's attributes dictionary (`/Attributes`).
        ATTRIBUTES = "Attributes";

        /// A function's input intervals (`/Domain`).
        DOMAIN = "Domain";
        /// Which function flavour a dictionary describes (`/FunctionType`).
        FUNCTION_TYPE = "FunctionType";
        /// A sampled function's sample-grid extents (`/Size`).
        SIZE = "Size";
        /// A sampled function's sample width (`/BitsPerSample`).
        BITS_PER_SAMPLE = "BitsPerSample";
        /// A sampled function's input mapping (`/Encode`).
        ENCODE = "Encode";
        /// An exponential function's interpolation exponent (`/N` again).
        /// A type 2 function's value at the domain's low end (`/C0`).
        C0 = "C0";
        /// A type 2 function's value at the domain's high end (`/C1`).
        C1 = "C1";
        /// A stitching function's sub-functions (`/Functions`).
        FUNCTIONS = "Functions";
        /// A stitching function's sub-domain boundaries (`/Bounds`).
        BOUNDS = "Bounds";
        /// A shading's or pattern's tint transform (`/Function`).
        FUNCTION = "Function";

        /// Which shading geometry a dictionary describes (`/ShadingType`).
        SHADING_TYPE = "ShadingType";
        /// A shading's defining geometry (`/Coords`).
        COORDS = "Coords";
        /// Whether an axial or radial shading paints past its ends
        /// (`/Extend`).
        EXTEND = "Extend";
        /// A shading's colour outside its geometry (`/Background`).
        BACKGROUND = "Background";
        /// Whether to anti-alias a shading (`/AntiAlias`).
        ANTI_ALIAS = "AntiAlias";
        /// A mesh shading's coordinate sample width
        /// (`/BitsPerCoordinate`).
        BITS_PER_COORDINATE = "BitsPerCoordinate";
        /// A mesh shading's edge-flag width (`/BitsPerFlag`).
        BITS_PER_FLAG = "BitsPerFlag";
        /// A lattice mesh's row length (`/VerticesPerRow`).
        VERTICES_PER_ROW = "VerticesPerRow";

        /// Which pattern flavour a dictionary describes (`/PatternType`).
        PATTERN_TYPE = "PatternType";
        /// Whether a tiling pattern carries its own colour (`/PaintType`).
        PAINT_TYPE = "PaintType";
        /// A tiling pattern's tile spacing (`/XStep`).
        X_STEP = "XStep";
        /// A tiling pattern's tile spacing (`/YStep`).
        Y_STEP = "YStep";
        /// How a tiling pattern's cells overlap (`/TilingType`).
        TILING_TYPE = "TilingType";

        /// A form XObject's `/Subtype` value (`/Form`).
        FORM = "Form";
        /// An image XObject's `/Subtype` value (`/Image`).
        IMAGE = "Image";
        /// A `/Group`'s `/S` value for a transparency group
        /// (`/Transparency`).
        TRANSPARENCY_GROUP = "Transparency";
        /// A soft mask's alpha-source `/S` value (`/Alpha`).
        ALPHA = "Alpha";
        /// A soft mask's luminosity-source `/S` value (`/Luminosity`).
        LUMINOSITY_MASK = "Luminosity";

        /// An `/ExtGState` line width (`/LW`).
        LW = "LW";
        /// An `/ExtGState` line cap (`/LC`).
        LC = "LC";
        /// An `/ExtGState` line join (`/LJ`).
        LJ = "LJ";
        /// An `/ExtGState` miter limit (`/ML`).
        ML = "ML";
        /// An `/ExtGState` dash pattern (`/D`).
        D = "D";
        /// An `/ExtGState` blend mode (`/BM`).
        BM = "BM";
        /// An `/ExtGState` stroking alpha (`/CA`), lower-case for
        /// non-stroking.
        CA_LOWER = "ca";
        /// An `/ExtGState` transfer function replacement (`/TR2`).
        TR2 = "TR2";
        /// An `/ExtGState` black generation (`/BG`).
        BG = "BG";
        /// An `/ExtGState` black generation replacement (`/BG2`).
        BG2 = "BG2";
        /// An `/ExtGState` undercolour removal (`/UCR`).
        UCR = "UCR";
        /// An `/ExtGState` undercolour removal replacement (`/UCR2`).
        UCR2 = "UCR2";
        /// An `/ExtGState` halftone (`/HT`).
        HT = "HT";
        /// An `/ExtGState` flatness tolerance (`/FL`).
        FL_FLATNESS = "FL";
        /// An `/ExtGState` smoothness tolerance (`/SM`).
        SM = "SM";
        /// An `/ExtGState` automatic stroke adjustment (`/SA`).
        SA = "SA";
        /// An `/ExtGState` alpha-is-shape flag (`/AIS`).
        AIS = "AIS";
        /// An `/ExtGState` text knockout flag (`/TK`).
        TK = "TK";
        /// An `/ExtGState` overprint flag (`/OP`), lower-case for
        /// non-stroking.
        OP_UPPER = "OP";
        /// An `/ExtGState` non-stroking overprint flag (`/op`).
        OP_LOWER = "op";
        /// An `/ExtGState` overprint mode (`/OPM`).
        OPM = "OPM";

        /// An optional-content membership's visibility expression (`/VE`).
        VE = "VE";
        /// An optional-content membership's policy (`/P`).
        P = "P";
        /// The optional-content groups a membership or configuration names
        /// (`/OCGs`).
        OCGS = "OCGs";
        /// A group's per-usage settings (`/Usage`).
        USAGE = "Usage";
        /// A configuration's starting state for every group (`/BaseState`).
        BASE_STATE = "BaseState";
        /// A configuration's explicitly-on groups (`/ON`).
        ON = "ON";
        /// A configuration's explicitly-off groups (`/OFF`).
        OFF = "OFF";
        /// A configuration's automatic-state entries (`/AS`).
        AS = "AS";
        /// An automatic-state entry's trigger (`/Event`).
        EVENT = "Event";
        /// The optional-content configurations a catalog offers
        /// (`/Configs`).
        CONFIGS = "Configs";
        /// The default optional-content configuration (`/D`), also an
        /// `/ExtGState` dash pattern.
        D_CONFIG = "D";
        /// What a group or configuration is intended for (`/Intent`).
        INTENT = "Intent";
    }
}

#[cfg(test)]
mod send_sync {
    // Rendering pages in parallel with rayon must Just Work (STYLE.md §4).
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
