//! Composite-font loading and the CID glyph ladder.

// Test expectations are exact values by design.
#![allow(clippy::float_cmp)]
// Test fixtures are fixed-size arrays with known contents.
#![allow(clippy::indexing_slicing)]

use super::*;
use crate::test_resolve::TestStore;
use crate::testfonts;
use crate::{FontFlags, GlyphSource};
use pdfrum_object::{Array, Name, NoResolve};

fn dict_of(pairs: Vec<(&Name, Object)>) -> Dict {
    Dict::from_pairs(pairs.into_iter().map(|(k, v)| (k.clone(), v)))
}

fn type0(encoding: Object, descendant: Dict, extra: Vec<(&Name, Object)>) -> Dict {
    let mut pairs = vec![
        (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
        (names::ENCODING.clone(), encoding),
        (
            names::DESCENDANT_FONTS.clone(),
            Object::Array(Array::of([Object::Dict(descendant)])),
        ),
    ];
    pairs.extend(extra.into_iter().map(|(k, v)| (k.clone(), v)));
    Dict::from_pairs(pairs)
}

fn descendant(subtype: &str, base: &str, extra: Vec<(&Name, Object)>) -> Dict {
    let mut pairs = vec![
        (names::SUBTYPE.clone(), Object::Name(Name::from(subtype))),
        (names::BASE_FONT.clone(), Object::Name(Name::from(base))),
    ];
    pairs.extend(extra.into_iter().map(|(k, v)| (k.clone(), v)));
    Dict::from_pairs(pairs)
}

fn load_it(dict: &Dict) -> Result<Type0Font, Error> {
    load_with(dict, &NoResolve)
}

fn load_with(dict: &Dict, r: &impl Resolve) -> Result<Type0Font, Error> {
    load(
        dict,
        r,
        &FontCache::new(),
        &SubstitutionOptions::default(),
        &Limits::default(),
        &mut Diagnostics::default(),
    )
}

// ---------------------------------------------------------------------------
// The four ways loading fails — the reason this font kind returns `Result`.
// ---------------------------------------------------------------------------

#[test]
fn a_missing_descendant_fonts_fails() {
    let d = Dict::from_pairs([
        (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
        (
            names::ENCODING.clone(),
            Object::Name(Name::from("Identity-H")),
        ),
    ]);
    assert_eq!(load_it(&d).err(), Some(Error::BadDescendantFonts));
}

#[test]
fn a_descendant_fonts_of_the_wrong_length_fails() {
    for count in [0usize, 2] {
        let kids: Vec<Object> = (0..count).map(|_| Object::Dict(Dict::new())).collect();
        let d = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
            (
                names::ENCODING.clone(),
                Object::Name(Name::from("Identity-H")),
            ),
            (
                names::DESCENDANT_FONTS.clone(),
                Object::Array(Array::of(kids)),
            ),
        ]);
        assert_eq!(
            load_it(&d).err(),
            Some(Error::BadDescendantFonts),
            "count {count}"
        );
    }
}

