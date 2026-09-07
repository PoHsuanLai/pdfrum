//! Generator for `tables/cmaps.bin`, the committed blob of predefined CJK CMap
//! data.
//!
//! The blob is a build product that is **committed to the repository**: a
//! normal `cargo build` only reads it, so the crate builds hermetically and
//! offline for anyone without the C++ oracle checked out. Regeneration is
//! opt-in and reviewable as a binary diff plus a JSON manifest:
//!
//! ```text
//! PDFRUM_REGEN_CMAP_TABLES=1 \
//! PDFRUM_ORACLE_CHECKOUT=/path/to/pdfium-c++ \
//!   cargo build -p pdfrum-cmap
//! ```
//!
//! Everything below runs only in that mode. It parses the 4 registry index
//! tables, the 56 word arrays, the 3 dword arrays and the 4 CID→Unicode
//! arrays out of `core/fpdfapi/cmaps/`, checks every structural invariant the
//! runtime lookup depends on (record counts, sortedness under the comparator
//! `lower_bound` uses, chain reachability and termination), and writes the
//! blob plus `tables/cmaps.manifest.json`.
//!
//! A failed invariant aborts the build naming the offending symbol. None of
//! them is auto-repaired: the runtime reproduces the C++'s `lower_bound` on
//! whatever order the tables are in, so silently re-sorting would change
//! observable CIDs.

// A generator, not library code: a violated invariant must stop the build
// loudly and name the symbol, which is exactly what a panic does here. The
// no-panic rule governs the crate's runtime, not its toolchain.
#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Registries in `CidSet` ordinal order (GB1 = 1 … Korea1 = 4).
const REGISTRIES: [Registry; 4] = [
    Registry {
        dir: "GB1",
        index_file: "cmaps_gb1.inc",
        index_symbol: "kGB1_cmaps",
        cid2unicode_file: "Adobe-GB1-UCS2_5.inc",
        cid2unicode_symbol: "kGB1CID2Unicode_5",
        expect_entries: 14,
        expect_cid2unicode_len: 30284,
    },
    Registry {
        dir: "CNS1",
        index_file: "cmaps_cns1.inc",
        index_symbol: "kCNS1_cmaps",
        cid2unicode_file: "Adobe-CNS1-UCS2_5.inc",
        cid2unicode_symbol: "kCNS1CID2Unicode_5",
        expect_entries: 14,
        expect_cid2unicode_len: 19088,
    },
    Registry {
        dir: "Japan1",
        index_file: "cmaps_japan1.inc",
        index_symbol: "kJapan1_cmaps",
        cid2unicode_file: "Adobe-Japan1-UCS2_4.inc",
        cid2unicode_symbol: "kJapan1CID2Unicode_4",
        expect_entries: 20,
        expect_cid2unicode_len: 15444,
    },
    Registry {
        dir: "Korea1",
        index_file: "cmaps_korea1.inc",
        index_symbol: "kKorea1_cmaps",
        cid2unicode_file: "Adobe-Korea1-UCS2_2.inc",
        cid2unicode_symbol: "kKorea1CID2Unicode_2",
        expect_entries: 11,
        expect_cid2unicode_len: 18352,
    },
];

struct Registry {
    dir: &'static str,
    index_file: &'static str,
    index_symbol: &'static str,
    cid2unicode_file: &'static str,
    cid2unicode_symbol: &'static str,
    expect_entries: usize,
    expect_cid2unicode_len: usize,
}

/// Bytes of fixed header before the index table. Kept in sync with `blob.rs`'s
/// reader constants by the blob-integrity test.
const HEADER: usize = 32;
/// Bytes per index entry.
const ENTRY: usize = 20;

/// One row of a `k<Registry>_cmaps[]` index table.
struct IndexRow {
    name: String,
    word_symbol: String,
    dword_symbol: Option<String>,
    word_count: u16,
    dword_count: u16,
    /// `false` = `kSingle`, `true` = `kRange`.
    is_range: bool,
    use_offset: i8,
}

