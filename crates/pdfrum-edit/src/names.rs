//! The dictionary keys this crate writes, added to the shared table.
//!
//! `pdfrum-object`'s `names` module owns the keys several crates share; the
//! ones below are keys only a *writer* has occasion to spell.

#[expect(unused_imports, reason = "the shared keys are re-exported as one set")]
pub(crate) use pdfrum_object::names::{
    ART_BOX, BITS_PER_COMPONENT, BLEED_BOX, CONTENTS, COUNT, CROP_BOX, DCT_DECODE, DECODE_PARMS,
    ENCRYPT, FILTER, FIRST, FLATE_DECODE, ID, INDEX, INFO, JPX_DECODE, KIDS, LENGTH, MEDIA_BOX,
    METADATA, PAGE, PAGES, PARENT, PREV, RESOURCES, ROOT, ROTATE, SIZE, SUBTYPE, TRIM_BOX, TYPE,
    VIEWER_PREFERENCES, W, WIN_ANSI_ENCODING, XML, XREF, XREF_STM,
};

pdfrum_object::names! {
    // ---- Catalog and page tree, writer side (tables 28, 29) ----

    /// The document catalog's own `/Type` value (`/Catalog`).
    CATALOG = "Catalog";
    /// The producing application (`/Producer`), stamped on import.
    PRODUCER = "Producer";

    // ---- Resource categories the content generator maintains (table 33) ----

    /// Font resources (`/Font`).
    FONT = "Font";
    /// External object resources — images and forms alike (`/XObject`).
    XOBJECT = "XObject";
    /// Graphics-state parameter dictionaries (`/ExtGState`).
    EXT_GSTATE = "ExtGState";
    /// Marked-content property lists (`/Properties`).
    PROPERTIES = "Properties";

    // ---- Form XObjects (table 95) ----

    /// The form's bounding box (`/BBox`).
    BBOX = "BBox";
    /// The form's own matrix (`/Matrix`).
    MATRIX = "Matrix";
    /// Always 1 (`/FormType`).
    FORM_TYPE = "FormType";
    /// A form external object's `/Subtype` value (`/Form`).
    FORM = "Form";
    /// An image external object's `/Subtype` value (`/Image`).
    IMAGE = "Image";

    // ---- Image XObjects (table 89) ----

    /// Image width in samples (`/Width`).
    WIDTH = "Width";
    /// Image height in samples (`/Height`).
    HEIGHT = "Height";
    /// The samples' colour space (`/ColorSpace`).
    COLOR_SPACE = "ColorSpace";
    /// Sample-value remapping (`/Decode`).
    DECODE = "Decode";
    /// Whether the image is a stencil mask (`/ImageMask`).
    IMAGE_MASK = "ImageMask";
    /// The soft mask holding this image's alpha (`/SMask`).
    SMASK = "SMask";
    /// Whether a DCT stream's components were transformed
    /// (`/ColorTransform`).
    COLOR_TRANSFORM = "ColorTransform";
    /// One grey component per sample (`/DeviceGray`).
    DEVICE_GRAY = "DeviceGray";
    /// Three additive components per sample (`/DeviceRGB`).
    DEVICE_RGB = "DeviceRGB";
    /// Four subtractive components per sample (`/DeviceCMYK`).
    DEVICE_CMYK = "DeviceCMYK";

    // ---- Graphics state parameters the generator emits (table 58) ----

    /// Non-stroking alpha (`/ca`).
    CA_LOWER = "ca";
    /// Blend mode (`/BM`).
    BM = "BM";

    // ---- Fonts and subsetting (tables 111, 117, 120, 122) ----

    /// The PostScript name of the font (`/BaseFont`).
    BASE_FONT = "BaseFont";
    /// The descendant CIDFont of a Type 0 font (`/DescendantFonts`).
    DESCENDANT_FONTS = "DescendantFonts";
    /// The font descriptor (`/FontDescriptor`).
    FONT_DESCRIPTOR = "FontDescriptor";
    /// The descriptor's own name for the font (`/FontName`).
    FONT_NAME = "FontName";
    /// The descriptor's characteristic flags (`/Flags`).
    FLAGS = "Flags";
    /// An embedded Type 1 program (`/FontFile`).
    FONT_FILE = "FontFile";
    /// An embedded TrueType program (`/FontFile2`).
    FONT_FILE2 = "FontFile2";
    /// An embedded program in some other format, CFF included (`/FontFile3`).
    FONT_FILE3 = "FontFile3";
    /// Length of an uncompressed TrueType program (`/Length1`).
    LENGTH1 = "Length1";
    /// The code-to-Unicode CMap (`/ToUnicode`).
    TO_UNICODE = "ToUnicode";
    /// The character encoding or CMap (`/Encoding`).
    ENCODING = "Encoding";
    /// The identity-mapping horizontal CMap (`/Identity-H`).
    IDENTITY_H = "Identity-H";
    /// A Type 0 (composite) font's `/Subtype` value (`/Type0`).
    TYPE0 = "Type0";
    /// A simple font with a Type 1 program (`/Type1`).
    TYPE1 = "Type1";
    /// A simple font with a TrueType program (`/TrueType`).
    TRUE_TYPE = "TrueType";
    /// A CIDFont with CFF glyphs (`/CIDFontType0`).
    CID_FONT_TYPE0 = "CIDFontType0";
    /// A CIDFont with TrueType glyphs (`/CIDFontType2`).
    CID_FONT_TYPE2 = "CIDFontType2";
    /// An OpenType-wrapped program (`/OpenType`).
    OPEN_TYPE = "OpenType";
    /// Maps a CID to a glyph index (`/CIDToGIDMap`).
    CID_TO_GID_MAP = "CIDToGIDMap";
    /// The CMap's own type, always 2 for a ToUnicode map (`/CMapType`).
    CMAP_TYPE = "CMapType";
    /// First code a simple font's `/Widths` covers (`/FirstChar`).
    FIRST_CHAR = "FirstChar";
    /// Last code a simple font's `/Widths` covers (`/LastChar`).
    LAST_CHAR = "LastChar";
    /// Per-code advances of a simple font (`/Widths`).
    WIDTHS = "Widths";
    /// Glyph extents (`/FontBBox`).
    FONT_BBOX = "FontBBox";
    /// Degrees clockwise from vertical (`/ItalicAngle`).
    ITALIC_ANGLE = "ItalicAngle";
    /// Maximum height above the baseline (`/Ascent`).
    ASCENT = "Ascent";
    /// Maximum depth below the baseline (`/Descent`).
    DESCENT = "Descent";
    /// Height of a capital letter (`/CapHeight`).
    CAP_HEIGHT = "CapHeight";
    /// Vertical stem thickness (`/StemV`).
    STEM_V = "StemV";
    /// The CID collection (`/CIDSystemInfo`).
    CID_SYSTEM_INFO = "CIDSystemInfo";
    /// Registry name inside `/CIDSystemInfo` (`/Registry`).
    REGISTRY = "Registry";
    /// Ordering name inside `/CIDSystemInfo` (`/Ordering`).
    ORDERING = "Ordering";
    /// Supplement number inside `/CIDSystemInfo` (`/Supplement`).
    SUPPLEMENT = "Supplement";
    /// Encrypted portion of a Type 1 program (`/Length2`).
    LENGTH2 = "Length2";
    /// Trailer portion of a Type 1 program (`/Length3`).
    LENGTH3 = "Length3";
}