#[test]
fn a_non_dictionary_descendant_fails() {
    let d = Dict::from_pairs([
        (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
        (
            names::ENCODING.clone(),
            Object::Name(Name::from("Identity-H")),
        ),
        (
            names::DESCENDANT_FONTS.clone(),
            Object::Array(Array::of([Object::Int(7)])),
        ),
    ]);
    assert_eq!(load_it(&d).err(), Some(Error::BadDescendantFonts));
}

#[test]
fn a_missing_encoding_fails() {
    let d = Dict::from_pairs([
        (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
        (
            names::DESCENDANT_FONTS.clone(),
            Object::Array(Array::of([Object::Dict(Dict::new())])),
        ),
    ]);
    assert_eq!(load_it(&d).err(), Some(Error::BadCidEncoding));
}

#[test]
fn an_encoding_that_is_neither_a_name_nor_a_stream_fails() {
    for value in [
        Object::Int(3),
        Object::Dict(Dict::new()),
        Object::Array(Array::new()),
        Object::Null,
    ] {
        let d = type0(value.clone(), Dict::new(), vec![]);
        assert_eq!(load_it(&d).err(), Some(Error::BadCidEncoding), "{value:?}");
    }
}

// ---------------------------------------------------------------------------
// `cpdf_cidfont_unittest.cpp`'s `Bug920636` — the CourierStd rescue.
// ---------------------------------------------------------------------------

#[test]
fn the_courier_std_rescue_offsets_every_code_by_thirty_one() {
    // `/Identity−H` with a **U+2212 MINUS SIGN**, not a hyphen: a deliberately
    // malformed name that fails the predefined lookup, leaving the CMap on its
    // Identity fallback with no collection — which is what drives the ladder
    // all the way to the `+= 31` rescue.
    let mut name = b"Identity".to_vec();
    name.extend_from_slice(&[0xE2, 0x88, 0x92]); // U+2212 in UTF-8
    name.push(b'H');

    let d = type0(
        Object::Name(Name::new(name)),
        descendant("CIDFontType0", "CourierStd", vec![]),
        vec![],
    );
    let f = load_it(&d).expect("the font still constructs");
    assert!(f.adobe_courier_std);
    assert_eq!(f.charset, pdfrum_cmap::CidSet::Unknown);

    for (code, expected) in [(0u32, 31u16), (256, 287), (34661, 34692)] {
        let (gid, vertical) = f.glyph_from_charcode(CharCode(code));
        assert_eq!(gid, Some(Gid(expected)), "code {code} should be code + 31");
        assert!(!vertical);
    }
}

#[test]
fn all_four_courier_std_spellings_are_recognised() {
    for name in [
        "CourierStd",
        "CourierStd-Bold",
        "CourierStd-BoldOblique",
        "CourierStd-Oblique",
    ] {
        let d = type0(
            Object::Name(Name::from("Identity-H")),
            descendant("CIDFontType0", name, vec![]),
            vec![],
        );
        assert!(load_it(&d).expect("loads").adobe_courier_std, "{name}");
    }
    // And a near miss is not.
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant("CIDFontType0", "CourierStd-Italic", vec![]),
        vec![],
    );
    assert!(!load_it(&d).expect("loads").adobe_courier_std);
}

// ---------------------------------------------------------------------------
// The embedded half.
// ---------------------------------------------------------------------------

fn embedded_descendant(store: &mut TestStore, subtype: &str, extra: Vec<(&Name, Object)>) -> Dict {
    let stream = store.add_stream(testfonts::load("tt_unicode_31.ttf"));
    let desc = dict_of(vec![(names::FONT_FILE2, stream)]);
    let mut pairs = vec![(names::FONT_DESCRIPTOR, Object::Dict(desc))];
    pairs.extend(extra);
    descendant(subtype, "Embedded", pairs)
}

#[test]
fn an_embedded_cidfonttype0_uses_its_cid_as_its_glyph_index() {
    let mut store = TestStore::new();
    let desc = embedded_descendant(&mut store, "CIDFontType0", vec![]);
    let d = type0(Object::Name(Name::from("Identity-H")), desc, vec![]);
    let f = load_with(&d, &store).expect("loads");
    assert!(f.embedded);
    assert_eq!(f.kind, CidFontKind::Type1);
    // Identity-H maps a two-byte code to itself.
    assert_eq!(f.glyph_from_charcode(CharCode(0x0001)).0, Some(Gid(1)));
    assert_eq!(f.glyph_from_charcode(CharCode(0x0005)).0, Some(Gid(5)));
}

#[test]
fn an_embedded_cidfonttype2_with_a_predefined_cmap_also_uses_cid_as_gid() {
    let mut store = TestStore::new();
    let desc = embedded_descendant(&mut store, "CIDFontType2", vec![]);
    let d = type0(Object::Name(Name::from("Identity-H")), desc, vec![]);
    let f = load_with(&d, &store).expect("loads");
    assert_eq!(f.kind, CidFontKind::TrueType);
    assert!(f.cmap.has_no_direct_table(), "Identity-H is predefined");
    assert_eq!(f.glyph_from_charcode(CharCode(0x0001)).0, Some(Gid(1)));
}

#[test]
fn a_cid_to_gid_stream_is_a_big_endian_u16_table() {
    // CID 0 -> 0x0102, CID 1 -> 0x0304.
    let mut store = TestStore::new();
    let table = store.add_stream(vec![0x01, 0x02, 0x03, 0x04]);
    let desc = embedded_descendant(
        &mut store,
        "CIDFontType2",
        vec![(names::CID_TO_GID_MAP, table)],
    );
    let d = type0(Object::Name(Name::from("Identity-H")), desc, vec![]);
    let f = load_with(&d, &store).expect("loads");
    assert!(matches!(f.cid_to_gid, CidToGid::Stream(_)));
    assert_eq!(f.glyph_from_charcode(CharCode(0)).0, Some(Gid(0x0102)));
    assert_eq!(f.glyph_from_charcode(CharCode(1)).0, Some(Gid(0x0304)));
    // Past the table's end: nothing, not a wrapped read.
    assert_eq!(f.glyph_from_charcode(CharCode(2)).0, None);
    assert_eq!(f.glyph_from_charcode(CharCode(u16::MAX.into())).0, None);
}

#[test]
fn an_indirect_cid_to_gid_identity_still_reads_as_the_identity_mapping() {
    // `GetDirectObjectFor` resolves the reference before looking at the type
    // (`core/fpdfapi/parser/cpdf_dictionary.cpp:89-93`), so the name behind
    // `/CIDToGIDMap 1 0 R` reaches the `IsName()` test in
    // `core/fpdfapi/font/cpdf_cidfont.cpp:507-518` unchanged.
    let mut store = TestStore::new();
    let name = store.add(Object::Name(Name::from("Identity")));
    let desc = embedded_descendant(
        &mut store,
        "CIDFontType2",
        vec![(names::CID_TO_GID_MAP, name)],
    );
    let d = type0(Object::Name(Name::from("Identity-H")), desc, vec![]);
    let f = load_with(&d, &store).expect("loads");
    assert_eq!(f.cid_to_gid, CidToGid::Identity);
}

#[test]
fn cid_to_gid_identity_needs_an_embedded_program() {
    // Named `/Identity` *without* a program is not the Identity mapping — the
    // C++ guards the assignment on `font_file_`.
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant(
            "CIDFontType2",
            "NotEmbedded",
            vec![(names::CID_TO_GID_MAP, Object::Name(Name::from("Identity")))],
        ),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    assert_ne!(f.cid_to_gid, CidToGid::Identity);
}

// ---------------------------------------------------------------------------
// Widths and vertical metrics.
// ---------------------------------------------------------------------------

#[test]
fn widths_come_from_w_and_dw() {
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant(
            "CIDFontType0",
            "Test",
            vec![
                (names::DW, Object::Int(1234)),
                (
                    names::W,
                    Object::Array(Array::of([
                        Object::Int(1),
                        Object::Int(3),
                        Object::Int(500),
                    ])),
                ),
            ],
        ),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    assert_eq!(f.char_width(CharCode(2)), 500.0);
    assert_eq!(f.char_width(CharCode(9)), 1234.0);
}

#[test]
fn a_vertical_cmap_loads_vertical_metrics() {
    let d = type0(
        Object::Name(Name::from("Identity-V")),
        descendant(
            "CIDFontType0",
            "Test",
            vec![(
                names::W2,
                Object::Array(Array::of([
                    Object::Int(1),
                    Object::Int(3),
                    Object::Int(-900),
                    Object::Int(450),
                    Object::Int(800),
                ])),
            )],
        ),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    assert!(f.is_vertical());
    assert!(f.vertical.is_some());
    assert_eq!(f.vert_width(CharCode(2)), -900.0);
    assert_eq!(f.vert_origin(CharCode(2)), (450.0, 800.0));
    // A CID with no record takes half the horizontal width and DW2[0].
    assert_eq!(f.vert_origin(CharCode(9)), (500.0, 880.0));
}

#[test]
fn a_horizontal_cmap_loads_no_vertical_metrics_at_all() {
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant("CIDFontType0", "Test", vec![]),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    assert!(!f.is_vertical());
    assert!(f.vertical.is_none());
    // The accessors still answer, with the ISO defaults.
    assert_eq!(f.vert_width(CharCode(1)), -1000.0);
    assert_eq!(f.vert_origin(CharCode(1)).1, 880.0);
}

// ---------------------------------------------------------------------------
// Decoding.
// ---------------------------------------------------------------------------

#[test]
fn identity_h_decodes_two_bytes_at_a_time() {
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant("CIDFontType0", "Test", vec![]),
        vec![],
    );
    let font = crate::Font::Type0(Box::new(load_it(&d).expect("loads")));
    let items: Vec<_> = font.decode(&[0x00, 0x41, 0x00, 0x42]).collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].code.0, 0x0041);
    assert_eq!(items[1].code.0, 0x0042);
    assert_eq!(items[0].cid, Some(Cid(0x41)));
}

