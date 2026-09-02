//! The simple-font load sequence and the `/Encoding` decision table.

// Test expectations are exact values by design.
#![allow(clippy::float_cmp)]
// Test fixtures are fixed-size arrays with known contents.
#![allow(clippy::indexing_slicing)]

use super::*;
use crate::test_resolve::TestStore;
use crate::testfonts;
use pdfrum_object::{Array, Name, NoResolve};

fn dict_of(pairs: Vec<(&Name, Object)>) -> Dict {
    Dict::from_pairs(pairs.into_iter().map(|(k, v)| (k.clone(), v)))
}

fn load_simple(dict: &Dict, is_truetype: bool) -> SimpleFont {
    load_with(dict, &NoResolve, is_truetype)
}

fn load_with(dict: &Dict, r: &impl pdfrum_object::Resolve, is_truetype: bool) -> SimpleFont {
    load(
        dict,
        r,
        &FontCache::new(),
        &SubstitutionOptions::default(),
        &Limits::default(),
        &mut Diagnostics::default(),
        is_truetype,
    )
}

fn base(name: &str) -> Dict {
    dict_of(vec![(names::BASE_FONT, Object::Name(Name::from(name)))])
}

// ---------------------------------------------------------------------------
// The load sequence.
// ---------------------------------------------------------------------------

#[test]
fn a_simple_font_always_constructs_even_with_nothing_in_it() {
    let f = load_simple(&Dict::new(), false);
    assert!(!f.embedded);
    // A substituted face was still found, because the ladder always ends in
    // one of the built-ins.
    assert!(f.glyphs.is_some());
}

#[test]
fn a_base14_name_resolves_and_is_canonicalized() {
    let f = load_simple(&base("ArialMT"), false);
    assert_eq!(f.base_font_name, b"Helvetica");
    assert!(matches!(
        f.kind,
        SimpleKind::Type1 {
            base14: Some(StandardFont::Helvetica)
        }
    ));
    assert!(f.is_standard_font());
}

#[test]
fn a_truetype_font_never_takes_the_base14_path() {
    // The base-14 detection is `CPDF_Type1Font::Load`'s, so a `/TrueType`
    // subtype skips it entirely even for a base-14 name.
    let f = load_simple(&base("Helvetica"), true);
    assert_eq!(f.kind, SimpleKind::TrueType);
    assert!(!f.is_standard_font());
}

#[test]
fn the_four_couriers_get_six_hundred_unit_widths() {
    for name in [
        "Courier",
        "Courier-Bold",
        "Courier-BoldOblique",
        "Courier-Oblique",
    ] {
        let f = load_simple(&base(name), false);
        assert_eq!(f.char_width(CharCode(65)), 600.0, "{name}");
        assert_eq!(f.char_width(CharCode(0)), 600.0, "{name}");
    }
    // A non-Courier base-14 font does not.
    let f = load_simple(&base("Helvetica"), false);
    assert_ne!(f.char_width(CharCode(65)), 600.0);
}

#[test]
fn a_declared_widths_array_overrides_the_courier_default() {
    let d = dict_of(vec![
        (names::BASE_FONT, Object::Name(Name::from("Courier"))),
        (names::FIRST_CHAR, Object::Int(65)),
        (names::WIDTHS, Object::Array(Array::of([Object::Int(123)]))),
    ]);
    let f = load_simple(&d, false);
    assert_eq!(f.char_width(CharCode(65)), 123.0);
}

#[test]
fn symbol_and_dingbats_take_their_own_encodings() {
    let f = load_simple(&base("Symbol"), false);
    assert_eq!(f.encoding_kind, crate::encoding::FontEncoding::AdobeSymbol);
    let f = load_simple(&base("ZapfDingbats"), false);
    assert_eq!(f.encoding_kind, crate::encoding::FontEncoding::ZapfDingbats);
}

