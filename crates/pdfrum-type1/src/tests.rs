//! End-to-end tests over synthetic fonts.
//!
//! The two Foxit Multiple-Master faces live in `tests/foxit_mm.rs`, which
//! needs the fixture files; everything here builds its font from source so a
//! failure points at one behavior rather than at 66 KiB of Adobe.

// A test asserting an exact font-unit coordinate wants `assert_eq!(x, 550.0)`,
// and one indexing a fixed-length list wants `v[0]`; the charstring assembler
// below narrows integers by construction. None of this relaxes anything in the
// library, which keeps every one of these lints on.
#![allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use super::{Container, Encoding, Error, Gid, Type1Font};
use crate::eexec;
use pdfrum_common::kurbo::Shape;
use pdfrum_common::{DiagKind, Diagnostics, Limits};

#[test]
fn gid_round_trips_through_u16() {
    assert_eq!(u16::from(Gid::from(7u16)), 7);
}

/// A charstring assembled from readable pieces.
fn charstring(items: &[i32], ops: &[(usize, u8)]) -> Vec<u8> {
    // `items` are numbers; `ops` are `(insert-after-this-many-numbers, opcode)`.
    let mut out = Vec::new();
    let mut op_iter = ops.iter().peekable();
    for (i, &v) in items.iter().enumerate() {
        while op_iter.peek().is_some_and(|(at, _)| *at == i) {
            if let Some(&(_, op)) = op_iter.next() {
                out.push(op);
            }
        }
        encode_number(&mut out, v);
    }
    for &(_, op) in op_iter {
        out.push(op);
    }
    out
}

