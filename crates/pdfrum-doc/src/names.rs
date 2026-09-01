//! The dictionary keys this crate reads, added to the shared table.
//!
//! `pdfrum-object`'s `names` module owns the keys several crates share; the
//! ones below are document-feature keys that only appear here.

#[allow(unused_imports)]
pub(crate) use pdfrum_object::names::{
    AA, AC, ACRO_FORM, ANNOTS, AP, AS, BC, BG, BORDER, C, CA, CONTENTS, COUNT, CROP_BOX, DA, DESTS,
    DV, EMBEDDED_FILES, F, FF, FIRST, FT, I, ID, INK_LIST, IX, JAVA_SCRIPT, K, KIDS, L, LENGTH,
    MEDIA_BOX, METADATA, NAMES, NORMAL, OUTLINES, P, PAGE, PAGE_LABELS, PARENT, Q, R, RC, RECT,
    RESOURCES, RI, ROTATE, S, STRUCT_PARENT, SUBTYPE, T, TITLE, TU, TYPE, V, VERTICES,
    VIEWER_PREFERENCES, W, XML,
};

pdfrum_object::names! {
    // ---- Outline ----

    /// The next sibling of an outline item (`/Next`).
    NEXT = "Next";
    /// A destination, on an outline item, link or action (`/Dest`).
    DEST = "Dest";
    /// An action dictionary (`/A`).
    A = "A";
    /// The `/Type` an action dictionary declares, when it declares one
    /// (`/Action`).
    ANNOT_ACTION = "Action";

    // ---- Name and number trees ----

    /// A tree node's least and greatest key (`/Limits`).
    LIMITS = "Limits";
    /// A number tree leaf's key/value pairs (`/Nums`).
    NUMS = "Nums";
    /// A destination indirection inside a dictionary-valued name-tree entry
    /// (`/D`), also an action's destination.
    D = "D";

    // ---- Actions ----

    /// A file specification's Windows-specific launch parameters (`/Win`).
    WIN = "Win";
    /// A uniform resource identifier, and the catalog's base-URI dictionary
    /// (`/URI`).
    URI = "URI";
    /// The base against which a relative `/URI` resolves (`/Base`).
    BASE = "Base";
    /// Whether a Hide action hides or shows (`/H`), also a widget's
    /// highlighting mode.
    H = "H";
    /// A named action's name (`/N`), also the normal appearance stream.
    N = "N";
    /// A submit-form or reset-form action's flag word (`/Flags`).
    FLAGS = "Flags";
    /// The fields a form action applies to (`/Fields`).
    FIELDS = "Fields";
    /// The order a recalculation visits fields in (`/AcroForm /CO`).
    CALCULATION_ORDER = "CO";
    /// A JavaScript action's program (`/JS`).
    JS = "JS";
    /// The action a reader runs when the document opens (`/OpenAction`).
    /// Also legally a destination array, which is not an action.
    OPEN_ACTION = "OpenAction";

    // ---- File specifications ----

    /// The Unicode file name (`/UF`).
    UF = "UF";
    /// The file-system name, `URL` meaning the path is a URL (`/FS`).
    FS = "FS";
    /// A DOS-flavoured file name (`/DOS`).
    DOS = "DOS";
    /// A Mac OS-flavoured file name (`/Mac`).
    MAC = "Mac";
    /// A Unix-flavoured file name (`/Unix`).
    UNIX = "Unix";
    /// The embedded file streams of a file specification (`/EF`).
    EF = "EF";
    /// An embedded file stream's parameter dictionary (`/Params`).
    PARAMS = "Params";

    // ---- Structure tree ----

    /// The catalog's structure tree root (`/StructTreeRoot`).
    STRUCT_TREE_ROOT = "StructTreeRoot";
    /// Whether the document is tagged (`/MarkInfo`).
    MARK_INFO = "MarkInfo";
    /// The tagging flag inside `/MarkInfo` (`/Marked`).
    MARKED = "Marked";
    /// The structure element type to standard type mapping (`/RoleMap`).
    ROLE_MAP = "RoleMap";
    /// The number tree from `/StructParents` keys to structure elements
    /// (`/ParentTree`).
    PARENT_TREE = "ParentTree";
    /// A page's or form's key into the parent tree (`/StructParents`).
    ///
    /// Not to be confused with the singular `/StructParent`, which is what an
    /// *annotation* carries; the two are different keys with different
    /// meanings and only one of them appears on a page.
    STRUCT_PARENTS = "StructParents";
    /// A structure element's page (`/Pg`).
    PG = "Pg";
    /// A marked-content reference's content stream (`/Stm`).
    STM = "Stm";
    /// A marked-content identifier (`/MCID`).
    MCID = "MCID";
    /// An object reference's target (`/Obj`).
    OBJ = "Obj";
    /// A marked-content reference (`/MCR`).
    MCR = "MCR";
    /// An object reference kid (`/OBJR`).
    OBJR = "OBJR";
    /// A structure element (`/StructElem`).
    STRUCT_ELEM = "StructElem";
    /// Alternate text for a structure element (`/Alt`).
    ALT = "Alt";
    /// The exact text a structure element replaces (`/ActualText`).
    ACTUAL_TEXT = "ActualText";
    /// An abbreviation's expansion (`/E`).
    E = "E";
    /// A natural-language identifier (`/Lang`).
    LANG = "Lang";

    // ---- Page labels ----

    /// A page label's prefix (`/P`), also a widget's page and a permissions
    /// word — the meaning follows the dictionary.
    ST = "St";

    // ---- Viewer preferences ----

    /// The reading order of the page display (`/Direction`).
    DIRECTION = "Direction";
    /// Whether the print dialog offers page scaling (`/PrintScaling`).
    PRINT_SCALING = "PrintScaling";
    /// The default number of printed copies (`/NumCopies`).
    NUM_COPIES = "NumCopies";
    /// The default printed page range (`/PrintPageRange`).
    PRINT_PAGE_RANGE = "PrintPageRange";
    /// The default duplex handling (`/Duplex`).
    DUPLEX = "Duplex";

    // ---- Annotations ----

    /// A text markup annotation's quadrilaterals (`/QuadPoints`).
    QUAD_POINTS = "QuadPoints";
    /// An annotation's interior colour (`/IC`).
    IC = "IC";
    /// A border style dictionary (`/BS`).
    BS = "BS";
    /// The marker key PDFium writes on an annotation whose appearance it
    /// generated (`/PDFIUM_HasGeneratedAP`).
    HAS_GENERATED_AP = "PDFIUM_HasGeneratedAP";
    /// A popup annotation attached to a markup annotation (`/Popup`).
    POPUP = "Popup";
    /// An annotation dictionary's type value (`/Annot`).
    ANNOT = "Annot";

    // ---- Appearance streams and their resources ----

    /// A form XObject's bounding box (`/BBox`).
    BBOX = "BBox";
    /// A form XObject's transformation matrix (`/Matrix`).
    MATRIX = "Matrix";
    /// A form XObject's type (`/XObject`).
    XOBJECT = "XObject";
    /// The form XObject subtype (`/Form`).
    FORM = "Form";
    /// The form XObject generation number (`/FormType`).
    FORM_TYPE = "FormType";
    /// A graphics state parameter dictionary (`/ExtGState`).
    EXT_GSTATE = "ExtGState";
    /// The single graphics state entry every generated appearance uses
    /// (`/GS`).
    GS = "GS";
    /// Stroking alpha in a graphics state dictionary (`/CA`); the fill alpha
    /// is the lowercase `/ca`.
    CA_LOWER = "ca";
    /// Whether alpha is a shape or an opacity (`/AIS`).
    AIS = "AIS";
    /// The blend mode (`/BM`).
    BM = "BM";
    /// The one blend mode a generated appearance ever asks for beyond normal
    /// (`/Multiply`), used by the highlight generator so the text below shows
    /// through.
    MULTIPLY_BLEND = "Multiply";
    /// A font resource dictionary (`/Font`).
    FONT = "Font";
    /// A Type 1 font subtype (`/Type1`).
    TYPE1 = "Type1";
    /// A TrueType font subtype (`/TrueType`), which is what a face added for
    /// a charset the `/DA` font cannot write is written as.
    TRUE_TYPE = "TrueType";
    /// A simple font's encoding (`/Encoding`).
    ENCODING = "Encoding";
    /// The encoding a `/Differences` array modifies (`/BaseEncoding`).
    BASE_ENCODING = "BaseEncoding";
    /// Per-code overrides on the base encoding (`/Differences`).
    DIFFERENCES = "Differences";
    /// The PostScript name of a font (`/BaseFont`).
    BASE_FONT = "BaseFont";
    /// A font's descriptor (`/FontDescriptor`).
    FONT_DESCRIPTOR = "FontDescriptor";

    // ---- Form fields ----

    /// The default resources a form's `/DA` strings name (`/DR`).
    DR = "DR";
    /// A text field's maximum length, and a comb field's cell count
    /// (`/MaxLen`).
    MAX_LEN = "MaxLen";
    /// A choice field's options (`/Opt`).
    OPT = "Opt";
    /// A list box's first visible option (`/TI`).
    TI = "TI";
    /// Whether the viewer must regenerate every widget appearance
    /// (`/NeedAppearances`).
    NEED_APPEARANCES = "NeedAppearances";
    /// A widget annotation subtype (`/Widget`).
    WIDGET = "Widget";
    /// A widget's appearance characteristics (`/MK`).
    MK = "MK";
    /// An icon's fitting rules (`/IF`).
    IF = "IF";
    /// How an icon scales (`/SW`).
    SW = "SW";
    /// Whether an icon scales proportionally (`/S`), also a border style —
    /// the meaning follows the dictionary.
    S_ICON = "S";
    /// Whether an icon's bounding box is fitted (`/FB`).
    FB = "FB";
    /// A caption's position relative to its icon (`/TP`).
    TP = "TP";
    /// The state a widget shows when off (`/Off`).
    OFF = "Off";
    /// The default on-state name when a checkbox names none (`/Yes`).
    YES = "Yes";
}
