//! The standard 14 fonts: their canonical names, the 89 aliases real files
//! spell them with, and the Foxit programs that draw them.
//!
//! Every PDF reader is required to supply these fourteen faces. PDFium ships
//! Foxit's bare-CFF clones of all fourteen plus two Multiple-Master faces for
//! everything else, and they are what makes the substitution ladder always
//! terminate in *something*.

use super::tables::{ALT_FONT_NAMES, BASE14_FONT_NAMES};

/// One of the fourteen standard fonts.
///
/// The discriminants are load-bearing, not cosmetic. Within a family the order
/// is **Regular, Bold, BoldOblique, Oblique** — so `index % 4` decides the
/// style and `index + 1` / `+2` / `+3` is how a style is *applied* to a family
///. Changing the order silently changes substitution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum StandardFont {
    /// `Courier`.
    Courier = 0,
    /// `Courier-Bold`.
    CourierBold = 1,
    /// `Courier-BoldOblique`.
    CourierBoldOblique = 2,
    /// `Courier-Oblique`.
    CourierOblique = 3,
    /// `Helvetica`.
    Helvetica = 4,
    /// `Helvetica-Bold`.
    HelveticaBold = 5,
    /// `Helvetica-BoldOblique`.
    HelveticaBoldOblique = 6,
    /// `Helvetica-Oblique`.
    HelveticaOblique = 7,
    /// `Times-Roman`.
    Times = 8,
    /// `Times-Bold`.
    TimesBold = 9,
    /// `Times-BoldItalic`. Note the family asymmetry: Courier and Helvetica
    /// say `Oblique` where Times says `Italic`.
    TimesBoldOblique = 10,
    /// `Times-Italic`.
    TimesOblique = 11,
    /// `Symbol`.
    Symbol = 12,
    /// `ZapfDingbats`.
    Dingbats = 13,
}

impl StandardFont {
    /// The index into the base-14 tables.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Build from an index, for the arithmetic of the former working note step 10.
    #[must_use]
    pub fn from_index(i: usize) -> Option<Self> {
        Some(match i {
            0 => Self::Courier,
            1 => Self::CourierBold,
            2 => Self::CourierBoldOblique,
            3 => Self::CourierOblique,
            4 => Self::Helvetica,
            5 => Self::HelveticaBold,
            6 => Self::HelveticaBoldOblique,
            7 => Self::HelveticaOblique,
            8 => Self::Times,
            9 => Self::TimesBold,
            10 => Self::TimesBoldOblique,
            11 => Self::TimesOblique,
            12 => Self::Symbol,
            13 => Self::Dingbats,
            _ => return None,
        })
    }

    /// Symbol and ZapfDingbats are the two whose glyphs are not Latin text,
    /// which changes both their default flags and their default encoding.
    #[must_use]
    pub fn is_symbolic(self) -> bool {
        matches!(self, Self::Symbol | Self::Dingbats)
    }

    /// The four Couriers, whose every glyph is 600 units wide.
    #[must_use]
    pub fn is_fixed(self) -> bool {
        self.index() < 4
    }

    /// Can a style be applied to this index by arithmetic? Only the three
    /// family *heads* can (`IsStylableBaseFont`).
    #[must_use]
    pub fn is_stylable(self) -> bool {
        matches!(self, Self::Courier | Self::Helvetica | Self::Times)
    }
}

/// The canonical PostScript name.
#[must_use]
pub fn canonical_font_name(f: StandardFont) -> &'static str {
    BASE14_FONT_NAMES.get(f.index()).copied().unwrap_or("")
}

/// Resolve a `/BaseFont` name to a standard font, through the 89-entry alias
/// table.
///
/// **Case-insensitive**, which is why `"arial"` resolves. PDFium binary-searches
/// a table sorted by `FXSYS_stricmp`; a case-insensitive scan is behaviorally
/// identical and is what we do, so the table's sortedness stops being a
/// correctness requirement.
#[must_use]
pub fn standard_font_index(name: &[u8]) -> Option<StandardFont> {
    let name = std::str::from_utf8(name).ok()?;
    ALT_FONT_NAMES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, f)| *f)
}

/// Is this **exactly** one of the fourteen canonical names?
///
/// Case-**sensitive**, and it does not consult the alias table — a different
/// question from [`standard_font_index`], and PDFium asks both.
#[must_use]
#[cfg(test)]
pub fn is_standard_font_name(name: &[u8]) -> bool {
    std::str::from_utf8(name).is_ok_and(|n| BASE14_FONT_NAMES.contains(&n))
}

