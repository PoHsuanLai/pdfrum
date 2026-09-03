//! `/ToUnicode` parsing and lookup, over [`parse`] / [`ToUnicode`].

// The upstream half of this file is every assertion of
// `cpdf_tounicodemap_unittest.cpp`, restated; the rest are the cases it has no
// test for and Tier-A depends on.

// Test fixtures are fixed-size arrays with known contents.
#![allow(clippy::indexing_slicing)]

use super::*;
use std::fmt::Write as _;

#[test]
fn a_two_entry_bfchar_block_maps_both_ways() {
    let map = parse(
        b"2 beginbfchar <0041> <0061> <0042> <0062> endbfchar",
        &Limits::default(),
        &mut Diagnostics::default(),
    );
    assert_eq!(map.lookup(0x41.into()).as_slice(), ['a']);
    assert_eq!(map.reverse('a').0, 0x41);
}

fn map_of(program: &str) -> ToUnicode {
    parse(
        program.as_bytes(),
        &Limits::default(),
        &mut Diagnostics::default(),
    )
}

fn chars(map: &ToUnicode, code: u32) -> Vec<char> {
    map.lookup(CharCode(code)).into_vec()
}

fn text(map: &ToUnicode, code: u32) -> String {
    chars(map, code).into_iter().collect()
}

// ---------------------------------------------------------------------------
// StringToCode — all 18 rows of the oracle's table (§1.6.3).
// ---------------------------------------------------------------------------

