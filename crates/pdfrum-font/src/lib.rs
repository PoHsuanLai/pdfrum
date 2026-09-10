#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
// Every byte reaching this crate came from an untrusted font program or font
// dictionary: index with `get()`, never with `[]`.
#![warn(clippy::indexing_slicing)]
// Font units are integers carried as floats, character codes are `u8`s cut out
// of wider values, and the metric normalizers are the C++'s own integer
// arithmetic. Each conversion below is pinned by a test.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

// Every module is private and the `pub use` block below is the whole surface.
// A type a sibling crate names is re-exported here; a function
// only this crate uses is not.
mod cid;
mod descriptor;
mod encoding;
mod error;
mod fallback;
mod glyphs;
mod ids;
mod load;
mod names;
mod simple;
mod subst;
#[cfg(test)]
mod test_resolve;
#[cfg(test)]
mod testfonts;
mod tounicode;
mod type3;
mod widths;

pub use cid::{CidTransform, Type0Font, cid_transform_to_float};
pub use encoding::FaceEncoding;
pub use encoding::adobe_name_from_unicode;
pub use error::Error;
pub use fallback::GlyphFallback;
pub use glyphs::{
    Charmap, CharmapId, Face, GlyphCache, GlyphKey, GlyphSource, SynthGlyph, em_adjust,
};
pub use ids::{CharCode, Cid, FontFlags, FontId, Gid};
pub use simple::SimpleFont;
pub use subst::{
    Charset, StandardFont, SubstFont, SubstitutionOptions, canonical_font_name,
    charset_from_unicode,
};

pub use load::{CharItem, Font, FontCache, load, load_with_options};
pub use pdfrum_type1::FontFile as Type1FontFile;
pub use tounicode::invert_to_unicode;
pub use type3::{MAX_TYPE3_DEPTH, Type3Font};

/// A Type 1 program as the `/FontFile` stream a PDF writer stores, with the
/// ISO 32000-1 table 127 lengths that partition it.
///
/// See [`pdfrum_type1::font_file`]: a PFB is unwrapped into the raw program
/// its records carry, because the container's framing is not part of the font.
#[must_use]
pub fn type1_font_file(bytes: &[u8]) -> Type1FontFile {
    pdfrum_type1::font_file(bytes)
}

#[cfg(test)]
mod tests {
    // Test expectations are exact values by design.
    #![allow(clippy::float_cmp)]

