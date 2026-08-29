//! `subset` — arbitrary bytes offered to the font subsetter as a font.
//!
//! Property: never panics, and never returns a glyph map naming a glyph it
//! did not produce.
//!
//! The `subsetter` crate forbids `unsafe` and is well-tested, but it is young
//! and it is the only dependency in this crate's ring that *parses* a
//! structured format on our behalf. What it hands back also becomes the
//! `/W` and `/ToUnicode` keys of a font a reader will trust, so a map that
//! disagrees with the bytes is a correctness bug in our output rather than
//! only a crash in theirs.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_edit::subset;

/// How many glyphs to ask for, at most. A real subset asks for the glyphs one
/// document uses; a fuzzer asking for tens of thousands only measures
/// allocation.
const MAX_GIDS: usize = 64;

fuzz_target!(|data: &[u8]| {
    // The first two bytes choose the glyph list; the rest is the font.
    let (head, font) = data.split_at(data.len().min(2));
    let count = head.first().copied().unwrap_or(1) as usize % MAX_GIDS;
    let stride = u16::from(head.get(1).copied().unwrap_or(1)).max(1);

    let gids: Vec<u16> = (0..count)
        .map(|i| u16::try_from(i).unwrap_or(u16::MAX).saturating_mul(stride))
        .collect();

    let Ok(result) = subset(font, &gids) else {
        return;
    };

    // Glyph 0 is always kept: a font without `.notdef` is malformed, and the
    // subsetter's own output always starts with it.
    assert_eq!(
        result.gid_map.get(0),
        Some(0),
        "`.notdef` must survive and stay at zero"
    );

    // The new identifiers are a contiguous run from zero. Everything
    // PDF-side — `/W`, `/ToUnicode`, an Identity-H content stream's char
    // codes — is re-keyed through this map, so a gap in it would name a
    // glyph the font does not have.
    let mut new: Vec<u16> = result.gid_map.pairs().map(|(_, n)| n).collect();
    new.sort_unstable();
    for (expected, actual) in new.iter().enumerate() {
        assert_eq!(
            u16::try_from(expected).unwrap_or(u16::MAX),
            *actual,
            "the remapped glyph space must be contiguous from zero"
        );
    }

    // Every glyph in the map was one we asked for.
    for (old, _) in result.gid_map.pairs() {
        assert!(
            old == 0 || gids.contains(&old),
            "glyph {old} was never requested"
        );
    }
});
