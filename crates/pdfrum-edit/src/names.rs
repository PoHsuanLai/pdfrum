//! The dictionary keys this crate writes, added to the shared table.
//!
//! `pdfrum-object`'s `names` module owns the keys several crates share; the
//! ones below are keys only a *writer* has occasion to spell.

#[expect(unused_imports, reason = "the shared keys are re-exported as one set")]
pub(crate) use pdfrum_object::names::{
    ANNOTS, ART_BOX, ASCENT, AUTHOR, BASE_FONT, BBOX, BITS_PER_COMPONENT, BLEED_BOX, BM, BORDER, C,
    CA, CA_LOWER, CAP_HEIGHT, CID_SYSTEM_INFO, CID_TO_GID_MAP, COLOR_SPACE, COLUMNS, CONTENTS,
    COUNT, CREATOR, CROP_BOX, DA, DCT_DECODE, DECODE, DECODE_PARMS, DESCENDANT_FONTS, DESCENT,
    ENCODING, ENCRYPT, EXT_G_STATE, F, FILTER, FIRST, FIRST_CHAR, FLAGS, FLATE_DECODE, FONT,
    FONT_BBOX, FONT_DESCRIPTOR, FONT_FILE, FONT_FILE2, FONT_FILE3, FORM, FORM_TYPE, FT, HEIGHT, ID,
    IMAGE, IMAGE_MASK, INDEX, INFO, INK_LIST, ITALIC_ANGLE, JPX_DECODE, KEYWORDS, KIDS, LAST_CHAR,
    LENGTH, M, MATRIX, MEDIA_BOX, METADATA, N, NM, NORMAL, ORDERING, P, PAGE, PAGES, PARENT, PREV,
    PROPERTIES, QUAD_POINTS, R, RECT, RESOURCES, ROOT, ROTATE, S, SIZE, SMASK, STANDARD, STEM_V,
    SUBJECT, SUBTYPE, T, TITLE, TO_UNICODE, TRIM_BOX, TRUE_TYPE, TYPE, TYPE1, VIEWER_PREFERENCES,
    W, WIDTH, WIDTHS, WIN_ANSI_ENCODING, XML, XOBJECT, XREF, XREF_STM,
};

pdfrum_object::names! {
    // ---- Catalog and page tree, writer side (tables 28, 29) ----

    /// The document catalog's own `/Type` value (`/Catalog`).
    CATALOG = "Catalog";

    // ---- Image XObjects (table 89) ----

    /// Whether a DCT stream's components were transformed
    /// (`/ColorTransform`).
    COLOR_TRANSFORM = "ColorTransform";
    /// One grey component per sample (`/DeviceGray`).
    DEVICE_GRAY = "DeviceGray";
    /// Three additive components per sample (`/DeviceRGB`).
    DEVICE_RGB = "DeviceRGB";
    /// Four subtractive components per sample (`/DeviceCMYK`).
    DEVICE_CMYK = "DeviceCMYK";

    // ---- Fonts and subsetting (tables 111, 117, 120, 122) ----

    /// The descriptor's own name for the font (`/FontName`).
    FONT_NAME = "FontName";
    /// Length of an uncompressed TrueType program (`/Length1`).
    LENGTH1 = "Length1";
    /// The identity-mapping horizontal CMap (`/Identity-H`).
    IDENTITY_H = "Identity-H";
    /// A Type 0 (composite) font's `/Subtype` value (`/Type0`).
    TYPE0 = "Type0";
    /// A CIDFont with CFF glyphs (`/CIDFontType0`).
    CID_FONT_TYPE0 = "CIDFontType0";
    /// A CIDFont with TrueType glyphs (`/CIDFontType2`).
    CID_FONT_TYPE2 = "CIDFontType2";
    /// An OpenType-wrapped program (`/OpenType`).
    OPEN_TYPE = "OpenType";
    /// The CMap's own type, always 2 for a ToUnicode map (`/CMapType`).
    CMAP_TYPE = "CMapType";
    /// Registry name inside `/CIDSystemInfo` (`/Registry`).
    REGISTRY = "Registry";
    /// Supplement number inside `/CIDSystemInfo` (`/Supplement`).
    SUPPLEMENT = "Supplement";
    /// Encrypted portion of a Type 1 program (`/Length2`).
    LENGTH2 = "Length2";
    /// Trailer portion of a Type 1 program (`/Length3`).
    LENGTH3 = "Length3";

    // ---- Annotations (ISO 32000-1 §12.5) ----

    /// An annotation dictionary's `/Type` value (`/Annot`).
    ANNOT = "Annot";
    /// Border style dictionary (`/BS`).
    BS = "BS";
    /// Icon name for a text annotation (`/Name`).
    NAME = "Name";
    /// Whether a text annotation's pop-up starts open (`/Open`).
    OPEN = "Open";
    /// Comment icon name (`/Comment`).
    COMMENT = "Comment";
    /// Action dictionary (`/A`), e.g. a URI action on a Link.
    A = "A";
    /// Uniform resource identifier (`/URI`).
    URI = "URI";
    /// Line endpoints (`/L`) for a Line annotation.
    L = "L";
    // Subtype spellings (/Highlight, /Text, …) come from
    // `pdfrum_doc::Subtype::as_bytes` at write time — do not duplicate them here.
}
