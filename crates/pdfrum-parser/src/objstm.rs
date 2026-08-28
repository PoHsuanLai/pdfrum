//! Object streams: many small objects packed into one compressed stream
//! (ISO 32000-1 §7.5.7).
//!
//! # Two regions, one stream
//!
//! The decoded bytes are a header of `N` pairs — object number and offset —
//! followed at byte `/First` by the object bodies those offsets index into.
//! Both regions are read leniently, because the header is written by tools
//! and tools get it wrong:
//!
//! - A pair whose object number is not a number is **dropped**, but its
//!   offset token is still consumed. So one garbage number does not shift
//!   every following pair — it removes exactly one entry.
//! - A pair whose *offset* is not a number reads as offset zero and is kept.
//! - Offsets are unsigned, so a written `-1` becomes 4294967295 and simply
//!   never resolves.
//! - Duplicate object numbers are legal, and the cross-reference entry's
//!   index is what disambiguates them.
//!
//! # Why the header can overrun
//!
//! Reading stops after `N` pairs or at the end of the data, whichever comes
//! first — *not* at `/First`. A stream declaring more objects than it holds
//! therefore reads pairs out of the body region, which is how an object
//! stream whose `/N` is too large acquires members nobody wrote.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Object, Resolve, Stream, names};

use crate::lexer::{Lexer, Token, atoui};
use crate::syntax::{Context, Strictness, body};

/// One member of an object stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjStmEntry {
    /// The object's number.
    pub num: u32,
    /// Where its body starts, relative to `/First`.
    pub offset: u32,
}

/// A decoded object stream and its member table.
///
/// Built once per container and cached by the store: decoding a stream is
/// expensive and a document reads most of its objects out of a handful of
/// them.
#[derive(Debug, Clone)]
pub struct ObjStm {
    /// The decoded bytes: header pairs then bodies.
    data: Vec<u8>,
    /// Where the bodies begin.
    first: usize,
    /// The members, in the order the header listed them.
    entries: Vec<ObjStmEntry>,
}

impl ObjStm {
    /// Validate a stream as an object stream and read its member table.
    ///
    /// `None` when the dictionary does not describe one. The checks are all
    /// non-resolving and all type-exact: `/Type` must be the *name* `ObjStm`
    /// (a string spelling it is not), and `/N` and `/First` must be literal
    /// integers, so a float or a reference disqualifies the stream.
    pub(crate) fn build<R: Resolve + ?Sized>(
        stream: &Stream,
        limits: &Limits,
        diags: &mut Diagnostics,
        store: &R,
    ) -> Option<Self> {
        let dict = &stream.dict;
        if dict.name(names::TYPE) != Some(names::OBJ_STM) {
            return None;
        }
        let count = literal_int(dict, names::N)?;
        if count < 0 || count > i64::from(limits.max_object_number) {
            return None;
        }
        let first = literal_int(dict, names::FIRST)?;
        if first < 0 {
            return None;
        }

        let data = crate::decode::decoded_bytes(stream, store, limits, diags);
        let entries = read_header(&data, count.cast_unsigned(), limits, diags);
        Some(Self {
            data,
            first: usize::try_from(first).unwrap_or(usize::MAX),
            entries,
        })
    }

    /// The member table, in header order.
    #[must_use]
    pub fn entries(&self) -> &[ObjStmEntry] {
        &self.entries
    }