    use super::names;
    use super::*;
    use crate::load::wants_chinese_cid_rescue;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object};

    fn simple_dict(subtype: &str, base: &str) -> Dict {
        Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from(subtype))),
            (names::BASE_FONT.clone(), Object::Name(Name::from(base))),
        ])
    }

    #[test]
    fn a_missing_subtype_loads_as_a_type1_font() {
        let dict = Dict::from_pairs([(
            names::BASE_FONT.clone(),
            Object::Name(Name::from("Helvetica")),
        )]);
        let font = load(
            &dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a simple font always constructs");
        assert!(matches!(font, Font::Simple(_)));
    }

    #[test]
    fn garbage_subtypes_also_load_as_type1() {
        for subtype in ["Type1", "MMType1", "NotAFontType", ""] {
            let font = load(
                &simple_dict(subtype, "Helvetica"),
                &NoResolve,
                &FontCache::new(),
                &Limits::default(),
                &mut Diagnostics::default(),
            );
            assert!(matches!(font, Some(Font::Simple(_))), "{subtype}");
        }
    }

    #[test]
    fn a_type3_subtype_loads_as_type3() {
        let font = load(
            &simple_dict("Type3", ""),
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("Type3 always constructs");
        assert!(font.type3().is_some());
        // A Type3 font has no outlines at all, by construction.
        assert!(font.glyph_path(Gid(0)).is_none());
    }

    #[test]
    fn a_type0_font_without_descendants_fails_to_load() {
        // The one font kind whose load can fail, and the reason the public
        // entry point returns `Option`.
        assert!(
            load(
                &simple_dict("Type0", "Foo"),
                &NoResolve,
                &FontCache::new(),
                &Limits::default(),
                &mut Diagnostics::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn the_chinese_name_rescue_reroutes_a_truetype_font() {
        // 宋体 in GBK, with no descriptor at all.
        let mut name = vec![0xcb, 0xce, 0xcc, 0xe5];
        name.extend_from_slice(b"-Extra");
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("TrueType"))),
            (names::BASE_FONT.clone(), Object::Name(Name::new(name))),
        ]);
        assert!(wants_chinese_cid_rescue(&dict, &NoResolve));
    }

    #[test]
    fn the_chinese_rescue_does_not_fire_for_an_embedded_font() {
        let desc = Dict::from_pairs([(
            names::FONT_FILE2.clone(),
            Object::Ref(pdfrum_object::ObjRef::new(7, 0)),
        )]);
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("TrueType"))),
            (
                names::BASE_FONT.clone(),
                Object::Name(Name::new(vec![0xcb, 0xce, 0xcc, 0xe5])),
            ),
            (names::FONT_DESCRIPTOR.clone(), Object::Dict(desc)),
        ]);
        assert!(!wants_chinese_cid_rescue(&dict, &NoResolve));
    }

    #[test]
    fn a_name_shorter_than_four_bytes_never_matches() {
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("TrueType"))),
            (names::BASE_FONT.clone(), Object::Name(Name::from("ab"))),
        ]);
        assert!(!wants_chinese_cid_rescue(&dict, &NoResolve));
    }

    #[test]
    fn the_standard_fourteen_all_load_and_name_themselves() {
        let cache = FontCache::new();
        for which in subst::ALL_STANDARD_FONTS {
            let font = Font::load_standard(which, &cache);
            assert_eq!(
                font.base_font_name(),
                subst::canonical_font_name(which).as_bytes(),
                "{which:?}"
            );
            assert!(font.glyph_path(Gid(1)).is_some() || font.glyph_path(Gid(2)).is_some());
        }
    }

    #[test]
    fn a_standard_font_round_trips_ascii_both_ways() {
        let font = Font::load_standard(StandardFont::Times, &FontCache::new());
        for ch in "The quick brown fox! 0123".chars() {
            let Some(code) = font.char_code_from_unicode(ch) else {
                panic!("{ch:?} should be encodable in a Latin font");
            };
            assert_eq!(
                font.unicode_from_charcode(code).as_slice(),
                [ch],
                "{ch:?} did not round-trip"
            );
        }
    }

    #[test]
    fn char_code_from_unicode_declines_what_the_font_cannot_express() {
        let font = Font::load_standard(StandardFont::Helvetica, &FontCache::new());
        for ch in ['\u{4e00}', '\u{3042}', '\u{10000}'] {
            assert_eq!(font.char_code_from_unicode(ch), None, "{ch:?}");
        }
    }

    #[test]
    fn append_char_writes_one_byte_for_a_simple_font() {
        let font = Font::load_standard(StandardFont::Helvetica, &FontCache::new());
        let mut out = Vec::new();
        for ch in "Hello, world!".chars() {
            let code = font
                .char_code_from_unicode(ch)
                .unwrap_or_else(|| panic!("{ch:?} is encodable"));
            font.append_char(&mut out, code);
        }
        assert_eq!(out, b"Hello, world!");
    }

    #[test]
    fn append_char_writes_two_bytes_for_an_identity_composite_font() {
        // A composite font's codespace decides the width, which is the reason
        // `append_char` is a method rather than a byte cast at the call site.
        let descendant = Dict::from_pairs([
            (
                names::SUBTYPE.clone(),
                Object::Name(Name::from("CIDFontType0")),
            ),
            (names::BASE_FONT.clone(), Object::Name(Name::from("Test"))),
        ]);
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
            (
                names::ENCODING.clone(),
                Object::Name(Name::from("Identity-H")),
            ),
            (
                names::DESCENDANT_FONTS.clone(),
                Object::Array(pdfrum_object::Array::of([Object::Dict(descendant)])),
            ),
        ]);
        let font = load(
            &dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a Type0 font with one descendant loads");

        let mut out = Vec::new();
        font.append_char(&mut out, CharCode(0x0041));
        assert_eq!(out, vec![0x00, 0x41]);
    }

    #[test]
    fn the_courier_widths_are_the_fixed_six_hundred() {
        let font = Font::load_standard(StandardFont::CourierBold, &FontCache::new());
        for ch in "iWm ".chars() {
            let code = font.char_code_from_unicode(ch).expect("encodable");
            assert_eq!(font.char_width(code), 600.0, "{ch:?}");
        }
    }

    #[test]
    fn font_ids_are_distinct() {
        let cache = FontCache::new();
        let a = cache.next_id();
        let b = cache.next_id();
        assert_ne!(a, b);
    }

    #[test]
    fn a_non_embedded_truetype_falls_back_to_arial_for_an_unmapped_code() {
        // `bug_1442723`: `/Subtype /TrueType /BaseFont /Symbol` with no
        // `/ToUnicode`. Codes the ladder cannot place take Arial, not `.notdef`.
        let font = load(
            &simple_dict("TrueType", "Symbol"),
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a simple font always constructs");
        assert!(!font.should_use_own_glyph(None));
        assert!(!font.should_use_own_glyph(Some(Gid(0))));
        let fb = font.glyph_fallback().expect("Arial is a built-in stand-in");
        assert!(fb.gid(&['A'], CharCode(u32::from(b'A'))).is_some());
    }

    #[test]
    fn a_truetype_symbol_code_keeps_its_encoding_unicode() {
        // `bug_1442723`: `/Subtype /TrueType /BaseFont /Symbol`, Flags
        // symbolic, no `/Encoding`. The ladder's Unicode table stays 0
        // because `MsSymbol` has no glyph names; the encoding table still
        // names U+2297 for 0xD9, which the Arial stand-in looks up.
        let desc = Dict::from_pairs([(names::FLAGS.clone(), Object::Int(6))]);
        let dict = Dict::from_pairs([
            (
                names::SUBTYPE.clone(),
                Object::Name(Name::from("TrueType")),
            ),
            (
                names::BASE_FONT.clone(),
                Object::Name(Name::from("Symbol")),
            ),
            (names::FONT_DESCRIPTOR.clone(), Object::Dict(desc)),
        ]);
        let font = load(
            &dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a simple font always constructs");
        let chars = font.unicode_from_charcode(CharCode(0xD9));
        assert_eq!(chars.as_slice(), &['\u{2227}']);
    }
}

#[cfg(test)]
mod send_sync {
    //! Every public type is `Send + Sync`, so rendering pages in parallel
    //! with `rayon` needs no wrapper. A compile failure here is the whole
    //! test.

    use super::*;
    use pdfrum_object::ObjRef;
    use std::sync::Arc;

    const fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn every_public_type_is_send_and_sync() {
        assert_send_sync::<Font>();
        assert_send_sync::<CharItem>();
        assert_send_sync::<SimpleFont>();
        assert_send_sync::<Type0Font>();
        assert_send_sync::<Type3Font>();
        assert_send_sync::<FontCache>();
        assert_send_sync::<GlyphCache>();
        assert_send_sync::<GlyphKey>();
        assert_send_sync::<GlyphSource>();
        assert_send_sync::<crate::tounicode::ToUnicode>();
        assert_send_sync::<crate::descriptor::FontDescriptor>();
        assert_send_sync::<crate::ids::GlyphName>();
        assert_send_sync::<SubstFont>();
        assert_send_sync::<SubstitutionOptions>();
        assert_send_sync::<crate::widths::CidWidths>();
        assert_send_sync::<crate::widths::VerticalMetrics>();
        assert_send_sync::<crate::cid::CidToGid>();
        assert_send_sync::<Error>();
        assert_send_sync::<subst::FontRequest>();
        assert_send_sync::<subst::Substitution>();
        assert_send_sync::<subst::TestFontDb>();
        assert_send_sync::<subst::SystemFontDb>();
        assert_send_sync::<FontCache>();
    }

    /// The cache exists so a font is loaded once per reference, and so every
    /// later ask gets *that* instance: text extraction's duplicate
    /// suppression compares fonts by pointer, so a fresh instance per page
    /// would silently stop it firing.
    #[test]
    fn one_reference_loads_once_and_shares_the_instance() {
        let cache = FontCache::new();
        let reference = ObjRef::new(7, 0);
        let mut loads = 0;

        let first = cache
            .get_or_load(reference, || {
                loads += 1;
                Some(Font::load_standard(StandardFont::Helvetica, &cache))
            })
            .expect("the standard font always loads");
        let second = cache
            .get_or_load(reference, || {
                loads += 1;
                Some(Font::load_standard(StandardFont::Helvetica, &cache))
            })
            .expect("the cached font is still there");

        assert_eq!(loads, 1, "the second ask must not reach the loader");
        assert!(
            Arc::ptr_eq(&first, &second),
            "both asks must yield one instance"
        );
    }

    /// A different reference is a different font, and `None` is as cacheable
    /// an answer as a font: a dictionary that will not load is stable.
    #[test]
    fn a_second_reference_loads_separately_and_a_failure_is_cached() {
        let cache = FontCache::new();
        let mut loads = 0;
        let mut load = |cache: &FontCache, reference| {
            cache.get_or_load(reference, || {
                loads += 1;
                Some(Font::load_standard(StandardFont::Helvetica, cache))
            })
        };

        let first = load(&cache, ObjRef::new(7, 0)).expect("loads");
        let second = load(&cache, ObjRef::new(8, 0)).expect("loads");
        assert_eq!(loads, 2, "two references are two fonts");
        assert!(!Arc::ptr_eq(&first, &second));

        let mut failures = 0;
        let missing = ObjRef::new(9, 0);
        for _ in 0..2 {
            assert!(
                cache
                    .get_or_load(missing, || {
                        failures += 1;
                        None
                    })
                    .is_none()
            );
        }
        assert_eq!(failures, 1, "a failure is derived once, not per page");
    }

    /// The cache is what many threads share, so two threads asking at once
    /// must both come away with a font and neither must panic.
    #[test]
    fn threads_sharing_one_cache_all_get_a_font() {
        let cache = Arc::new(FontCache::new());
        let reference = ObjRef::new(7, 0);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let cache = Arc::clone(&cache);
                    scope.spawn(move || {
                        cache
                            .get_or_load(reference, || {
                                Some(Font::load_standard(StandardFont::Helvetica, &cache))
                            })
                            .is_some()
                    })
                })
                .collect();
            for handle in handles {
                assert!(handle.join().expect("no thread panics"));
            }
        });
        // Whoever inserted first is the shared instance from then on, so a
        // ninth ask after the race reaches no loader at all.
        let after = cache.get_or_load(reference, || panic!("must be cached by now"));
        assert!(after.is_some());
    }
}