fn main() {
    println!("cargo:rerun-if-env-changed=PDFRUM_REGEN_CMAP_TABLES");
    println!("cargo:rerun-if-env-changed=PDFRUM_ORACLE_CHECKOUT");
    println!("cargo:rerun-if-changed=tables/cmaps.bin");

    if std::env::var_os("PDFRUM_REGEN_CMAP_TABLES").is_none() {
        return;
    }
    let oracle = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
        PathBuf::from,
    );
    let cmaps = oracle.join("core/fpdfapi/cmaps");
    assert!(
        cmaps.is_dir(),
        "PDFRUM_REGEN_CMAP_TABLES is set but {} is not a directory; \
         point PDFRUM_ORACLE_CHECKOUT at a pdfium checkout",
        cmaps.display()
    );
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("tables");
    std::fs::create_dir_all(&out).expect("create tables/");
    generate(&cmaps, &out);
}

// ---------------------------------------------------------------------------
// Source scanning
// ---------------------------------------------------------------------------

/// Strip `//` and `/* */` comments so the literal scanners never see one.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], bytes.get(i + 1)) {
            (b'/', Some(b'/')) => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            (b'/', Some(b'*')) => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            (c, _) => {
                out.push(char::from(c));
                i += 1;
            }
        }
    }
    out
}

/// The text between the balanced braces of `<symbol>[...] = { ... }`, plus the
/// declared dimension text inside `[...]`.
fn find_array_body<'a>(src: &'a str, symbol: &str) -> Option<(&'a str, String)> {
    // Match the symbol only as a whole identifier.
    let mut from = 0;
    let (decl_start, dim) = loop {
        let at = from + src.get(from..)?.find(symbol)?;
        let before_ok = src[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after = at + symbol.len();
        let after_ok = src[after..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if before_ok && after_ok {
            let rest = &src[after..];
            let lb = rest.find('[')?;
            if rest[..lb].trim().is_empty() {
                let rb = rest.find(']')?;
                break (after + rb + 1, rest[lb + 1..rb].trim().to_owned());
            }
        }
        from = at + symbol.len();
    };
    let rest = &src[decl_start..];
    let open = rest.find('{')?;
    let mut depth = 0i32;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&rest[open + 1..open + i], dim));
                }
            }
            _ => {}
        }
    }
    None
}

/// Every `0x….`/decimal integer literal in `body`, in source order.
fn scan_u16_literals(body: &str, what: &str) -> Vec<u16> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let (radix, start) =
                if bytes[i] == b'0' && matches!(bytes.get(i + 1), Some(b'x' | b'X')) {
                    (16u32, i + 2)
                } else {
                    (10u32, i)
                };
            let mut end = start;
            while end < bytes.len() && char::from(bytes[end]).is_digit(radix) {
                end += 1;
            }
            let text = &body[start..end];
            let v = u32::from_str_radix(text, radix)
                .unwrap_or_else(|e| panic!("{what}: bad literal {text:?}: {e}"));
            let v = u16::try_from(v)
                .unwrap_or_else(|_| panic!("{what}: literal {text:?} does not fit in u16"));
            out.push(v);
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Split an index-table body into its top-level `{...}` rows.
fn split_rows(body: &str) -> Vec<&str> {
    let mut rows = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in body.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start = i + 1;
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    rows.push(&body[start..i]);
                }
            }
            _ => {}
        }
    }
    rows
}

/// Split a row on top-level commas (there are none nested here, but string
/// literals may contain one).
fn split_fields(row: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_string = false;
    for c in row.chars() {
        match c {
            '"' => {
                in_string = !in_string;
                cur.push(c);
            }
            ',' if !in_string => {
                fields.push(cur.trim().to_owned());
                cur = String::new();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        fields.push(cur.trim().to_owned());
    }
    fields
}

fn parse_index(src: &str, symbol: &str) -> Vec<IndexRow> {
    let (body, _) = find_array_body(src, symbol)
        .unwrap_or_else(|| panic!("index array {symbol} not found in its .inc"));
    split_rows(body)
        .into_iter()
        .map(|row| {
            let f = split_fields(row);
            assert_eq!(f.len(), 7, "{symbol}: expected 7 fields, got {f:?}");
            let name = f[0]
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or_else(|| panic!("{symbol}: field 0 {:?} is not a string", f[0]))
                .to_owned();
            let is_range = match f[5].as_str() {
                "CMap::Type::kRange" => true,
                "CMap::Type::kSingle" => false,
                other => panic!("{symbol}/{name}: unknown word_map_type {other:?}"),
            };
            IndexRow {
                name,
                word_symbol: f[1].clone(),
                dword_symbol: (f[2] != "nullptr").then(|| f[2].clone()),
                word_count: f[3].parse().expect("word_count"),
                dword_count: f[4].parse().expect("dword_count"),
                is_range,
                use_offset: f[6].parse().expect("use_offset"),
            }
        })
        .collect()
}

/// Find the `.cpp` in `dir` declaring `symbol` and return its literal values
/// together with the declared `[N * K]` dimension.
fn load_symbol(dir: &Path, symbol: &str) -> (Vec<u16>, String) {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "cpp"))
        .collect();
    candidates.sort();
    for path in candidates {
        let src = strip_comments(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
        );
        if let Some((body, dim)) = find_array_body(&src, symbol) {
            return (scan_u16_literals(body, symbol), dim);
        }
    }
    panic!(
        "array {symbol} not found in any .cpp under {}",
        dir.display()
    );
}

