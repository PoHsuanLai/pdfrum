//! Generator for `tables/unicode.bin`, the committed blob of character-class,
//! bidi and normalization data (`docs/design/pdfrum-text.md` §3.3, D1).
//!
//! The blob is a build product that is **committed to the repository**, the
//! same arrangement `pdfrum-cmap` uses for its CJK CMaps: a normal
//! `cargo build` only reads it, so the crate builds hermetically and offline
//! for anyone without the C++ oracle. Regeneration is opt-in:
//!
//! ```text
//! PDFRUM_REGEN_TEXT_TABLES=1 \
//! PDFRUM_ORACLE_CHECKOUT=/path/to/pdfium-c++ \
//!   cargo build -p pdfrum-text
//! ```
//!
//! Five table groups go in, from two kinds of source:
//!
//! - **Bidi class + mirror index** and the **mirror pair array**, transcribed
//!   from `core/fxcrt/fx_ucddata.inc` and `core/fxcrt/fx_unicode.cpp`.
//! - **The four normalization tables**, from
//!   `core/fpdftext/unicodenormalizationdata.cpp`.
//! - **`u_isalpha` / `u_isalnum` / `u_tolower`**, which are *not* in any
//!   oracle source file: they are ICU calls. Their values arrive as a text
//!   dump produced by linking a three-line probe against the oracle's own
//!   built `libicuuc` (ICU 78 / Unicode 17.0). The exact recipe is in
//!   `tables/PROVENANCE.md`; the dump is expected at
//!   `tables/icu-properties.txt` and is committed beside the blob so the
//!   regeneration is reproducible without rebuilding the oracle.
//!
//! Sourcing all five from the oracle's own data rather than from a Unicode
//! crate is the fidelity requirement: this crate is Tier-A byte-exact, and a
//! newer Unicode revision classifies some code points differently.

// A generator, not library code: a violated invariant must stop the build
// loudly and name the table, which is exactly what a panic does here. The
// no-panic rule (STYLE.md §3) governs the crate's runtime, not its toolchain.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_panics_doc,
    missing_docs,
    reason = "build-time generator: a bad table must abort the build loudly"
)]

use std::path::{Path, PathBuf};

/// The nineteen bidi classes, in the oracle's own numbering
/// (`core/fxcrt/fx_unicode.h`). Order is load-bearing: the value stored in
/// the packed property word is this index.
const BIDI_CLASSES: [&str; 19] = [
    "ON", "L", "R", "AN", "EN", "AL", "NSM", "CS", "ES", "ET", "BN", "S", "WS", "B", "RLO", "RLE",
    "LRO", "LRE", "PDF",
];

fn main() {
    println!("cargo:rerun-if-env-changed=PDFRUM_REGEN_TEXT_TABLES");
    println!("cargo:rerun-if-env-changed=PDFRUM_ORACLE_CHECKOUT");
    println!("cargo:rerun-if-changed=tables/unicode.bin");

    if std::env::var_os("PDFRUM_REGEN_TEXT_TABLES").is_none() {
        return;
    }
    let oracle = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
        PathBuf::from,
    );
    assert!(
        oracle.join("core/fxcrt/fx_ucddata.inc").is_file(),
        "PDFRUM_REGEN_TEXT_TABLES is set but {} holds no fx_ucddata.inc; \
         point PDFRUM_ORACLE_CHECKOUT at a pdfium checkout",
        oracle.display()
    );
    let tables = Path::new(env!("CARGO_MANIFEST_DIR")).join("tables");
    std::fs::create_dir_all(&tables).expect("create tables/");
    generate(&oracle, &tables);
}

fn generate(oracle: &Path, tables: &Path) {
    let ucd = read_ucd(oracle);
    let mirrors = read_mirror_pairs(oracle);
    let norm = read_normalization(oracle);
    let icu = read_icu_properties(tables);

    // Every mirror index in the property table must address the pair array or
    // be the "no mirror" sentinel; the C++ asserts the same at compile time.
    for &word in &ucd {
        let index = usize::from(word >> 5);
        assert!(
            index == 0x1FF || index < mirrors.len(),
            "mirror index {index} out of range for {} pairs",
            mirrors.len()
        );
    }

    let mut blob = Blob::default();
    blob.section(*b"UCDR", &rle(&ucd));
    blob.section(*b"MIRR", &words(&mirrors));
    blob.section(*b"NRMR", &rle(&norm.main));
    blob.section(*b"NRM1", &words(&norm.map1));
    blob.section(*b"NRM2", &words(&norm.map2));
    blob.section(*b"NRM3", &words(&norm.map3));
    blob.section(*b"NRM4", &words(&norm.map4));
    blob.section(*b"ALPH", &ranges(&icu.alpha));
    blob.section(*b"ALNM", &ranges(&icu.alnum));
    blob.section(*b"LOWR", &deltas(&icu.lower));

    let bytes = blob.finish();
    std::fs::write(tables.join("unicode.bin"), &bytes).expect("write unicode.bin");
    std::fs::write(
        tables.join("unicode.manifest.json"),
        manifest(&ucd, &mirrors, &norm, &icu, bytes.len()),
    )
    .expect("write manifest");
}