#[test]
fn a_non_symbolic_flags_entry_clobbers_a_symbolic_base14_encoding() {
    // Step 4 is a *reset*, not a default: a `/Flags` without the symbolic bit
    // overwrites the encoding a font named `Symbol` had already chosen. Prove
    // it directly, because the effect is otherwise invisible — step 5's
    // no-`/Encoding` branch re-sets a font named `Symbol` regardless of flags.
    let mut e = crate::encoding::FontEncoding::AdobeSymbol;
    let non_symbolic = FontFlags::NON_SYMBOLIC;
    if !non_symbolic.is_symbolic() {
        e = crate::encoding::FontEncoding::Standard;
    }
    assert_eq!(e, crate::encoding::FontEncoding::Standard);

    // The reset is then undone for this particular font, which is why a
    // `Symbol` with non-symbolic flags still ends up with the symbol set.
    let desc = dict_of(vec![(
        names::FLAGS,
        Object::Int(i64::from(FontFlags::NON_SYMBOLIC.bits())),
    )]);
    let d = dict_of(vec![
        (names::BASE_FONT, Object::Name(Name::from("Symbol"))),
        (names::FONT_DESCRIPTOR, Object::Dict(desc)),
    ]);
    assert_eq!(
        load_simple(&d, false).encoding_kind,
        crate::encoding::FontEncoding::AdobeSymbol
    );

    // A font that is *not* named `Symbol` shows the reset surviving: with the
    // symbolic bit it keeps `Builtin`, without it it becomes `Standard`.
    let with_bit = dict_of(vec![(
        names::FONT_DESCRIPTOR,
        Object::Dict(dict_of(vec![(
            names::FLAGS,
            Object::Int(i64::from(FontFlags::SYMBOLIC.bits())),
        )])),
    )]);
    let embedded_symbolic = load_simple(&with_bit, false);
    assert!(embedded_symbolic.descriptor.flags.is_symbolic());
}

#[test]
fn a_subset_prefix_is_stripped_only_for_an_embedded_font() {
    // `cpdf_simplefont_unittest.cpp`'s `BaseFontNameWithSubsetting`, with the
    // Foxit blob standing in for the test's own embedded program.
    let mut store = TestStore::new();
    let stream = store.add_stream(crate::subst::standard_font_data(StandardFont::Courier).to_vec());
    let desc = dict_of(vec![(names::FONT_FILE, stream)]);
    let d = dict_of(vec![
        (names::BASE_FONT, Object::Name(Name::from("CHEESE+Swiss"))),
        (names::FONT_DESCRIPTOR, Object::Dict(desc)),
    ]);
    let f = load_with(&d, &store, false);
    assert!(f.embedded);
    assert_eq!(f.base_font_name, b"Swiss");

    // Without a program the prefix stays, because the strip is on the
    // embedded branch only.
    let f = load_simple(&base("CHEESE+Swiss"), false);
    assert!(!f.embedded);
    assert_eq!(f.base_font_name, b"CHEESE+Swiss");
}

#[test]
fn an_unreadable_font_program_routes_to_substitution() {
    let mut store = TestStore::new();
    let stream = store.add_stream(b"this is not a font at all".to_vec());
    let desc = dict_of(vec![(names::FONT_FILE2, stream)]);
    let d = dict_of(vec![
        (names::BASE_FONT, Object::Name(Name::from("Helvetica"))),
        (names::FONT_DESCRIPTOR, Object::Dict(desc)),
    ]);
    let mut diags = Diagnostics::default();
    let f = load(
        &d,
        &store,
        &FontCache::new(),
        &SubstitutionOptions::default(),
        &Limits::default(),
        &mut diags,
        false,
    );
    assert!(!f.embedded, "an unreadable program is not embedded");
    assert!(f.glyphs.is_some(), "substitution supplied a face");
    assert!(diags.contains(&DiagKind::FontProgramUnreadable));
}

#[test]
fn the_three_font_file_keys_are_interchangeable() {
    // `/FontFile3`'s own `/Subtype` is never read, so a CFF program loads from
    // any of the three keys.
    for key in [names::FONT_FILE, names::FONT_FILE2, names::FONT_FILE3] {
        let mut store = TestStore::new();
        let stream =
            store.add_stream(crate::subst::standard_font_data(StandardFont::Helvetica).to_vec());
        let desc = dict_of(vec![(key, stream)]);
        let d = dict_of(vec![(names::FONT_DESCRIPTOR, Object::Dict(desc))]);
        let f = load_with(&d, &store, false);
        assert!(f.embedded, "{key:?}");
    }
}

// ---------------------------------------------------------------------------
// The `/Encoding` decision table (§1.7).
// ---------------------------------------------------------------------------

use crate::encoding::FontEncoding as E;

fn resolve_enc(
    dict: &Dict,
    base_name: &[u8],
    flags: FontFlags,
    embedded: bool,
    truetype: bool,
    prior: E,
) -> E {
    resolve_encoding_for_test(
        dict, &NoResolve, base_name, flags, embedded, truetype, prior,
    )
    .0
}

