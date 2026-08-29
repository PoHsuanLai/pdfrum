//! The substitution ladder's decisions, rung by rung.
//!
//! Ports `cfx_fontmapper_unittest.cpp`'s two end-to-end assertions and adds
//! the per-rung coverage the C++ has none of, using a mock database
//! programmed to fail at successive rungs.

use super::*;
use crate::FontFlags;

fn quiet() -> Diagnostics {
    Diagnostics::default()
}

fn helvetica_bytes() -> Arc<[u8]> {
    Arc::from(standard_font_data(StandardFont::Helvetica))
}

/// A database carrying one real, drawable face under a chosen name.
fn db_with(name: &str, styles: u32, charsets: Vec<Charset>) -> TestFontDb {
    let mut db = TestFontDb::new();
    db.push_with_bytes(name, styles, charsets, helvetica_bytes());
    db
}

fn request(name: &str) -> FontRequest {
    FontRequest {
        name: name.as_bytes().to_vec(),
        ..FontRequest::default()
    }
}

// ---------------------------------------------------------------------------
// The end-to-end oracle assertions.
// ---------------------------------------------------------------------------

/// `cfx_fontmapper_unittest.cpp`'s
/// `FindSubstFaceForRegularStandardFontWithBoldWeight`.
///
/// The expected weight of **700** is a known upstream bug — the test itself
/// carries a `TODO(crbug.com/500640684): Should be 400`. We reproduce it,
/// because it is what the oracle does.
#[test]
fn an_italic_alias_reaches_the_database_as_helvetica_oblique() {
    /// One `find_font` call, as weight, italic, charset, pitch bits and family.
    type SeenQuery = (i32, bool, Charset, u32, String);

    /// A database that records the query it was asked rather than answering it.
    #[derive(Default)]
    struct Recorder {
        faces: Vec<FaceInfo>,
        seen: std::cell::RefCell<Vec<SeenQuery>>,
    }
    impl FontDb for Recorder {
        fn faces(&self) -> &[FaceInfo] {
            &self.faces
        }
        fn face_bytes(&self, _: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
            None
        }
        fn find_font(
            &self,
            weight: i32,
            italic: bool,
            charset: Charset,
            pitch: PitchFamily,
            family: &str,
            _must_match_name: bool,
        ) -> Option<FaceHandle> {
            self.seen
                .borrow_mut()
                .push((weight, italic, charset, pitch.0, family.to_owned()));
            None
        }
    }

    let db = Recorder {
        // One face so the ladder does not short-circuit at step 7.
        faces: vec![FaceInfo {
            name: "Something".to_owned(),
            styles: 0,
            charsets: vec![Charset::Ansi],
        }],
        ..Recorder::default()
    };
    let req = FontRequest {
        name: b"Arial-ItalicMT".to_vec(),
        is_truetype: true,
        flags: FontFlags(FontFlags::USE_EXTERN_ATTR),
        weight: 700,
        italic_angle: 0,
        code_page: CodePage::DefAnsi,
        vertical: false,
    };
    let _ = resolve(&req, &db, &SubstitutionOptions::default(), &mut quiet());

    let seen = db.seen.borrow();
    let query = seen.first().expect("the database was queried");
    assert_eq!(query.4, "Helvetica-Oblique", "the alias canonicalized");
    assert!(query.1, "the Oblique index implies italic");
    assert_eq!(query.2, Charset::Ansi);
    assert_eq!(query.3, 0, "index 7 is neither fixed nor Roman");
    // The known-buggy weight: `n_style != Normal` so Branch B does not reset
    // it to 400. crbug.com/500640684.
    assert_eq!(query.0, 700);
}