#[test]
fn a_truncated_final_code_does_not_loop_forever() {
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant("CIDFontType0", "Test", vec![]),
        vec![],
    );
    let font = crate::Font::Type0(Box::new(load_it(&d).expect("loads")));
    // Three bytes under a two-byte scheme: the trailing byte must terminate
    // iteration rather than stalling the offset.
    let items: Vec<_> = font.decode(&[0x00, 0x41, 0x00]).collect();
    assert!(items.len() <= 2);
}

// ---------------------------------------------------------------------------
// The collection.
// ---------------------------------------------------------------------------

#[test]
fn cid_system_info_supplies_a_collection_the_cmap_did_not() {
    let info = dict_of(vec![(
        names::ORDERING,
        Object::Str(pdfrum_object::PdfString::literal(b"Japan1")),
    )]);
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant(
            "CIDFontType0",
            "Test",
            vec![(names::CID_SYSTEM_INFO, Object::Dict(info))],
        ),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    assert_eq!(f.charset, pdfrum_cmap::CidSet::Japan1);
}

#[test]
fn a_predefined_cjk_cmap_supplies_its_own_collection() {
    let d = type0(
        Object::Name(Name::from("GBK-EUC-H")),
        descendant("CIDFontType2", "Test", vec![]),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    assert_eq!(f.charset, pdfrum_cmap::CidSet::Gb1);
    assert!(f.is_unicode_compatible());
}

#[test]
fn an_embedded_cmap_stream_is_parsed() {
    let program = b"1 begincodespacerange <00> <ff> endcodespacerange\n\
                    1 begincidrange <20> <7e> 1 endcidrange"
        .to_vec();
    let mut store = TestStore::new();
    let encoding = store.add_stream(program);
    let d = type0(encoding, descendant("CIDFontType0", "Test", vec![]), vec![]);
    let f = load_with(&d, &store).expect("loads");
    assert!(!f.cmap.has_no_direct_table(), "an embedded CMap has one");
}

// ---------------------------------------------------------------------------
// The GB2312 rescue.
// ---------------------------------------------------------------------------

#[test]
fn the_gb2312_path_fixes_ascii_widths_and_uses_gb1() {
    let d = dict_of(vec![
        (names::SUBTYPE, Object::Name(Name::from("TrueType"))),
        (
            names::BASE_FONT,
            Object::Name(Name::new(vec![0xcb, 0xce, 0xcc, 0xe5])),
        ),
    ]);
    let f = load_gb2312(
        &d,
        &NoResolve,
        &FontCache::new(),
        &SubstitutionOptions::default(),
        &Limits::default(),
        &mut Diagnostics::default(),
    )
    .expect("the rescue loads");
    assert_eq!(f.charset, pdfrum_cmap::CidSet::Gb1);
    assert!(f.ansi_widths_fixed);
    assert_eq!(f.char_width(CharCode(65)), 500.0);
    assert_eq!(f.char_width(CharCode(10)), 0.0);
}

// ---------------------------------------------------------------------------
// The Japan1 transform.
// ---------------------------------------------------------------------------

#[test]
fn the_japan1_transform_applies_only_to_a_non_embedded_japan1_font() {
    let info = dict_of(vec![(
        names::ORDERING,
        Object::Str(pdfrum_object::PdfString::literal(b"Japan1")),
    )]);
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant(
            "CIDFontType0",
            "Test",
            vec![(names::CID_SYSTEM_INFO, Object::Dict(info.clone()))],
        ),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    assert!(!f.embedded);
    // CID 97 is in the table.
    assert!(f.japan1_transform(CharCode(97)).is_some());
    assert!(f.japan1_transform(CharCode(98)).is_none());

    // An embedded font carries its own vertical forms and is not transformed.
    let mut store = TestStore::new();
    let desc = embedded_descendant(
        &mut store,
        "CIDFontType0",
        vec![(names::CID_SYSTEM_INFO, Object::Dict(info))],
    );
    let d = type0(Object::Name(Name::from("Identity-H")), desc, vec![]);
    let f = load_with(&d, &store).expect("loads");
    assert!(f.embedded);
    assert!(f.japan1_transform(CharCode(97)).is_none());
}

// ---------------------------------------------------------------------------
// Damage.
// ---------------------------------------------------------------------------

#[test]
fn arbitrary_dictionaries_never_panic() {
    let mut store = TestStore::new();
    let garbage = store.add_stream(b"garbage garbage garbage".to_vec());
    let empty = store.add_stream(Vec::new());
    let short_c2g = store.add_stream(vec![0u8]);
    let encodings = [
        Object::Name(Name::from("Identity-H")),
        Object::Name(Name::from("")),
        Object::Name(Name::from("NoSuchCMap")),
        garbage,
        empty,
    ];
    let descendants = [
        Dict::new(),
        descendant("CIDFontType0", "", vec![]),
        descendant(
            "CIDFontType2",
            "X",
            vec![(
                names::W,
                Object::Array(Array::of([Object::Int(4_294_967_295)])),
            )],
        ),
        descendant(
            "CIDFontType2",
            "X",
            vec![(names::CID_TO_GID_MAP, short_c2g.clone())],
        ),
        descendant(
            "CIDFontType2",
            "X",
            vec![(names::DW, Object::Int(-2_147_483_648))],
        ),
    ];
    for enc in &encodings {
        for desc in &descendants {
            let d = type0(enc.clone(), desc.clone(), vec![]);
            let Ok(f) = load_with(&d, &store) else {
                continue;
            };
            for code in [0u32, 1, 0x41, 0xffff, 0x1_0000, u32::MAX] {
                let _ = f.glyph_from_charcode(CharCode(code));
                let _ = f.char_width(CharCode(code));
                let _ = f.vert_width(CharCode(code));
                let _ = f.vert_origin(CharCode(code));
                let _ = f.unicode_from_charcode(CharCode(code));
                let _ = f.scalar_unicode(CharCode(code));
                let _ = f.char_bbox(CharCode(code));
                let _ = f.cid_from_charcode(CharCode(code));
            }
            for ch in ['\0', 'A', '\u{4e00}'] {
                let _ = f.charcode_from_unicode(ch);
            }
            let _ = f.is_unicode_compatible();
            let font = crate::Font::Type0(Box::new(f));
            let _: Vec<_> = font.decode(&[0, 1, 2, 3, 0xff]).collect();
        }
    }
}

#[test]
fn a_font_with_no_face_still_answers_every_accessor() {
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant("CIDFontType0", "Test", vec![]),
        vec![],
    );
    let mut f = load_it(&d).expect("loads");
    f.glyphs = GlyphSource::None;
    let _ = f.glyph_from_charcode(CharCode(1));
    assert_eq!(f.char_bbox(CharCode(1)), pdfrum_common::kurbo::Rect::ZERO);
}

