//! The `/Font` dictionary keys this crate reads.
//!
//! Keys another crate also reads or writes live in `pdfrum-object`'s shared
//! table and are re-exported here so font code still writes `names::BASE_FONT`.

#![allow(unused_imports)] // the re-export is the crate's one names table

pub(crate) use pdfrum_object::names::{
    ASCENT, BASE_ENCODING, BASE_FONT, CAP_HEIGHT, CID_SYSTEM_INFO, CID_TO_GID_MAP,
    DESCENDANT_FONTS, DESCENT, DIFFERENCES, ENCODING, FIRST_CHAR, FLAGS, FONT, FONT_BBOX,
    FONT_DESCRIPTOR, FONT_FILE, FONT_FILE2, FONT_FILE3, ITALIC_ANGLE, LAST_CHAR, ORDERING,
    RESOURCES, STEM_V, SUBTYPE, TO_UNICODE, TYPE, W, WIDTHS, WIN_ANSI_ENCODING,
};

pdfrum_object::names! {
    /// The width of a code outside `/Widths` (`/MissingWidth`).
    MISSING_WIDTH = "MissingWidth";
    /// The weight, 100 to 900 (`/FontWeight`).
    FONT_WEIGHT = "FontWeight";
    /// The default horizontal advance (`/DW`).
    DW = "DW";
    /// Per-CID vertical metrics (`/W2`).
    W2 = "W2";
    /// The default vertical metrics (`/DW2`).
    DW2 = "DW2";
    /// The glyph-space to text-space transform (`/FontMatrix`).
    FONT_MATRIX = "FontMatrix";
    /// A Type3 font's glyph procedures (`/CharProcs`).
    CHAR_PROCS = "CharProcs";
    /// The CMap an `/Encoding` stream inherits from (`/UseCMap`).
    USE_CMAP = "UseCMap";
}
