//! The dictionary-key names the specification defines, as constants.
//!
//! One declaration site for the whole workspace: a key spelled here is never
//! spelled again at a use site, so a typo cannot silently produce a lookup
//! that never matches. Grouped the way ISO 32000-1 groups them; crates add
//! their keys to the group they belong to as they land.
//!
//! ```
//! use pdfrum_object::{Dict, Object, names};
//!
//! let dict = Dict::from_pairs([(names::TYPE.clone(), Object::Name(names::PAGE.clone()))]);
//! assert_eq!(dict.raw(names::TYPE), Some(&Object::Name(names::PAGE.clone())));
//! ```

use crate::names;

names! {
    // ---- Entries common to all stream dictionaries (table 5) ----

    /// Number of bytes of stream data (`/Length`).
    LENGTH = "Length";
    /// Filter or filter chain applied to the stream data (`/Filter`).
    FILTER = "Filter";
    /// Parameters for the filters (`/DecodeParms`).
    DECODE_PARMS = "DecodeParms";
    /// External file holding the data, also an annotation's flag word
    /// (`/F` — the meaning follows the dictionary, not the key).
    F = "F";

    // ---- Stream filters and their abbreviations (tables 6 and 92) ----

    /// Deflate compression (`/FlateDecode`).
    FLATE_DECODE = "FlateDecode";
    /// Inline-image abbreviation for `/FlateDecode` (`/Fl`).
    FL = "Fl";
    /// LZW compression (`/LZWDecode`).
    LZW_DECODE = "LZWDecode";
    /// Inline-image abbreviation for `/LZWDecode` (`/LZW`).
    LZW = "LZW";
    /// Base-85 text encoding (`/ASCII85Decode`).
    ASCII85_DECODE = "ASCII85Decode";
    /// Inline-image abbreviation for `/ASCII85Decode` (`/A85`).
    A85 = "A85";
    /// Hexadecimal text encoding (`/ASCIIHexDecode`).
    ASCII_HEX_DECODE = "ASCIIHexDecode";
    /// Inline-image abbreviation for `/ASCIIHexDecode` (`/AHx`).
    AHX = "AHx";
    /// Byte-oriented run-length compression (`/RunLengthDecode`).
    RUN_LENGTH_DECODE = "RunLengthDecode";
    /// Inline-image abbreviation for `/RunLengthDecode` (`/RL`).
    RL = "RL";
    /// Group 3/4 fax compression (`/CCITTFaxDecode`).
    CCITT_FAX_DECODE = "CCITTFaxDecode";
    /// Inline-image abbreviation for `/CCITTFaxDecode` (`/CCF`).
    CCF = "CCF";
    /// Baseline JPEG (`/DCTDecode`).
    DCT_DECODE = "DCTDecode";
    /// Inline-image abbreviation for `/DCTDecode` (`/DCT`).
    DCT = "DCT";
    /// JPEG 2000 (`/JPXDecode`).
    JPX_DECODE = "JPXDecode";
    /// Bi-level JBIG2 compression (`/JBIG2Decode`).
    JBIG2_DECODE = "JBIG2Decode";
    /// The crypt filter placeholder (`/Crypt`).
    CRYPT = "Crypt";

    // ---- Filter parameters (tables 8, 10, 11 and 12) ----

    /// Which predictor was applied before compression (`/Predictor`).
    PREDICTOR = "Predictor";
    /// Colour components per sample, for a predictor (`/Colors`).
    COLORS = "Colors";
    /// Bits per colour component (`/BitsPerComponent`).
    BITS_PER_COMPONENT = "BitsPerComponent";
    /// Samples per row (`/Columns`).
    COLUMNS = "Columns";
    /// Whether LZW code lengths grow one code early (`/EarlyChange`).
    EARLY_CHANGE = "EarlyChange";
    /// The CCITT encoding scheme selector (`/K`).
    K = "K";
    /// Whether CCITT rows are terminated by end-of-line codes (`/EndOfLine`).
    END_OF_LINE = "EndOfLine";
    /// Whether each CCITT row starts on a byte boundary (`/EncodedByteAlign`).
    ENCODED_BYTE_ALIGN = "EncodedByteAlign";
    /// Number of rows in a CCITT image (`/Rows`).
    ROWS = "Rows";
    /// Whether a CCITT 1 bit means black (`/BlackIs1`).
    BLACK_IS_1 = "BlackIs1";

    // ---- Trailer and cross-reference (tables 15 and 17) ----

    /// The document catalog (`/Root`).
    ROOT = "Root";
    /// The document information dictionary (`/Info`).
    INFO = "Info";
    /// One past the highest object number (`/Size`).
    SIZE = "Size";
    /// Offset of the previous cross-reference section (`/Prev`).
    PREV = "Prev";
    /// Offset of a hybrid file's cross-reference stream (`/XRefStm`).
    XREF_STM = "XRefStm";
    /// The encryption dictionary (`/Encrypt`).
    ENCRYPT = "Encrypt";
    /// The file identifier pair (`/ID`).
    ID = "ID";
    /// Subsection object-number ranges of a cross-reference stream (`/Index`).
    INDEX = "Index";
    /// Field widths of a cross-reference stream's entries (`/W`).
    W = "W";
    /// What kind of dictionary this is (`/Type`).
    TYPE = "Type";
    /// A cross-reference stream (`/XRef`).
    XREF = "XRef";
    /// An object stream (`/ObjStm`).
    OBJ_STM = "ObjStm";
    /// Number of objects in an object stream (`/N`).
    N = "N";
    /// Offset of the first object in an object stream (`/First`).
    FIRST = "First";

    // ---- Document information dictionary (table 317) ----

    /// The document's title (`/Title`).
    TITLE = "Title";
    /// Who wrote the document (`/Author`).
    AUTHOR = "Author";
    /// What the document is about (`/Subject`).
    SUBJECT = "Subject";
    /// Keywords associated with the document (`/Keywords`).
    KEYWORDS = "Keywords";
    /// The application that produced the original document (`/Creator`).
    CREATOR = "Creator";
    /// The application that converted it to PDF (`/Producer`).
    PRODUCER = "Producer";
    /// When the document was created (`/CreationDate`).
    CREATION_DATE = "CreationDate";
    /// When the document was last modified (`/ModDate`).
    MOD_DATE = "ModDate";

    // ---- Catalog and page tree (tables 28 and 29) ----

    /// The root of the page tree (`/Pages`).
    PAGES = "Pages";
    /// A leaf of the page tree (`/Page`).
    PAGE = "Page";
    /// The page label number tree (`/PageLabels`).
    PAGE_LABELS = "PageLabels";
    /// The name dictionary (`/Names`).
    NAMES = "Names";
    /// The named-destination dictionary (`/Dests`).
    DESTS = "Dests";
    /// The embedded-file name tree (`/EmbeddedFiles`).
    EMBEDDED_FILES = "EmbeddedFiles";
    /// The `/Type` of an embedded file stream (`/EmbeddedFile`).
    EMBEDDED_FILE = "EmbeddedFile";
    /// The document-level JavaScript name tree (`/JavaScript`).
    JAVA_SCRIPT = "JavaScript";
    /// A portable collection, i.e. a portfolio (`/Collection`).
    COLLECTION = "Collection";
    /// Viewer preferences (`/ViewerPreferences`).
    VIEWER_PREFERENCES = "ViewerPreferences";
    /// The outline (bookmark) tree root (`/Outlines`).
    OUTLINES = "Outlines";
    /// The interactive form dictionary (`/AcroForm`).
    ACRO_FORM = "AcroForm";
    /// Children of a page-tree node (`/Kids`).
    KIDS = "Kids";
    /// Number of leaf pages below a page-tree node (`/Count`).
    COUNT = "Count";
    /// The parent node of a page-tree node or form field (`/Parent`).
    PARENT = "Parent";
    /// The resources a page or form needs (`/Resources`). Inheritable
    /// through the page tree.
    RESOURCES = "Resources";
    /// The sheet a page is imaged on (`/MediaBox`). Inheritable.
    MEDIA_BOX = "MediaBox";
    /// The region of a page a viewer displays (`/CropBox`). Inheritable.
    CROP_BOX = "CropBox";
    /// The region clipped to when producing output (`/BleedBox`).
    BLEED_BOX = "BleedBox";
    /// The intended finished dimensions after trimming (`/TrimBox`).
    TRIM_BOX = "TrimBox";
    /// The extent of the page's meaningful content (`/ArtBox`).
    ART_BOX = "ArtBox";
    /// Clockwise display rotation in degrees (`/Rotate`). Inheritable.
    ROTATE = "Rotate";
    /// Metadata stream (`/Metadata`).
    METADATA = "Metadata";
    /// A more specific type within a `/Type` (`/Subtype`).
    SUBTYPE = "Subtype";
    /// An XML metadata stream's subtype (`/XML`).
    XML = "XML";

    // ---- Resource categories, form and image XObjects (tables 33, 89 and 95) ----

    /// The font resource category, and a font dictionary's own `/Type`
    /// value (`/Font`).
    FONT = "Font";
    /// The external-object resource category — images and forms alike —
    /// and an external object's own `/Type` value (`/XObject`).
    XOBJECT = "XObject";
    /// The graphics-state parameter resource category (`/ExtGState`).
    EXT_G_STATE = "ExtGState";
    /// The marked-content property resource category (`/Properties`).
    PROPERTIES = "Properties";
    /// The shading resource category (`/Shading`), which the `sh` operator
    /// names its shading in.
    SHADING = "Shading";
    /// A form's, pattern's or shading's coordinate mapping (`/Matrix`).
    MATRIX = "Matrix";
    /// A form's or pattern's clipping rectangle (`/BBox`).
    BBOX = "BBox";
    /// Image width in samples (`/Width`).
    WIDTH = "Width";
    /// Image height in samples (`/Height`).
    HEIGHT = "Height";
    /// An image's or shading's colour space (`/ColorSpace`).
    COLOR_SPACE = "ColorSpace";
    /// Sample-value remapping (`/Decode`).
    DECODE = "Decode";
    /// Whether the image is a stencil mask (`/ImageMask`).
    IMAGE_MASK = "ImageMask";
    /// A soft mask, either an image's or an `/ExtGState`'s (`/SMask`).
    SMASK = "SMask";

    // ---- Encryption (tables 20 through 27) ----

    /// Algorithm version, also a form field's value (`/V`).
    V = "V";
    /// Standard security handler revision (`/R`), also an appearance's
    /// rollover state.
    R = "R";
    /// Owner password hash (`/O`), also a linearized file's first page
    /// object number.
    O = "O";
    /// User password hash (`/U`).
    U = "U";
    /// Permission flags (`/P`), also an annotation's page reference and a
    /// linearized file's first-page offset.
    P = "P";
    /// Owner encryption key, revision 5 and 6 (`/OE`).
    OE = "OE";
    /// User encryption key, revision 5 and 6 (`/UE`).
    UE = "UE";
    /// Encrypted permissions, revision 5 and 6 (`/Perms`).
    PERMS = "Perms";
    /// Crypt filter used for streams (`/StmF`).
    STM_F = "StmF";
    /// Crypt filter used for strings (`/StrF`).
    STR_F = "StrF";
    /// Crypt filter used for embedded files (`/EFF`).
    EFF = "EFF";
    /// The crypt filter dictionary (`/CF`).
    CF = "CF";
    /// A crypt filter's method (`/CFM`).
    CFM = "CFM";
    /// When a crypt filter's key is requested (`/AuthEvent`).
    AUTH_EVENT = "AuthEvent";
    /// Whether the document metadata is encrypted (`/EncryptMetadata`).
    ENCRYPT_METADATA = "EncryptMetadata";
    /// The standard security handler (`/Standard`).
    STANDARD = "Standard";
    /// The pass-through crypt filter (`/Identity`).
    IDENTITY = "Identity";

    // ---- Linearization (annex F) ----

    /// Length of the whole file (`/L`), also a line annotation's endpoints.
    L = "L";
    /// Offset and length of the hint stream (`/H`).
    H = "H";
    /// Offset of the end of the first page (`/E`).
    E = "E";
    /// Offset of the main cross-reference table (`/T`), also a form field's
    /// partial name.
    T = "T";

    // ---- Entries common to all annotations (table 168) ----

    /// A page's annotation array (`/Annots`).
    ANNOTS = "Annots";
    /// The annotation's rectangle (`/Rect`).
    RECT = "Rect";
    /// The intent of a markup or screen annotation (`/IT`).
    IT = "IT";
    /// The annotation's text, or a page's content stream (`/Contents`).
    CONTENTS = "Contents";
    /// The annotation's name, unique within the page (`/NM`).
    NM = "NM";
    /// Last-modified date (`/M`).
    M = "M";
    /// The appearance dictionary (`/AP`).
    AP = "AP";
    /// The appearance state selecting a sub-appearance (`/AS`).
    AS = "AS";
    /// Border characteristics (`/Border`).
    BORDER = "Border";
    /// Colour (`/C`).
    C = "C";
    /// Optional-content membership (`/OC`).
    OC = "OC";
    /// Ink annotation stroke list (`/InkList`).
    INK_LIST = "InkList";

    // ---- Appearance characteristics (table 189) ----

    /// Background colour (`/BG`), also a widget's background.
    BG = "BG";
    /// Border colour (`/BC`), also a soft mask's backdrop.
    BC = "BC";
    /// Normal caption (`/CA`), also the non-stroking alpha constant.
    CA = "CA";
    /// Normal icon (`/I`), also a transparency group's isolation flag.
    I = "I";
    /// Rollover icon (`/RI`).
    RI = "RI";
    /// Alternate (down) icon (`/IX`).
    IX = "IX";

    // ---- Interactive form fields (tables 220 and 228) ----

    /// The field type (`/FT`).
    FT = "FT";
    /// The XFA form packet, whose presence makes a form an XFA one (`/XFA`).
    XFA = "XFA";
    /// Alternate field name, shown to the user (`/TU`).
    TU = "TU";
    /// Field flags (`/Ff`).
    FF = "Ff";
    /// Default value (`/DV`).
    DV = "DV";
    /// Additional-actions dictionary (`/AA`).
    AA = "AA";
    /// Signature field type (`/Sig`).
    SIG = "Sig";
    /// Default appearance string (`/DA`).
    DA = "DA";
    /// Quadding — the text alignment code (`/Q`).
    Q = "Q";
    /// Default style string (`/DS`).
    DS = "DS";

    // ---- Predefined encodings (annex D) ----

    /// The Mac OS standard encoding (`/MacRomanEncoding`).
    MAC_ROMAN_ENCODING = "MacRomanEncoding";
    /// The Windows code page 1252 encoding (`/WinAnsiEncoding`).
    WIN_ANSI_ENCODING = "WinAnsiEncoding";
    /// The document-string encoding (`/PDFDocEncoding`).
    PDF_DOC_ENCODING = "PDFDocEncoding";
    /// The expert-set encoding (`/MacExpertEncoding`).
    MAC_EXPERT_ENCODING = "MacExpertEncoding";

    // ---- Transparency: blend modes and groups (tables 136 and 144) ----

    /// The `Normal` blend mode.
    NORMAL = "Normal";
    /// The `Multiply` blend mode.
    MULTIPLY = "Multiply";
    /// The `Screen` blend mode.
    SCREEN = "Screen";
    /// The `Overlay` blend mode.
    OVERLAY = "Overlay";
    /// The `Darken` blend mode.
    DARKEN = "Darken";
    /// The `Lighten` blend mode.
    LIGHTEN = "Lighten";
    /// The `ColorDodge` blend mode.
    COLOR_DODGE = "ColorDodge";
    /// The `ColorBurn` blend mode.
    COLOR_BURN = "ColorBurn";
    /// The `HardLight` blend mode.
    HARD_LIGHT = "HardLight";
    /// The `SoftLight` blend mode.
    SOFT_LIGHT = "SoftLight";
    /// The `Difference` blend mode.
    DIFFERENCE = "Difference";
    /// The `Exclusion` blend mode.
    EXCLUSION = "Exclusion";
    /// The `Hue` blend mode.
    HUE = "Hue";
    /// The `Saturation` blend mode.
    SATURATION = "Saturation";
    /// The `Color` blend mode.
    COLOR = "Color";
    /// The `Luminosity` blend mode.
    LUMINOSITY = "Luminosity";
    /// A soft mask's or transparency group's subtype (`/S`).
    S = "S";
    /// An alpha soft mask (`/Alpha`).
    ALPHA = "Alpha";
    /// The form `XObject` a soft mask draws (`/G`).
    G = "G";
    /// A soft mask's transfer function (`/TR`).
    TR = "TR";
    /// A group's colour space (`/CS`).
    CS = "CS";
}

#[cfg(test)]
mod tests {
    use super::{FILTER, LENGTH, PAGE, ROOT, TYPE};
    use crate::Name;

    #[test]
    fn constants_carry_the_specification_spelling() {
        assert_eq!(LENGTH.as_str(), Some("Length"));
        assert_eq!(FILTER.as_str(), Some("Filter"));
        assert_eq!(ROOT.as_str(), Some("Root"));
        assert_eq!(TYPE.as_str(), Some("Type"));
        assert_eq!(PAGE.as_str(), Some("Page"));
    }

    #[test]
    fn constants_equal_names_parsed_from_a_file() {
        assert_eq!(LENGTH, &Name::decode(b"Length"));
        assert_eq!(LENGTH, &Name::decode(b"Lengt#68"));
    }
}