    /// Parse the member at `index`, which must also carry object number
    /// `num`.
    ///
    /// Both have to match: the cross-reference entry names each, and a table
    /// whose index and object number disagree is describing a stream that has
    /// since been rewritten.
    pub(crate) fn member<R: Resolve + ?Sized>(
        &self,
        num: u32,
        index: u32,
        limits: &Limits,
        diags: &mut Diagnostics,
        store: &R,
        depth: u32,
    ) -> Option<Object> {
        let entry = self.entries.get(usize::try_from(index).ok()?)?;
        if entry.num != num {
            return None;
        }
        let at = self
            .first
            .checked_add(usize::try_from(entry.offset).ok()?)?;
        if at >= self.data.len() {
            return None;
        }
        let mut lx = Lexer::at(&self.data, at);
        let mut ctx = Context {
            limits,
            diags,
            // No shared file: a member's stream payload, were one somehow
            // present, would have to be copied out of the decoded buffer.
            file: None,
            store: Some(store),
        };
        body(&mut lx, &mut ctx, Strictness::Loose, depth).ok()
    }
}

/// Read a key that must be a literal integer, without resolving.
fn literal_int(dict: &pdfrum_object::Dict, key: &pdfrum_object::Name) -> Option<i64> {
    match dict.raw(key)? {
        Object::Int(v) => Some(*v),
        // A real is not an integer here, even when it has no fraction.
        _ => None,
    }
}

