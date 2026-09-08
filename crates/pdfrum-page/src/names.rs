//! The `/ExtGState`, colorspace, shading, pattern and image dictionary keys
//! this crate reads.
//!
//! Keys another crate also reads or writes live in `pdfrum-object`'s shared
//! table and are re-exported here so page code still writes `names::FONT`.

#![allow(unused_imports)] // the re-export is the crate's one names table

pub(crate) use pdfrum_object::names::D as D_CONFIG;
pub(crate) use pdfrum_object::names::{
    AS, BBOX, BC, BITS_PER_COMPONENT, COLOR_SPACE, CROP_BOX, CS, DECODE, DECODE_PARMS, EXT_G_STATE,
    FILTER, FONT, G, HEIGHT, I, IMAGE_MASK, K, MATRIX, MEDIA_BOX, N, OC, P, PROPERTIES, RESOURCES,
    ROTATE, S, SHADING, SIZE, SMASK, SUBTYPE, TR, TYPE, WIDTH, XOBJECT,
};

pdfrum_object::names! {
    /// Whether to smooth the image when scaling (`/Interpolate`).
    INTERPOLATE = "Interpolate";
    /// A stencil or colour-key mask (`/Mask`).
    MASK = "Mask";
    /// The pre-blended background colour behind a soft-masked image
    /// (`/Matte`).
    MATTE = "Matte";
    /// How a JPEG 2000 codestream carries its alpha (`/SMaskInData`).
    SMASK_IN_DATA = "SMaskInData";
    /// A JBIG2 stream's shared segment dictionary (`/JBIG2Globals`).
    JBIG2_GLOBALS = "JBIG2Globals";
    /// The pattern resource category (`/Pattern`).
    PATTERN = "Pattern";
    /// A form XObject's transparency group attributes (`/Group`).
    GROUP = "Group";
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
    /// An `ICCBased` stream's fallback space (`/Alternate`).
    ALTERNATE = "Alternate";
    /// A function's input intervals (`/Domain`).
    DOMAIN = "Domain";
    /// Which function flavour a dictionary describes (`/FunctionType`).
    FUNCTION_TYPE = "FunctionType";
    /// A sampled function's sample width (`/BitsPerSample`).
    BITS_PER_SAMPLE = "BitsPerSample";
    /// A sampled function's input mapping (`/Encode`).
    ENCODE = "Encode";
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
    /// An `/ExtGState` transfer function replacement (`/TR2`).
    TR2 = "TR2";
    /// An `/ExtGState` black generation replacement (`/BG2`).
    BG2 = "BG2";
    /// An `/ExtGState` undercolour removal replacement (`/UCR2`).
    UCR2 = "UCR2";
    /// An `/ExtGState` non-stroking overprint flag (`/op`).
    OP_LOWER = "op";
    /// An optional-content membership's visibility expression (`/VE`).
    VE = "VE";
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
    /// An automatic-state entry's trigger (`/Event`).
    EVENT = "Event";
    /// The optional-content configurations a catalog offers
    /// (`/Configs`).
    CONFIGS = "Configs";
    /// What a group or configuration is intended for (`/Intent`).
    INTENT = "Intent";
}