/// `cfx_fontmapper_unittest.cpp`'s `SetSubstFontNameWhenGetFaceNameFails`.
#[test]
fn a_database_that_cannot_name_a_face_falls_back_to_the_faces_own_name() {
    // A face registered under an empty name: the substitution reads the name
    // out of the font program instead.
    let mut db = TestFontDb::new();
    db.push_with_bytes("", 0, vec![Charset::Ansi], helvetica_bytes());
    let s = resolve(
        &request("Whatever"),
        &db,
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert!(!s.subst.family.is_empty(), "a name was recovered");
}

// ---------------------------------------------------------------------------
// Step 0 — the weight/attr discard.
// ---------------------------------------------------------------------------

#[test]
fn without_use_extern_attr_the_weight_and_slant_are_discarded() {
    let req = FontRequest {
        name: b"NoSuchFont".to_vec(),
        flags: FontFlags(FontFlags::NON_SYMBOLIC),
        weight: 900,
        italic_angle: -20,
        ..FontRequest::default()
    };
    let s = resolve(
        &req,
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    // Neither survived into the terminal rung's record.
    assert_eq!(s.subst.weight, Some(400));
    assert_eq!(s.subst.italic_angle, 0);
}

#[test]
fn with_use_extern_attr_they_survive() {
    let req = FontRequest {
        name: b"NoSuchFont".to_vec(),
        flags: FontFlags(FontFlags::NON_SYMBOLIC | FontFlags::USE_EXTERN_ATTR),
        weight: 900,
        italic_angle: -20,
        ..FontRequest::default()
    };
    let s = resolve(
        &req,
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert_eq!(s.subst.weight, Some(900));
    assert_eq!(s.subst.italic_angle, -20);
}

#[test]
fn a_zero_weight_becomes_four_hundred() {
    let req = FontRequest {
        name: b"NoSuchFont".to_vec(),
        flags: FontFlags(FontFlags::USE_EXTERN_ATTR),
        weight: 0,
        ..FontRequest::default()
    };
    let s = resolve(
        &req,
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert_eq!(s.subst.weight, Some(400));
}

// ---------------------------------------------------------------------------
// Step 2 — the two symbolic short-circuits.
// ---------------------------------------------------------------------------

#[test]
fn symbol_short_circuits_only_for_a_non_truetype_font() {
    let s = resolve(
        &request("Symbol"),
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert_eq!(s.subst.family, "Chrome Symbol");
    assert_eq!(s.subst.charset, Charset::Symbol);
    assert_eq!(s.standard, Some(StandardFont::Symbol));

    // As a TrueType font it takes the ordinary path instead.
    let req = FontRequest {
        is_truetype: true,
        ..request("Symbol")
    };
    let s = resolve(
        &req,
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert_ne!(s.subst.family, "Chrome Symbol");
}

#[test]
fn zapfdingbats_short_circuits_regardless_of_truetype() {
    for is_truetype in [false, true] {
        let req = FontRequest {
            is_truetype,
            ..request("ZapfDingbats")
        };
        let s = resolve(
            &req,
            &TestFontDb::new(),
            &SubstitutionOptions::default(),
            &mut quiet(),
        );
        assert_eq!(s.subst.family, "Chrome Dingbats", "truetype {is_truetype}");
        assert_eq!(s.standard, Some(StandardFont::Dingbats));
    }
}

// ---------------------------------------------------------------------------
// Steps 4, 5 and 10 — the base-14 arithmetic.
// ---------------------------------------------------------------------------

#[test]
fn style_from_a_standard_font_index_reads_position_modulo_four() {
    for (f, bold, italic) in [
        (StandardFont::Courier, false, false),
        (StandardFont::CourierBold, true, false),
        (StandardFont::CourierBoldOblique, true, true),
        (StandardFont::CourierOblique, false, true),
        (StandardFont::Helvetica, false, false),
        (StandardFont::HelveticaBoldOblique, true, true),
        (StandardFont::Times, false, false),
        (StandardFont::TimesOblique, false, true),
    ] {
        let s = style_from_standard_font(f);
        assert_eq!(s & style_bits::FORCE_BOLD != 0, bold, "{f:?}");
        assert_eq!(s & style_bits::ITALIC != 0, italic, "{f:?}");
    }
}

#[test]
fn a_style_is_applied_to_a_family_head_by_addition() {
    assert_eq!(
        adjust_base_font_for_style(StandardFont::Helvetica, style_bits::FORCE_BOLD),
        StandardFont::HelveticaBold
    );
    assert_eq!(
        adjust_base_font_for_style(
            StandardFont::Helvetica,
            style_bits::FORCE_BOLD | style_bits::ITALIC
        ),
        StandardFont::HelveticaBoldOblique
    );
    assert_eq!(
        adjust_base_font_for_style(StandardFont::Helvetica, style_bits::ITALIC),
        StandardFont::HelveticaOblique
    );
    // No style: unchanged.
    assert_eq!(
        adjust_base_font_for_style(StandardFont::Helvetica, style_bits::NORMAL),
        StandardFont::Helvetica
    );
}

#[test]
fn an_already_styled_index_is_not_styled_again() {
    // Only the three family heads are stylable; adding to `HelveticaOblique`
    // would run off the family.
    assert_eq!(
        adjust_base_font_for_style(StandardFont::HelveticaOblique, style_bits::FORCE_BOLD),
        StandardFont::HelveticaOblique
    );
    assert_eq!(
        adjust_base_font_for_style(StandardFont::Symbol, style_bits::FORCE_BOLD),
        StandardFont::Symbol
    );
}

// ---------------------------------------------------------------------------
// Step 7 and the terminal rung.
// ---------------------------------------------------------------------------

#[test]
fn an_empty_database_goes_straight_to_the_built_ins() {
    let s = resolve(
        &request("Helvetica"),
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert_eq!(s.standard, Some(StandardFont::Helvetica));
    assert!(s.glyphs.is_some(), "the Foxit blob always draws");
    // A resolved base-14 index leaves the substitution record untouched:
    // weight, angle and the generic flag are all still at their defaults.
    assert!(!s.subst.is_builtin_generic);
    assert_eq!(s.subst.weight, None);
    assert!(s.subst.family.is_empty());
}

#[test]
fn an_unknown_name_reaches_the_multiple_master_generic() {
    let s = resolve(
        &request("SomeFontNobodyHas"),
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert_eq!(s.standard, None);
    assert!(s.subst.is_builtin_generic);
    assert_eq!(s.subst.family, "Chrome Sans");
    assert!(
        matches!(s.glyphs, GlyphSource::Type1(_)),
        "the generic is a Type 1 Multiple-Master face"
    );
    // The generic flag zeroes both embolden levels, because its design space
    // already carries the weight.
    assert_eq!(s.subst.embolden_level_for_load(), 0);
}

#[test]
fn a_serif_request_reaches_the_serif_generic_with_a_scaled_weight() {
    let req = FontRequest {
        name: b"SomeSerifNobodyHas".to_vec(),
        flags: FontFlags(FontFlags::SERIF | FontFlags::USE_EXTERN_ATTR),
        weight: 500,
        ..FontRequest::default()
    };
    let s = resolve(
        &req,
        &TestFontDb::new(),
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert_eq!(s.subst.family, "Chrome Serif");
    // `use_chrome_serif` scales the weight by four fifths.
    assert_eq!(s.subst.weight, Some(400));
    assert!(matches!(s.glyphs, GlyphSource::Type1(_)));
}

#[test]
fn both_built_in_generics_parse_and_draw() {
    for serif in [false, true] {
        let (glyphs, family) = builtin_generic(serif);
        assert!(glyphs.is_some(), "{family} must parse");
        assert!(glyphs.num_glyphs() > 100, "{family}");
        let gid = crate::Gid(glyphs.name_index(b"A"));
        assert_ne!(gid.0, 0, "{family} has an A");
        assert!(
            glyphs
                .outline(gid, &crate::glyphs::GlyphParams::default())
                .is_some(),
            "{family} draws"
        );
    }
}

// ---------------------------------------------------------------------------
// Rungs 1 and 2.
// ---------------------------------------------------------------------------

#[test]
fn rung_one_takes_a_matching_database_face() {
    let db = db_with("Helvetica", 0, vec![Charset::Ansi]);
    let s = resolve(
        &request("Helvetica"),
        &db,
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    assert!(matches!(s.glyphs, GlyphSource::Fontations(_)));
    assert_eq!(s.subst.family, "Helvetica");
}

#[test]
fn a_face_the_database_cannot_supply_bytes_for_falls_through() {
    // A face registered with *no* bytes: the ladder must not stop there.
    let mut db = TestFontDb::new();
    db.push("Helvetica", 0, vec![Charset::Ansi]);
    let s = resolve(
        &request("Helvetica"),
        &db,
        &SubstitutionOptions::default(),
        &mut quiet(),
    );
    // It fell through to the built-in Helvetica.
    assert_eq!(s.standard, Some(StandardFont::Helvetica));
    assert!(s.glyphs.is_some());
}

// ---------------------------------------------------------------------------
// Rung 3 — the symbolic retry.
// ---------------------------------------------------------------------------

#[test]
fn a_symbolic_request_retries_once_as_a_plain_one() {
    let req = FontRequest {
        name: b"SomeSymbolicFont".to_vec(),
        flags: FontFlags(FontFlags::SYMBOLIC),
        ..FontRequest::default()
    };
    let mut db = TestFontDb::new();
    db.push("Unrelated", 0, vec![Charset::Ansi]);
    // The retry re-enters with the symbolic bit cleared, so `request_charset`
    // returns ANSI and rung 3 cannot fire again — the call terminates.
    let s = resolve(&req, &db, &SubstitutionOptions::default(), &mut quiet());
    assert!(s.glyphs.is_some());
}

#[test]
fn a_symbolic_request_named_symbol_takes_the_chrome_symbol_face() {
    // Reaching rung 3 with the name still `Symbol` requires it to be a
    // TrueType request, which skipped the step-2 short circuit.
    let req = FontRequest {
        name: b"Symbol".to_vec(),
        is_truetype: true,
        flags: FontFlags(FontFlags::SYMBOLIC),
        ..FontRequest::default()
    };
    let mut db = TestFontDb::new();
    db.push("Unrelated", 0, vec![Charset::ChineseSimplified]);
    let s = resolve(&req, &db, &SubstitutionOptions::default(), &mut quiet());
    assert_eq!(s.subst.family, "Chrome Symbol");
    assert_eq!(s.standard, Some(StandardFont::Symbol));
}

// ---------------------------------------------------------------------------
// Branch A's CJK arm.
// ---------------------------------------------------------------------------

#[test]
fn a_cjk_request_records_its_weight_separately() {
    let req = FontRequest {
        name: b"SomeJapaneseFont-Bold".to_vec(),
        flags: FontFlags(FontFlags::USE_EXTERN_ATTR),
        weight: 700,
        code_page: CodePage::ShiftJis,
        ..FontRequest::default()
    };
    let mut db = TestFontDb::new();
    db.push("Unrelated", 0, vec![Charset::Ansi]);
    let s = resolve(&req, &db, &SubstitutionOptions::default(), &mut quiet());
    assert!(s.subst.subst_cjk);
    assert_eq!(s.subst.weight_cjk, Some(700));
    // ...and the CJK weight is what an *effective* weight reads for a CID font.
    assert_eq!(s.subst.effective_weight(true), 700);
}

#[test]
fn the_narrow_rewrite_uses_the_linux_family() {
    assert_eq!(NARROW_FAMILY, "LiberationSansNarrow");
}

// ---------------------------------------------------------------------------
// The enumeration knob (D7 / OQ-6a).
// ---------------------------------------------------------------------------

#[test]
fn skip_font_enumeration_changes_whether_the_weight_survives_branch_a() {
    /// Records the weight the database was asked for.
    struct Recorder(std::cell::RefCell<Vec<i32>>, Vec<FaceInfo>);
    impl FontDb for Recorder {
        fn faces(&self) -> &[FaceInfo] {
            &self.1
        }
        fn face_bytes(&self, _: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
            None
        }
        fn find_font(
            &self,
            weight: i32,
            _: bool,
            _: Charset,
            _: PitchFamily,
            _: &str,
            _: bool,
        ) -> Option<FaceHandle> {
            self.0.borrow_mut().push(weight);
            None
        }
    }

    // A `-Bold` suffix infers weight 700 at step 5; Branch A then either keeps
    // it or resets it to the pre-inference value.
    let req = FontRequest {
        name: b"SomeUnknownFont-Bold".to_vec(),
        flags: FontFlags(FontFlags::USE_EXTERN_ATTR),
        weight: 400,
        ..FontRequest::default()
    };
    let faces = vec![FaceInfo {
        name: "Unrelated".to_owned(),
        styles: 0,
        charsets: vec![Charset::Ansi],
    }];

    // The assertion is on the query the ladder issued, not on what it settled
    // for afterwards.
    let db = Recorder(std::cell::RefCell::new(Vec::new()), faces.clone());
    let _ = resolve(
        &req,
        &db,
        &SubstitutionOptions {
            skip_font_enumeration: false,
            ..SubstitutionOptions::default()
        },
        &mut quiet(),
    );
    assert_eq!(
        db.0.borrow().first().copied(),
        Some(400),
        "the enumeration path resets the weight"
    );

    let db = Recorder(std::cell::RefCell::new(Vec::new()), faces);
    let _ = resolve(
        &req,
        &db,
        &SubstitutionOptions {
            skip_font_enumeration: true,
            ..SubstitutionOptions::default()
        },
        &mut quiet(),
    );
    assert_eq!(
        db.0.borrow().first().copied(),
        Some(700),
        "the database path keeps the inferred bold weight"
    );
}

#[test]
fn the_default_matches_the_oracles_enumeration_build() {
    // OQ-6(a): the oracle's Linux build drives `CFX_FolderFontInfo`, which
    // enumerates, so the default is the enumeration behavior.
    assert!(!SubstitutionOptions::default().skip_font_enumeration);
}

// ---------------------------------------------------------------------------
// Damage.
// ---------------------------------------------------------------------------

#[test]
fn arbitrary_names_never_panic_and_always_produce_a_face() {
    let names: [&[u8]; 10] = [
        b"",
        b",",
        b"-",
        b"@",
        b"@,-+",
        b"ABCDEF+",
        b"Arial,Bold,Italic,Bold",
        b"\xff\xfe\x00",
        b"ScriptScriptScript",
        b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ];
    for name in names {
        for is_truetype in [false, true] {
            for flags in [0, FontFlags::SYMBOLIC, FontFlags::USE_EXTERN_ATTR] {
                let req = FontRequest {
                    name: name.to_vec(),
                    is_truetype,
                    flags: FontFlags(flags),
                    weight: 700,
                    italic_angle: -12,
                    ..FontRequest::default()
                };
                let s = resolve(
                    &req,
                    &TestFontDb::new(),
                    &SubstitutionOptions::default(),
                    &mut quiet(),
                );
                assert!(
                    s.glyphs.is_some(),
                    "{:?} must still draw",
                    String::from_utf8_lossy(name)
                );
            }
        }
    }
}

#[test]
fn every_standard_font_resolves_to_its_own_blob() {
    for f in ALL_STANDARD_FONTS {
        let s = resolve(
            &request(canonical_font_name(f)),
            &TestFontDb::new(),
            &SubstitutionOptions::default(),
            &mut quiet(),
        );
        assert_eq!(s.standard, Some(f), "{f:?}");
        assert!(s.glyphs.is_some(), "{f:?}");
    }
}

/// `RenameFontForTesting`, which the oracle applies to every request when
/// `--croscore-font-names` is given.
mod croscore {
    use super::super::croscore_name;

    #[test]
    fn the_three_families_map_to_their_metric_compatible_faces() {
        // `test_fonts` holds no Arial, Times or Courier: it holds the
        // metric-compatible Croscore faces, and the rename is what reaches
        // them. Matching is by *substring*, so a subset-prefixed or
        // vendor-qualified spelling maps too.
        for (asked, want) in [
            ("Arial", "Arimo"),
            ("ArialMT", "Arimo"),
            ("Calibri", "Arimo"),
            ("Helvetica", "Arimo"),
            ("Times", "Tinos"),
            ("TimesNewRoman", "Tinos"),
            ("Courier", "Cousine"),
            ("CourierNew", "Cousine"),
        ] {
            assert_eq!(croscore_name(asked), want, "{asked}");
        }
    }

    #[test]
    fn an_empty_name_is_serif() {
        // The one case that is not a substring test: `face.IsEmpty()` shares
        // the Times arm.
        assert_eq!(croscore_name(""), "Tinos");
    }

    #[test]
    fn anything_else_is_left_alone() {
        // Deliberate: some fixtures want the built-in fallback, and reaching
        // it depends on *not* being renamed into a face that exists.
        for name in ["HonMincho-M", "Wingdings", "Symbol", "ZapfDingbats"] {
            assert_eq!(croscore_name(name), name, "{name}");
        }
    }

    #[test]
    fn a_subset_prefixed_name_still_matches() {
        // The test is `Contains`, not equality, so the six-letter subset tag
        // a producer prepends does not hide the family. `bug_717.pdf`'s
        // `ABCDEE+Calibri` is a real instance.
        assert_eq!(croscore_name("ABCDEE+Calibri"), "Arimo");
        assert_eq!(croscore_name("ABCDEE+Calibri-Bold"), "Arimo Bold");
    }

    #[test]
    fn both_style_suffixes_can_apply_and_in_order() {
        assert_eq!(croscore_name("Arial-Bold"), "Arimo Bold");
        assert_eq!(croscore_name("Arial-Italic"), "Arimo Italic");
        // `Oblique` is the other spelling of italic, and a name carrying
        // both words gets both suffixes, bold first.
        assert_eq!(croscore_name("Arial-BoldOblique"), "Arimo Bold Italic");
        assert_eq!(
            croscore_name("TimesNewRoman,BoldItalic"),
            "Tinos Bold Italic"
        );
    }

    #[test]
    fn the_family_arms_are_tried_in_order() {
        // A name matching two arms takes the first: `Arial` beats `Times`
        // because the sans arm is tested first, which is only observable on
        // a name carrying both words.
        assert_eq!(croscore_name("ArialTimes"), "Arimo");
    }
}
