//! Inline images: `BI` … `ID` … `EI` (ISO 32000-1 §8.9.7).
//!
//! The quirkiest corner of content parsing. Three things happen that no other
//! operator does:
//!
//! 1. **Abbreviations are expanded**, keys and values from two different
//!    tables, whole-key matches only — so `WW` does *not* become
//!    `WidthWidth`, and `I` means `Interpolate` as a key but `Indexed` as a
//!    value.
//! 2. **The data length is inferred**, not declared. An unfiltered image is
//!    sized from `W * H * BPC * components`; a filtered one is sized by
//!    actually decoding and asking how many source bytes that took. `JPX` and
//!    `JBIG2` can answer neither question, so an inline image using them
//!    produces nothing at all.
//! 3. **The terminator is found by resynchronizing**: everything between the
//!    end of the inferred data and the next standalone `EI` *token* is
//!    absorbed into the image. Whitespace, garbage, and even a `(string
//!    containing EI)` are swallowed, because the tokenizer sees the string as
//!    one element.

use crate::ops::InlineImage;
use crate::tokenize::{ContentLexer, Element};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_filters::{
    Filter, PredictorParams, decode_ascii_hex, decode_ascii85, decode_run_length,
};
use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

/// Key abbreviations, expanded on a whole-key match only.
const KEY_ABBR: [(&[u8], &[u8]); 9] = [
    (b"BPC", b"BitsPerComponent"),
    (b"CS", b"ColorSpace"),
    (b"D", b"Decode"),
    (b"DP", b"DecodeParms"),
    (b"F", b"Filter"),
    (b"H", b"Height"),
    (b"IM", b"ImageMask"),
    (b"I", b"Interpolate"),
    (b"W", b"Width"),
];

/// Value abbreviations, applied to name values only, recursively through
/// arrays and dictionaries.
const VALUE_ABBR: [(&[u8], &[u8]); 11] = [
    (b"G", b"DeviceGray"),
    (b"RGB", b"DeviceRGB"),
    (b"CMYK", b"DeviceCMYK"),
    (b"I", b"Indexed"),
    (b"AHx", b"ASCIIHexDecode"),
    (b"A85", b"ASCII85Decode"),
    (b"LZW", b"LZWDecode"),
    (b"Fl", b"FlateDecode"),
    (b"RL", b"RunLengthDecode"),
    (b"CCF", b"CCITTFaxDecode"),
    (b"DCT", b"DCTDecode"),
];

/// The full spelling of an abbreviated key, or the key unchanged.
#[must_use]
pub fn expand_key_abbreviation(key: &[u8]) -> &[u8] {
    KEY_ABBR
        .iter()
        .find(|(abbr, _)| *abbr == key)
        .map_or(key, |(_, full)| *full)
}

/// The full spelling of an abbreviated name value, or the value unchanged.
#[must_use]
pub fn expand_value_abbreviation(value: &[u8]) -> &[u8] {
    VALUE_ABBR
        .iter()
        .find(|(abbr, _)| *abbr == value)
        .map_or(value, |(_, full)| *full)
}