/// Read up to `count` object-number/offset pairs.
fn read_header(
    data: &[u8],
    count: u64,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Vec<ObjStmEntry> {
    let mut entries = Vec::new();
    let mut lx = Lexer::new(data);
    for _ in 0..count {
        if lx.pos() >= data.len() {
            break;
        }
        let num = direct_num(&mut lx, limits);
        let offset = direct_num(&mut lx, limits);
        // Object number zero means nothing, and PDFium drops such a pair
        // while still having consumed its offset — so the pairs that follow
        // stay aligned.
        if num == 0 {
            diags.record(Severity::Suspicious, DiagKind::ObjStmEntryDropped, None);
            continue;
        }
        entries.push(ObjStmEntry { num, offset });
    }
    entries
}

/// Read one number token, or zero when the next word is not one.
///
/// The token is consumed either way, which is what keeps a garbage offset
/// from shifting the pairs that follow it.
fn direct_num(lx: &mut Lexer<'_>, limits: &Limits) -> u32 {
    match lx.next_word(limits) {
        Token::Number(word) => atoui(word),
        // A name, a keyword, punctuation, or the end of the data: the token
        // is spent and the value is nothing.
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{ObjStm, ObjStmEntry};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, Stream, names};

    /// The shape every C++ golden uses: a sixteen-byte header then three
    /// bodies — a dictionary, an array, and a number.
    const NORMAL: &[u8] = b"10 0 11 14 12 21<</Name /Foo>>[1 2 3]4";

    fn dict(pairs: impl IntoIterator<Item = (&'static str, Object)>) -> Dict {
        Dict::from_pairs(pairs.into_iter().map(|(k, v)| (Name::from(k), v)))
    }

    fn objstm_dict(count: i64, first: i64) -> Dict {
        dict([
            ("Type", Object::Name(names::OBJ_STM.clone())),
            ("N", Object::Int(count)),
            ("First", Object::Int(first)),
        ])
    }

    fn build(d: Dict, data: &[u8]) -> Option<ObjStm> {
        let stream = Stream::new(d, ByteSpan::from(data.to_vec()));
        ObjStm::build(
            &stream,
            &Limits::default(),
            &mut Diagnostics::default(),
            &NoResolve,
        )
    }

    fn entries(o: &ObjStm) -> Vec<(u32, u32)> {
        o.entries().iter().map(|e| (e.num, e.offset)).collect()
    }

    fn member(o: &ObjStm, num: u32, index: u32) -> Option<Object> {
        o.member(
            num,
            index,
            &Limits::default(),
            &mut Diagnostics::default(),
            &NoResolve,
            0,
        )
    }

    #[test]
    fn a_normal_stream_indexes_its_members() {
        let o = build(objstm_dict(3, 16), NORMAL).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (11, 14), (12, 21)]);
        assert!(member(&o, 10, 0).expect("member").as_dict().is_some());
        assert!(member(&o, 11, 1).expect("member").as_array().is_some());
        assert!(member(&o, 12, 2).expect("member").as_number().is_some());
    }

    #[test]
    fn both_the_index_and_the_number_must_match() {
        let o = build(objstm_dict(3, 16), NORMAL).expect("object stream");
        for (num, index) in [
            (10, 1),
            (10, 2),
            (10, 3),
            (11, 0),
            (11, 2),
            (11, 3),
            (12, 0),
            (12, 1),
            (12, 3),
        ] {
            assert!(member(&o, num, index).is_none(), "({num}, {index})");
        }
    }

    #[test]
    fn dictionaries_that_do_not_describe_an_object_stream() {
        // No /Type at all.
        assert!(build(Dict::new(), NORMAL).is_none());
        assert!(
            build(
                dict([("N", Object::Int(3)), ("First", Object::Int(5))]),
                NORMAL
            )
            .is_none()
        );
        // /Type as a string rather than a name.
        assert!(
            build(
                dict([
                    (
                        "Type",
                        Object::Str(pdfrum_object::PdfString::literal(b"ObjStm"))
                    ),
                    ("N", Object::Int(3)),
                    ("First", Object::Int(5)),
                ]),
                NORMAL,
            )
            .is_none()
        );
        // A /Type that is a name but the wrong one.
        assert!(
            build(
                dict([
                    ("Type", Object::Name(Name::from("ObjStmmmm"))),
                    ("N", Object::Int(3)),
                    ("First", Object::Int(5)),
                ]),
                NORMAL,
            )
            .is_none()
        );
    }

    #[test]
    fn the_count_must_be_a_literal_nonnegative_integer() {
        let base = |n: Object| {
            dict([
                ("Type", Object::Name(names::OBJ_STM.clone())),
                ("N", n),
                ("First", Object::Int(5)),
            ])
        };
        // Missing.
        assert!(
            build(
                dict([
                    ("Type", Object::Name(names::OBJ_STM.clone())),
                    ("First", Object::Int(5)),
                ]),
                NORMAL,
            )
            .is_none()
        );
        assert!(build(base(Object::Real(2.2)), NORMAL).is_none());
        assert!(build(base(Object::Int(-1)), NORMAL).is_none());
        assert!(build(base(Object::Int(999_999_999)), NORMAL).is_none());
    }

    #[test]
    fn the_first_offset_must_be_a_literal_nonnegative_integer() {
        let base = |f: Object| {
            dict([
                ("Type", Object::Name(names::OBJ_STM.clone())),
                ("N", Object::Int(3)),
                ("First", f),
            ])
        };
        assert!(
            build(
                dict([
                    ("Type", Object::Name(names::OBJ_STM.clone())),
                    ("N", Object::Int(3)),
                ]),
                NORMAL,
            )
            .is_none()
        );
        assert!(build(base(Object::Real(5.5)), NORMAL).is_none());
        assert!(build(base(Object::Int(-5)), NORMAL).is_none());
    }

    #[test]
    fn a_first_past_the_data_parses_the_table_but_no_members() {
        // The data is 38 bytes; /First is 39.
        let o = build(objstm_dict(3, 39), NORMAL).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (11, 14), (12, 21)]);
        assert!(member(&o, 10, 0).is_none());
        assert!(member(&o, 11, 1).is_none());
        assert!(member(&o, 12, 2).is_none());
    }

    #[test]
    fn a_count_smaller_than_the_table_stops_early() {
        let o = build(objstm_dict(2, 16), NORMAL).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (11, 14)]);
        assert!(member(&o, 10, 0).expect("member").as_dict().is_some());
        assert!(member(&o, 11, 1).expect("member").as_array().is_some());
        assert!(member(&o, 12, 2).is_none());
    }

    #[test]
    fn a_count_larger_than_the_table_reads_into_the_bodies() {
        // Nine pairs are asked for; the header holds three, and the reader
        // then picks `2` and `3` out of the array literal `[1 2 3]`.
        let o = build(objstm_dict(9, 16), NORMAL).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (11, 14), (12, 21), (2, 3)]);
        for index in 0..5 {
            assert!(member(&o, 2, index).is_none());
        }
    }

    #[test]
    fn a_garbage_object_number_drops_exactly_one_pair() {
        let data = b"10 0 hi 14 12 21<</Name /Foo>>[1 2 3]4";
        let o = build(objstm_dict(3, 19), data).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (12, 21)]);
    }

    #[test]
    fn a_garbage_offset_reads_as_zero_and_keeps_the_entry() {
        let data = b"10 0 11 hi 12 21<</Name /Foo>>[1 2 3]4";
        let o = build(objstm_dict(3, 16), data).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (11, 0), (12, 21)]);
        // Both members now parse the same bytes.
        assert!(member(&o, 10, 0).expect("member").as_dict().is_some());
        assert!(member(&o, 11, 1).expect("member").as_dict().is_some());
    }

    #[test]
    fn a_negative_offset_wraps_into_an_unreachable_one() {
        let data = b"10 0 11 -1 12 21<</Name /Foo>>[1 2 3]4";
        let o = build(objstm_dict(3, 16), data).expect("object stream");
        assert_eq!(
            o.entries().get(1),
            Some(&ObjStmEntry {
                num: 11,
                offset: 4_294_967_295,
            })
        );
        assert!(member(&o, 11, 1).is_none());
    }

    #[test]
    fn an_offset_past_the_data_yields_no_member() {
        let data = b"10 0 11 999 12 21<</Name /Foo>>[1 2 3]4";
        let o = build(objstm_dict(3, 17), data).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (11, 999), (12, 21)]);
        assert!(member(&o, 11, 1).is_none());
    }

    #[test]
    fn duplicate_object_numbers_are_told_apart_by_index() {
        let data = b"10 0 10 14 12 21<</Name /Foo>>[1 2 3]4";
        let o = build(objstm_dict(3, 16), data).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 0), (10, 14), (12, 21)]);
        assert!(member(&o, 10, 0).expect("member").as_dict().is_some());
        assert!(member(&o, 10, 1).expect("member").as_array().is_some());
        assert!(member(&o, 10, 2).is_none());
        assert!(member(&o, 10, 3).is_none());
        assert!(member(&o, 12, 2).expect("member").as_number().is_some());
    }

    #[test]
    fn object_numbers_need_not_ascend() {
        let data = b"11 0 12 14 10 21<</Name /Foo>>[1 2 3]4";
        let o = build(objstm_dict(3, 16), data).expect("object stream");
        assert_eq!(entries(&o), vec![(11, 0), (12, 14), (10, 21)]);
        assert!(member(&o, 10, 2).expect("member").as_number().is_some());
        assert!(member(&o, 11, 0).expect("member").as_dict().is_some());
        assert!(member(&o, 12, 1).expect("member").as_array().is_some());
    }

    #[test]
    fn offsets_need_not_ascend_either() {
        let data = b"10 21 11 0 12 14<</Name /Foo>>[1 2 3]4";
        let o = build(objstm_dict(3, 16), data).expect("object stream");
        assert_eq!(entries(&o), vec![(10, 21), (11, 0), (12, 14)]);
        assert!(member(&o, 10, 0).expect("member").as_number().is_some());
        assert!(member(&o, 11, 1).expect("member").as_dict().is_some());
        assert!(member(&o, 12, 2).expect("member").as_array().is_some());
    }

    #[test]
    fn never_panics_on_arbitrary_bytes() {
        for data in [
            &b""[..],
            b"0 0 0 0",
            b"999999999999 999999999999",
            b"\xff\x80\x00",
            b"1",
        ] {
            for (n, first) in [(0, 0), (3, 0), (9, 100), (1, 1)] {
                let _ = build(objstm_dict(n, first), data);
            }
        }
    }
}