#[test]
fn a_cid_char_box_grows_its_top_by_a_sixty_fourth() {
    let mut store = TestStore::new();
    let desc = embedded_descendant(&mut store, "CIDFontType2", vec![]);
    let d = type0(Object::Name(Name::from("Identity-H")), desc, vec![]);
    let f = load_with(&d, &store).expect("loads");

    // A glyph with a top to grow. Identity-H makes the code the glyph index.
    let code = (1..64u32)
        .find(|g| {
            f.glyphs
                .glyph_bbox(Gid(*g as u16))
                .is_some_and(|b| b.y1 > 64.0)
        })
        .expect("some glyph has a tall box");
    let raw = f.glyphs.glyph_bbox(Gid(code as u16)).expect("has a box");

    // `cfx_face.cpp:1211-1216` adds a sixty-fourth of the top to the top on
    // the way out of `GetCharBBox`, and `cpdf_cidfont.cpp:550` is its only
    // caller — so every CID box carries it, and the simple-font path, which
    // reads `GetGlyphBBox` directly, carries none.
    let top = raw.y1 as i32;
    assert_eq!(f.char_bbox(CharCode(code)).y1, f64::from(top + top / 64));
    assert_ne!(f.char_bbox(CharCode(code)).y1, raw.y1, "the top moved");
    assert_eq!(f.char_bbox(CharCode(code)).y0, raw.y0, "only the top moves");
}

