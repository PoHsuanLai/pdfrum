//! Name and style parsing: the string surgery `FindSubstFace` performs on a
//! `/BaseFont` name before it asks a font database anything.
//!
//! Five short functions, each with a quirk that changes which font a document
//! gets.

use super::standard::{canonical_font_name, standard_font_index};

/// A style word and the bits it contributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontStyle {
    /// The word, matched **exactly and case-sensitively**.
    pub name: &'static str,
    /// The style bits it sets.
    pub style: u32,
}

/// The style bits a face or a `/BaseFont` name carries.
///
/// Not a dense enum: the values are the bit positions the substitution
/// scoring reads, and `FaceInfo::similarity_score` weights bold and italic
/// separately, so the two must stay distinguishable in a single `u32`.
pub mod style_bits {
    /// No style.
    pub const NORMAL: u32 = 0;
    /// Bold, whether declared or inferred.
    pub const FORCE_BOLD: u32 = 1 << 18;
    /// Italic or oblique.
    pub const ITALIC: u32 = 1 << 6;
}

/// The style-word table, **in order** — the first match wins, which is why
/// `BoldItalic` must precede `Italic` and `Regular` must precede `Reg`.
///
/// Five entries, and the omissions matter as much as the entries: there is no
/// `Oblique`, no `Light`, no `Black`, no `Medium` and no `Semibold`, so a font
/// named `Helvetica-Light` has its "Light" read as an unrecognised token.
pub const FONT_STYLES: [FontStyle; 5] = [
    FontStyle {
        name: "Regular",
        style: style_bits::NORMAL,
    },
    FontStyle {
        name: "Reg",
        style: style_bits::NORMAL,
    },
    FontStyle {
        name: "BoldItalic",
        style: style_bits::FORCE_BOLD | style_bits::ITALIC,
    },
    FontStyle {
        name: "Italic",
        style: style_bits::ITALIC,
    },
    FontStyle {
        name: "Bold",
        style: style_bits::FORCE_BOLD,
    },
];

/// Family rewrites applied before a database is asked. Substring matches,
/// case-sensitive, first hit wins.
pub const ALT_FONT_FAMILIES: [(&str, &str); 3] = [
    ("AGaramondPro", "Adobe Garamond Pro"),
    ("BankGothicBT-Medium", "BankGothic Md BT"),
    ("ForteMT", "Forte"),
];

/// The narrow family substituted on Linux, matching the oracle's build.
pub const NARROW_FAMILY: &str = "LiberationSansNarrow";

/// Match a style word at the start or end of a name.
///
/// Comparison is **exact and case-sensitive**, and the table order decides
/// ties — so `"…BoldItalic"` matches the combined entry rather than `Italic`.
#[must_use]
pub fn style_type(name: &str, reverse: bool) -> Option<FontStyle> {
    if name.is_empty() {
        return None;
    }
    FONT_STYLES.into_iter().find(|s| {
        if s.name.len() > name.len() {
            return false;
        }
        let slice = if reverse {
            name.get(name.len() - s.name.len()..)
        } else {
            name.get(..s.name.len())
        };
        slice == Some(s.name)
    })
}

/// Strip a subset prefix: exactly six uppercase ASCII letters and a `+`.
///
/// The exactness is the point — `AB+Foo` and `ABCDEFG+Foo` both keep their
/// prefixes, because a subset tag is defined to be six letters.
#[must_use]
pub fn strip_subset_prefix(name: &[u8]) -> &[u8] {
    const LEN: usize = 6;
    if name.len() > LEN
        && name.get(LEN) == Some(&b'+')
        && name
            .get(..LEN)
            .is_some_and(|p| p.iter().all(u8::is_ascii_uppercase))
    {
        return name.get(LEN + 1..).unwrap_or(name);
    }
    name
}

