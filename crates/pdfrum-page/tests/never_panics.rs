//! Sweeps asserting the three infallible entry points really are.
//!
//! [`parse_content`] is infallible **by contract**, and `decode_image` and
//! `Function::eval` may return an error but must never panic. Every byte
//! reaching them came from an untrusted file, so "never panics" is not a
//! quality goal here — it is the contract STYLE.md §3 states and the fuzz
//! ring enforces continuously. These tests are the cheap version that runs
//! on every commit.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Array, ByteSpan, Dict, Name, NoResolve, Object, PdfString, Stream};
use pdfrum_page::function::FunctionCache;
use pdfrum_page::image::RequestedSize;
use pdfrum_page::{
    BuildContext, Resources, build_page, decode_image, decode_jbig2, decode_jpx, parse_content,
};

/// A small deterministic pseudo-random byte source, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        // xorshift64*, plenty for generating adversarial byte soup.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| u8::try_from(self.next() & 0xFF).unwrap_or(0))
            .collect()
    }

    /// Bytes drawn from an alphabet that makes content-stream syntax likely,
    /// which reaches far deeper than uniform noise.
    fn contentish(&mut self, len: usize) -> Vec<u8> {
        const ALPHABET: &[u8] =
            b"0123456789 .-+/[]<>(){}%\nqQcmwWnfSBbsTjJTfTJTdTDTmT*BTETgsDoshBIIDEIscnrgk";
        (0..len)
            .map(|_| {
                let i = usize::try_from(self.next() % ALPHABET.len() as u64).unwrap_or(0);
                ALPHABET.get(i).copied().unwrap_or(b' ')
            })
            .collect()
    }
}

#[test]
fn parse_content_survives_random_bytes() {
    let limits = Limits::default();
    let mut rng = Rng::new(0x0000_5EED);
    for _ in 0..2000 {
        let len = usize::try_from(rng.next() % 512).unwrap_or(0);
        let bytes = rng.bytes(len);
        let mut diags = Diagnostics::default();
        // The contract is that this returns, whatever the bytes say.
        let _ = parse_content(&bytes, &limits, &mut diags);
    }
}

#[test]
fn parse_content_survives_content_shaped_soup() {
    let limits = Limits::default();
    let mut rng = Rng::new(0x00C0_FFEE);
    for _ in 0..2000 {
        let len = usize::try_from(rng.next() % 1024).unwrap_or(0);
        let bytes = rng.contentish(len);
        let mut diags = Diagnostics::default();
        let _ = parse_content(&bytes, &limits, &mut diags);
    }
}

#[test]
fn parse_content_terminates_on_pathological_nesting() {
    let limits = Limits::default();
    // Deep arrays, deep dictionaries, and unterminated everything: each of
    // these is a real shape a fuzzer finds within seconds.
    let cases: Vec<Vec<u8>> = vec![
        b"[".repeat(10_000),
        b"<<".repeat(10_000),
        b"(".repeat(10_000),
        b"<".repeat(10_000),
        b"{".repeat(10_000),
        b"[/A".repeat(5_000),
        b"<</A".repeat(5_000),
        // Twenty thousand operands before one operator, which exercises the
        // ring's eviction under load.
        {
            let mut v = Vec::new();
            for _ in 0..20_000 {
                v.extend_from_slice(b"1 ");
            }
            v.extend_from_slice(b"cm");
            v
        },
        // An inline image whose declared size vastly exceeds the data.
        b"BI /W 99999 /H 99999 /BPC 8 ID \x00\x01\x02".to_vec(),
        // A `BI` with no `ID` at all.
        b"BI /W 1 /H 1".to_vec(),
    ];
    for bytes in cases {
        let mut diags = Diagnostics::default();
        let _ = parse_content(&bytes, &limits, &mut diags);
    }
}

#[test]
fn build_page_survives_whatever_parse_content_produced() {
    let limits = Limits::default();
    let mut rng = Rng::new(0x0000_BEEF);
    for _ in 0..500 {
        let len = usize::try_from(rng.next() % 512).unwrap_or(0);
        let bytes = rng.contentish(len);
        let mut diags = Diagnostics::default();
        let ops = parse_content(&bytes, &limits, &mut diags);
        let mut ctx = BuildContext::new();
        let _ = build_page(
            &ops,
            &Resources::default(),
            &NoResolve,
            &mut ctx,
            &limits,
            &mut diags,
        );
    }
}