/// Evaluate a declared dimension such as `90 * 3` or `1017`.
fn eval_dim(dim: &str, what: &str) -> usize {
    dim.split('*')
        .map(|p| {
            p.trim()
                .parse::<usize>()
                .unwrap_or_else(|e| panic!("{what}: dimension {dim:?}: {e}"))
        })
        .product()
}

// ---------------------------------------------------------------------------
// Invariant checks
// ---------------------------------------------------------------------------

/// Sortedness under exactly the key `std::lower_bound` compares on. A
/// violation is escalated, never repaired: the runtime must reproduce the
/// C++'s result on the C++'s order.
fn assert_sorted_words(symbol: &str, values: &[u16], is_range: bool) {
    let (stride, key) = if is_range {
        (3usize, 1usize)
    } else {
        (2usize, 0usize)
    };
    let mut prev = None;
    for (i, rec) in values.chunks_exact(stride).enumerate() {
        let k = rec[key];
        if let Some(p) = prev {
            assert!(
                p <= k,
                "{symbol}: record {i} breaks sortedness on the lower_bound key \
                 ({p:#06x} then {k:#06x}); ESCALATE — do not re-sort"
            );
        }
        prev = Some(k);
    }
    if is_range {
        for (i, rec) in values.chunks_exact(3).enumerate() {
            assert!(
                rec[0] <= rec[1],
                "{symbol}: range record {i} has low {:#06x} > high {:#06x}",
                rec[0],
                rec[1]
            );
        }
    }
}

fn assert_sorted_dwords(symbol: &str, values: &[u16]) {
    let mut prev = None;
    for (i, rec) in values.chunks_exact(4).enumerate() {
        let k = (rec[0], rec[2]);
        if let Some(p) = prev {
            assert!(
                p <= k,
                "{symbol}: dword record {i} breaks sortedness on (hi_word, lo_word_high) \
                 ({p:?} then {k:?}); ESCALATE — do not re-sort"
            );
        }
        prev = Some(k);
        assert!(
            rec[1] <= rec[2],
            "{symbol}: dword record {i} has lo_word_low > lo_word_high"
        );
    }
}

/// Every chain reachable through `use_offset` stays in bounds, terminates, and
/// has a non-null word array (the C++'s `CHECK(cmap->word_map_)`).
fn assert_chains(registry: &str, rows: &[IndexRow]) {
    for start in 0..rows.len() {
        let mut at = start;
        let mut seen = vec![false; rows.len()];
        let mut links = 0usize;
        loop {
            assert!(
                !seen[at],
                "{registry}: use_offset chain from row {start} cycles at row {at}"
            );
            seen[at] = true;
            links += 1;
            assert!(
                links <= rows.len(),
                "{registry}: chain from {start} too long"
            );
            let off = rows[at].use_offset;
            if off == 0 {
                break;
            }
            let next = isize::try_from(at).expect("row index") + isize::from(off);
            let next = usize::try_from(next).ok().filter(|&n| n < rows.len());
            let Some(next) = next else {
                panic!(
                    "{registry}: row {at} ({}) use_offset {off} leaves the table",
                    rows[at].name
                )
            };
            at = next;
        }
    }
}

// ---------------------------------------------------------------------------
// Blob emission
// ---------------------------------------------------------------------------