/// Normalize a `/BaseFont` name for substitution (`GetSubstName`).
///
/// An **either/or** that is easy to misread: a TrueType name beginning with
/// `@` — the vertical-writing marker — loses the `@` and *keeps its spaces*;
/// every other name loses **all** its spaces and keeps nothing else. Then the
/// subset prefix goes, and finally an alias resolves to its canonical name.
#[must_use]
pub fn subst_name(name: &[u8], is_truetype: bool) -> Vec<u8> {
    let mut s: Vec<u8> = if is_truetype && name.first() == Some(&b'@') {
        name.get(1..).unwrap_or_default().to_vec()
    } else {
        name.iter().copied().filter(|c| *c != b' ').collect()
    };
    s = strip_subset_prefix(&s).to_vec();
    if let Some(f) = standard_font_index(&s) {
        s = canonical_font_name(f).as_bytes().to_vec();
    }
    s
}

/// Normalize an *installed* face name for comparison (`TT_NormalizeName`).
///
/// A second, looser subset strip that is not [`strip_subset_prefix`]: it
/// removes all spaces, hyphens and commas **first**, then truncates at a `+`
/// found at a non-zero index — so the indices have already shifted by the time
/// the `+` is looked for, and a name like `Foo-Bar+Baz` truncates at a
/// different place than the strict strip would.
#[must_use]
pub fn tt_normalize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .filter(|c| *c != ' ' && *c != '-' && *c != ',')
        .collect();
    if let Some(p) = s.find('+').filter(|p| *p != 0) {
        s.truncate(p);
    }
    s.to_ascii_lowercase()
}

/// Split a name at its first comma, canonicalizing the family half.
///
/// Note step 1 already canonicalized the *whole* name when it matched an
/// alias, so `"Arial,Bold"` arrives here as `"Helvetica-Bold"` — which has no
/// comma and takes the second arm.
#[must_use]
pub fn split_style(subst_name: &[u8]) -> (Vec<u8>, Vec<u8>, bool) {
    match subst_name.iter().position(|c| *c == b',') {
        Some(p) => {
            let mut family = subst_name.get(..p).unwrap_or_default().to_vec();
            if let Some(f) = standard_font_index(&family) {
                family = canonical_font_name(f).as_bytes().to_vec();
            }
            (
                family,
                subst_name.get(p + 1..).unwrap_or_default().to_vec(),
                true,
            )
        }
        None => (subst_name.to_vec(), Vec::new(), false),
    }
}

/// What [`parse_styles`] learned and changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedStyles {
    /// Whether the caller should **abandon** the parsed family and style and
    /// fall back to the whole name.
    pub abort: bool,
    /// Whether any token was recognised as a style word.
    pub is_style_available: bool,
    /// The weight after bold inference.
    pub weight: i32,
    /// The accumulated style bits.
    pub style: u32,
}

/// Parse a comma-separated style suffix (`ParseStyles`).
///
/// The interesting result is `abort`, which means "this name's style half is
/// not a style at all — use the whole name as the family". Three rules produce
/// it, and they are why `"Bold,Italic"` and `"BoldItalic"` behave differently:
///
/// - An **unrecognised first token** aborts.
/// - An **italic token that is not the first item** aborts — and a preceding
///   bold token is what makes it not first. So `"Bold,Italic"` aborts while
///   the single token `"BoldItalic"` yields bold and italic at weight 700.
/// - A later token when nothing has been recognised yet aborts.
///
/// And one rule produces a weight nothing else does: **`"Bold,Bold"` yields
/// 900**, because the second bold sees the first already applied.
#[must_use]
pub fn parse_styles(style_str: &[u8], mut weight: i32, mut style: u32) -> ParsedStyles {
    let mut is_style_available = false;
    if style_str.is_empty() {
        return ParsedStyles {
            abort: false,
            is_style_available,
            weight,
            style,
        };
    }

    let mut i = 0usize;
    let mut is_first_item = true;
    while i < style_str.len() {
        let rest = style_str.get(i..).unwrap_or_default();
        let token = match rest.iter().position(|c| *c == b',') {
            Some(p) => rest.get(..p).unwrap_or_default(),
            None => rest,
        };
        let parsed = std::str::from_utf8(token)
            .ok()
            .and_then(|t| style_type(t, false));

        if (i != 0 && !is_style_available) || (i == 0 && parsed.is_none()) {
            return ParsedStyles {
                abort: true,
                is_style_available,
                weight,
                style,
            };
        }
        let bits = match parsed {
            Some(s) => {
                is_style_available = true;
                s.style
            }
            None => style_bits::NORMAL,
        };

        if bits & style_bits::FORCE_BOLD != 0 {
            if style & style_bits::FORCE_BOLD != 0 {
                // Double bold: a weight no single token produces.
                weight = 900;
            } else {
                weight = 700;
                style |= style_bits::FORCE_BOLD;
            }
            is_first_item = false;
        }
        if bits & style_bits::ITALIC != 0 && bits & style_bits::FORCE_BOLD != 0 {
            style |= style_bits::ITALIC;
        } else if bits & style_bits::ITALIC != 0 {
            if !is_first_item {
                return ParsedStyles {
                    abort: true,
                    is_style_available,
                    weight,
                    style,
                };
            }
            style |= style_bits::ITALIC;
            break;
        }
        i += token.len() + 1;
    }
    ParsedStyles {
        abort: false,
        is_style_available,
        weight,
        style,
    }
}