#[test]
fn decode_image_survives_random_dictionaries_and_data() {
    let limits = Limits::default();
    let mut rng = Rng::new(0x0001_A6E5);
    let spaces = [
        Object::Name(Name::from("DeviceGray")),
        Object::Name(Name::from("DeviceRGB")),
        Object::Name(Name::from("DeviceCMYK")),
        Object::Array(Array::of([
            Object::Name(Name::from("Indexed")),
            Object::Name(Name::from("DeviceRGB")),
            Object::Int(3),
            Object::Str(PdfString::literal([0u8; 12])),
        ])),
        Object::Null,
    ];
    let filters = [
        Object::Null,
        Object::Name(Name::from("FlateDecode")),
        Object::Name(Name::from("DCTDecode")),
        Object::Name(Name::from("JPXDecode")),
        Object::Name(Name::from("JBIG2Decode")),
        Object::Name(Name::from("CCITTFaxDecode")),
        Object::Name(Name::from("RunLengthDecode")),
        Object::Name(Name::from("NotAFilter")),
    ];

    for _ in 0..1500 {
        let mut dict = Dict::new();
        let pick = |rng: &mut Rng, n: u64| usize::try_from(rng.next() % n).unwrap_or(0);
        dict.push(
            Name::from("Width"),
            Object::Int(i64::try_from(rng.next() % 40).unwrap_or(0)),
        );
        dict.push(
            Name::from("Height"),
            Object::Int(i64::try_from(rng.next() % 40).unwrap_or(0)),
        );
        dict.push(
            Name::from("BitsPerComponent"),
            Object::Int(i64::try_from(rng.next() % 20).unwrap_or(0)),
        );
        let space = pick(&mut rng, 5);
        if let Some(cs) = spaces.get(space)
            && !cs.is_null()
        {
            dict.push(Name::from("ColorSpace"), cs.clone());
        }
        let filter = pick(&mut rng, 8);
        if let Some(f) = filters.get(filter)
            && !f.is_null()
        {
            dict.push(Name::from("Filter"), f.clone());
        }
        if rng.next().is_multiple_of(4) {
            dict.push(Name::from("ImageMask"), Object::Bool(true));
        }
        if rng.next().is_multiple_of(4) {
            dict.push(
                Name::from("Decode"),
                Object::Array(Array::of([Object::Int(1), Object::Int(0)])),
            );
        }
        if rng.next().is_multiple_of(5) {
            dict.push(
                Name::from("Mask"),
                Object::Array(Array::of([Object::Int(0), Object::Int(10)])),
            );
        }
        let len = usize::try_from(rng.next() % 256).unwrap_or(0);
        let stream = Stream::new(dict, ByteSpan::from(rng.bytes(len)));

        let mut functions = FunctionCache::new();
        let mut diags = Diagnostics::default();
        // An error is fine; a panic is not.
        let _ = decode_image(
            &stream,
            None,
            None,
            RequestedSize::Full,
            &NoResolve,
            &mut functions,
            &limits,
            &mut diags,
        );
    }
}

#[test]
fn the_codec_entry_points_survive_random_bytes() {
    let limits = Limits::default();
    let mut rng = Rng::new(0x000C_0DEC);
    for _ in 0..500 {
        let len = usize::try_from(rng.next() % 512).unwrap_or(0);
        let data = rng.bytes(len);
        let _ = decode_jbig2(None, &data, 16, 16, &limits);
        let _ = decode_jbig2(Some(&data), &data, 16, 16, &limits);
        let _ = decode_jpx(&data, None, 0, 0, &limits);
        let _ = decode_jpx(&data, None, 1, 3, &limits);
    }
}

#[test]
fn function_eval_survives_random_definitions_and_inputs() {
    let limits = Limits::default();
    let mut rng = Rng::new(0x0000_F00D);
    let mut evaluated = 0usize;

    for _ in 0..2000 {
        let kind = i64::try_from(rng.next() % 8).unwrap_or(0);
        let inputs = usize::try_from(rng.next() % 4 + 1).unwrap_or(1);
        let outputs = usize::try_from(rng.next() % 5).unwrap_or(0);
        let mut dict = Dict::new();
        dict.push(Name::from("FunctionType"), Object::Int(kind));
        dict.push(
            Name::from("Domain"),
            Object::Array(Array::of(
                (0..inputs * 2).map(|i| Object::Real(if i % 2 == 0 { 0.0 } else { 1.0 })),
            )),
        );
        if outputs > 0 {
            dict.push(
                Name::from("Range"),
                Object::Array(Array::of(
                    (0..outputs * 2).map(|i| Object::Real(if i % 2 == 0 { 0.0 } else { 1.0 })),
                )),
            );
        }
        dict.push(
            Name::from("N"),
            Object::Real(f32::from(i16::try_from(rng.next() % 7).unwrap_or(1)) - 3.0),
        );
        dict.push(
            Name::from("Size"),
            Object::Array(Array::of((0..inputs).map(|_| Object::Int(2)))),
        );
        dict.push(
            Name::from("BitsPerSample"),
            Object::Int(i64::try_from(rng.next() % 40).unwrap_or(8)),
        );

        let mut cache = FunctionCache::new();
        let mut diags = Diagnostics::default();
        let object = if rng.next().is_multiple_of(2) {
            Object::Dict(dict)
        } else {
            Object::Stream(Stream::new(dict, ByteSpan::from(rng.bytes(64))))
        };
        let Some(function) = cache.load(&object, &NoResolve, &limits, &mut diags) else {
            continue;
        };
        evaluated += 1;

        // Feed it the wrong arity, extreme values, and NaN — none may panic.
        let declared = function.input_count();
        let mut out = vec![0.0f32; function.output_count().max(1)];
        for arity in [0usize, 1, declared, declared + 1, 8] {
            let values: Vec<f32> = (0..arity)
                .map(|i| match i % 5 {
                    0 => 0.0,
                    1 => 1.0,
                    2 => f32::NAN,
                    3 => f32::INFINITY,
                    _ => -1e30,
                })
                .collect();
            let _ = function.eval(&values, &mut out);
            let _ = function.eval_into(&values, &mut out);
        }
        // A short output slice is an error, never a write past the end.
        let _ = function.eval(&vec![0.5f32; declared], &mut []);
    }

    assert!(
        evaluated > 100,
        "the generator should produce loadable functions, got {evaluated}"
    );
}