#[test]
fn the_charmap_chooser_takes_the_legacy_subtable_the_coding_scheme_names() {
    // `tt_sjis_and_unicode.ttf` carries `(3,1)` first and `(3,2)` second, so a
    // chooser that ignored the coding scheme would always pick index 0. The
    // JIS scheme must reach index 1.
    let bytes = testfonts::load("tt_sjis_and_unicode.ttf");
    let face = crate::glyphs::Face::new(std::sync::Arc::from(bytes.as_slice()), 0).expect("parses");
    let glyphs = GlyphSource::Fontations(face);

    assert_eq!(
        cid_charmap(&glyphs, CidCoding::Jis),
        crate::glyphs::Charmap::Index(1),
        "Shift-JIS must take the (3,2) subtable"
    );
    // Every other scheme has no legacy subtable here and falls to Unicode.
    for coding in [
        CidCoding::Gb,
        CidCoding::Big5,
        CidCoding::Korea,
        CidCoding::Cid,
        CidCoding::Ucs2,
        CidCoding::Utf16,
        CidCoding::Unknown,
    ] {
        assert_eq!(
            cid_charmap(&glyphs, coding),
            crate::glyphs::Charmap::Index(0),
            "{coding:?} should fall through to Unicode"
        );
    }
}

#[test]
fn the_legacy_encoding_ids_are_the_freetype_ones() {
    // Shift-JIS 2, GB2312 3, Big5 4, **Johab 6** — not Wansung's 5, which is
    // the surprising one and the reason a Wansung-only font falls through.
    assert_eq!(legacy_encoding_id(CidCoding::Jis), Some(2));
    assert_eq!(legacy_encoding_id(CidCoding::Gb), Some(3));
    assert_eq!(legacy_encoding_id(CidCoding::Big5), Some(4));
    assert_eq!(legacy_encoding_id(CidCoding::Korea), Some(6));
    for coding in [
        CidCoding::Unknown,
        CidCoding::Ucs2,
        CidCoding::Cid,
        CidCoding::Utf16,
    ] {
        assert_eq!(legacy_encoding_id(coding), None, "{coding:?}");
    }
}

