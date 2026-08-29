//! The predefined-CMap path: name lookup, then decoding text through the
//! CMap that came back.
//!
//! The predefined tables are static data, so the fuzzer's leverage is not the
//! table but the *text* run through it — every mixed-two-byte coding scheme
//! has codes the decoder can reach and codes it cannot, and the boundary is
//! where the offset arithmetic lives.
//!
//! Property: `next_char` always advances, never runs past the input, and the
//! iterator and the counter agree on how many codes a string holds.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_object::Name;

/// One name per coding scheme and per direction, plus the two that must
/// *fail* to resolve: an unknown name, and the empty name.
const NAMES: &[&str] = &[
    "Identity-H",
    "Identity-V",
    "GB-EUC-H",
    "GB-EUC-V",
    "UniGB-UCS2-H",
    "B5pc-H",
    "ETen-B5-H",
    "90ms-RKSJ-H",
    "90ms-RKSJ-V",
    "UniJIS-UCS2-H",
    "KSC-EUC-H",
    "UniKS-UCS2-H",
    "",
    "NotACMapName",
];

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let selector = usize::from(split.byte());
    let text = split.rest();

    let mut diags = pdfrum_fuzz::diags();
    let name = Name::new(NAMES[selector % NAMES.len()]);

    // Both entry points: the one that reports a miss, and the one that
    // substitutes a fallback so a font can still be built.
    let reported = pdfrum_cmap::predefined(&name);
    let cmap = pdfrum_cmap::from_encoding_name(&name, &mut diags);
    if reported.is_some() {
        assert!(cmap.is_loaded());
    }

    let mut offset = 0usize;
    while offset < text.len() {
        let before = offset;
        let code = cmap.next_char(text, &mut offset);
        assert!(offset > before, "next_char did not advance at {before}");
        assert!(offset <= text.len(), "next_char ran past the input");

        let cid = cmap.cid(code);
        let _ = cmap.charcode_from_cid(cid);
        let _ = pdfrum_cmap::unicode_from_cid(cmap.charset(), cid);
        let mut round = Vec::new();
        cmap.append_char(&mut round, code);
    }

    assert_eq!(cmap.decode(text).count(), cmap.count_chars(text));
});