#[test]
fn postscript_programs_survive_random_source() {
    const TOKENS: &[&str] = &[
        "{",
        "}",
        "add",
        "sub",
        "mul",
        "div",
        "idiv",
        "mod",
        "neg",
        "abs",
        "ceiling",
        "floor",
        "round",
        "truncate",
        "sqrt",
        "sin",
        "cos",
        "atan",
        "exp",
        "ln",
        "log",
        "cvi",
        "cvr",
        "eq",
        "ne",
        "gt",
        "ge",
        "lt",
        "le",
        "and",
        "or",
        "xor",
        "not",
        "bitshift",
        "true",
        "false",
        "pop",
        "exch",
        "dup",
        "copy",
        "index",
        "roll",
        "if",
        "ifelse",
        "1",
        "0",
        "-1",
        "1e30",
        "invalid",
        "%comment\n",
    ];
    let mut rng = Rng::new(0x5CA1_AB1E);
    for _ in 0..2000 {
        let count = usize::try_from(rng.next() % 60).unwrap_or(0);
        let mut program = String::from("{");
        for _ in 0..count {
            let i = usize::try_from(rng.next() % TOKENS.len() as u64).unwrap_or(0);
            program.push_str(TOKENS.get(i).copied().unwrap_or(" "));
            program.push(' ');
        }
        program.push('}');
        let Some(function) =
            pdfrum_page::function::parse_program(program.as_bytes(), &[0.0, 1.0], &[0.0, 1.0])
        else {
            continue;
        };
        let mut out = [0.0f32];
        let mut diags = Diagnostics::default();
        for input in [0.0f32, 1.0, f32::NAN, f32::INFINITY, -1e30] {
            let _ = function.eval_with_diagnostics(&[input], &mut out, &mut diags);
            let _ = function.stack_depth_after(&[input]);
        }
    }
}

#[test]
fn colorspace_loading_survives_random_arrays() {
    let limits = Limits::default();
    let mut rng = Rng::new(0x000C_0104);
    let families = [
        "DeviceGray",
        "DeviceRGB",
        "DeviceCMYK",
        "CalGray",
        "CalRGB",
        "Lab",
        "ICCBased",
        "Indexed",
        "Separation",
        "DeviceN",
        "Pattern",
        "I",
        "NotAFamily",
    ];
    for _ in 0..2000 {
        let len = usize::try_from(rng.next() % 6).unwrap_or(0);
        let elements: Vec<Object> = (0..len)
            .map(|_| match rng.next() % 6 {
                0 => {
                    let i = usize::try_from(rng.next() % families.len() as u64).unwrap_or(0);
                    Object::Name(Name::from(families.get(i).copied().unwrap_or("X")))
                }
                1 => Object::Int(i64::try_from(rng.next() % 512).unwrap_or(0)),
                2 => Object::Str(PdfString::literal([0u8; 8])),
                3 => Object::Dict(Dict::new()),
                4 => Object::Array(Array::new()),
                _ => Object::Null,
            })
            .collect();
        let object = Object::Array(Array::of(elements));
        let mut functions = FunctionCache::new();
        let mut diags = Diagnostics::default();
        let space = pdfrum_page::color::load_colorspace(
            &object,
            None,
            &NoResolve,
            &mut functions,
            &limits,
            &mut diags,
        );
        // Whatever loaded, converting must not panic on any component set.
        if let Some(space) = space {
            for comps in [
                &[][..],
                &[0.0],
                &[1.0, 1.0, 1.0],
                &[f32::NAN; 4],
                &[f32::INFINITY, -1e30, 0.5, 2.0],
            ] {
                let _ = space.to_rgb(comps);
                let _ = space.try_to_rgb(comps, true);
            }
            // And so must the bulk path, at every sample count.
            let mut dest = vec![0u8; 3 * 8];
            let samples = [0u8; 64];
            space.translate_image_line(&mut dest, &samples, 8, false, false);
            space.translate_image_line(&mut dest, &samples, 8, true, true);
            // Including a destination too short to hold the pixels.
            space.translate_image_line(&mut [], &samples, 8, false, false);
        }
    }
}