#[test]
fn string_to_code_matches_the_oracle() {
    for (input, expected) in [
        (&b"<0001>"[..], Some(1)),
        (b"<c2>", Some(194)),
        (b"<A2>", Some(162)),
        (b"<Af2>", Some(2802)),
        (b"<FFFFFFFF>", Some(4_294_967_295)),
        (b"<00\n0\r1>", Some(1)),
        (b"<c 2>", Some(194)),
        (b"<A2\r\n>", Some(162)),
        // `u32` overflow rejects outright rather than wrapping.
        (b"<100000000>", None),
        (b"<1abcdFFFF>", None),
        (b"", None),
        // Length 2 fails the `len <= 2` guard even though it is well formed.
        (b"<>", None),
        (b"12", None),
        (b"<12", None),
        (b"12>", None),
        // `-` is neither whitespace nor a hex digit.
        (b"<1-7>", None),
        (b"00AB", None),
        (b"<00NN>", None),
    ] {
        assert_eq!(
            string_to_code(input),
            expected,
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
}

// ---------------------------------------------------------------------------
// StringToWideString — all 14 rows (§1.6.4).
// ---------------------------------------------------------------------------

#[test]
fn string_to_units_matches_the_oracle() {
    for input in [&b""[..], b"1234", b"<c2", b"<c2D2", b"c2ab>"] {
        assert!(
            string_to_units(input).is_empty(),
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
    assert_eq!(string_to_units(b"<c2ab>"), vec![0xC2AB]);
    // A trailing partial group of four is discarded.
    assert_eq!(string_to_units(b"<c2abab>"), vec![0xC2AB]);
    assert_eq!(string_to_units(b"<c2abFaAb>"), vec![0xC2AB, 0xFAAB]);
    assert_eq!(string_to_units(b"<c2abFaAb12>"), vec![0xC2AB, 0xFAAB]);
    // Whitespace anywhere is ignored, including between the digits of one unit.
    assert_eq!(string_to_units(b"<c2ab FaAb>"), vec![0xC2AB, 0xFAAB]);
    assert_eq!(string_to_units(b"<c2ab FaAb12>"), vec![0xC2AB, 0xFAAB]);
    assert_eq!(string_to_units(b"<c2ab FaAb 12>"), vec![0xC2AB, 0xFAAB]);
    assert_eq!(
        string_to_units(b"< c 2 a b  F a A b  1 2 >"),
        vec![0xC2AB, 0xFAAB]
    );
}

// ---------------------------------------------------------------------------
// bfchar (§1.6.5).
// ---------------------------------------------------------------------------

#[test]
fn bfchar_rejects_invalid_cid_values() {
    // Bad hex, and a `u32` overflow: each invalidates the whole block.
    for program in [
        "1 beginbfchar<00NN><0041>endbfchar",
        "1 beginbfchar<100000000><0041>endbfchar",
    ] {
        let map = map_of(program);
        assert_eq!(text(&map, 0), "", "{program}");
        assert_eq!(map.reverse('A').0, 0, "{program}");
        assert_eq!(map.len(), 0, "{program}");
    }
}

#[test]
fn bfchar_commits_nothing_when_the_count_disagrees() {
    // Two entries declared as one, and two declared as three: both discarded.
    for program in [
        "1 beginbfchar<1><0041><2><0042>endbfchar",
        "3 beginbfchar<1><0041><2><0042>endbfchar",
    ] {
        let map = map_of(program);
        assert!(map.is_empty(), "{program}");
        assert_eq!(text(&map, 1), "", "{program}");
        assert_eq!(text(&map, 2), "", "{program}");
    }
}

#[test]
fn bfchar_commits_when_the_count_agrees() {
    let map = map_of("2 beginbfchar<1><0041><2><0042>endbfchar");
    assert_eq!(text(&map, 1), "A");
    assert_eq!(text(&map, 2), "B");
    assert_eq!(map.len(), 2);
}

#[test]
fn bfchar_tolerates_an_out_of_spec_count() {
    // The specification caps a block at 100 entries; PDFium accepts far more
    // because real files exceed it. 112 entries, declared as 112.
    let mut program = String::from("112 beginbfchar");
    for i in 0..112u32 {
        let _ = write!(program, "<{:04X}><{:04X}>", i + 1, 0x0041 + i - 40);
    }
    program.push_str("endbfchar");
    let map = map_of(&program);
    assert_eq!(map.len(), 112);
    // The oracle's own two spot checks, restated for our destination values.
    assert_eq!(chars(&map, 9).len(), 1);
    assert_eq!(chars(&map, 111).len(), 1);
}

#[test]
fn a_count_above_the_out_of_spec_limit_drains_without_committing() {
    let map = map_of("160001 beginbfchar<1><0041>endbfchar");
    assert!(map.is_empty());
    // The block was drained, so a *following* block still parses.
    let map = map_of("160001 beginbfchar<1><0041>endbfchar 1 beginbfchar<2><0042>endbfchar");
    assert_eq!(text(&map, 2), "B");
    assert_eq!(text(&map, 1), "");
}

#[test]
fn a_garbage_count_token_reads_as_zero_and_any_entry_then_invalidates() {
    let map = map_of("abc beginbfchar<1><0041>endbfchar");
    assert!(map.is_empty());
    // ...but a genuinely empty block with a garbage count is fine, because
    // zero collected equals zero expected.
    let map = map_of("abc beginbfchar endbfchar 1 beginbfchar<5><0045>endbfchar");
    assert_eq!(text(&map, 5), "E");
}

// ---------------------------------------------------------------------------
// bfrange — the high-code mask, all six cases (§1.6.6).
// ---------------------------------------------------------------------------

#[test]
fn bfrange_rejects_invalid_cid_values() {
    // 1-3: `<FFFFFFFF> <FFFFFFFF>` in each of the three destination forms.
    // The mask leaves the high code at 0xFFFFFFFF, so `lowcode > kCidLimit`.
    for program in [
        "1 beginbfrange<FFFFFFFF><FFFFFFFF>[<0041>]endbfrange",
        "1 beginbfrange<FFFFFFFF><FFFFFFFF><0041>endbfrange",
        "1 beginbfrange<FFFFFFFF><FFFFFFFF><00410042>endbfrange",
    ] {
        let map = map_of(program);
        assert!(map.is_empty(), "{program}");
    }

    // 4: `<0001> <10000>` masks to high code 0x0000, which is below the low
    // code — the whole block goes.
    let map = map_of("1 beginbfrange<0001><10000><0041>endbfrange");
    assert_eq!(text(&map, 0x0001), "");
    assert_eq!(text(&map, 0xffff), "");
    assert_eq!(text(&map, 0x10000), "");

    // 5: a low code above the CID limit.
    let map = map_of("1 beginbfrange<10000><10001><0041>endbfrange");
    assert!(map.is_empty());

    // 6: `<0006> <0004>` — the mask keeps 0x04, which is below 6.
    let map = map_of("1 beginbfrange<0006><0004><0041>endbfrange");
    for code in 4..=6 {
        assert_eq!(text(&map, code), "");
    }
}

#[test]
fn the_high_code_mask_truncates_a_range_that_crosses_a_block() {
    // `<0100> <02FF>` declares 512 codes; the mask keeps only 0x0100..=0x01FF.
    let map = map_of("1 beginbfrange<0100><02FF><0041>endbfrange");
    assert_eq!(text(&map, 0x0100), "A");
    assert_eq!(
        text(&map, 0x01FF),
        char::from_u32(0x41 + 0xFF).unwrap().to_string()
    );
    // The second half of the declared range never existed.
    assert_eq!(text(&map, 0x0200), "");
    assert_eq!(map.len(), 256);
}

#[test]
fn the_high_code_mask_keeps_a_range_inside_one_block() {
    // `<0010> <00ff>` stays as declared: both are in block 0x00.
    let map = map_of("1 beginbfrange<0010><00ff><0041>endbfrange");
    assert_eq!(map.len(), 240);
}

#[test]
fn bfrange_rejects_a_mismatched_bracket() {
    let map = map_of("1 beginbfrange<3><3>[<0041>}endbfrange");
    assert!(map.is_empty());
}

#[test]
fn bfrange_commits_nothing_when_the_count_disagrees() {
    for program in [
        "1 beginbfrange<1><2><0040><4><5><0050>endbfrange",
        "3 beginbfrange<1><2><0040><4><5><0050>endbfrange",
    ] {
        let map = map_of(program);
        assert!(map.is_empty(), "{program}");
        for code in 0..=6 {
            assert_eq!(text(&map, code), "", "{program} code {code}");
        }
    }
}

#[test]
fn bfrange_commits_when_the_count_agrees() {
    let map = map_of("2 beginbfrange<1><2><0040><4><5><0050>endbfrange");
    assert_eq!(map.reverse('\u{40}').0, 1);
    assert_eq!(map.reverse('\u{41}').0, 2);
    assert_eq!(map.reverse('\u{42}').0, 0);
    assert_eq!(map.reverse('\u{50}').0, 4);
    assert_eq!(map.reverse('\u{51}').0, 5);
    assert_eq!(map.reverse('\u{52}').0, 0);
    for (code, count) in [(0, 0), (1, 1), (2, 1), (3, 0), (4, 1), (5, 1), (6, 0)] {
        assert_eq!(chars(&map, code).len(), count, "code {code}");
    }
}

// ---------------------------------------------------------------------------
// The U+FFFF indicator collision, and OQ-3's measured answers.
// ---------------------------------------------------------------------------

#[test]
fn a_destination_of_exactly_u_ffff_is_unrepresentable() {
    // `HandleBeginBFRangeDestLargeValue`: values run 0xFFF0, 0xFFF1, … and the
    // sixteenth would be 0xFFFF, which is the multi-character indicator — so
    // it is read back as multi-char entry 0, which does not exist.
    let map = map_of("1 beginbfrange<0010><00ff><fff0>endbfrange");
    assert_eq!(text(&map, 0x10), "\u{fff0}");
    assert_eq!(text(&map, 0x11), "\u{fff1}");
    assert_eq!(text(&map, 0x1f), "", "0xFFFF collides with the indicator");

    // **OQ-3(a), measured against the oracle**: the next value is 0x10000,
    // whose low half is 0, so `lookup` returns a single NUL rather than
    // nothing. The oracle's `--txt` for this exact case emits U+0000, which is
    // only reachable through this path — its "no unicode" fallback would have
    // emitted the charcode instead.
    assert_eq!(chars(&map, 0x20), vec!['\0']);
    assert_eq!(chars(&map, 0x21), vec!['\u{1}']);

    assert_eq!(map.reverse('\u{fff0}').0, 16);
    assert_eq!(map.reverse('\u{fff1}').0, 17);
    assert_eq!(map.reverse('\u{ffff}').0, 31);
    // 0x10000 is a real scalar and reverse-maps normally.
    assert_eq!(map.reverse('\u{10000}').0, 32);
}

#[test]
fn a_bfchar_mapping_to_u_ffff_is_lost_the_same_way() {
    let map = map_of("2 beginbfchar<0041><FFFF><0042><0043>endbfchar");
    assert_eq!(text(&map, 0x41), "");
    assert_eq!(text(&map, 0x42), "C");
}

// ---------------------------------------------------------------------------
// StringDataAdd — OQ-3(b), measured.
// ---------------------------------------------------------------------------

#[test]
fn string_data_add_increments_the_last_unit() {
    assert_eq!(string_data_add(&[0x0041]), vec![0x0042]);
    assert_eq!(string_data_add(&[0x00FF]), vec![0x0100]);
    assert_eq!(string_data_add(&[0x0041, 0x0041]), vec![0x0041, 0x0042]);
}

#[test]
fn string_data_add_does_not_carry_at_0xffff() {
    // The design brief describes a base-65536 increment in which 0xFFFF wraps
    // to 0 and carries. It does not: the C++'s `wchar_t` is 32 bits, so
    // 0xFFFF + 1 is 0x10000 and the carry test `ch < str[i-1]` is false.
    // Confirmed by an oracle probe (docs/status/pdfrum-font.md).
    assert_eq!(string_data_add(&[0xFFFF]), vec![0x10000]);
    assert_eq!(string_data_add(&[0x0041, 0xFFFF]), vec![0x0041, 0x10000]);
    assert_eq!(string_data_add(&[0xFFFF, 0xFFFF]), vec![0xFFFF, 0x10000]);
    // The string therefore never lengthens, which is the second half of the
    // brief's claim that also does not hold.
    assert_eq!(string_data_add(&[0xFFFF]).len(), 1);
}

#[test]
fn the_incrementing_bfrange_form_walks_past_0xffff() {
    // `<0041FFFF>` over three codes. The oracle's `--txt` output for exactly
    // this program is [41 FFFF], [41 10000], [41 10001].
    let map = map_of("1 beginbfrange<0030><0032><0041FFFF>endbfrange");
    assert_eq!(chars(&map, 0x30), vec!['A', '\u{ffff}']);
    assert_eq!(chars(&map, 0x31), vec!['A', '\u{10000}']);
    assert_eq!(chars(&map, 0x32), vec!['A', '\u{10001}']);
}

#[test]
fn the_incrementing_bfrange_form_is_plain_for_ordinary_values() {
    let map = map_of("1 beginbfrange<0030><0032><00410041>endbfrange");
    assert_eq!(chars(&map, 0x30), vec!['A', 'A']);
    assert_eq!(chars(&map, 0x31), vec!['A', 'B']);
    assert_eq!(chars(&map, 0x32), vec!['A', 'C']);
}

// ---------------------------------------------------------------------------
// InsertIntoMaps — the three collision scenarios (§1.6.6).
// ---------------------------------------------------------------------------

#[test]
fn distinct_entries_map_both_ways() {
    let map = map_of("2 beginbfchar<1><0041><2><0042>endbfchar");
    assert_eq!(map.reverse('A').0, 1);
    assert_eq!(map.reverse('B').0, 2);
    assert_eq!(map.unicode_count(1), 1);
    assert_eq!(map.unicode_count(2), 1);
}

#[test]
fn one_code_mapping_to_two_unicodes_keeps_the_lower_and_both_reverses() {
    let map = map_of("2 beginbfrange<0><0><0041><0><0><0042>endbfrange");
    assert_eq!(map.reverse('A').0, 0);
    assert_eq!(map.reverse('B').0, 0);
    // Forward keeps the minimum...
    assert_eq!(text(&map, 0), "A");
    // ...while the reverse map holds *two* entries pointing at code 0.
    assert_eq!(map.unicode_count(0), 2);
}

#[test]
fn the_same_pair_declared_twice_is_stored_once() {
    let map = map_of("1 beginbfrange<0><0>[<0041>]endbfrange\n1 beginbfchar<0><0041>endbfchar");
    assert_eq!(map.reverse('A').0, 0);
    assert_eq!(map.unicode_count(0), 1);
}

#[test]
fn lowest_value_wins_in_both_directions() {
    // Declared in the "wrong" order, so the later entry is the smaller one.
    let map = map_of("2 beginbfchar<1><0042><1><0041>endbfchar");
    assert_eq!(text(&map, 1), "A");
    // And two codes onto one unicode keep the lower code.
    let map = map_of("2 beginbfchar<5><0041><3><0041>endbfchar");
    assert_eq!(map.reverse('A').0, 3);
}

// ---------------------------------------------------------------------------
// Non-BMP and surrogates.
// ---------------------------------------------------------------------------

#[test]
fn a_surrogate_pair_becomes_one_char() {
    let map = map_of("1 beginbfchar<01><d841de76>endbfchar");
    assert_eq!(chars(&map, 1), vec!['\u{20676}']);
    // The reverse map is keyed on the packed indicator, not the scalar, so the
    // round trip deliberately does not close.
    assert_eq!(map.reverse('\u{20676}').0, 0);
}

#[test]
fn an_unpaired_surrogate_becomes_the_replacement_character() {
    // Divergence D3: `char` cannot hold a lone surrogate.
    let map = map_of("1 beginbfchar<01><d841 0041>endbfchar");
    assert_eq!(chars(&map, 1), vec!['\u{fffd}', 'A']);
    // A high surrogate as the *only* unit is a single-unit destination, so it
    // goes through the packed path rather than the multi-char one.
    let map = map_of("1 beginbfchar<01><d841>endbfchar");
    assert_eq!(chars(&map, 1), vec!['\u{fffd}']);
}

// ---------------------------------------------------------------------------
// Multi-character accumulation and index shifting (§1.6.1).
// ---------------------------------------------------------------------------

#[test]
fn multi_char_indices_are_assigned_in_declaration_order() {
    let map = map_of("2 beginbfchar<1><00410042><2><00430044>endbfchar");
    assert_eq!(chars(&map, 1), vec!['A', 'B']);
    assert_eq!(chars(&map, 2), vec!['C', 'D']);
}

#[test]
fn a_multi_char_entry_that_loses_the_min_race_still_shifts_later_indices() {
    // Entry 1 wins index 0. Entry 2 collides on code 1 with a *smaller*
    // stored value (the single 0x0041) and is therefore unreachable — but it
    // was still pushed, so entry 3 gets index 2, not index 1.
    let map = map_of("3 beginbfchar<1><00410042><1><0041><9><00430044>endbfchar");
    // Code 1 kept the single-unit value, which is numerically smaller than any
    // indicator.
    assert_eq!(chars(&map, 1), vec!['A']);
    // Code 9's string is still found, which is only true if the wasted push
    // was preserved.
    assert_eq!(chars(&map, 9), vec!['C', 'D']);
}

// ---------------------------------------------------------------------------
// The registry base map.
// ---------------------------------------------------------------------------

#[test]
fn an_adobe_ucs2_token_wires_up_the_registry_table() {
    let map = map_of("/Adobe-Japan1-UCS2 usecmap");
    assert_eq!(map.base_set(), CidSet::Japan1);
    // A miss now consults the registry rather than returning nothing.
    let looked_up = map.lookup(CharCode(34));
    assert_eq!(looked_up.len(), 1);
    // CID 0 is unmapped in every registry and yields the replacement char,
    // which is still a one-element result — callers read non-empty as success.
    assert_eq!(map.lookup(CharCode(0)).len(), 1);
}

#[test]
fn each_of_the_four_registry_tokens_is_recognised() {
    for (token, expected) in [
        ("/Adobe-Korea1-UCS2", CidSet::Korea1),
        ("/Adobe-Japan1-UCS2", CidSet::Japan1),
        ("/Adobe-CNS1-UCS2", CidSet::Cns1),
        ("/Adobe-GB1-UCS2", CidSet::Gb1),
    ] {
        // A `/Name` running to end-of-data yields an empty word and
        // terminates the lexer, so every probe carries the `usecmap`
        // operand that follows it in a real CMap.
        let program = format!("{token} usecmap");
        assert_eq!(map_of(&program).base_set(), expected, "{token}");
    }
    // `/Adobe-Identity-UCS` is deliberately not in the list.
    assert_eq!(
        map_of("/Adobe-Identity-UCS usecmap").base_set(),
        CidSet::Unknown
    );
}

#[test]
fn an_explicit_entry_beats_the_registry_table() {
    let map = map_of("/Adobe-Japan1-UCS2 usecmap 1 beginbfchar<0022><0041>endbfchar");
    assert_eq!(text(&map, 0x22), "A");
}

// ---------------------------------------------------------------------------
// Damage tolerance.
// ---------------------------------------------------------------------------

#[test]
fn whitespace_between_every_token_parses_identically() {
    let tight = map_of("2 beginbfchar<1><0041><2><0042>endbfchar");
    let loose = map_of("2\r\nbeginbfchar\r\n<1>\r\n<0041>\r\n<2>\r\n<0042>\r\nendbfchar\r\n");
    assert_eq!(text(&tight, 1), text(&loose, 1));
    assert_eq!(text(&tight, 2), text(&loose, 2));
    assert_eq!(tight.len(), loose.len());
}

#[test]
fn truncated_programs_yield_an_empty_map_rather_than_hanging() {
    for program in [
        "2 beginbfchar<1><0041>",
        "1 beginbfrange<0><10>[",
        "1 beginbfrange<0><10>[<0041>",
        "1 beginbfchar<0041",
        "beginbfchar",
        "1 beginbfrange<0><ff>",
        "",
        "endbfchar endbfrange",
    ] {
        let map = map_of(program);
        assert!(map.is_empty(), "{program:?} produced {} entries", map.len());
    }
}

#[test]
fn a_rejected_block_is_recorded_as_a_diagnostic() {
    let mut diags = Diagnostics::default();
    let map = parse(
        b"1 beginbfchar<1><0041><2><0042>endbfchar",
        &Limits::default(),
        &mut diags,
    );
    assert!(map.is_empty());
    assert!(diags.contains(&DiagKind::ToUnicodeBlockRejected));
}

#[test]
fn a_clean_program_records_nothing() {
    let mut diags = Diagnostics::default();
    // The map itself is not the point here — the empty diagnostics are.
    let _ = parse(
        b"1 beginbfchar<1><0041>endbfchar",
        &Limits::default(),
        &mut diags,
    );
    assert!(diags.is_empty());
}

#[test]
fn arbitrary_bytes_never_panic() {
    let seeds: [&[u8]; 6] = [
        b"beginbfrange",
        b"\xff\xff\xff\xff beginbfchar <",
        b"999999999999999999999 beginbfchar <1><2> endbfchar",
        b"-1 beginbfrange <0><ff> [ ] endbfrange",
        b"1 beginbfrange<0000><00ff><0000>endbfrange",
        b"<><><><><><>",
    ];
    for seed in seeds {
        let mut diags = Diagnostics::default();
        let map = parse(seed, &Limits::default(), &mut diags);
        // Drive every accessor over a spread of codes.
        for code in [0u32, 1, 0x20, 0xff, 0xffff, 0x10000, u32::MAX] {
            let _ = map.lookup(CharCode(code));
        }
        for ch in ['\0', 'A', '\u{ffff}', '\u{10ffff}'] {
            let _ = map.reverse(ch);
        }
        let _ = map.is_empty();
        let _ = map.base_set();
    }
    // Every single-byte corruption of a good program.
    let good = b"2 beginbfchar<0041><0061><0042><0062>endbfchar".to_vec();
    for i in 0..good.len() {
        for byte in [0u8, b'<', b'>', b'[', 0xff] {
            let mut bad = good.clone();
            bad[i] = byte;
            let map = parse(&bad, &Limits::default(), &mut Diagnostics::default());
            let _ = map.lookup(CharCode(0x41));
        }
    }
}

#[test]
fn a_range_spanning_a_whole_block_is_bounded_by_the_mask() {
    // The array form consumes `high - low + 1` words unconditionally, so the
    // mask is what stops a malformed file from eating the rest of the stream.
    let mut program = String::from("1 beginbfrange<0000><00ff>[");
    for i in 0..256u32 {
        let _ = write!(program, "<{:04X}>", 0x0041 + i);
    }
    program.push_str("]endbfrange");
    let map = map_of(&program);
    assert_eq!(map.len(), 256);
    assert_eq!(text(&map, 0), "A");
}