/// The Foxit font program for a standard font.
///
/// All fourteen are **bare CFF** (`01 00 04 02`), which `read-fonts` reads
/// natively — no OpenType wrapper, no Type 1 container. Vendored from the
/// oracle under its BSD licence; see `fontdata/PROVENANCE.md`.
#[must_use]
pub fn standard_font_data(f: StandardFont) -> &'static [u8] {
    match f {
        StandardFont::Courier => include_bytes!("../../fontdata/FoxitFixed.cff"),
        StandardFont::CourierBold => include_bytes!("../../fontdata/FoxitFixedBold.cff"),
        StandardFont::CourierBoldOblique => {
            include_bytes!("../../fontdata/FoxitFixedBoldItalic.cff")
        }
        StandardFont::CourierOblique => include_bytes!("../../fontdata/FoxitFixedItalic.cff"),
        StandardFont::Helvetica => include_bytes!("../../fontdata/FoxitSans.cff"),
        StandardFont::HelveticaBold => include_bytes!("../../fontdata/FoxitSansBold.cff"),
        StandardFont::HelveticaBoldOblique => {
            include_bytes!("../../fontdata/FoxitSansBoldItalic.cff")
        }
        StandardFont::HelveticaOblique => include_bytes!("../../fontdata/FoxitSansItalic.cff"),
        StandardFont::Times => include_bytes!("../../fontdata/FoxitSerif.cff"),
        StandardFont::TimesBold => include_bytes!("../../fontdata/FoxitSerifBold.cff"),
        StandardFont::TimesBoldOblique => include_bytes!("../../fontdata/FoxitSerifBoldItalic.cff"),
        StandardFont::TimesOblique => include_bytes!("../../fontdata/FoxitSerifItalic.cff"),
        StandardFont::Symbol => include_bytes!("../../fontdata/FoxitSymbol.cff"),
        StandardFont::Dingbats => include_bytes!("../../fontdata/FoxitDingbats.cff"),
    }
}

/// Every standard font, in index order.
#[cfg(test)]
pub const ALL_STANDARD_FONTS: [StandardFont; 14] = [
    StandardFont::Courier,
    StandardFont::CourierBold,
    StandardFont::CourierBoldOblique,
    StandardFont::CourierOblique,
    StandardFont::Helvetica,
    StandardFont::HelveticaBold,
    StandardFont::HelveticaBoldOblique,
    StandardFont::HelveticaOblique,
    StandardFont::Times,
    StandardFont::TimesBold,
    StandardFont::TimesBoldOblique,
    StandardFont::TimesOblique,
    StandardFont::Symbol,
    StandardFont::Dingbats,
];