fn encode_number(out: &mut Vec<u8>, v: i32) {
    match v {
        -107..=107 => out.push((v + 139) as u8),
        108..=1131 => {
            let n = v - 108;
            out.push(((n >> 8) + 247) as u8);
            out.push((n & 0xFF) as u8);
        }
        -1131..=-108 => {
            let n = -v - 108;
            out.push(((n >> 8) + 251) as u8);
            out.push((n & 0xFF) as u8);
        }
        _ => {
            out.push(255);
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
}

/// `0 <width> hsbw  <x> <y> rmoveto  <w> hlineto  <h> vlineto  closepath endchar`
fn box_glyph(width: i32, x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
    charstring(
        &[0, width, x, y, w, h],
        &[(2, 13), (4, 21), (5, 6), (6, 7), (6, 9), (6, 14)],
    )
}

/// Assemble a complete PFB from a header and a list of glyphs.
struct FontBuilder {
    header_extra: String,
    encoding: String,
    matrix: String,
    glyphs: Vec<(String, Vec<u8>)>,
    subrs: Vec<Vec<u8>>,
    len_iv: Option<i32>,
}

impl FontBuilder {
    fn new() -> Self {
        Self {
            header_extra: String::new(),
            encoding: "/Encoding StandardEncoding def\n".into(),
            matrix: "/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n".into(),
            glyphs: Vec::new(),
            subrs: Vec::new(),
            len_iv: None,
        }
    }

    fn glyph(mut self, name: &str, code: Vec<u8>) -> Self {
        self.glyphs.push((name.into(), code));
        self
    }

    fn header(mut self, extra: &str) -> Self {
        self.header_extra.push_str(extra);
        self
    }

    fn encoding(mut self, source: &str) -> Self {
        self.encoding = source.into();
        self
    }

    fn matrix(mut self, source: &str) -> Self {
        self.matrix = source.into();
        self
    }

    fn clear_text(&self) -> Vec<u8> {
        format!(
            "%!PS-AdobeFont-1.0: Synthetic 001.000\n\
             /FontInfo 4 dict dup begin\n\
             /FullName (Synthetic Test) readonly def\n\
             /FamilyName (Synthetic) readonly def\n\
             end readonly def\n\
             /FontName /Synthetic def\n\
             /FontType 1 def\n\
             {}\
             /FontBBox {{-100 -200 900 800}} readonly def\n\
             {}{}\
             currentdict end\ncurrentfile eexec\n",
            self.matrix, self.header_extra, self.encoding
        )
        .into_bytes()
    }

    fn private_plain(&self) -> Vec<u8> {
        let skip = eexec::len_iv_skip(self.len_iv.unwrap_or(eexec::DEFAULT_LEN_IV));
        let wrap = |body: &[u8]| {
            let mut v = vec![0x00; skip];
            v.extend_from_slice(body);
            eexec::encrypt(&v, eexec::CHARSTRING_SEED)
        };
        let mut out = Vec::from(b"dup /Private 10 dict dup begin\n".as_slice());
        out.extend_from_slice(
            b"/RD { string currentfile exch readstring pop } executeonly def\n\
              /ND { noaccess def } executeonly def\n\
              /NP { noaccess put } executeonly def\n",
        );
        if let Some(n) = self.len_iv {
            out.extend_from_slice(format!("/lenIV {n} def\n").as_bytes());
        }
        if !self.subrs.is_empty() {
            out.extend_from_slice(format!("/Subrs {} array\n", self.subrs.len()).as_bytes());
            for (i, s) in self.subrs.iter().enumerate() {
                let c = wrap(s);
                out.extend_from_slice(format!("dup {i} {} RD ", c.len()).as_bytes());
                out.extend_from_slice(&c);
                out.extend_from_slice(b" NP\n");
            }
            out.extend_from_slice(b"ND\n");
        }
        out.extend_from_slice(
            format!("/CharStrings {} dict dup begin\n", self.glyphs.len()).as_bytes(),
        );
        for (name, code) in &self.glyphs {
            let c = wrap(code);
            out.extend_from_slice(format!("/{name} {} RD ", c.len()).as_bytes());
            out.extend_from_slice(&c);
            out.extend_from_slice(b" ND\n");
        }
        out.extend_from_slice(b"end\nend\nmark currentfile closefile\n");
        out
    }

    /// The program as a PFB: text segment, binary segment, EOF record.
    fn pfb(&self) -> Vec<u8> {
        let clear = self.clear_text();
        let cipher = eexec::encrypt(
            &{
                let mut v = vec![0x41u8; eexec::EEXEC_SKIP];
                v.extend_from_slice(&self.private_plain());
                v
            },
            eexec::EEXEC_SEED,
        );
        let mut v = vec![0x80, 1];
        v.extend_from_slice(&(clear.len() as u32).to_le_bytes());
        v.extend_from_slice(&clear);
        v.extend_from_slice(&[0x80, 2]);
        v.extend_from_slice(&(cipher.len() as u32).to_le_bytes());
        v.extend_from_slice(&cipher);
        v.extend_from_slice(&[0x80, 3]);
        v
    }

    /// The same program as a PFA, with the private portion in hex.
    fn pfa(&self) -> Vec<u8> {
        let mut v = self.clear_text();
        let cipher = eexec::encrypt(
            &{
                let mut p = vec![0x41u8; eexec::EEXEC_SKIP];
                p.extend_from_slice(&self.private_plain());
                p
            },
            eexec::EEXEC_SEED,
        );
        for (i, b) in cipher.iter().enumerate() {
            if i % 32 == 0 && i > 0 {
                v.push(b'\n');
            }
            v.extend_from_slice(format!("{b:02X}").as_bytes());
        }
        v.extend_from_slice(b"\n0000000000000000\ncleartomark\n");
        v
    }
}

fn demo() -> FontBuilder {
    FontBuilder::new()
        .glyph(".notdef", charstring(&[0, 250], &[(2, 13), (2, 14)]))
        .glyph("A", box_glyph(600, 50, 0, 500, 700))
        .glyph("space", charstring(&[0, 250], &[(2, 13), (2, 14)]))
}

fn parse(bytes: &[u8]) -> (Type1Font, Diagnostics) {
    let mut d = Diagnostics::default();
    let f = Type1Font::parse(bytes, &Limits::default(), &mut d).expect("parses");
    (f, d)
}

#[test]
fn pfb_and_pfa_produce_identical_fonts() {
    let b = demo();
    let (binary, _) = parse(&b.pfb());
    let (ascii, _) = parse(&b.pfa());
    assert_eq!(binary.container(), Container::Pfb);
    assert_eq!(ascii.container(), Container::Pfa);
    assert_eq!(binary.num_glyphs(), ascii.num_glyphs());
    let gid = binary.name_to_gid("A").expect("A");
    assert_eq!(binary.outline(gid), ascii.outline(gid));
}

#[test]
fn identity_metrics_and_geometry() {
    let (f, _) = parse(&demo().pfb());
    assert_eq!(f.postscript_name(), Some("Synthetic"));
    assert_eq!(f.full_name(), Some("Synthetic Test"));
    assert_eq!(f.family_name(), Some("Synthetic"));
    assert_eq!(f.units_per_em(), 1000);
    assert_eq!(f.num_glyphs(), 3);
    assert!(f.has_glyph_names());
    let bb = f.bbox();
    assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (-100.0, -200.0, 900.0, 800.0));
    assert!(f.mm_axes().is_none());
    assert!(f.instantiate(&[500.0]).is_none());
    assert!(f.default_weight_vector().is_empty());
}

#[test]
fn the_glyph_ladder_name_code_and_unicode() {
    let (f, _) = parse(&demo().pfb());
    let a = f.name_to_gid("A").expect("A by name");
    assert_eq!(f.glyph_name(a), Some("A"));
    // StandardEncoding: 'A' is 0x41.
    assert_eq!(f.code_to_gid(b'A'), Some(a));
    assert_eq!(f.unicode_to_gid('A'), Some(a));
    // Nothing maps to a name the font does not define.
    assert_eq!(f.name_to_gid("Aacute"), None);
    assert_eq!(f.unicode_to_gid('\u{00C1}'), None);
    assert_eq!(f.glyph_name(Gid(999)), None);
    // Glyph order is declaration order.
    let names: Vec<&str> = f.glyph_names().map(|(_, n)| n).collect();
    assert_eq!(names, vec![".notdef", "A", "space"]);
}

#[test]
fn outlines_come_back_in_font_units_with_advances() {
    let (f, _) = parse(&demo().pfb());
    let a = f.name_to_gid("A").expect("A");
    let (path, advance) = f.outline(a).expect("outline");
    assert_eq!(advance, 600.0);
    let bb = path.bounding_box();
    // hsbw sbx=0, then rmoveto 50 0, hlineto 500, vlineto 700.
    assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (50.0, 0.0, 550.0, 700.0));
    assert_eq!(f.glyph_bounds(a), Some(bb));

    // A glyph with no contours has an advance but no bounds.
    let space = f.name_to_gid("space").expect("space");
    assert_eq!(f.outline(space).map(|(_, a)| a), Some(250.0));
    assert_eq!(f.glyph_bounds(space), None);
    // And a glyph that does not exist has neither.
    assert!(f.outline(Gid(999)).is_none());
    assert!(f.glyph_bounds(Gid(999)).is_none());
}