/// Rewrite a name value and everything nested inside a composite one.
fn expand_value(value: &Object) -> Object {
    match value {
        Object::Name(n) => Object::Name(Name::new(expand_value_abbreviation(n.as_bytes()))),
        Object::Array(a) => Object::Array(a.iter().map(expand_value).collect()),
        Object::Dict(d) => Object::Dict(
            d.iter()
                .map(|(k, v)| (k.clone(), expand_value(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Expand a scanned inline-image dictionary in place.
///
/// The key rename happens first and the value replacement is applied under
/// the *new* key, matching the C++'s two-phase collect-then-apply.
fn expand_dict(dict: &Dict) -> Dict {
    dict.iter()
        .map(|(key, value)| {
            (
                Name::new(expand_key_abbreviation(key.as_bytes())),
                expand_value(value),
            )
        })
        .collect()
}

/// Read one inline image starting just after the `BI` keyword.
///
/// Returns `None` — with the cursor rewound to just after `BI` — when the
/// dictionary scan meets a keyword other than `ID`, and `None` with the
/// cursor left where the scan ended when the data length cannot be
/// determined. Both cases record a diagnostic.
pub(crate) fn read(
    lexer: &mut ContentLexer<'_>,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<InlineImage> {
    let start = lexer.pos();
    let mut dict = Dict::new();

    loop {
        let save = lexer.pos();
        match lexer.next_element() {
            Element::Keyword(word) if &*word != b"ID" => {
                // A keyword that is not `ID` abandons the whole `BI`: the
                // stream is re-read from just after it as ordinary content.
                let _ = save;
                lexer.seek(start);
                diags.record(
                    Severity::Recovered,
                    DiagKind::InlineImageAbandoned,
                    Some(start as u64),
                );
                return None;
            }
            Element::Name(key) => {
                let value = read_dict_value(lexer);
                dict.push(key, value);
            }
            // A keyword `ID` reaches here through the first arm's guard and
            // breaks out, which is the normal exit; so does anything that is
            // neither a keyword nor a name.
            _ => break,
        }
    }

    let dict = expand_dict(&dict);
    let data = read_stream(lexer, &dict, limits, diags)?;

    // A second `EI` scan runs from wherever the length inference left the
    // cursor. For an unfiltered image this is the scan that finds the `EI`;
    // for a filtered one the cursor is usually already past it.
    //
    // **A scan that reaches the end of the stream drops the image.** Upstream
    // spells this as a bare `return` out of `AddImageFromStream`'s caller
    // (`cpdf_streamcontentparser.cpp:691-700`), and it is not a formality:
    // the scan has already consumed everything after the `ID`, so the
    // operators the missing `EI` swallowed are gone whatever we do — and
    // *also* keeping the image would draw a picture the oracle does not.
    // `bug_412524377.in`'s second page is exactly this, and its comment says
    // so: "This page only renders as blue due to the missing EI operator."
    if scan_for_ei(lexer) == EiScan::EndOfData {
        diags.record(
            Severity::Recovered,
            DiagKind::InlineImageAbandoned,
            Some(start as u64),
        );
        return None;
    }

    let mut dict = dict;
    // `/Subtype /Image` is established rather than written back into the
    // parsed object (design brief D3).
    if dict.raw(&Name::from("Subtype")).is_none() {
        dict.push(Name::from("Subtype"), Object::Name(Name::from("Image")));
    }
    Some(InlineImage {
        dict,
        data: data.into_boxed_slice(),
    })
}

/// Read one dictionary value with the object grammar, as `BI`'s scan does.
fn read_dict_value(lexer: &mut ContentLexer<'_>) -> Object {
    match lexer.next_element() {
        Element::Number(n) => Object::Real(n),
        Element::Name(n) => Object::Name(n),
        Element::Object(o) => o,
        Element::Keyword(_) | Element::Eof => Object::Null,
    }
}

/// The first filter of the chain, which is the only one length inference
/// honours, plus its parameters.
fn first_filter(dict: &Dict) -> (Option<Name>, Dict) {
    let params = dict.raw(&Name::from("DecodeParms"));
    match dict.raw(&Name::from("Filter")) {
        Some(Object::Name(n)) => (
            Some(n.clone()),
            match params {
                Some(Object::Dict(d)) => d.clone(),
                _ => Dict::new(),
            },
        ),
        Some(Object::Array(a)) => (
            a.name_at(0).cloned(),
            match params.and_then(Object::as_array).and_then(|p| p.raw_at(0)) {
                Some(Object::Dict(d)) => d.clone(),
                _ => Dict::new(),
            },
        ),
        _ => (None, Dict::new()),
    }
}

/// Take the image's bytes, deciding how many there are.
fn read_stream(
    lexer: &mut ContentLexer<'_>,
    dict: &Dict,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Vec<u8>> {
    // Exactly one whitespace byte after `ID` is skipped. A second becomes
    // image data.
    if matches!(
        lexer.data().get(lexer.pos()),
        Some(0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
    ) {
        lexer.seek(lexer.pos() + 1);
    }
    let data_start = lexer.pos();
    let rest = lexer.data().get(data_start..).unwrap_or(&[]);

    let (filter, params) = first_filter(dict);
    let width = dict.int(&Name::from("Width"), &NoResolve).unwrap_or(0);
    let height = dict.int(&Name::from("Height"), &NoResolve).unwrap_or(0);

    let size = match filter.as_ref().and_then(Filter::from_name) {
        // Unfiltered: the sample count decides, clamped to what is left.
        None if filter.is_none() => {
            let (bpc, comps) = unfiltered_sample_shape(dict);
            let pitch = pitch8(bpc, comps, width)?;
            let total = pitch.checked_mul(u64::try_from(height).ok()?)?;
            usize::try_from(total).ok()?.min(rest.len())
        }
        // A filter name we do not know at all: no decoder, no length.
        None => {
            diags.record(
                Severity::Suspicious,
                DiagKind::InlineImageUnsupported,
                Some(data_start as u64),
            );
            return None;
        }
        Some(f) => decoded_source_len(f, rest, &params, width, height, limits, diags)?,
    };

    lexer.seek(data_start + size);
    // Absorb everything up to the next standalone `EI` token. A scan that runs
    // out of stream instead drops the image and keeps the cursor at the end,
    // which is a recovery and not a silence.
    let Some(absorbed) = absorb_to_ei(lexer, data_start + size) else {
        diags.record(
            Severity::Recovered,
            DiagKind::InlineImageAbandoned,
            Some(data_start as u64),
        );
        return None;
    };
    if absorbed > 0 {
        diags.record(
            Severity::Recovered,
            DiagKind::InlineImageResync,
            Some(data_start as u64),
        );
    }
    let end = (data_start + size + absorbed).min(lexer.data().len());
    let data = lexer.data().get(data_start..end).unwrap_or(&[]).to_vec();
    lexer.seek(end);
    Some(data)
}

/// The bits-per-component and component count length inference assumes.
///
/// Both default to **1** and only move when a `/ColorSpace` is present — so a
/// `/BPC 8` grayscale image with no `/CS` is sized as one bit per pixel,
/// which is exactly PDFium's behaviour and exactly why such files leave
/// surplus bytes to be tokenized as content.
fn unfiltered_sample_shape(dict: &Dict) -> (i64, i64) {
    let Some(cs) = dict.raw(&Name::from("ColorSpace")) else {
        return (1, 1);
    };
    let comps = match cs {
        Object::Name(n) => match n.as_bytes() {
            b"DeviceGray" => 1,
            b"DeviceCMYK" => 4,
            // `/DeviceRGB`, and every name that will not resolve here, count
            // as three — which is the C++'s fallback for a failed load.
            _ => 3,
        },
        // An `/Indexed` array is one component per sample.
        Object::Array(a) if a.name_at(0).is_some_and(|n| n.as_bytes() == b"Indexed") => 1,
        _ => 3,
    };
    let bpc = dict
        .int(&Name::from("BitsPerComponent"), &NoResolve)
        .unwrap_or(0);
    (bpc, comps)
}

/// `CalculatePitch8`: bytes per scanline, `None` on overflow or a negative
/// dimension.
fn pitch8(bpc: i64, comps: i64, width: i64) -> Option<u64> {
    if bpc < 0 || comps < 0 || width < 0 {
        return None;
    }
    let bits = u64::try_from(bpc)
        .ok()?
        .checked_mul(u64::try_from(comps).ok()?)?
        .checked_mul(u64::try_from(width).ok()?)?;
    Some(bits.checked_add(7)? / 8)
}

/// How many source bytes the first filter consumes.
///
/// The three byte-oriented filters report this directly. Flate and LZW do
/// not, so their whole remainder is taken and the `EI` scan finds the end —
/// which is what PDFium's `bytes_consumed` gives for a stream that decodes to
/// completion anyway. `DCTDecode` and `CCITTFaxDecode` are sized from their
/// own framing. `JPXDecode` and `JBIG2Decode` can be sized by neither, and
/// PDFium refuses them outright.
fn decoded_source_len(
    filter: Filter,
    data: &[u8],
    params: &Dict,
    width: i64,
    height: i64,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<usize> {
    match filter {
        Filter::AsciiHex => Some(decode_ascii_hex(data).1),
        Filter::Ascii85 => decode_ascii85(data).ok().map(|(_, used)| used),
        Filter::RunLength => decode_run_length(data, diags).ok().map(|(_, used)| used),
        // Flate and LZW report no consumed count, and a `/Crypt` filter is
        // the identity: all three take the remainder and let the `EI` scan
        // find the end.
        Filter::Flate | Filter::Lzw | Filter::Crypt | Filter::CcittFax => {
            let _ = (params, limits, width, height);
            Some(data.len())
        }
        Filter::Dct => jpeg_frame_len(data).or(Some(data.len())),
        Filter::Jpx | Filter::Jbig2 => {
            diags.record(Severity::Suspicious, DiagKind::InlineImageUnsupported, None);
            None
        }
    }
}

/// The length of a JPEG datastream: everything up to and including `FFD9`.
///
/// PDFium learns this by decoding every scanline and asking the decoder how
/// far it read; scanning for the end-of-image marker reaches the same byte
/// for a well-formed stream and degrades to "take the rest" for a truncated
/// one, which the `EI` scan then corrects.
fn jpeg_frame_len(data: &[u8]) -> Option<usize> {
    let mut i = 2usize; // past the SOI
    while i + 1 < data.len() {
        if data.get(i) == Some(&0xFF) && data.get(i + 1) == Some(&0xD9) {
            return Some(i + 2);
        }
        i += 1;
    }
    None
}

/// Walk tokens from `from`, adding the span of every non-`EI` element to the
/// image, until a standalone `EI` keyword or the end of the data.
///
/// `None` means the data ran out first, which fails the whole inline image —
/// and **leaves the cursor at the end**, because everything after the `ID` has
/// by then been consumed looking for the terminator that never came. Rewinding
/// instead makes the parser read the image's own sample bytes as operators,
/// which on `bug_412524377.in`'s second page finds a `re f` inside them and
/// paints a rectangle the oracle does not.
fn absorb_to_ei(lexer: &mut ContentLexer<'_>, from: usize) -> Option<usize> {
    let mut absorbed = 0usize;
    let mut cursor = from;
    lexer.seek(from);
    loop {
        let before = lexer.pos();
        match lexer.next_element() {
            Element::Eof => return None,
            Element::Keyword(word) if &*word == b"EI" => {
                lexer.seek(cursor);
                return Some(absorbed);
            }
            _ => {
                let after = lexer.pos();
                absorbed = absorbed.saturating_add(after.saturating_sub(before));
                cursor = after;
            }
        }
    }
}

/// How the scan for a closing `EI` ended.
///
/// The distinction is the whole reason this returns anything: an image whose
/// `EI` never arrives is **not drawn**, and the caller cannot tell which
/// happened from the cursor alone — both leave it at the end of what the scan
/// consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EiScan {
    /// A standalone `EI` keyword closed the image.
    Closed,
    /// The stream ran out first.
    EndOfData,
}

/// Consume tokens until a standalone `EI` or the end of the data.
fn scan_for_ei(lexer: &mut ContentLexer<'_>) -> EiScan {
    loop {
        match lexer.next_element() {
            Element::Eof => return EiScan::EndOfData,
            Element::Keyword(word) if &*word == b"EI" => return EiScan::Closed,
            _ => {}
        }
    }
}

/// The predictor parameters an inline image's `/DecodeParms` names, for the
/// image path to reuse without re-reading abbreviations.
#[must_use]
pub fn inline_predictor_params(dict: &Dict) -> PredictorParams {
    let (_, params) = first_filter(dict);
    PredictorParams::from_dict(&params, &NoResolve).unwrap_or_default()
}

/// The filter names an inline image declares, expanded, in order.
#[must_use]
pub fn inline_filters(dict: &Dict) -> Vec<Name> {
    match dict.raw(&Name::from("Filter")) {
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).cloned().collect(),
        _ => Vec::new(),
    }
}

/// An inline image dictionary rebuilt as a standalone image `XObject`
/// dictionary, so the image path never needs to know it came from `BI`.
#[must_use]
pub fn as_xobject_dict(image: &InlineImage) -> Dict {
    let mut dict = image.dict.clone();
    dict.push(
        Name::from("Length"),
        Object::Int(i64::try_from(image.data.len()).unwrap_or(0)),
    );
    dict
}

/// The `/ColorSpace` name an inline image gives, when it is a name that needs
/// resolving through the page's `/ColorSpace` resources.
///
/// The three device names resolve on their own; everything else is a resource
/// lookup, and a name that resolves to nothing is left in place to fail later.
#[must_use]
pub fn inline_colorspace_name(dict: &Dict) -> Option<Name> {
    match dict.raw(&Name::from("ColorSpace")) {
        Some(Object::Name(n))
            if !matches!(
                n.as_bytes(),
                b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" | b"G" | b"RGB" | b"CMYK"
            ) =>
        {
            Some(n.clone())
        }
        _ => None,
    }
}

/// Substitute a resolved colorspace object into an inline image's dictionary.
#[must_use]
pub fn with_colorspace(dict: &Dict, cs: Object) -> Dict {
    let mut out = Dict::new();
    let mut replaced = false;
    for (k, v) in dict.iter() {
        if k.as_bytes() == b"ColorSpace" {
            out.push(k.clone(), cs.clone());
            replaced = true;
        } else {
            out.push(k.clone(), v.clone());
        }
    }
    if !replaced {
        out.push(Name::from("ColorSpace"), cs);
    }
    out
}

/// The `/Array`-shaped `/Decode` an inline image declares, if any.
#[must_use]
pub fn inline_decode(dict: &Dict) -> Option<Array> {
    dict.raw(&Name::from("Decode"))
        .and_then(Object::as_array)
        .cloned()
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{expand_key_abbreviation, expand_value_abbreviation};
    use crate::ops::Op;
    use pdfrum_common::{DiagKind, Diagnostics, Limits};
    use pdfrum_object::{Name, NoResolve, Object};

    fn parse(src: &[u8]) -> (Vec<Op>, Diagnostics) {
        let mut diags = Diagnostics::default();
        let ops = crate::parse_content(src, &Limits::default(), &mut diags);
        (ops, diags)
    }

    #[test]
    fn key_abbreviations_match_whole_keys_only() {
        assert_eq!(expand_key_abbreviation(b"BPC"), b"BitsPerComponent");
        assert_eq!(expand_key_abbreviation(b"W"), b"Width");
        assert_eq!(expand_key_abbreviation(b""), b"");
        assert_eq!(expand_key_abbreviation(b"NoInList"), b"NoInList");
        // A prefix must not match: `WW` stays `WW`.
        assert_eq!(expand_key_abbreviation(b"WW"), b"WW");
    }

    #[test]
    fn value_abbreviations_match_whole_values_only() {
        assert_eq!(expand_value_abbreviation(b"G"), b"DeviceGray");
        assert_eq!(expand_value_abbreviation(b"DCT"), b"DCTDecode");
        assert_eq!(expand_value_abbreviation(b""), b"");
        assert_eq!(expand_value_abbreviation(b"NoInList"), b"NoInList");
        assert_eq!(expand_value_abbreviation(b"II"), b"II");
    }

    #[test]
    fn i_is_interpolate_as_a_key_and_indexed_as_a_value() {
        assert_eq!(expand_key_abbreviation(b"I"), b"Interpolate");
        assert_eq!(expand_value_abbreviation(b"I"), b"Indexed");
    }

    #[test]
    fn an_inline_image_with_no_ei_takes_the_rest_of_the_stream_with_it() {
        // `bug_412524377.in`'s second page, whose own comment reads "This page
        // only renders as blue due to the missing EI operator". The scan for
        // `EI` consumes everything after the `ID` looking for a terminator
        // that never comes, so the image is dropped *and* so is every
        // operator after it — including the green rectangle here, which the
        // oracle does not paint.
        let (ops, diags) = parse(
            b"0 0 1 rg 0 0 200 200 re f\n              BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\x66\xcc\xff\n              0 1 0 rg 100 0 100 100 re f",
        );
        assert!(
            !ops.iter().any(|op| matches!(op, Op::InlineImage(_))),
            "the unterminated image is not emitted"
        );
        // The blue rectangle before the `BI` survives; nothing after it does.
        let fills = ops.iter().filter(|op| matches!(op, Op::Fill())).count();
        assert_eq!(
            fills, 1,
            "only the fill before the BI is parsed, got {ops:?}"
        );
        assert!(
            diags
                .entries()
                .iter()
                .any(|d| d.what == DiagKind::InlineImageAbandoned),
            "and the recovery is recorded rather than silent"
        );
    }

    #[test]
    fn an_inline_image_that_does_close_leaves_the_rest_of_the_stream_alone() {
        // The same content with the `EI` present: the image is emitted and
        // the operators after it are parsed as usual. This is the pair the
        // test above needs — dropping everything is right only when the
        // terminator is genuinely missing.
        let (ops, _) = parse(
            b"0 0 1 rg 0 0 200 200 re f\n              BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\x66\xcc\xff EI\n              0 1 0 rg 100 0 100 100 re f",
        );
        assert!(
            ops.iter().any(|op| matches!(op, Op::InlineImage(_))),
            "a closed image is emitted, got {ops:?}"
        );
        assert_eq!(
            ops.iter().filter(|op| matches!(op, Op::Fill())).count(),
            2,
            "and both fills are parsed"
        );
    }

    #[test]
    fn an_unfiltered_inline_image_is_sized_from_its_samples() {
        // 2x2, 1 bpc, no colorspace: one byte per row, two bytes total.
        let (ops, _) = parse(b"BI /W 2 /H 2 /BPC 1 ID \x00\xff EI Q");
        let Some(Op::InlineImage(img)) = ops.first() else {
            panic!("expected an inline image, got {ops:?}");
        };
        assert_eq!(&*img.data, b"\x00\xff");
        assert_eq!(
            img.dict.int(&Name::from("Width"), &NoResolve),
            Some(2),
            "the /W abbreviation should have been expanded"
        );
        // Parsing continued past the image.
        assert!(matches!(ops.get(1), Some(Op::RestoreState())));
    }

    #[test]
    fn a_non_id_keyword_abandons_the_image() {
        let (ops, diags) = parse(b"BI /W 2 Tj 5 w");
        assert!(diags.contains(&DiagKind::InlineImageAbandoned));
        // The stream is re-read as ordinary content from just after `BI`.
        assert!(ops.iter().any(|op| matches!(op, Op::ShowText(_))));
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::SetLineWidth(w) if (*w - 5.0).abs() < 1e-6))
        );
    }

    #[test]
    fn exactly_one_whitespace_byte_after_id_is_skipped() {
        // Two spaces: the second is image data.
        let (ops, _) = parse(b"BI /W 2 /H 1 /BPC 8 /CS /G ID  \x01 EI");
        let Some(Op::InlineImage(img)) = ops.first() else {
            panic!("expected an inline image, got {ops:?}");
        };
        assert_eq!(&*img.data, b" \x01");
    }

    #[test]
    fn inline_jpx_and_jbig2_produce_nothing() {
        for filter in [&b"/JPXDecode"[..], b"/JBIG2Decode"] {
            let mut src = b"BI /W 2 /H 2 /F ".to_vec();
            src.extend_from_slice(filter);
            src.extend_from_slice(b" ID \x01\x02 EI 3 w");
            let (ops, diags) = parse(&src);
            assert!(
                !ops.iter().any(|op| matches!(op, Op::InlineImage(_))),
                "{filter:?} should produce no image"
            );
            assert!(diags.contains(&DiagKind::InlineImageUnsupported));
        }
    }

    #[test]
    fn a_string_containing_ei_inside_the_data_is_absorbed() {
        // The sample count says two bytes; the `(EI)` after it is one token,
        // so it is swallowed and only the standalone `EI` stops the scan.
        let (ops, diags) = parse(b"BI /W 2 /H 2 /BPC 1 ID \x01\x02 (EI) EI");
        let Some(Op::InlineImage(img)) = ops.first() else {
            panic!("expected an inline image, got {ops:?}");
        };
        assert!(img.data.len() > 2, "the trailing token should be absorbed");
        assert!(diags.contains(&DiagKind::InlineImageResync));
    }

    #[test]
    fn an_overstated_length_clamps_to_the_remaining_stream() {
        // 1000x1000 at 1 bpc claims 125000 bytes; only a handful exist.
        let (ops, _) = parse(b"BI /W 1000 /H 1000 /BPC 1 ID \x01\x02\x03");
        // With no `EI` at all the image fails rather than being produced.
        assert!(!ops.iter().any(|op| matches!(op, Op::InlineImage(_))));
    }

    #[test]
    fn value_abbreviations_expand_through_arrays() {
        let (ops, _) = parse(b"BI /W 1 /H 1 /BPC 8 /F [/AHx] ID 41> EI");
        let Some(Op::InlineImage(img)) = ops.first() else {
            panic!("expected an inline image, got {ops:?}");
        };
        let filters = super::inline_filters(&img.dict);
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].as_bytes(), b"ASCIIHexDecode");
    }

    #[test]
    fn subtype_image_is_established() {
        let (ops, _) = parse(b"BI /W 1 /H 1 /BPC 1 ID \x00 EI");
        let Some(Op::InlineImage(img)) = ops.first() else {
            panic!("expected an inline image, got {ops:?}");
        };
        assert_eq!(
            img.dict.raw(&Name::from("Subtype")),
            Some(&Object::Name(Name::from("Image")))
        );
    }
}