// ---------------------------------------------------------------------------
// Blob assembly

/// A tagged-section container: a four-byte magic, a section count, then for
/// each section a four-byte tag, a `u32` length and its payload. The reader
/// walks it with checked indexing and never trusts a length.
#[derive(Default)]
struct Blob {
    sections: Vec<([u8; 4], Vec<u8>)>,
}

impl Blob {
    fn section(&mut self, tag: [u8; 4], payload: &[u8]) {
        self.sections.push((tag, payload.to_vec()));
    }

    fn finish(self) -> Vec<u8> {
        let mut out = b"PDRT".to_vec();
        out.extend_from_slice(&u32::try_from(self.sections.len()).unwrap().to_le_bytes());
        for (tag, payload) in &self.sections {
            out.extend_from_slice(tag);
            out.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
            out.extend_from_slice(payload);
        }
        out
    }
}

/// Run-length encoding over a `u16` table: pairs of `(value, run length)`,
/// both little-endian `u16`. A run longer than `u16::MAX` splits.
fn rle(values: &[u16]) -> Vec<u8> {
    let mut runs: Vec<(u16, u16)> = Vec::new();
    for &value in values {
        match runs.last_mut() {
            Some((v, len)) if *v == value && *len < u16::MAX => *len += 1,
            _ => runs.push((value, 1)),
        }
    }
    let mut out = Vec::with_capacity(runs.len() * 4);
    for (value, len) in runs {
        out.extend_from_slice(&value.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
    }
    out
}

fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Inclusive `(start, end)` code-point ranges as `u32` pairs.
fn ranges(rows: &[(u32, u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.len() * 8);
    for &(start, end) in rows {
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&end.to_le_bytes());
    }
    out
}

/// Inclusive `(start, end, delta)` lowercase runs as two `u32`s and an `i32`.
fn deltas(rows: &[(u32, u32, i32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.len() * 12);
    for &(start, end, delta) in rows {
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&end.to_le_bytes());
        out.extend_from_slice(&delta.to_le_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// Source scanning

/// The 65 536 packed `(mirror << 5) | bidi_class` property words.
fn read_ucd(oracle: &Path) -> Vec<u16> {
    let src = std::fs::read_to_string(oracle.join("core/fxcrt/fx_ucddata.inc"))
        .expect("read fx_ucddata.inc");
    let mut out = Vec::with_capacity(65536);
    for line in src.lines() {
        let Some(rest) = line.trim_start().strip_prefix("CHARPROP____(") else {
            continue;
        };
        let mut fields = rest.split(',');
        let mirror = fields.next().expect("mirror field").trim();
        let mirror = mirror.trim_end_matches('u');
        let mirror = u16::from_str_radix(mirror.trim_start_matches("0x"), 16)
            .unwrap_or_else(|_| panic!("bad mirror index {mirror:?}"));
        // Field 2 is the XFA char type, field 3 the bidi class.
        let bidi = fields.nth(1).expect("bidi field").trim();
        let bidi = bidi.strip_prefix('k').unwrap_or(bidi);
        let class = BIDI_CLASSES
            .iter()
            .position(|name| *name == bidi)
            .unwrap_or_else(|| panic!("unknown bidi class {bidi:?}"));
        out.push((mirror << 5) | u16::try_from(class).unwrap());
    }
    assert_eq!(out.len(), 65536, "fx_ucddata.inc is not 65536 rows");
    out
}

/// `kFXTextLayoutBidiMirror`, the flat pair-encoded mirror array.
fn read_mirror_pairs(oracle: &Path) -> Vec<u16> {
    let src =
        std::fs::read_to_string(oracle.join("core/fxcrt/fx_unicode.cpp")).expect("read fx_unicode");
    let body = brace_body(&src, "kFXTextLayoutBidiMirror[] =");
    hex_words(&body)
}

struct Normalization {
    main: Vec<u16>,
    map1: Vec<u16>,
    map2: Vec<u16>,
    map3: Vec<u16>,
    map4: Vec<u16>,
}

fn read_normalization(oracle: &Path) -> Normalization {
    let src = std::fs::read_to_string(oracle.join("core/fpdftext/unicodenormalizationdata.cpp"))
        .expect("read unicodenormalizationdata.cpp");
    let table = |anchor: &str, expected: usize| -> Vec<u16> {
        let values = hex_words(&brace_body(&src, anchor));
        assert_eq!(values.len(), expected, "{anchor} has the wrong length");
        values
    };
    Normalization {
        main: table("kUnicodeDataNormalization =", 65536),
        map1: table("kUnicodeDataNormalizationMap1 =", 5376),
        map2: table("kUnicodeDataNormalizationMap2 =", 1724),
        map3: table("kUnicodeDataNormalizationMap3 =", 1164),
        map4: table("kUnicodeDataNormalizationMap4 =", 488),
    }
}

#[derive(Default)]
struct IcuProperties {
    /// Inclusive `(start, end)` ranges where `u_isalpha` holds.
    alpha: Vec<(u32, u32)>,
    /// Inclusive `(start, end)` ranges where `u_isalnum` holds.
    alnum: Vec<(u32, u32)>,
    /// Inclusive `(start, end, delta)` runs of a constant lowercase offset.
    lower: Vec<(u32, u32, i32)>,
}

/// Reads the checked-in probe dump: three `[name] count` sections of
/// inclusive code-point ranges, hexadecimal, ascending, one per line.
fn read_icu_properties(tables: &Path) -> IcuProperties {
    let path = tables.join("icu-properties.txt");
    let src = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "{} is missing ({err}); see tables/PROVENANCE.md for the probe recipe",
            path.display()
        )
    });
    let mut props = IcuProperties::default();
    let mut section = "";
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            section = rest.split(']').next().expect("closing bracket");
            continue;
        }
        let mut fields = line.split_whitespace();
        let mut hex = || {
            let field = fields.next().expect("a range needs two bounds");
            u32::from_str_radix(field, 16).unwrap_or_else(|_| panic!("bad bound {field:?}"))
        };
        let (start, end) = (hex(), hex());
        assert!(start <= end, "range {start:X}..{end:X} is inverted");
        match section {
            "alpha" => props.alpha.push((start, end)),
            "alnum" => props.alnum.push((start, end)),
            "lower" => {
                let delta: i32 = fields
                    .next()
                    .expect("a lowercase run needs a delta")
                    .parse()
                    .expect("delta is signed decimal");
                props.lower.push((start, end, delta));
            }
            other => panic!("unknown section {other:?}"),
        }
    }
    for table in [&props.alpha, &props.alnum] {
        assert!(
            table.windows(2).all(|w| w[0].1 < w[1].0),
            "ranges are not ascending and disjoint"
        );
    }
    assert!(!props.alpha.is_empty(), "no alphabetic ranges");
    props
}