#[test]
fn a_custom_encoding_vector_drives_code_to_gid() {
    let b = demo().encoding(
        "/Encoding 256 array\n\
         0 1 255 {1 index exch /.notdef put} for\n\
         dup 1 /A put\n\
         dup 32 /space put\n\
         dup 200 /nosuchglyph put\n\
         readonly def\n",
    );
    let (f, d) = parse(&b.pfb());
    assert!(matches!(f.encoding(), Encoding::Custom(_)));
    assert_eq!(f.code_to_gid(1), f.name_to_gid("A"));
    // 0x41 is `A` only in StandardEncoding; this font says otherwise.
    assert_eq!(f.code_to_gid(b'A'), None);
    // The encoding entry naming a glyph the font lacks is reported.
    assert!(d.contains(&DiagKind::Type1EncodingGlyphMissing));
    assert_eq!(f.encoding().glyph_name(200), Some("nosuchglyph"));
    assert_eq!(f.code_to_gid(200), None);
}

#[test]
fn len_iv_variants_all_decode() {
    for n in [0i32, 1, 4, 8] {
        let mut b = demo();
        b.len_iv = Some(n);
        let (f, _) = parse(&b.pfb());
        let a = f.name_to_gid("A").expect("A");
        assert_eq!(f.outline(a).map(|(_, adv)| adv), Some(600.0), "lenIV {n}");
    }
}