/// Rewrite a family name before querying a database (`GetFontFamily`).
///
/// The `Script` ladder short-circuits: a family containing `Script` never
/// reaches the alias table, even when it would have matched.
#[must_use]
pub fn font_family(style: u32, family: &str) -> Option<&'static str> {
    if family.contains("Script") {
        if style & style_bits::FORCE_BOLD != 0 {
            return Some("ScriptMTBold");
        }
        if family.contains("Palace") {
            return Some("PalaceScriptMT");
        }
        if family.contains("French") {
            return Some("FrenchScriptMT");
        }
        if family.contains("FreeStyle") {
            return Some("FreeStyleScript");
        }
        return None;
    }
    ALT_FONT_FAMILIES
        .iter()
        .find(|(needle, _)| family.contains(needle))
        .map(|(_, fam)| *fam)
}

/// Is this a narrow or condensed face? The marker must appear at a **non-zero**
/// index, so a family literally called `Narrow` does not match.
#[must_use]
pub fn is_narrow_font_name(name: &str) -> bool {
    matches!(name.find("Narrow"), Some(p) if p != 0)
        || matches!(name.find("Condensed"), Some(p) if p != 0)
}

/// The one third-party family PDFium special-cases, which also clears the
/// Roman pitch bit.
#[must_use]
pub fn is_third_party_font(name: &str) -> bool {
    name == "MyriadPro"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_words_match_exactly_and_in_table_order() {
        // `BoldItalic` precedes `Italic`, so the combined entry wins.
        assert_eq!(
            style_type("BoldItalic", false).map(|s| s.style),
            Some(style_bits::FORCE_BOLD | style_bits::ITALIC)
        );
        // `Regular` precedes `Reg`, so a full "Regular" is not read as "Reg".
        assert_eq!(
            style_type("Regular", false).map(|s| s.name),
            Some("Regular")
        );
        assert_eq!(style_type("Reg", false).map(|s| s.name), Some("Reg"));
        // Case-sensitive.
        assert_eq!(style_type("bold", false), None);
        assert_eq!(style_type("BOLD", false), None);
        // The words that are *not* in the table.
        for absent in ["Oblique", "Light", "Black", "Medium", "Semibold"] {
            assert_eq!(style_type(absent, false), None, "{absent}");
        }
        assert_eq!(style_type("", false), None);
    }

    #[test]
    fn reverse_matching_reads_the_suffix() {
        assert_eq!(
            style_type("HelveticaBold", true).map(|s| s.name),
            Some("Bold")
        );
        assert_eq!(style_type("HelveticaBold", false), None);
        assert_eq!(
            style_type("BoldHelvetica", false).map(|s| s.name),
            Some("Bold")
        );
    }

    #[test]
    fn the_subset_prefix_is_exactly_six_uppercase_letters_and_a_plus() {
        assert_eq!(strip_subset_prefix(b"ABCDEF+Swiss"), b"Swiss");
        assert_eq!(strip_subset_prefix(b"CHEESE+Swiss"), b"Swiss");
        // Five letters, seven letters, lowercase, no plus: all kept whole.
        assert_eq!(strip_subset_prefix(b"ABCDE+Swiss"), b"ABCDE+Swiss");
        assert_eq!(strip_subset_prefix(b"ABCDEFG+Swiss"), b"ABCDEFG+Swiss");
        assert_eq!(strip_subset_prefix(b"abcdef+Swiss"), b"abcdef+Swiss");
        assert_eq!(strip_subset_prefix(b"ABCDEF-Swiss"), b"ABCDEF-Swiss");
        // Exactly seven bytes is not "> 6" plus a name.
        assert_eq!(strip_subset_prefix(b"ABCDEF+"), b"");
        assert_eq!(strip_subset_prefix(b""), b"");
    }

    #[test]
    fn subst_name_is_an_either_or_on_the_at_prefix() {
        // A TrueType `@`-name keeps its spaces.
        assert_eq!(subst_name(b"@MS Mincho", true), b"MS Mincho");
        // Everything else loses all of them.
        assert_eq!(subst_name(b"MS Mincho", true), b"MSMincho");
        assert_eq!(subst_name(b"MS Mincho", false), b"MSMincho");
        // A non-TrueType `@`-name takes the space-stripping arm, `@` included.
        assert_eq!(subst_name(b"@MS Mincho", false), b"@MSMincho");
    }

    #[test]
    fn subst_name_strips_then_canonicalizes() {
        assert_eq!(subst_name(b"ABCDEF+ArialMT", false), b"Helvetica");
        assert_eq!(subst_name(b"Arial MT", false), b"Helvetica");
        assert_eq!(subst_name(b"NoSuchFont", false), b"NoSuchFont");
    }

    #[test]
    fn tt_normalize_removes_punctuation_before_finding_the_plus() {
        assert_eq!(tt_normalize("Times New Roman"), "timesnewroman");
        assert_eq!(tt_normalize("Helvetica-Bold"), "helveticabold");
        assert_eq!(tt_normalize("Arial,Italic"), "arialitalic");
        // The `+` is found *after* the punctuation is gone.
        assert_eq!(tt_normalize("ABCDEF+Foo"), "abcdef");
        // A leading `+` is at index 0 and does not truncate.
        assert_eq!(tt_normalize("+Foo"), "+foo");
    }

    #[test]
    fn tt_normalize_and_strip_subset_prefix_disagree() {
        // The brief's §4.5 case: the two strips are genuinely different rules.
        let name = "ABCDEF+Foo Bar-Baz";
        assert_eq!(strip_subset_prefix(name.as_bytes()), b"Foo Bar-Baz");
        assert_eq!(tt_normalize(name), "abcdef");
    }

    #[test]
    fn splitting_at_a_comma_canonicalizes_the_family() {
        let (family, style, has_comma) = split_style(b"Arial,Bold");
        assert_eq!(family, b"Helvetica");
        assert_eq!(style, b"Bold");
        assert!(has_comma);

        let (family, style, has_comma) = split_style(b"Helvetica-Bold");
        assert_eq!(family, b"Helvetica-Bold");
        assert!(style.is_empty());
        assert!(!has_comma);
    }

    // ---- ParseStyles, the aborts ------------------------------------------

    #[test]
    fn bold_comma_italic_aborts_but_bolditalic_does_not() {
        // The single most surprising rule in the whole ladder.
        let split = parse_styles(b"Bold,Italic", 400, 0);
        assert!(split.abort, "`Bold,Italic` must abort");

        let joined = parse_styles(b"BoldItalic", 400, 0);
        assert!(!joined.abort);
        assert_eq!(joined.weight, 700);
        assert_eq!(joined.style, style_bits::FORCE_BOLD | style_bits::ITALIC);
    }

    #[test]
    fn bold_comma_bold_yields_nine_hundred() {
        let r = parse_styles(b"Bold,Bold", 400, 0);
        assert!(!r.abort);
        assert_eq!(r.weight, 900);
    }

    #[test]
    fn an_unrecognised_first_token_aborts() {
        for s in [&b"Nonesuch"[..], b"Light", b"Oblique", b"Condensed"] {
            assert!(
                parse_styles(s, 400, 0).abort,
                "{:?}",
                std::str::from_utf8(s)
            );
        }
    }

    #[test]
    fn an_unrecognised_later_token_is_tolerated_once_a_style_is_known() {
        let r = parse_styles(b"Bold,Nonesuch", 400, 0);
        assert!(!r.abort);
        assert!(r.is_style_available);
        assert_eq!(r.weight, 700);
    }

    #[test]
    fn a_later_token_with_nothing_recognised_yet_aborts() {
        // `Regular` *is* recognised, so this does not abort...
        assert!(!parse_styles(b"Regular,Bold", 400, 0).abort);
        // ...but a first token that is only tolerated leaves nothing available.
        assert!(parse_styles(b"Zzz,Bold", 400, 0).abort);
    }

    #[test]
    fn italic_alone_is_first_and_therefore_fine() {
        let r = parse_styles(b"Italic", 400, 0);
        assert!(!r.abort);
        assert_eq!(r.style, style_bits::ITALIC);
        assert_eq!(r.weight, 400);
    }

    #[test]
    fn an_empty_style_string_does_nothing() {
        let r = parse_styles(b"", 500, style_bits::ITALIC);
        assert!(!r.abort);
        assert!(!r.is_style_available);
        assert_eq!(r.weight, 500);
        assert_eq!(r.style, style_bits::ITALIC);
    }

    #[test]
    fn an_incoming_bold_style_makes_a_bold_token_double_bold() {
        let r = parse_styles(b"Bold", 400, style_bits::FORCE_BOLD);
        assert_eq!(r.weight, 900);
    }

    // ---- family rewriting -------------------------------------------------

    #[test]
    fn the_script_ladder_has_four_outcomes_and_a_none() {
        assert_eq!(
            font_family(style_bits::FORCE_BOLD, "AnyScript"),
            Some("ScriptMTBold")
        );
        assert_eq!(font_family(0, "PalaceScript"), Some("PalaceScriptMT"));
        assert_eq!(font_family(0, "FrenchScript"), Some("FrenchScriptMT"));
        assert_eq!(font_family(0, "FreeStyleScript"), Some("FreeStyleScript"));
        assert_eq!(font_family(0, "PlainScript"), None);
        // Bold wins over every more specific arm.
        assert_eq!(
            font_family(style_bits::FORCE_BOLD, "PalaceScript"),
            Some("ScriptMTBold")
        );
    }

    #[test]
    fn the_three_alt_families_rewrite_by_substring() {
        assert_eq!(font_family(0, "AGaramondPro"), Some("Adobe Garamond Pro"));
        assert_eq!(
            font_family(0, "XxAGaramondProYy"),
            Some("Adobe Garamond Pro")
        );
        assert_eq!(
            font_family(0, "BankGothicBT-Medium"),
            Some("BankGothic Md BT")
        );
        assert_eq!(font_family(0, "ForteMT"), Some("Forte"));
        assert_eq!(font_family(0, "Helvetica"), None);
        // Case-sensitive.
        assert_eq!(font_family(0, "fortemt"), None);
    }

    #[test]
    fn a_script_family_never_reaches_the_alias_table() {
        // `ForteMT` would rewrite, but `Script` short-circuits first.
        assert_eq!(font_family(0, "ForteMTScript"), None);
    }

    #[test]
    fn narrow_and_condensed_must_appear_at_a_non_zero_index() {
        assert!(is_narrow_font_name("ArialNarrow"));
        assert!(is_narrow_font_name("HelveticaCondensed"));
        // At index 0 the marker does not count.
        assert!(!is_narrow_font_name("Narrow"));
        assert!(!is_narrow_font_name("Condensed"));
        assert!(!is_narrow_font_name("Helvetica"));
    }

    #[test]
    fn only_myriadpro_is_the_third_party_font() {
        assert!(is_third_party_font("MyriadPro"));
        assert!(!is_third_party_font("MyriadPro-Bold"));
        assert!(!is_third_party_font("myriadpro"));
    }
}