/// Byte offsets are assigned in the order the sections are written; nothing
/// depends on a section's internal order except the per-registry entry order,
/// which is the C++'s and is load-bearing (`use_offset` is an index delta).
fn generate(cmaps: &Path, out: &Path) {
    let mut all: Vec<Vec<IndexRow>> = Vec::new();
    // symbol -> (offset in section, record count)
    let mut word_off: BTreeMap<String, u32> = BTreeMap::new();
    let mut dword_off: BTreeMap<String, u32> = BTreeMap::new();
    let mut words: Vec<u8> = Vec::new();
    let mut dwords: Vec<u8> = Vec::new();
    let mut cid2uni: Vec<Vec<u16>> = Vec::new();
    let mut manifest = String::from("{\n  \"generator\": \"crates/pdfrum-cmap/build.rs\",\n");
    let _ = writeln!(manifest, "  \"blob_version\": 1,\n  \"registries\": [");

    for (ri, reg) in REGISTRIES.iter().enumerate() {
        let dir = cmaps.join(reg.dir);
        let inc = strip_comments(
            &std::fs::read_to_string(dir.join(reg.index_file)).expect("read index .inc"),
        );
        let rows = parse_index(&inc, reg.index_symbol);
        assert_eq!(
            rows.len(),
            reg.expect_entries,
            "{}: expected {} index entries, found {}",
            reg.index_symbol,
            reg.expect_entries,
            rows.len()
        );
        assert_chains(reg.index_symbol, &rows);
        {
            let mut names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
            names.sort_unstable();
            let before = names.len();
            names.dedup();
            assert_eq!(before, names.len(), "{}: duplicate name", reg.index_symbol);
        }

        let _ = writeln!(
            manifest,
            "    {{ \"registry\": \"{}\", \"entries\": [",
            reg.dir
        );
        for (i, row) in rows.iter().enumerate() {
            let stride = if row.is_range { 3usize } else { 2 };
            if !word_off.contains_key(&row.word_symbol) {
                let (vals, dim) = load_symbol(&dir, &row.word_symbol);
                let want = usize::from(row.word_count) * stride;
                assert_eq!(
                    vals.len(),
                    want,
                    "{}: {} declares word_count {} ({} u16) but the array holds {}",
                    reg.index_symbol,
                    row.word_symbol,
                    row.word_count,
                    want,
                    vals.len()
                );
                assert_eq!(
                    eval_dim(&dim, &row.word_symbol),
                    want,
                    "{}: declared dimension [{dim}] disagrees with word_count",
                    row.word_symbol
                );
                assert_sorted_words(&row.word_symbol, &vals, row.is_range);
                let off = u32::try_from(words.len()).expect("words section < 4 GiB");
                words.extend(vals.iter().flat_map(|v| v.to_le_bytes()));
                word_off.insert(row.word_symbol.clone(), off);
            }
            if let Some(sym) = &row.dword_symbol
                && !dword_off.contains_key(sym)
            {
                let (vals, dim) = load_symbol(&dir, sym);
                let want = usize::from(row.dword_count) * 4;
                assert_eq!(vals.len(), want, "{sym}: dword record count mismatch");
                assert_eq!(
                    eval_dim(&dim, sym) * 4,
                    want,
                    "{sym}: declared dimension [{dim}] disagrees with dword_count"
                );
                assert_sorted_dwords(sym, &vals);
                let off = u32::try_from(dwords.len()).expect("dwords section < 4 GiB");
                dwords.extend(vals.iter().flat_map(|v| v.to_le_bytes()));
                dword_off.insert(sym.clone(), off);
            }
            let _ = writeln!(
                manifest,
                "      {{ \"i\": {i}, \"name\": \"{}\", \"word\": \"{}\", \"dword\": {}, \
                 \"word_count\": {}, \"dword_count\": {}, \"type\": \"{}\", \"use_offset\": {} }}{}",
                row.name,
                row.word_symbol,
                row.dword_symbol
                    .as_ref()
                    .map_or_else(|| "null".to_owned(), |s| format!("\"{s}\"")),
                row.word_count,
                row.dword_count,
                if row.is_range { "Range" } else { "Single" },
                row.use_offset,
                if i + 1 == rows.len() { "" } else { "," }
            );
        }
        let _ = writeln!(
            manifest,
            "    ] }}{}",
            if ri + 1 == REGISTRIES.len() { "" } else { "," }
        );

        let u = strip_comments(
            &std::fs::read_to_string(dir.join(reg.cid2unicode_file)).expect("read UCS2 .inc"),
        );
        let (body, _) = find_array_body(&u, reg.cid2unicode_symbol).expect("UCS2 array");
        let vals = scan_u16_literals(body, reg.cid2unicode_symbol);
        assert_eq!(
            vals.len(),
            reg.expect_cid2unicode_len,
            "{}: expected {} entries, found {}",
            reg.cid2unicode_symbol,
            reg.expect_cid2unicode_len,
            vals.len()
        );
        assert_eq!(
            vals.first().copied(),
            Some(0xFFFD),
            "{}: index 0 must be U+FFFD",
            reg.cid2unicode_symbol
        );
        cid2uni.push(vals);
        all.push(rows);
    }
    let _ = writeln!(manifest, "  ]\n}}");

    // Names section, deduplicated by spelling.
    let mut names: Vec<u8> = Vec::new();
    let mut name_off: BTreeMap<String, u32> = BTreeMap::new();
    for rows in &all {
        for row in rows {
            if !name_off.contains_key(&row.name) {
                let off = u32::try_from(names.len()).expect("names section");
                let bytes = row.name.as_bytes();
                names.push(u8::try_from(bytes.len()).expect("name < 256 bytes"));
                names.extend_from_slice(bytes);
                name_off.insert(row.name.clone(), off);
            }
        }
    }

    // Layout: header, index table, words, dwords, cid2unicode, names.
    let index_len = 4 * 8 + all.iter().map(|r| r.len() * ENTRY).sum::<usize>();
    let index_off = HEADER;
    let words_off = index_off + index_len;
    let dwords_off = words_off + words.len();
    let cid2uni_off = dwords_off + dwords.len();
    let cid2uni_dir = 4 * 8;
    let names_off = cid2uni_off + cid2uni_dir + cid2uni.iter().map(|v| v.len() * 2).sum::<usize>();
    let total = names_off + names.len();

    let mut blob = Vec::with_capacity(total);
    blob.extend(0x504D_4331u32.to_le_bytes()); // "PMC1" (little-endian)
    blob.extend(1u16.to_le_bytes());
    blob.extend(4u16.to_le_bytes());
    for v in [index_off, words_off, dwords_off, cid2uni_off, total] {
        blob.extend(u32::try_from(v).expect("offset fits u32").to_le_bytes());
    }
    blob.extend(
        u32::try_from(names_off)
            .expect("names offset")
            .to_le_bytes(),
    );
    assert_eq!(blob.len(), HEADER);

    // Registry headers, then the entries, in registry order.
    let mut entry_at = index_off + 4 * 8;
    for rows in &all {
        blob.extend(u32::try_from(entry_at).expect("entry offset").to_le_bytes());
        blob.extend(
            u16::try_from(rows.len())
                .expect("entry count")
                .to_le_bytes(),
        );
        blob.extend(0u16.to_le_bytes());
        entry_at += rows.len() * ENTRY;
    }
    for rows in &all {
        for row in rows {
            blob.extend(name_off[&row.name].to_le_bytes());
            blob.extend(word_off[&row.word_symbol].to_le_bytes());
            blob.extend(
                row.dword_symbol
                    .as_ref()
                    .map_or(u32::MAX, |s| dword_off[s])
                    .to_le_bytes(),
            );
            blob.extend(row.word_count.to_le_bytes());
            blob.extend(row.dword_count.to_le_bytes());
            blob.push(u8::from(row.is_range));
            blob.push(row.use_offset.to_le_bytes()[0]);
            blob.extend(0u16.to_le_bytes());
        }
    }
    assert_eq!(blob.len(), words_off);
    blob.extend(&words);
    blob.extend(&dwords);
    assert_eq!(blob.len(), cid2uni_off);
    let mut at = cid2uni_off + cid2uni_dir;
    for vals in &cid2uni {
        blob.extend(u32::try_from(at).expect("cid2unicode offset").to_le_bytes());
        blob.extend(
            u32::try_from(vals.len())
                .expect("cid2unicode len")
                .to_le_bytes(),
        );
        at += vals.len() * 2;
    }
    for vals in &cid2uni {
        blob.extend(vals.iter().flat_map(|v| v.to_le_bytes()));
    }
    assert_eq!(blob.len(), names_off);
    blob.extend(&names);
    assert_eq!(blob.len(), total);

    std::fs::write(out.join("cmaps.bin"), &blob).expect("write cmaps.bin");
    std::fs::write(out.join("cmaps.manifest.json"), manifest.as_bytes())
        .expect("write cmaps.manifest.json");
    println!(
        "cargo:warning=regenerated tables/cmaps.bin ({} bytes)",
        blob.len()
    );
}