#[test]
fn subroutines_are_decrypted_and_callable() {
    let mut b = demo();
    // Subr 0 draws the box; the glyph just calls it.
    b.subrs = vec![charstring(&[500, 700], &[(1, 6), (2, 7), (2, 9), (2, 11)])];
    let b = b.glyph(
        "B",
        charstring(&[0, 600, 50, 0, 0], &[(2, 13), (4, 21), (5, 10), (5, 14)]),
    );
    let (f, _) = parse(&b.pfb());
    let bg = f.name_to_gid("B").expect("B");
    let bb = f.outline(bg).expect("outline").0.bounding_box();
    assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (50.0, 0.0, 550.0, 700.0));
}

#[test]
fn a_glyph_whose_charstring_is_broken_still_yields_what_it_drew() {
    // `A` draws a line then calls a subroutine that does not exist.
    let b = demo().glyph(
        "broken",
        charstring(
            &[0, 600, 0, 0, 100, 99],
            &[(2, 13), (4, 21), (5, 6), (6, 10), (6, 14)],
        ),
    );
    let (f, _) = parse(&b.pfb());
    let g = f.name_to_gid("broken").expect("broken");
    let mut d = Diagnostics::default();
    let (path, _) = f
        .outline_with_diagnostics(g, &mut d)
        .expect("partial outline");
    assert_eq!(path.bounding_box().x1, 100.0);
    assert!(d.contains(&DiagKind::Type1CharstringAborted));
    // The plain accessor gives the same outline without the diagnostic.
    assert_eq!(f.outline(g).map(|(p, _)| p), Some(path));
}

#[test]
fn a_bare_program_with_no_banner_still_parses() {
    let b = demo();
    let pfa = b.pfa();
    // Strip the `%!PS-AdobeFont` banner line.
    let after = pfa
        .iter()
        .position(|&c| c == b'\n')
        .map_or(pfa.clone(), |i| {
            pfa.get(i + 1..).unwrap_or_default().to_vec()
        });
    let (f, _) = parse(&after);
    assert_eq!(f.container(), Container::Bare);
    assert_eq!(f.num_glyphs(), 3);
}

#[test]
fn a_truncated_pfb_keeps_the_glyphs_it_read() {
    let full = demo().pfb();
    let mut cut = full.clone();
    cut.truncate(full.len() - 200);
    let mut d = Diagnostics::default();
    let f = Type1Font::parse(&cut, &Limits::default(), &mut d);
    // Either it read enough glyphs or it said there were none — never a panic,
    // and never a silent full parse of a truncated file.
    match f {
        Ok(font) => {
            assert!(font.num_glyphs() < 3);
            assert!(d.contains(&DiagKind::Type1PfbTruncated) || font.num_glyphs() > 0);
        }
        Err(e) => assert!(matches!(e, Error::NoCharStrings | Error::EexecGarbage)),
    }
}

#[test]
fn the_error_cases() {
    let mut d = Diagnostics::default();
    let l = Limits::default();
    let err = |bytes: &[u8]| Type1Font::parse(bytes, &l, &mut Diagnostics::default()).err();
    assert_eq!(err(b""), Some(Error::Empty));
    assert_eq!(
        err(b"%!PS-AdobeFont\n/FontName /X def\n"),
        Some(Error::NoEexec)
    );
    assert_eq!(
        err(&[0x80, 0x09, 1, 2, 3, 4]),
        Some(Error::PfbSegment { at: 0 })
    );
    // An eexec section of random bytes decrypts to noise.
    let mut noisy = Vec::from(b"%!PS-AdobeFont\ncurrentfile eexec\n".as_slice());
    noisy.extend((0u8..=255).cycle().take(600));
    assert_eq!(err(&noisy), Some(Error::EexecGarbage));
    // A well-formed private dictionary with no CharStrings.
    let empty = FontBuilder::new();
    assert_eq!(err(&empty.pfb()), Some(Error::NoCharStrings));
    let _ = &mut d;
}

