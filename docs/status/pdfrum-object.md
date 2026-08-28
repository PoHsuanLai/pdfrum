# `pdfrum-common` + `pdfrum-object` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green

First two library crates of M1. Contracts: SPEC.md §1 and §2; behavior:
`docs/design/pdfrum-object.md` (and §1.20/§1.21 of `docs/design/pdfrum-parser.md`
for the limits and diagnostics tables).

## `pdfrum-common`

`Diagnostics` / `Diagnostic` / `Severity` / `DiagKind` (18 variants seeded from
the parser brief's §1.21 mapping), `Limits`, and the `kurbo` re-export.
Nothing else — SPEC §1 lists the contents exhaustively.

- `Diagnostics` is a **bounded** sink: `DEFAULT_LIMIT` 4096, entries past the
  cap counted (`recorded()`, `dropped()`) but not stored, so the recovery
  channel cannot become an out-of-memory condition on a pathological file.
- `Limits` defaults are the PDFium constants consolidated in the parser
  brief §1.20: nesting 64, xref size 25 165 825, object number 25 165 824,
  header scan 1024, startxref scan 4096, word length 256, page-tree depth
  1024, page count 0xFFFFF, decoded stream 1 GiB (corrected from 20 MB when
  `pdfrum-filters` landed — see `docs/status/pdfrum-filters.md`).
  `max_string_len` and `max_array_len` are `usize::MAX` (PDFium has no such
  cap; the fields exist for fuzz budgets).
- No `Error` enum and no `thiserror` dependency: the crate has no fallible
  operation. Recorded in SPEC §1 rather than shipping an empty enum.

## `pdfrum-object`

Module plan followed as written in the brief §3, one file per concern:
`object` · `number` · `string` · `name` · `names` · `dict` · `array` ·
`stream` · `resolve` · `error` (+ `test_resolve`, test-only).

What is implemented, against the brief's inventory:

- **Object model.** `Object` (10 variants), `ObjRef`, `PdfString` (with the
  `hex` source-syntax flag), `Name`, `Array`, `Dict`, `Stream`, `ByteSpan`.
  All `Send + Sync`; equality structural.
- **Integer tri-state** (brief §1.2). `Int(i64)` stores the mathematical
  value; `as_c_int` reproduces PDFium's `GetSigned` (wrapping through `u32`),
  `as_c_float` its `GetFloat` (widening), `real_as_c_int` its saturating
  float cast. `INT_RANGE` documents and debug-asserts the reachable range.
- **Float spelling.** `fmt_number` ports `FloatToDecimal` over `ryu`'s
  shortest digits — all 18 goldens from
  `cpdf_contentstream_write_utils_unittest.cpp` and `cpdf_number_unittest.cpp`
  match, including the `-f32::MIN` denormal that hits the 48-character cap.
- **Text codecs** (brief §1.4). `PDF_DOC_ENCODING` table, `decode_text`
  (`FE FF` UTF-16BE / `FF FE` UTF-16LE / `EF BB BF` UTF-8 / PDFDocEncoding,
  with U+001B language-code stripping in the marked paths), `encode_text`,
  `encode_string_literal`, `encode_string_hex`.
- **Names** (brief §1.5). `name_decode` (`#xx`, needing a byte after the pair;
  non-hex counts as zero), `name_encode` (bytes ≥ 0x80, whitespace,
  delimiters, `#`), the `names!` macro, and ~110 constants ported from
  `pdfium-c++/constants/` (`stream_dict_common`, `catalog`,
  `annotation_common`, `appearance`, `form_fields`, `font_encodings`,
  `transparency`) plus the trailer/xref/encryption/linearization seed set the
  brief §1.5 lists.
- **Accessor matrix** (brief §1.6/§1.7). All 16 rows, split explicitly into
  resolving and non-resolving flavours on both `Dict` and `Array` — the
  distinction is load-bearing recovery behavior (`direct_int` for `/Prev`,
  `int` for `/Length`), and the module docs say so at the call site's reading
  level. `rect`/`matrix` need the exact element count or fall back to the zero
  rect / identity, and rects are **not** normalized.
- **Resolution** (brief §1.8). One level, never two: `Resolved::as_direct`
  refuses a reference-to-reference target, which every typed accessor goes
  through. `NoResolve` (public) and `TestStore` (test-only) are the two
  non-parser `Resolve` implementations.
- **Cloning** (brief §1.9). `clone_direct` flattens references with a
  per-child copy of the ancestor set: sibling sharing survives, ancestor
  cycles are cut, and a cut edge disappears rather than becoming null.

## Tests

`cargo nextest run -p pdfrum-object`: **62** unit tests.
`cargo nextest run -p pdfrum-common`: **6**.
`cargo test --doc`: **26** doctests in `pdfrum-object`, **3** in
`pdfrum-common`.

Ported assertion sets: `cpdf_object_unittest.cpp` (the `GetString` /
`GetUnicodeText` / `GetNumber` / `GetInteger` / `GetDict` per-type tables,
`GetNameFor`/`GetByteStringFor`, `GetRect`/`GetMatrix` element counts, the
`CloneDirectObject` and `CloneCheckLoop` cases), `cpdf_array_unittest.cpp`
(`GetBooleanAt` strictness, `Find`/`Contains` restated structurally),
`cpdf_dictionary_unittest.cpp` (iteration, restated as document order with the
divergence noted in the test), `cpdf_number_unittest.cpp` +
`cpdf_contentstream_write_utils_unittest.cpp` (every float and integer
spelling golden), `fpdf_parser_decode_unittest.cpp` (`DecodeText`,
`...WithUnicodeEscapes`, `...WithInvalidUnicodeEscapes`,
`...WithUnpairedSurrogates`, `EncodeText`, `RoundTripText` over all 256
bytes), `fpdf_parser_utility_unittest.cpp` (`NameDecode`, `NameEncode`).

Plus the brief's hand-written `FX_Number` edge cases: `4294967295` reads −1 as
an integer and 4294967296.0 as a number; `2147483648` reads −2147483648.

Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D
warnings`, `cargo nextest run`, `cargo test --doc`, `bash scripts/ci.sh` — all
green.

## `[spec]` changes made

One `[spec]` commit, five edits to SPEC §2 plus a §1 clarification:

1. **`ObjRef.gen` → `ObjRef.generation`.** `gen` is a reserved keyword in Rust
   edition 2024; the field cannot be spelled without `r#gen` at every use
   site. Same shape, longer name.
2. **`Name(Box<[u8]>)` → `Name(Cow<'static, [u8]>)`.** SPEC §2 requires
   `pub const LENGTH: &Name` constants, which a `Box` cannot produce in const
   context — the only alternatives were a `LazyLock` registry (global state,
   banned by STYLE §1) or dropping the constants. `Cow::Borrowed` is const,
   so the ~110 constants are zero-cost statics while parsed names still own
   their bytes. Equality, `Send + Sync` and hashing are unaffected.
3. **`Array(pub Vec<Object>)` → private field + `Array::of(values)`.** The
   public field would let a caller insert a direct `Stream`, which
   ISO 32000-1 §7.3.8.1 forbids and the brief §1.7 asks be debug-asserted;
   the invariant needs a constructor to live in.
4. **`as_c_int() -> i32` → `as_c_int(i64) -> i64`** (free function in the
   `number` module, with `as_c_float`, `real_as_c_int`, `fmt_number`,
   `fmt_int` beside it). Every accessor that would call it returns
   `Option<i64>`, so returning `i32` only added a conversion at each call
   site; the value's range is unchanged.
5. **Non-resolving accessors named in SPEC**, not just in the brief — the
   resolving/non-resolving split is the load-bearing part of the contract and
   belongs where callers read it. Also recorded: equality is `PartialEq`-only
   on `Object` (`Real(f32)` has no total equality), and `pdfrum-common` ships
   no `Error` enum.

Brief item resolved differently: the brief §3 module plan lists a private
`FromObj` trait powering `Dict::get_as::<T>()`. It is **not** implemented. The
16 accessor rows do not share a uniform shape — resolving vs not, type-filtered
vs coercing, `Option` vs a fallback value — so a generic `get_as` would need a
type parameter per row and would erase exactly the distinction the matrix
exists to make. The explicit accessors are the design; adding `FromObj` later
is additive if a caller ever wants it.

## Not in scope here (next crates)

Fuzz targets for `decode_text` and `name_decode` (the brief's test plan asks
for them) land with the `fuzz/` workspace alongside the parser's lexer and
`parse_object` targets — the crate has no byte-consuming entry point of its
own until then. The `insta` snapshot of a composite tree is covered by a
hand-written `Debug`-dump assertion instead, since `insta` is not yet wired
into the workspace.