#[cfg(test)]
mod tests {
    // Test fixtures are fixed-size arrays with known contents.
    #![allow(clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn canonical_names_are_the_postscript_spellings() {
        assert_eq!(canonical_font_name(StandardFont::Times), "Times-Roman");
        assert_eq!(
            canonical_font_name(StandardFont::HelveticaOblique),
            "Helvetica-Oblique"
        );
    }

    /// `cfx_standardfont_unittest.cpp`'s `IsStandardFontName`.
    #[test]
    fn the_fourteen_canonical_names_are_standard_and_nothing_else_is() {
        for f in ALL_STANDARD_FONTS {
            assert!(is_standard_font_name(canonical_font_name(f).as_bytes()));
        }
        for name in [
            &b"Arial"[..],
            b"arial",
            b"Times-roman",
            b"",
            b"Helvetica-Bold-Extra",
        ] {
            assert!(
                !is_standard_font_name(name),
                "{:?} is not canonical",
                std::str::from_utf8(name)
            );
        }
    }

    /// `cfx_standardfont_unittest.cpp`'s `GetStandardFontIndex`.
    #[test]
    fn aliases_resolve_and_misses_do_not() {
        for (name, expected) in [
            (&b"Courier"[..], Some(StandardFont::Courier)),
            (b"Times-Roman", Some(StandardFont::Times)),
            (b"ZapfDingbats", Some(StandardFont::Dingbats)),
            (b"ArialMT", Some(StandardFont::Helvetica)),
            (b"Arial-BoldMT", Some(StandardFont::HelveticaBold)),
            (b"CourierNewPSMT", Some(StandardFont::Courier)),
            // Case-insensitive, which is the assertion that pins the whole
            // lookup being a `stricmp` compare rather than an ordered one.
            (b"arial", Some(StandardFont::Helvetica)),
            (b"ARIALMT", Some(StandardFont::Helvetica)),
            (b"Nonesuch", None),
            (b"", None),
        ] {
            assert_eq!(
                standard_font_index(name),
                expected,
                "{:?}",
                std::str::from_utf8(name)
            );
        }
    }

    /// `cfx_standardfont_unittest.cpp`'s `IsSymbolicFont` and `IsFixedFont`.
    #[test]
    fn symbolic_and_fixed_predicates_match_the_oracle() {
        assert!(StandardFont::Symbol.is_symbolic());
        assert!(StandardFont::Dingbats.is_symbolic());
        for f in [
            StandardFont::Courier,
            StandardFont::Helvetica,
            StandardFont::Times,
        ] {
            assert!(!f.is_symbolic());
        }
        for f in [
            StandardFont::Courier,
            StandardFont::CourierBold,
            StandardFont::CourierBoldOblique,
            StandardFont::CourierOblique,
        ] {
            assert!(f.is_fixed());
        }
        for f in [
            StandardFont::Helvetica,
            StandardFont::Times,
            StandardFont::Symbol,
        ] {
            assert!(!f.is_fixed());
        }
    }

    #[test]
    fn the_alias_table_has_exactly_eighty_nine_entries_and_all_resolve() {
        assert_eq!(ALT_FONT_NAMES.len(), 89);
        for (alias, expected) in ALT_FONT_NAMES {
            assert_eq!(
                standard_font_index(alias.as_bytes()),
                Some(*expected),
                "{alias}"
            );
        }
    }

    #[test]
    fn every_canonical_name_round_trips_through_the_alias_table() {
        for f in ALL_STANDARD_FONTS {
            let name = canonical_font_name(f);
            let back = standard_font_index(name.as_bytes())
                .unwrap_or_else(|| panic!("{name} is its own alias"));
            assert_eq!(canonical_font_name(back), name);
        }
    }

    #[test]
    fn indices_round_trip() {
        for f in ALL_STANDARD_FONTS {
            assert_eq!(StandardFont::from_index(f.index()), Some(f));
        }
        assert_eq!(StandardFont::from_index(14), None);
        assert_eq!(StandardFont::from_index(usize::MAX), None);
    }

    #[test]
    fn the_intra_family_order_is_regular_bold_bolditalic_italic() {
        // `GetStyleFromBaseFont` reads `index % 4`, and `AdjustBaseFontForStyle`
        // adds 1, 2 or 3 — both are wrong if this order ever changes.
        for family_head in [
            StandardFont::Courier,
            StandardFont::Helvetica,
            StandardFont::Times,
        ] {
            let base = family_head.index();
            assert_eq!(base % 4, 0);
            let names: Vec<&str> = (0..4)
                .filter_map(|i| StandardFont::from_index(base + i))
                .map(canonical_font_name)
                .collect();
            assert!(!names[0].contains("Bold") && !names[0].contains("Italic"));
            assert!(names[1].contains("Bold") && !names[1].contains("Italic"));
            assert!(names[2].contains("Bold"));
            assert!(names[3].contains("Oblique") || names[3].contains("Italic"));
        }
    }

    #[test]
    fn only_the_three_family_heads_are_stylable() {
        for f in ALL_STANDARD_FONTS {
            assert_eq!(
                f.is_stylable(),
                matches!(
                    f,
                    StandardFont::Courier | StandardFont::Helvetica | StandardFont::Times
                ),
                "{f:?}"
            );
        }
    }

    #[test]
    fn every_foxit_blob_is_bare_cff_of_the_expected_size() {
        // The sizes are the oracle's `std::array` bounds; a mis-extraction
        // would show here rather than as a mysteriously blank glyph.
        for (f, expected) in [
            (StandardFont::Courier, 17_597),
            (StandardFont::CourierBold, 18_055),
            (StandardFont::CourierBoldOblique, 19_151),
            (StandardFont::CourierOblique, 18_746),
            (StandardFont::Helvetica, 15_025),
            (StandardFont::HelveticaBold, 16_344),
            (StandardFont::HelveticaBoldOblique, 16_418),
            (StandardFont::HelveticaOblique, 16_339),
            (StandardFont::Times, 19_469),
            (StandardFont::TimesBold, 19_395),
            (StandardFont::TimesBoldOblique, 20_733),
            (StandardFont::TimesOblique, 21_227),
            (StandardFont::Symbol, 16_729),
            (StandardFont::Dingbats, 29_513),
        ] {
            let data = standard_font_data(f);
            assert_eq!(data.len(), expected, "{f:?}");
            // CFF major 1, minor 0, header size 4, offset size 2.
            assert_eq!(data.get(..4), Some(&[0x01, 0x00, 0x04, 0x02][..]), "{f:?}");
        }
    }

    #[test]
    fn every_foxit_blob_parses_into_a_usable_face() {
        for f in ALL_STANDARD_FONTS {
            let bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(standard_font_data(f));
            let face = crate::glyphs::Face::new(bytes, 0)
                .unwrap_or_else(|| panic!("{f:?} must be readable"));
            assert!(face.num_glyphs() > 1, "{f:?}");
            assert_eq!(face.units_per_em(), 1000, "{f:?}");
            assert!(!face.is_truetype(), "{f:?} is bare CFF, not TrueType");
        }
    }
}