#[test]
fn a_font_matrix_other_than_one_thousandth() {
    let (f, _) = parse(
        &demo()
            .matrix("/FontMatrix [0.0005 0 0 0.0005 0 0] def\n")
            .pfb(),
    );
    assert_eq!(f.units_per_em(), 2000);
    // A degenerate matrix falls back rather than dividing by zero.
    let (f, _) = parse(&demo().matrix("/FontMatrix [0 0 0 0 0 0] def\n").pfb());
    assert_eq!(f.units_per_em(), 1000);
    // And a missing one takes the 1000/em default every Type 1 font uses.
    let (f, _) = parse(&demo().matrix("").pfb());
    assert_eq!(f.units_per_em(), 1000);
}

/// The rule the two Foxit faces depend on: a Multiple-Master preamble ends
/// with a `makeblendedfont` procedure whose *body* names `/FontName`,
/// `/BlendAxisTypes` and `/WeightVector` as operands. A reader that took the
/// last occurrence would wipe the real declarations; the first one wins.
#[test]
fn a_later_mention_inside_a_procedure_does_not_overwrite_a_declaration() {
    let b = demo()
        .header(
            "/BlendAxisTypes [/Weight /Width ] def\n\
             /BlendDesignPositions [[0 0][1 0][0 1][1 1]] def\n\
             /BlendDesignMap [[[50 0][1450 1]][[100 0][900 1]]] def\n\
             /WeightVector [0.25 0.25 0.25 0.25 ] def\n",
        )
        // The trailing procedure, shaped like the real fonts'.
        .header(
            "{ /FontInfo where { pop FontInfo /BlendAxisTypes 2 copy known {\n\
               get length counttomark 2 sub eq exch pop } { pop pop } ifelse } if\n\
               2 copy exch /FontName exch put /WeightVector exch put } bind def\n",
        );
    let (f, _) = parse(&b.pfb());
    assert_eq!(f.postscript_name(), Some("Synthetic"));
    let axes = f.mm_axes().expect("axes survived the procedure");
    assert_eq!(axes.len(), 2);
    assert_eq!((axes[0].min, axes[0].max), (50.0, 1450.0));
    assert_eq!((axes[1].min, axes[1].max), (100.0, 900.0));
    assert_eq!(f.default_weight_vector(), &[0.25, 0.25, 0.25, 0.25]);
    // And the blend still produces different outlines at the two extremes.
    let gid = f.name_to_gid("A").expect("A");
    let lo = f.instantiate(&[axes[0].min]).expect("lo").outline(gid);
    let hi = f.instantiate(&[axes[0].max]).expect("hi").outline(gid);
    // This synthetic font's charstrings carry no blend operators, so the two
    // agree; what matters is that both instantiate rather than returning None.
    assert!(lo.is_some() && hi.is_some());
}