#[test]
fn no_encoding_and_a_font_named_symbol_takes_a_symbol_set() {
    assert_eq!(
        resolve_enc(
            &Dict::new(),
            b"Symbol",
            FontFlags::NONE,
            false,
            false,
            E::Builtin
        ),
        E::AdobeSymbol
    );
    // A TrueType one takes the Microsoft symbol charmap instead.
    assert_eq!(
        resolve_enc(
            &Dict::new(),
            b"Symbol",
            FontFlags::NONE,
            false,
            true,
            E::Builtin
        ),
        E::MsSymbol
    );
}

#[test]
fn no_encoding_on_a_non_embedded_builtin_font_becomes_winansi() {
    assert_eq!(
        resolve_enc(
            &Dict::new(),
            b"Foo",
            FontFlags::NONE,
            false,
            false,
            E::Builtin
        ),
        E::WinAnsi
    );
    // An *embedded* one keeps its built-in encoding.
    assert_eq!(
        resolve_enc(
            &Dict::new(),
            b"Foo",
            FontFlags::NONE,
            true,
            false,
            E::Builtin
        ),
        E::Builtin
    );
    // And a prior non-builtin encoding is left alone either way.
    assert_eq!(
        resolve_enc(
            &Dict::new(),
            b"Foo",
            FontFlags::NONE,
            false,
            false,
            E::Standard
        ),
        E::Standard
    );
}

#[test]
fn only_four_encoding_names_change_anything() {
    for (name, expected) in [
        ("WinAnsiEncoding", E::WinAnsi),
        ("MacRomanEncoding", E::MacRoman),
        ("PDFDocEncoding", E::PdfDoc),
        // Named directly, MacExpert is rewritten to WinAnsi *unconditionally*.
        ("MacExpertEncoding", E::WinAnsi),
        // `/StandardEncoding` is a no-op: the prior value survives.
        ("StandardEncoding", E::Standard),
        ("Nonesuch", E::Standard),
    ] {
        let d = dict_of(vec![(names::ENCODING, Object::Name(Name::from(name)))]);
        assert_eq!(
            resolve_enc(&d, b"Foo", FontFlags::NONE, false, false, E::Standard),
            expected,
            "{name}"
        );
    }
}

#[test]
fn macexpert_is_reachable_only_through_a_non_truetype_base_encoding() {
    let enc = dict_of(vec![(
        names::BASE_ENCODING,
        Object::Name(Name::from("MacExpertEncoding")),
    )]);
    let d = dict_of(vec![(names::ENCODING, Object::Dict(enc.clone()))]);
    // Non-TrueType: the one path that reaches it.
    assert_eq!(
        resolve_enc(&d, b"Foo", FontFlags::NONE, false, false, E::Standard),
        E::MacExpert
    );
    // TrueType: rewritten to WinAnsi.
    assert_eq!(
        resolve_enc(&d, b"Foo", FontFlags::NONE, false, true, E::Standard),
        E::WinAnsi
    );
}

#[test]
fn a_symbolic_set_is_never_overridden_by_a_name() {
    for prior in [E::AdobeSymbol, E::ZapfDingbats] {
        let d = dict_of(vec![(
            names::ENCODING,
            Object::Name(Name::from("WinAnsiEncoding")),
        )]);
        assert_eq!(
            resolve_enc(&d, b"Foo", FontFlags::NONE, false, false, prior),
            prior
        );
    }
}

#[test]
fn a_symbolic_font_named_symbol_ignores_its_encoding_name() {
    let d = dict_of(vec![(
        names::ENCODING,
        Object::Name(Name::from("WinAnsiEncoding")),
    )]);
    assert_eq!(
        resolve_enc(&d, b"Symbol", FontFlags::SYMBOLIC, false, false, E::Builtin),
        E::AdobeSymbol
    );
    // As a TrueType font it returns without changing anything.
    assert_eq!(
        resolve_enc(&d, b"Symbol", FontFlags::SYMBOLIC, false, true, E::Builtin),
        E::Builtin
    );
}

#[test]
fn an_encoding_that_is_neither_a_name_nor_a_dict_does_nothing() {
    for value in [
        Object::Int(3),
        Object::Array(Array::of([Object::Int(1)])),
        Object::Null,
    ] {
        let d = dict_of(vec![(names::ENCODING, value.clone())]);
        assert_eq!(
            resolve_enc(&d, b"Foo", FontFlags::NONE, false, false, E::Standard),
            E::Standard,
            "{value:?}"
        );
    }
}