#[test]
fn the_charmap_chooser_prefers_unicode_and_falls_back_to_the_first() {
    let bytes = testfonts::load("tt_unicode_31.ttf");
    let face = crate::glyphs::Face::new(std::sync::Arc::from(bytes.as_slice()), 0).expect("parses");
    let glyphs = GlyphSource::Fontations(face);
    assert_eq!(
        cid_charmap(&glyphs, CidCoding::Cid),
        crate::glyphs::Charmap::Index(0)
    );
    // With no charmaps at all there is nothing to select.
    assert_eq!(
        cid_charmap(&GlyphSource::None, CidCoding::Cid),
        crate::glyphs::Charmap::None
    );
}

#[test]
fn flags_reach_the_substitution_request() {
    // A descendant's descriptor flags drive substitution, which is the only
    // route a CID font has to a face when nothing is embedded.
    let desc = dict_of(vec![(
        names::FLAGS,
        Object::Int(i64::from(FontFlags::SERIF.bits())),
    )]);
    let d = type0(
        Object::Name(Name::from("Identity-H")),
        descendant(
            "CIDFontType0",
            "NoSuchFontAnywhere",
            vec![(names::FONT_DESCRIPTOR, Object::Dict(desc))],
        ),
        vec![],
    );
    let f = load_it(&d).expect("loads");
    // Serif routes to the serif generic.
    assert_eq!(
        f.subst.as_ref().map(|s| s.family.as_str()),
        Some("Chrome Serif")
    );
}