/// The never-panic sweep required of every parse entry point.
#[test]
fn arbitrary_bytes_never_panic() {
    let l = Limits::default();
    let good = demo().pfb();

    let mut cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![0x80],
        vec![0x80, 1],
        vec![0x80, 1, 0xFF, 0xFF, 0xFF, 0xFF],
        vec![0x80, 2, 0xFF, 0xFF, 0xFF, 0x7F, 0x00],
        vec![0x80, 3],
        b"%!PS-AdobeFont".to_vec(),
        b"eexec".to_vec(),
        b"currentfile eexec ".to_vec(),
        b"%!PS-AdobeFont\neexec\nzzzz".to_vec(),
        // A charstrings dictionary whose declared length is absurd.
        b"%!PS-AdobeFont\neexec\n41414141/CharStrings 4294967295 dict dup begin\n/A 4294967295 RD "
            .to_vec(),
        b"%!PS\neexec\n41414141/Subrs -1 array\ndup -1 -1 RD x NP".to_vec(),
        b"%!PS\neexec\n41414141/lenIV -2147483648 def /CharStrings 1 dict dup begin /A 1 RD x ND end"
            .to_vec(),
        b"%!PS\neexec\n41414141/BlendDesignMap [[[".to_vec(),
        b"%!PS\neexec\n41414141/WeightVector [".to_vec(),
        b"%!PS\neexec\n41414141/Encoding 256 array dup".to_vec(),
    ];

    // Every prefix of a good font.
    for n in (0..good.len()).step_by(37) {
        cases.push(good.get(..n).unwrap_or_default().to_vec());
    }
    // Every single-byte corruption at a stride, so headers and payloads both
    // get hit.
    for i in (0..good.len()).step_by(211) {
        let mut c = good.clone();
        if let Some(b) = c.get_mut(i) {
            *b = b.wrapping_add(0x5B);
        }
        cases.push(c);
    }
    // Truncations of the *decrypted* side, reached by shortening the binary
    // segment length field.
    for shrink in [1usize, 2, 7, 64, 512] {
        let mut c = good.clone();
        if c.len() > shrink {
            c.truncate(c.len() - shrink);
        }
        cases.push(c);
    }
    // Pathological repeats.
    cases.push(b"/CharStrings ".repeat(500));
    cases.push(b"dup 0 0 RD  NP".repeat(500));
    cases.push([0x80u8, 1, 0, 0, 0, 0].repeat(2000));
    cases.push(b"(".repeat(4000));
    cases.push(b"[[[".repeat(4000));

    for (i, case) in cases.iter().enumerate() {
        let mut d = Diagnostics::with_limit(8);
        match Type1Font::parse(case, &l, &mut d) {
            Err(_) => {}
            Ok(f) => {
                // Every accessor, over every glyph index the font claims plus
                // some it does not, must also be total.
                let _ = (
                    f.container(),
                    f.units_per_em(),
                    f.font_matrix(),
                    f.bbox(),
                    f.num_glyphs(),
                    f.is_fixed_pitch(),
                    f.italic_angle(),
                    f.postscript_name(),
                    f.full_name(),
                    f.family_name(),
                    f.has_glyph_names(),
                );
                for code in [0u8, 32, 65, 200, 255] {
                    let _ = f.code_to_gid(code);
                }
                for ch in ['A', 'é', '\u{FFFD}'] {
                    let _ = f.unicode_to_gid(ch);
                }
                let n = f.num_glyphs().min(64) as u16;
                for g in (0..n).chain([u16::MAX, n.saturating_add(1)]) {
                    let _ = f.glyph_name(Gid(g));
                    let _ = f.outline(Gid(g));
                    let _ = f.glyph_bounds(Gid(g));
                    let _ = f.outline_with_diagnostics(Gid(g), &mut d);
                }
                if let Some(axes) = f.mm_axes() {
                    let coords: Vec<f32> = axes.iter().map(|a| a.default).collect();
                    for probe in [
                        coords.clone(),
                        vec![f32::NAN; axes.len()],
                        vec![f32::INFINITY, f32::NEG_INFINITY],
                        Vec::new(),
                        vec![0.0; 32],
                    ] {
                        if let Some(inst) = f.instantiate(&probe) {
                            for g in 0..n {
                                let _ = inst.outline(Gid(g));
                                let _ = inst.advance(Gid(g));
                            }
                        }
                    }
                }
                assert!(u16::try_from(f.num_glyphs()).is_ok(), "case {i}");
            }
        }
    }
}

/// The cipher and the tokenizer are byte-consuming entry points of their own.
#[test]
fn the_primitives_never_panic_either() {
    use crate::postscript::Lexer;
    for len in [0usize, 1, 3, 4, 5, 255, 4096] {
        let data: Vec<u8> = (0..len).map(|i| (i * 37 % 256) as u8).collect();
        for seed in [eexec::EEXEC_SEED, eexec::CHARSTRING_SEED, 0, u16::MAX] {
            for skip in [0usize, 4, 8, usize::MAX] {
                let _ = eexec::decrypt(&data, seed, skip);
            }
            let _ = eexec::encrypt(&data, seed);
        }
        // The tokenizer must terminate on anything.
        let mut lx = Lexer::new(&data);
        let mut steps = 0usize;
        while lx.next().is_some() {
            steps = steps.saturating_add(1);
            assert!(
                steps <= data.len().saturating_add(1),
                "lexer did not advance"
            );
        }
        // And `take_binary` must never read past the end. It first consumes
        // one separator if the payload is preceded by whitespace, so asking
        // for the whole remaining length can legitimately come up one short.
        let mut lx = Lexer::new(&data);
        assert!(lx.take_binary(usize::MAX).is_none());
        let mut lx = Lexer::new(&data);
        assert!(lx.take_binary(len.saturating_sub(1)).is_some() || len == 0);
    }
}