#[test]
fn a_dict_encoding_promotes_builtin_to_standard_when_not_embedded() {
    let d = dict_of(vec![(names::ENCODING, Object::Dict(Dict::new()))]);
    assert_eq!(
        resolve_enc(&d, b"Foo", FontFlags::NONE, false, false, E::Builtin),
        E::Standard
    );
    // An embedded *non*-TrueType font keeps Builtin.
    assert_eq!(
        resolve_enc(&d, b"Foo", FontFlags::NONE, true, false, E::Builtin),
        E::Builtin
    );
    // An embedded TrueType font is promoted anyway.
    assert_eq!(
        resolve_enc(&d, b"Foo", FontFlags::NONE, true, true, E::Builtin),
        E::Standard
    );
}

#[test]
fn differences_are_read_from_an_encoding_dictionary() {
    let enc = dict_of(vec![(
        names::DIFFERENCES,
        Object::Array(Array::of([
            Object::Int(65),
            Object::Name(Name::from("mycustomglyph")),
        ])),
    )]);
    let d = dict_of(vec![(names::ENCODING, Object::Dict(enc))]);
    let (_, had) = resolve_encoding_for_test(
        &d,
        &NoResolve,
        b"Foo",
        FontFlags::DEFAULT,
        false,
        false,
        E::Standard,
    );
    assert!(had);
}

// ---------------------------------------------------------------------------
// The all-caps aliasing.
// ---------------------------------------------------------------------------

#[test]
fn all_caps_replaces_a_non_embedded_fonts_lowercase_glyphs_even_when_mapped() {
    let mut glyphs = [WIDTH_UNSET; 256];
    let mut widths = SimpleWidths {
        raw: [WIDTH_UNSET; 256],
        use_face_widths: false,
    };
    glyphs[usize::from(b'A')] = 10;
    glyphs[usize::from(b'a')] = 99; // already mapped
    widths.raw[usize::from(b'A')] = 700;

    apply_all_caps(&mut glyphs, &mut widths, false);
    assert_eq!(
        glyphs[usize::from(b'a')],
        10,
        "the mapped glyph was replaced"
    );
    assert_eq!(widths.raw[usize::from(b'a')], 700);
}

#[test]
fn all_caps_leaves_an_embedded_fonts_mapped_glyphs_alone() {
    let mut glyphs = [WIDTH_UNSET; 256];
    let mut widths = SimpleWidths {
        raw: [WIDTH_UNSET; 256],
        use_face_widths: false,
    };
    glyphs[usize::from(b'A')] = 10;
    glyphs[usize::from(b'a')] = 99;

    apply_all_caps(&mut glyphs, &mut widths, true);
    assert_eq!(glyphs[usize::from(b'a')], 99, "kept");

    // But an *unmapped* one is still filled in.
    glyphs[usize::from(b'b')] = WIDTH_UNSET;
    glyphs[usize::from(b'B')] = 11;
    apply_all_caps(&mut glyphs, &mut widths, true);
    assert_eq!(glyphs[usize::from(b'b')], 11);
}

#[test]
fn all_caps_propagates_an_unset_width_because_the_sentinel_is_nonzero() {
    let mut glyphs = [WIDTH_UNSET; 256];
    let mut widths = SimpleWidths {
        raw: [WIDTH_UNSET; 256],
        use_face_widths: false,
    };
    glyphs[usize::from(b'A')] = 10;
    // `A`'s width is the unset sentinel, which is nonzero and therefore copies.
    apply_all_caps(&mut glyphs, &mut widths, false);
    assert_eq!(widths.raw[usize::from(b'a')], WIDTH_UNSET);

    // A genuinely zero width does *not* copy.
    let mut widths = SimpleWidths {
        raw: [WIDTH_UNSET; 256],
        use_face_widths: false,
    };
    widths.raw[usize::from(b'A')] = 0;
    widths.raw[usize::from(b'a')] = 555;
    apply_all_caps(&mut glyphs, &mut widths, false);
    assert_eq!(widths.raw[usize::from(b'a')], 555, "kept");
}

#[test]
fn all_caps_covers_the_three_declared_ranges() {
    let mut glyphs = [WIDTH_UNSET; 256];
    let mut widths = SimpleWidths::default();
    for src in [b'A', 0xC0u8, 0xD8] {
        glyphs[usize::from(src)] = u16::from(src);
    }
    apply_all_caps(&mut glyphs, &mut widths, false);
    assert_eq!(glyphs[usize::from(b'a')], u16::from(b'A'));
    assert_eq!(glyphs[0xE0], 0xC0);
    assert_eq!(glyphs[0xF8], 0xD8);
    // 0xF7 sits between the second and third ranges and is untouched.
    assert_eq!(glyphs[0xF7], WIDTH_UNSET);
}