/// The text of the outermost brace-balanced initializer after `anchor`.
fn brace_body(src: &str, anchor: &str) -> String {
    let at = src
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor {anchor:?} not found"));
    let start = src[at..].find('{').expect("initializer") + at;
    let mut depth = 0usize;
    for (offset, byte) in src[start..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return src[start + 1..start + offset].to_owned();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced initializer after {anchor:?}")
}

/// Every `0xNNNN` literal in a table body, in order.
fn hex_words(body: &str) -> Vec<u16> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'0' && (bytes[index + 1] | 0x20) == b'x' {
            let start = index + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
                end += 1;
            }
            let text = &body[start..end];
            out.push(
                u16::from_str_radix(text, 16).unwrap_or_else(|_| panic!("bad literal {text:?}")),
            );
            index = end;
        } else {
            index += 1;
        }
    }
    out
}

fn manifest(
    ucd: &[u16],
    mirrors: &[u16],
    norm: &Normalization,
    icu: &IcuProperties,
    blob_len: usize,
) -> String {
    let mut out = String::from("{\n");
    let mut row = |key: &str, value: String, last: bool| {
        out.push_str("  \"");
        out.push_str(key);
        out.push_str("\": ");
        out.push_str(&value);
        out.push_str(if last { "\n" } else { ",\n" });
    };
    row(
        "generator",
        "\"crates/pdfrum-text/build.rs\"".to_owned(),
        false,
    );
    row("ucd_rows", ucd.len().to_string(), false);
    row("mirror_pairs", mirrors.len().to_string(), false);
    row("normalization_main", norm.main.len().to_string(), false);
    row(
        "normalization_maps",
        format!(
            "[{}, {}, {}, {}]",
            norm.map1.len(),
            norm.map2.len(),
            norm.map3.len(),
            norm.map4.len()
        ),
        false,
    );
    row("icu_alpha_ranges", icu.alpha.len().to_string(), false);
    row("icu_alnum_ranges", icu.alnum.len().to_string(), false);
    row("icu_lower_runs", icu.lower.len().to_string(), false);
    row("blob_bytes", blob_len.to_string(), true);
    out.push_str("}\n");
    out
}
