//! Behavior tests over the public surface: name resolution with all its
//! damage tolerance, and CMap programs of the shape real `/Encoding` streams
//! have.

use super::{
    CMap, CharCode, Cid, CidCoding, CidSet, CodingScheme, charcode_from_unicode,
    charset_from_ordering, from_encoding_name, parse_embedded, predefined, unicode_from_cid,
};
use pdfrum_common::{DiagKind, Diagnostics, Limits};
use pdfrum_object::Name;

fn resolve(name: &str) -> (CMap, Diagnostics) {
    let mut diags = Diagnostics::default();
    let cmap = from_encoding_name(&Name::from(name), &mut diags);
    (cmap, diags)
}

fn embedded(program: &[u8]) -> (CMap, Diagnostics) {
    let mut diags = Diagnostics::default();
    let cmap = parse_embedded(program, &Limits::default(), &mut diags);
    (cmap, diags)
}

// ---------------------------------------------------------------------------
// Predefined-name resolution
// ---------------------------------------------------------------------------

/// What each `/Encoding` spelling produces: scheme, coding, collection,
/// direction, whether it loaded, and whether it found a static table.
///
/// One table because these six answers move together, and the interesting
/// cases — the damaged names — are only legible beside the clean ones.
#[test]
fn encoding_names_resolve_to_the_expected_cmaps() {
    use CidCoding as C;
    use CidSet as S;
    use CodingScheme as K;

    /// `up` is vertical, `ok` is loaded, `st` is "found a static table".
    struct Row(&'static str, CodingScheme, CidCoding, CidSet, Flags);
    /// (vertical, loaded, static table)
    type Flags = (bool, bool, bool);

    const LOADED: Flags = (false, true, true);
    const LOADED_V: Flags = (true, true, true);
    const IDENT: Flags = (false, true, false);
    const IDENT_V: Flags = (true, true, false);
    const MISSED: Flags = (false, false, false);
    const MISSED_V: Flags = (true, false, false);

    let cases = [
        Row("Identity-H", K::TwoBytes, C::Cid, S::Unknown, IDENT),
        Row("Identity-V", K::TwoBytes, C::Cid, S::Unknown, IDENT_V),
        // A slash on the name is removed before anything else looks at it.
        Row("/Identity-H", K::TwoBytes, C::Cid, S::Unknown, IDENT),
        Row("GB-EUC-H", K::MixedTwoBytes, C::Gb, S::Gb1, LOADED),
        Row("GB-EUC-V", K::MixedTwoBytes, C::Gb, S::Gb1, LOADED_V),
        Row("KSCpc-EUC-H", K::MixedTwoBytes, C::Korea, S::Korea1, LOADED),
        Row("UniCNS-UTF16-V", K::TwoBytes, C::Utf16, S::Cns1, LOADED_V),
        Row("UniJIS-UCS2-HW-H", K::TwoBytes, C::Ucs2, S::Japan1, LOADED),
        Row("H", K::TwoBytes, C::Jis, S::Japan1, LOADED),
        Row("V", K::TwoBytes, C::Jis, S::Japan1, LOADED_V),
        // Two garbage bytes on the end still find the family's decoder,
        // because the truncation eats exactly those two; the table lookup
        // uses the full name and misses.
        Row("GB-EUCXY", K::MixedTwoBytes, C::Gb, S::Gb1, MISSED),
        // Three garbage bytes leave "GB-EUC-" behind, matching nothing — and
        // likewise the half-width name with no direction suffix, which
        // truncates to "UniJIS-UCS2-", trailing hyphen and all.
        Row("GB-EUC-XY", K::TwoBytes, C::Unknown, S::Unknown, MISSED),
        Row(
            "UniJIS-UCS2-HW",
            K::TwoBytes,
            C::Unknown,
            S::Unknown,
            MISSED,
        ),
        Row("Nonsense", K::TwoBytes, C::Unknown, S::Unknown, MISSED),
        Row("NonsenseV", K::TwoBytes, C::Unknown, S::Unknown, MISSED_V),
        Row("", K::TwoBytes, C::Unknown, S::Unknown, MISSED),
        Row("X", K::TwoBytes, C::Unknown, S::Unknown, MISSED),
    ];

    for Row(name, scheme, coding, charset, (up, ok, st)) in cases {
        let (cmap, diags) = resolve(name);
        assert_eq!(cmap.coding_scheme(), scheme, "{name}: scheme");
        assert_eq!(cmap.coding(), coding, "{name}: coding");
        assert_eq!(cmap.charset(), charset, "{name}: charset");
        assert_eq!(cmap.is_vertical(), up, "{name}: vertical");
        assert_eq!(cmap.is_loaded(), ok, "{name}: loaded");
        assert_eq!(cmap.has_static_map(), st, "{name}: static table");
        // A predefined CMap never has a dense table.
        assert!(cmap.has_no_direct_table(), "{name}");
        // Every failure is visible on the diagnostics channel.
        assert_eq!(diags.is_empty(), ok, "{name}: diagnostic");
    }
}

/// A CMap that resolved to no table maps every code to itself, which is what
/// keeps a file with a broken `/Encoding` rendering.
#[test]
fn unresolved_names_fall_back_to_identity() {
    for name in ["Nonsense", "GB-EUCXY", "UniJIS-UCS2-HW", "GB-EUC-XY", ""] {
        let (cmap, _) = resolve(name);
        for code in [0u32, 1, 0x41, 0x1234, 0xFFFF] {
            assert_eq!(
                cmap.cid(CharCode(code)),
                Cid(code as u16),
                "{name} {code:#x}"
            );
        }
        // Identity truncates rather than wrapping into the table.
        assert_eq!(cmap.cid(CharCode(0x1_0041)), Cid(0x0041), "{name}");
    }
}

/// `GB-EUC-XY` keeps the family's *decoder* even though its table is gone —
/// a correct byte splitter with no CID map is a real state.
#[test]
fn a_garbage_suffix_keeps_the_decoder_and_loses_the_table() {
    let (good, _) = resolve("GB-EUC-H");
    let (damaged, _) = resolve("GB-EUCXY");
    let bytes: &[u8] = &[0x41, 0xA1, 0xA1];
    let split: Vec<CharCode> = damaged.decode(bytes).map(|(c, _)| c).collect();
    assert_eq!(
        split,
        good.decode(bytes).map(|(c, _)| c).collect::<Vec<_>>()
    );
    // But the CIDs differ: one is the real table, the other the identity.
    assert_eq!(good.cid(CharCode(0xA1A1)), Cid(0x0060));
    assert_eq!(damaged.cid(CharCode(0xA1A1)), Cid(0xA1A1));
}

#[test]
fn predefined_reports_the_miss_that_from_encoding_name_absorbs() {
    assert!(predefined(&Name::from("Nonsense")).is_none());
    assert!(predefined(&Name::from("")).is_none());
    assert!(predefined(&Name::from("Identity-H")).is_some());
    assert!(predefined(&Name::from("GB-EUC-H")).is_some());
    // A name that finds a row but no table is still `Some` — the miss is
    // reported through `is_loaded`, not through the option.
    let damaged = predefined(&Name::from("GB-EUCXY")).unwrap();
    assert!(!damaged.is_loaded());
    // But a truncation that lands between rows is a plain miss.
    assert!(predefined(&Name::from("UniJIS-UCS2-HW")).is_none());
}

#[test]
fn identity_decodes_two_byte_codes_as_themselves() {
    let (cmap, _) = resolve("Identity-H");
    let pairs: Vec<(CharCode, Cid)> = cmap.decode(&[0x00, 0x41, 0x30, 0x42]).collect();
    assert_eq!(
        pairs,
        vec![
            (CharCode(0x0041), Cid(0x0041)),
            (CharCode(0x3042), Cid(0x3042)),
        ]
    );
    assert_eq!(cmap.count_chars(&[0x00, 0x41, 0x30]), 2);
    assert_eq!(cmap.charcode_from_cid(Cid(0x41)), CharCode(0));
}

/// Both vertical CMaps of a pair decode identically and differ only in CIDs.
#[test]
fn vertical_and_horizontal_share_a_decoder() {
    let (h, _) = resolve("GB-EUC-H");
    let (v, _) = resolve("GB-EUC-V");
    assert_eq!(h.coding_scheme(), v.coding_scheme());
    assert!(!h.is_vertical() && v.is_vertical());
    let bytes: &[u8] = &[0xA1, 0xA1, 0x41];
    let hc: Vec<CharCode> = h.decode(bytes).map(|(c, _)| c).collect();
    let vc: Vec<CharCode> = v.decode(bytes).map(|(c, _)| c).collect();
    assert_eq!(hc, vc);
}

#[test]
fn every_built_in_name_resolves_and_maps_something() {
    // Both direction suffixes of every family in the table, plus the two
    // Identity CMaps.
    let names = [
        "GB-EUC-H",
        "GB-EUC-V",
        "GBpc-EUC-H",
        "GBK-EUC-H",
        "GBKp-EUC-H",
        "GBK2K-H",
        "GBK2K-V",
        "UniGB-UCS2-H",
        "UniGB-UTF16-H",
        "B5pc-H",
        "HKscs-B5-H",
        "ETen-B5-H",
        "ETenms-B5-H",
        "UniCNS-UCS2-H",
        "UniCNS-UTF16-H",
        "83pv-RKSJ-H",
        "90ms-RKSJ-H",
        "90msp-RKSJ-H",
        "90pv-RKSJ-H",
        "Add-RKSJ-H",
        "EUC-H",
        "H",
        "V",
        "Ext-RKSJ-H",
        "UniJIS-UCS2-H",
        "UniJIS-UCS2-HW-H",
        "UniJIS-UTF16-H",
        "KSC-EUC-H",
        "KSCms-UHC-H",
        "KSCms-UHC-HW-H",
        "KSCpc-EUC-H",
        "UniKS-UCS2-H",
        "UniKS-UTF16-H",
    ];
    for name in names {
        let (cmap, diags) = resolve(name);
        assert!(cmap.is_loaded(), "{name} must load");
        assert!(cmap.has_static_map(), "{name} must find a table");
        assert!(diags.is_empty(), "{name} must resolve cleanly");
        assert_ne!(cmap.charset(), CidSet::Unknown, "{name}");
        let mapped = (0u32..=0xFFFF).any(|c| cmap.cid(CharCode(c)) != Cid(0));
        assert!(mapped, "{name} must map at least one code");
    }
}

/// `CNS-EUC-H` and `CNS-EUC-V` have static tables — including two of the three
/// four-byte tables — but no row in the name table, so no `/Encoding` spelling
/// can reach them: `CNS-EUC-H` truncates to `CNS-EUC`, which no row carries.
/// They are the only entries in the blob that are unreachable this way, and
/// they are shipped anyway so the tables stay a faithful copy.
#[test]
fn the_cns_euc_tables_are_unreachable_by_name() {
    for name in ["CNS-EUC-H", "CNS-EUC-V"] {
        let (cmap, diags) = resolve(name);
        assert!(!cmap.is_loaded(), "{name}");
        assert_eq!(cmap.charset(), CidSet::Unknown, "{name}");
        assert_eq!(cmap.coding_scheme(), CodingScheme::TwoBytes, "{name}");
        assert!(diags.contains(&DiagKind::CMapNameUnknown), "{name}");
    }
    // The vertical spelling is still recognised as vertical.
    assert!(resolve("CNS-EUC-V").0.is_vertical());
}

// ---------------------------------------------------------------------------
// CID → Unicode and the reverse scan
// ---------------------------------------------------------------------------

#[test]
fn a_predefined_cmap_reaches_unicode_through_its_collection() {
    let (gb, _) = resolve("GB-EUC-H");
    let cid = gb.cid(CharCode(0x20));
    assert_eq!(cid, Cid(0x1E24));
    assert_eq!(unicode_from_cid(gb.charset(), cid), Some(' '));
    assert_eq!(charcode_from_unicode(&gb, ' '), CharCode(0x20));
}

/// A CMap with no collection cannot reverse a Unicode scalar at all.
#[test]
fn charcode_from_unicode_needs_a_collection() {
    let (identity, _) = resolve("Identity-H");
    assert_eq!(charcode_from_unicode(&identity, 'A'), CharCode(0));
    let (nonsense, _) = resolve("Nonsense");
    assert_eq!(charcode_from_unicode(&nonsense, 'A'), CharCode(0));
}

#[test]
fn ordering_spellings_select_a_collection() {
    for (o, want) in [
        (&b"GB1"[..], CidSet::Gb1),
        (b"CNS1", CidSet::Cns1),
        (b"Japan1", CidSet::Japan1),
        (b"Korea1", CidSet::Korea1),
        (b"UCS", CidSet::Unicode),
        (b"UCS2", CidSet::Unknown),
        (b"Identity", CidSet::Unknown),
        (b"", CidSet::Unknown),
        (b"gb1", CidSet::Unknown),
    ] {
        assert_eq!(charset_from_ordering(o), want, "{o:?}");
    }
}

// ---------------------------------------------------------------------------
// Embedded CMap programs
// ---------------------------------------------------------------------------

/// A block declaring exactly one range keeps only its width: two bytes wide
/// gives a two-byte scheme, and *any other width* — including three and four —
/// gives a one-byte scheme.
#[test]
fn a_lone_codespace_range_contributes_only_its_width() {
    for (program, want) in [
        (
            &b"begincodespacerange <00> <ff> endcodespacerange"[..],
            CodingScheme::OneByte,
        ),
        (
            b"begincodespacerange <0000> <ffff> endcodespacerange",
            CodingScheme::TwoBytes,
        ),
        // Three and four bytes wide both fall to one byte.
        (
            b"begincodespacerange <000000> <ffffff> endcodespacerange",
            CodingScheme::OneByte,
        ),
        (
            b"begincodespacerange <00000000> <ffffffff> endcodespacerange",
            CodingScheme::OneByte,
        ),
    ] {
        let (cmap, diags) = embedded(program);
        assert_eq!(
            cmap.coding_scheme(),
            want,
            "{}",
            String::from_utf8_lossy(program)
        );
        assert!(diags.contains(&DiagKind::CMapCodespaceDropped));
    }
}

/// The lone range's *bounds* never reach the decoder, so a byte outside them
/// decodes exactly like one inside.
#[test]
fn a_lone_codespace_ranges_bounds_are_not_enforced() {
    let (cmap, _) = embedded(b"begincodespacerange <00> <7f> endcodespacerange");
    assert_eq!(cmap.coding_scheme(), CodingScheme::OneByte);
    let codes: Vec<CharCode> = cmap
        .decode(&[0x00, 0x7F, 0x80, 0xFE])
        .map(|(c, _)| c)
        .collect();
    assert_eq!(
        codes,
        vec![CharCode(0), CharCode(0x7F), CharCode(0x80), CharCode(0xFE)]
    );
}

/// Two or more ranges always give a four-byte-capable decoder, even when every
/// range is one byte wide.
#[test]
fn two_codespace_ranges_give_a_four_byte_decoder() {
    let (cmap, _) = embedded(b"begincodespacerange <00> <1f> <20> <7f> endcodespacerange");
    assert_eq!(cmap.coding_scheme(), CodingScheme::MixedFourBytes);
    let codes: Vec<CharCode> = cmap.decode(&[0x00, 0x41, 0x1F]).map(|(c, _)| c).collect();
    assert_eq!(codes, vec![CharCode(0), CharCode(0x41), CharCode(0x1F)]);
    // A byte outside both ranges is code 0 and still consumes its byte.
    let codes: Vec<CharCode> = cmap.decode(&[0x80]).map(|(c, _)| c).collect();
    assert_eq!(codes, vec![CharCode(0)]);
}

/// Blocks accumulate: the second block's count includes the first's surviving
/// ranges, so a lone range in a second block still sees more than one in total.
#[test]
fn codespace_blocks_accumulate_across_the_program() {
    let (cmap, _) = embedded(
        b"begincodespacerange <00> <1f> <20> <7f> endcodespacerange
          begincodespacerange <8140> <9ffc> endcodespacerange",
    );
    assert_eq!(cmap.coding_scheme(), CodingScheme::MixedFourBytes);
    // All three ranges are live: the two-byte one decodes a pair.
    let codes: Vec<CharCode> = cmap.decode(&[0x81, 0x40, 0x41]).map(|(c, _)| c).collect();
    assert_eq!(codes, vec![CharCode(0x8140), CharCode(0x41)]);
}

/// A stray token between two bounds is skipped without shifting the pairing.
#[test]
fn a_stray_token_does_not_shift_codespace_pairing() {
    let (clean, _) = embedded(b"begincodespacerange <0000> <ffff> <20> <7f> endcodespacerange");
    let (dirty, _) =
        embedded(b"begincodespacerange <0000> garbage <ffff> <20> <7f> endcodespacerange");
    // The stray token is skipped without advancing the pair counter, so the
    // *positions* stay aligned — but it also became `last_word`, so the pair
    // formed at the odd slot is (garbage, <ffff>), which is rejected for not
    // starting with `<`. One range survives instead of two, which flips the
    // whole scheme from four-byte to the surviving range's own width.
    assert_eq!(clean.coding_scheme(), CodingScheme::MixedFourBytes);
    assert_eq!(dirty.coding_scheme(), CodingScheme::OneByte);
}

#[test]
fn cidchar_and_cidrange_populate_the_table() {
    let (cmap, _) = embedded(
        b"begincodespacerange <0000> <ffff> endcodespacerange
          1 begincidchar <20> <100> endcidchar",
    );
    assert_eq!(cmap.cid(CharCode(0x20)), Cid(0x100));
    assert_eq!(cmap.cid(CharCode(0x21)), Cid(0));
    assert!(!cmap.has_no_direct_table());

    let (cmap, _) = embedded(
        b"begincodespacerange <0000> <ffff> endcodespacerange
          1 begincidrange <20> <7e> <100> endcidrange",
    );
    assert_eq!(cmap.cid(CharCode(0x20)), Cid(0x100));
    assert_eq!(cmap.cid(CharCode(0x7e)), Cid(0x100 + 0x5e));
    assert_eq!(cmap.cid(CharCode(0x7f)), Cid(0));
}

/// A later range overwrites an earlier one; there is no first-wins rule here.
#[test]
fn overlapping_cid_ranges_are_last_wins() {
    let (cmap, _) = embedded(b"begincidrange <20> <30> <100> <25> <35> <200> endcidrange");
    assert_eq!(cmap.cid(CharCode(0x20)), Cid(0x100));
    assert_eq!(cmap.cid(CharCode(0x24)), Cid(0x104));
    // Overwritten by the second range.
    assert_eq!(cmap.cid(CharCode(0x25)), Cid(0x200));
    assert_eq!(cmap.cid(CharCode(0x30)), Cid(0x20B));
    assert_eq!(cmap.cid(CharCode(0x35)), Cid(0x210));
}

/// The CID is a truncating 16-bit value, so a CID past 0xFFFF wraps to its low
/// half — `<10000>` becomes CID 0.
#[test]
fn cid_values_truncate_to_sixteen_bits() {
    let (cmap, _) = embedded(b"1 begincidchar <01> <10000> endcidchar");
    assert_eq!(cmap.cid(CharCode(1)), Cid(0));
    let (cmap, _) = embedded(b"1 begincidchar <01> <10041> endcidchar");
    assert_eq!(cmap.cid(CharCode(1)), Cid(0x41));
}

/// A range whose start is above its end maps nothing at all.
#[test]
fn a_reversed_cid_range_maps_nothing() {
    let (cmap, diags) = embedded(b"1 begincidrange <10> <05> <41> endcidrange");
    assert_eq!(cmap.cid(CharCode(5)), Cid(0));
    assert_eq!(cmap.cid(CharCode(0x10)), Cid(0));
    assert_eq!(cmap.cid(CharCode(0x0A)), Cid(0));
    assert!(diags.contains(&DiagKind::CMapReversedRange));
}

/// Codes at or above 0x10000 only survive in a four-byte CMap; a two-byte one
/// cannot produce such a code, so the mappings are discarded.
#[test]
fn wide_cid_ranges_need_a_four_byte_scheme() {
    let (narrow, diags) = embedded(
        b"begincodespacerange <0000> <ffff> endcodespacerange
          1 begincidrange <10000> <10010> <200> endcidrange",
    );
    assert_eq!(narrow.cid(CharCode(0x1_0005)), Cid(0));
    assert!(diags.contains(&DiagKind::CMapWideMappingsDropped));

    let (wide, _) = embedded(
        b"begincodespacerange <00> <7f> <8140> <9ffc> endcodespacerange
          1 begincidrange <10000> <10010> <200> endcidrange",
    );
    assert_eq!(wide.coding_scheme(), CodingScheme::MixedFourBytes);
    assert_eq!(wide.cid(CharCode(0x1_0000)), Cid(0x200));
    assert_eq!(wide.cid(CharCode(0x1_0005)), Cid(0x205));
    assert_eq!(wide.cid(CharCode(0x1_0010)), Cid(0x210));
    assert_eq!(wide.cid(CharCode(0x1_0011)), Cid(0));
    assert_eq!(wide.cid(CharCode(0x0_FFFF)), Cid(0));
}

/// `usecmap` contributes nothing: the base map's codes stay unmapped.
#[test]
fn usecmap_is_recognised_and_ignored() {
    let (cmap, diags) = embedded(
        b"/GBK-EUC-H usecmap
          begincodespacerange <0000> <ffff> endcodespacerange
          1 begincidchar <20> <100> endcidchar",
    );
    assert!(diags.contains(&DiagKind::CMapUsecmapIgnored));
    // Only the explicit override maps.
    assert_eq!(cmap.cid(CharCode(0x20)), Cid(0x100));
    // A code GBK-EUC-H covers well is nevertheless unmapped here.
    let (gbk, _) = resolve("GBK-EUC-H");
    assert_ne!(gbk.cid(CharCode(0xA1A1)), Cid(0));
    assert_eq!(cmap.cid(CharCode(0xA1A1)), Cid(0));
}

/// An operator appearing where an operand was expected resets the machine,
/// so a truncated block cannot corrupt the one after it.
#[test]
fn an_operator_resets_a_half_finished_block() {
    let (cmap, _) = embedded(b"begincidchar <20> begincidrange <21> <22> <1> endcidrange");
    // The cidchar never got its second operand and wrote nothing.
    assert_eq!(cmap.cid(CharCode(0x20)), Cid(0));
    // The cidrange that interrupted it worked normally.
    assert_eq!(cmap.cid(CharCode(0x21)), Cid(1));
    assert_eq!(cmap.cid(CharCode(0x22)), Cid(2));
}

#[test]
fn wmode_sets_the_writing_direction() {
    for (program, want) in [
        (&b"/WMode 1"[..], true),
        (b"/WMode 0", false),
        (b"/WMode <1>", true),
        (b"/WMode <0>", false),
        // A missing operand reads the next token as a number, and a
        // non-numeric one is zero.
        (b"/WMode endcmap", false),
        (b"/WMode", false),
    ] {
        let (cmap, _) = embedded(program);
        assert_eq!(
            cmap.is_vertical(),
            want,
            "{}",
            String::from_utf8_lossy(program)
        );
    }
}

/// `/Ordering (Japan1)` does *not* set the collection: the operand reader
/// drops two bytes, so the parenthesised string never matches a name. Real
/// files therefore get their collection from the font's `/CIDSystemInfo`.
#[test]
fn ordering_written_as_a_postscript_string_sets_nothing() {
    let (cmap, _) = embedded(b"/Ordering (Japan1) def");
    assert_eq!(cmap.charset(), CidSet::Unknown);

    // A spelling whose bytes happen to line up does set it — this is the only
    // shape that works, and it shows the slice is two bytes and not a decoder.
    let (cmap, _) = embedded(b"/Ordering xxJapan1 def");
    assert_eq!(cmap.charset(), CidSet::Japan1);
}

#[test]
fn registry_and_supplement_operands_are_discarded() {
    let (cmap, _) = embedded(
        b"/Registry (Adobe) def /Supplement 2 def
          1 begincidchar <20> <100> endcidchar",
    );
    assert_eq!(cmap.charset(), CidSet::Unknown);
    assert_eq!(cmap.cid(CharCode(0x20)), Cid(0x100));
}

/// An empty or hopeless program still yields a working CMap.
#[test]
fn a_program_that_says_nothing_still_decodes() {
    for program in [&b""[..], b"%just a comment", b"garbage garbage", b"<<>>"] {
        let (cmap, _) = embedded(program);
        assert_eq!(cmap.coding_scheme(), CodingScheme::TwoBytes);
        assert_eq!(cmap.cid(CharCode(0x41)), Cid(0));
        assert!(!cmap.has_no_direct_table());
        assert!(cmap.is_loaded());
        // And it decodes: two-byte codes, all mapping to CID 0.
        let pairs: Vec<_> = cmap.decode(&[0x00, 0x41]).collect();
        assert_eq!(pairs, vec![(CharCode(0x41), Cid(0))]);
    }
}

/// An embedded CMap always has a dense table, whatever the program said.
#[test]
fn embedded_cmaps_always_have_a_direct_table() {
    let (empty, _) = embedded(b"");
    assert!(!empty.has_no_direct_table());
    assert!(!empty.has_static_map());
    assert_eq!(empty.charcode_from_cid(Cid(1)), CharCode(0));
}

/// Comments and unusual whitespace do not derail a program.
#[test]
fn comments_and_odd_whitespace_are_skipped() {
    let (cmap, _) =
        embedded(b"% a leading comment\n1 begincidchar % mid-block\n<20> <100> endcidchar\n");
    assert_eq!(cmap.cid(CharCode(0x20)), Cid(0x100));
}

/// The range cap drops the excess rather than failing, and says so once.
#[test]
fn the_range_limit_drops_the_excess() {
    let mut program = Vec::from(
        &b"begincodespacerange <00> <7f> <8140> <9ffc> endcodespacerange 1 begincidrange"[..],
    );
    for i in 0..8u32 {
        let start = 0x1_0000 + i * 0x10;
        program.extend(format!(" <{start:x}> <{:x}> <{}>", start + 8, i + 1).into_bytes());
    }
    program.extend(b" endcidrange");
    let limits = Limits {
        max_cmap_ranges: 3,
        ..Limits::default()
    };
    let mut diags = Diagnostics::default();
    let cmap = parse_embedded(&program, &limits, &mut diags);
    assert!(diags.contains(&DiagKind::CMapRangeLimit));
    assert_eq!(cmap.cid(CharCode(0x1_0000)), Cid(1));
    assert_eq!(cmap.cid(CharCode(0x1_0020)), Cid(3));
    // The fourth range and everything after it was dropped.
    assert_eq!(cmap.cid(CharCode(0x1_0030)), Cid(0));
}

/// Decoding never stalls, whatever the program and whatever the string.
#[test]
fn decoding_terminates_on_every_input() {
    let programs: &[&[u8]] = &[
        b"",
        b"begincodespacerange <00> <ff> endcodespacerange",
        b"begincodespacerange <0000> <ffff> endcodespacerange",
        b"begincodespacerange <00> <1f> <8140> <9ffc> endcodespacerange",
        b"begincodespacerange <000000> <ffffff> <00> <ff> endcodespacerange",
    ];
    let inputs: &[&[u8]] = &[
        b"",
        b"A",
        &[0x81],
        &[0x81, 0x40],
        &[0xFF; 9],
        &[0x00, 0x81, 0x40, 0x7F],
    ];
    for program in programs {
        let (cmap, _) = embedded(program);
        for input in inputs {
            let count = cmap.decode(input).count();
            assert!(count <= input.len().max(1));
            assert_eq!(count, cmap.count_chars(input), "{program:?} over {input:?}");
        }
    }
}