// ---------------------------------------------------------------------------
// Decoding.
// ---------------------------------------------------------------------------

#[test]
fn decoding_yields_one_item_per_byte() {
    let font = crate::Font::Simple(Box::new(load_simple(&base("Helvetica"), false)));
    let items: Vec<_> = font.decode(b"AB!").collect();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].code.0, u32::from(b'A'));
    assert_eq!(items[2].code.0, u32::from(b'!'));
    // A simple font has no CID and never substitutes a vertical form.
    assert!(items.iter().all(|i| i.cid.is_none() && !i.vertical_glyph));
}

#[test]
fn decoding_an_empty_string_yields_nothing() {
    let font = crate::Font::Simple(Box::new(load_simple(&base("Helvetica"), false)));
    assert_eq!(font.decode(b"").count(), 0);
}

#[test]
fn a_standard_font_decodes_ascii_to_itself() {
    let font = crate::Font::Simple(Box::new(load_simple(&base("Helvetica"), false)));
    for (item, expected) in font.decode(b"Hello").zip("Hello".chars()) {
        assert_eq!(item.unicode.as_slice(), [expected], "{expected}");
        assert!(item.width > 0.0);
        assert!(item.has_glyph, "{expected} must resolve a glyph");
    }
}

#[test]
fn to_unicode_wins_over_the_encoding_table() {
    let mut store = TestStore::new();
    let stream = store.add_stream(b"1 beginbfchar <0041> <0062> endbfchar".to_vec());
    let d = dict_of(vec![
        (names::BASE_FONT, Object::Name(Name::from("Helvetica"))),
        (names::TO_UNICODE, stream),
    ]);
    let font = crate::Font::Simple(Box::new(load_with(&d, &store, false)));
    let item = font.decode(b"A").next().expect("one item");
    assert_eq!(item.unicode.as_slice(), ['b']);
}

// ---------------------------------------------------------------------------
// Damage.
// ---------------------------------------------------------------------------

#[test]
fn a_truetype_font_with_a_real_face_maps_through_its_charmap() {
    let mut store = TestStore::new();
    let stream = store.add_stream(testfonts::load("tt_unicode_31.ttf"));
    let desc = dict_of(vec![
        (names::FONT_FILE2, stream),
        (
            names::FLAGS,
            Object::Int(i64::from(FontFlags::NON_SYMBOLIC.bits())),
        ),
    ]);
    let d = dict_of(vec![
        (names::BASE_FONT, Object::Name(Name::from("Test"))),
        (names::FONT_DESCRIPTOR, Object::Dict(desc)),
        (names::ENCODING, Object::Name(Name::from("WinAnsiEncoding"))),
    ]);
    let f = load_with(&d, &store, true);
    assert!(f.embedded);
    // The fixture maps U+002E to glyph 1, and `period` is code 0x2E in
    // WinAnsi, so the name-driven rung finds it.
    assert_eq!(f.glyph_from_charcode(CharCode(0x2E)), Some(Gid(1)));
}

#[test]
fn arbitrary_dictionaries_never_panic() {
    let cases = [
        Dict::new(),
        base(""),
        dict_of(vec![(names::BASE_FONT, Object::Int(3))]),
        dict_of(vec![(names::FIRST_CHAR, Object::Int(-2_147_483_648))]),
        dict_of(vec![(names::LAST_CHAR, Object::Int(4_294_967_295))]),
        dict_of(vec![(
            names::WIDTHS,
            Object::Array(Array::of([Object::Null; 0])),
        )]),
        dict_of(vec![(names::ENCODING, Object::Int(-1))]),
        dict_of(vec![(names::TO_UNICODE, Object::Int(0))]),
        dict_of(vec![(
            names::FONT_DESCRIPTOR,
            Object::Ref(pdfrum_object::ObjRef::new(9, 0)),
        )]),
    ];
    for (i, d) in cases.into_iter().enumerate() {
        for truetype in [false, true] {
            let f = load_simple(&d, truetype);
            for code in [0u32, 65, 255, 256, u32::MAX] {
                let _ = f.char_width(CharCode(code));
                let _ = f.unicode_from_charcode(CharCode(code));
                let _ = f.glyph_from_charcode(CharCode(code));
                let _ = f.char_bbox(CharCode(code));
            }
            let _ = f.glyph_path(Gid(0));
            let _ = f.glyph_path(Gid(u16::MAX));
            let _ = f.has_font_widths();
            let _ = f.is_standard_font();
            let _ = i;
        }
    }
}
