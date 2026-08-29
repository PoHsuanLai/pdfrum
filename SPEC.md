# Implementation Spec — pdfrum

Concrete contracts per crate: the load-bearing types, public API shapes, and
design decisions that are **already made**. Agents implement against this;
design briefs (`docs/design/<crate>.md`) fill in file-level detail underneath
it, never above it. STYLE.md governs how the code is written.

## 0. Spec-change protocol (anti-drift)

- The signatures and type shapes below are contracts. An agent that discovers a
  contract is wrong (it happens) does **not** silently code around it: it makes
  a `[spec]` commit that edits this file *and* the code in the same commit,
  with a rationale paragraph in the commit message. Anything else is drift.
- Behavior source of truth: the C++ module named in each section + its
  unittests + the conformance oracle. Shape source of truth: this file.
- Names below are normative (types, variants, functions). Field lists marked
  *(abridged)* may grow in briefs; unmarked ones are complete.
- Every crate ships: `Error` enum (thiserror), `#![forbid(unsafe_code)]`,
  lints per STYLE.md §3, fuzz target if it consumes untrusted bytes.

---

## 1. `pdfrum-common`

Tiny. Re-exports `kurbo` (geometry: `Affine`, `BezPath`, `Rect`, `Point`).
Owns exactly:

```rust
pub struct Diagnostics { entries: Vec<Diagnostic>, limit: usize, recorded: usize }
pub struct Diagnostic { pub severity: Severity, pub what: DiagKind, pub at: Option<u64> /* byte offset */ }
pub enum Severity { Recovered, Suspicious }
pub enum DiagKind { /* grows: XrefRebuilt, LengthMismatch, BadEof, ... */ }

pub struct Limits {           // mirror pdfium's hard limits; values set in brief
    pub max_object_nesting: u32,
    pub max_string_len: usize,
    pub max_array_len: usize,
    pub max_xref_size: u32,
    /* (abridged) */
}
impl Default for Limits { /* pdfium-equivalent values */ }
```

`Diagnostics` is bounded: past `limit` (default 4096) entries are counted
(`recorded()`, `dropped()`) but not stored, so a pathological file cannot turn
the recovery channel into an out-of-memory condition. `Limits` is deliberately
**not** `#[non_exhaustive]` — §4 makes struct-update-over-`Default` the
configuration idiom and the attribute forbids exactly that across crates; new
fields are additive.

[spec] 2026-08-29 (cmap brief OQ-4): `Limits` gains `max_cmap_ranges`,
defaulting to **65 536** — a cap on how many codespace ranges, and separately
how many wide-code CID ranges, one embedded CMap program may declare. PDFium
caps neither; exceeding it drops further ranges with a diagnostic rather than
erroring, the same shape as the accepted `max_decoded_stream_len` divergence
(§4). No real CMap approaches the value.

This crate has no fallible operation, so — uniquely — it ships no `Error` enum
and no `thiserror` dependency. No string types, no stream traits, no "utils".
If something feels like it belongs here, it probably belongs in the crate that
uses it.

## 2. `pdfrum-object`  *(behavior: `core/fpdfapi/parser` object classes, `constants/`)*

The PDF object model as values. **No parsing here** — construction and access only.

```rust
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f32),                // f32 to match oracle formatting/rounding
    Str(PdfString),
    Name(Name),
    Array(Array),
    Dict(Dict),
    Stream(Stream),
    Ref(ObjRef),
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct ObjRef { pub num: u32, pub generation: u16 }   // `gen` is a reserved keyword in edition 2024

pub struct PdfString { pub bytes: Box<[u8]>, pub hex: bool }  // raw bytes + source spelling (hex vs literal — C++ round-trips it, CPDF_String::is_hex; the writer needs it). Helpers: as_text() -> Cow<str> (PDFDoc/UTF-16BE/UTF-8 detection)
pub struct Name(Cow<'static, [u8]>);      // Cow so `names` constants are const-constructible & zero-copy; parsed names own. Helpers: as_str(), from_static(); pub mod names { pub const LENGTH: &Name; ... } for constants/
pub struct Array(Vec<Object>);            // private field + `Array::of(values)`; push() accepts any Object, streams included (see the §7.3.8.1 note below)
pub struct Dict(Vec<(Name, Object)>);     // linear assoc — PDF dicts are small; get() is O(n) scan, last duplicate wins (match C++)
pub struct Stream { pub dict: Dict, pub data: ByteSpan }

/// Zero-copy window into the document bytes. The whole file is one Arc<[u8]>;
/// streams are ranges into it. Decoded data is NOT cached here (page layer owns caches).
pub struct ByteSpan { file: Arc<[u8]>, range: Range<usize> }   // Deref<Target=[u8]>
```

Indirect resolution is a trait implemented by the parser's store:

```rust
pub trait Resolve {
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, Error>;
}
/// Cow-like: resolving a direct object borrows, an indirect one shares.
pub enum Resolved<'a> { Direct(&'a Object), Indirect(Arc<Object>) }  // Deref<Target=Object>

impl Object {
    pub fn resolve<'a>(&'a self, r: &impl Resolve) -> Result<Resolved<'a>, Error>;
}
impl Dict {
    // Resolving accessors (one level; a ref-to-ref result counts as absent):
    pub fn get<'a>(&'a self, key: &Name, r: &impl Resolve) -> Option<Resolved<'a>>;
    pub fn int(&self, key: &Name, r: &impl Resolve) -> Option<i64>;   // C-int view
    pub fn number(&self, ...) -> Option<f32>;     // Int|Real coercion, as C++ GetNumber
    pub fn byte_string(&self, ...) -> Option<Vec<u8>>;  /* + text, array, dict, stream, rect, matrix */

    // Non-resolving accessors — NOT an optimization: their C++ counterparts
    // type-check before resolving, so a ref there reads as absence, and real
    // recovery behavior depends on it (an indirect /Prev is ignored, an
    // indirect /Length is chased). See the matrix in the design brief.
    pub fn raw(&self, key: &Name) -> Option<&Object>;
    pub fn direct_int(&self, key: &Name) -> Option<i64>;   // Number-typed only
    pub fn name(&self, key: &Name) -> Option<&Name>;
    pub fn bool(&self, key: &Name) -> Option<bool>;        // an Int(1) is not a bool
    pub fn number_obj(&self, key: &Name) -> Option<&Object>;
    pub fn string(&self, key: &Name) -> Option<&PdfString>;
    pub fn reference(&self, key: &Name) -> Option<ObjRef>;
}
// Array mirrors the same split, index for key (`*_at` suffix), plus
// `as_rect()` / `as_matrix()` (exact element count or zero rect / identity).
```

[spec] 2026-08-29 (object defect, found by the page corpus sweep): **ISO
32000-1 §7.3.8.1's "no direct stream values" rule is a file-format constraint,
not an in-memory invariant.** `Array::push`/`Dict::push` originally
debug-asserted it and so panicked on ordinary files, because
`Object::clone_direct` flattens references and a `/Resources` whose `/XObject`
entries are indirect streams — the common case — legitimately produces a
dictionary holding those streams directly. The C++ does the same: its ordinary
setters do reject a stream (`CHECK(!pObj->IsStream())` in
`cpdf_dictionary.cpp:285` / `cpdf_array.cpp:249,264,278`, plus `= delete`d
overloads and `static_assert`s), but `CPDF_Dictionary::CloneNonCyclic` and
`CPDF_Array::CloneNonCyclic` insert into `map_`/`objects_` **directly**
(`cpdf_dictionary.cpp:64`, `cpdf_array.cpp:57`), bypassing those checks, so
`CloneDirectObject()` really does yield an inline `CPDF_Stream` — and
`GetStreamFor`/`GetStreamAt` read it back (`ToStream(GetDirectObjectFor…)`).
So the assertions are removed; both containers accept a stream value and the
resolving accessors already return it. Enforcement moves to the two ends where
the C++ puts it: the **reader** drops a stream found inline while parsing
(already done, `cpdf_syntax_parser.cpp:591-596,645-649`), and the **writer**
(pdfrum-edit, M7) hoists a direct stream back to an indirect object at
serialization. `clone_direct` is otherwise unchanged — its `ObjRef`-keyed
ancestor set is the faithful Rust equivalent of the C++ pointer set, since an
owned `Object` tree can only form a cycle through a reference.

Decisions: recursion into `Object` is bounded by `Limits.max_object_nesting`
at *parse* time, so access code may recurse freely. `Object` is `Send + Sync`.
Equality is structural and `PartialEq`-only (`Real(f32)` has no total
equality); `ObjRef`, `Name` and `PdfString` are additionally `Eq + Hash`.
No interning v1 (revisit with benchmarks only).
Dict order: C++ stores dicts in a sorted `std::map` (so its writer emits keys
sorted); we deliberately keep insertion order in storage *and* serialization —
written-file byte layout is not an oracle target (round-trip fidelity is
semantic: reparse + re-render), so this divergence is accepted and permanent.
One exception (doc brief E2): --show-structure prints attribute dicts in
iteration order and the goldens are alphabetical — the two emission sites in
pdfrum-doc sort keys locally; storage order is unchanged.
Integer accessor semantics are tri-state (see the resolution matrix and
`FX_Number` inventory in `docs/design/pdfrum-object.md`): `Int(i64)` stores the
parsed value; the `number` module provides the C-int wrapping view
(`as_c_int(i64) -> i64`, staying in `i64` so accessors that return `Option<i64>`
need no second conversion) alongside `as_c_float`, `real_as_c_int`, and the
oracle-parity spellings `fmt_number(f32) -> String` / `fmt_int(i64) -> String`.
Also owned here, beside the tables they need: `decode_text`/`encode_text`
(PDFDocEncoding + BOM detection), `encode_string_literal`/`encode_string_hex`,
`name_decode`/`name_encode`, and `NoResolve` (the empty store: every reference
dangles, which is how a damaged file's references already behave).

## 3. `pdfrum-crypt`  *(behavior: `core/fdrm`, parser security handlers)*

Closed enum, not a trait: `pub enum SecurityHandler { Rc4V2 {..}, AesV4 {..}, AesV5 {..}, Identity }` with
`SecurityHandler::from_encrypt_dict(dict, file_id, password) -> Result<Self, Error>`
(handles /R 2..6, user+owner password verification, returns `WrongPassword` variant distinctly) and
`fn decrypt(&self, obj: ObjRef, class: CryptClass, data: &[u8]) -> Vec<u8>` where
`enum CryptClass { Stream, String, Embedded }` (per-class crypt filters /StmF /StrF).
Primitives from RustCrypto (`aes`+`cbc`, `md5`, `sha1`, `sha2`); RC4 written
in-crate (~30 lines, no dep).
Decisions (orchestrator, from the brief's open questions): passwords are NOT
capped at ISO's 127 bytes — match the C++ exactly (observable behavior wins);
all other brief divergences D1–D7 accepted as written.

**Ruling 2026-08-30 (M10): the brief's D2 "decrypt only" is LIFTED.** The
crate gains the encrypt direction, so an encrypted document can be saved
encrypted (§11):

```rust
pub struct Iv(pub [u8; 16]);
impl SecurityHandler {
    fn encrypt(&self, obj: ObjRef, class: CryptClass, iv: Iv, data: &[u8]) -> Vec<u8>;
}
```

RC4 is symmetric, so its encrypt is its decrypt. AES-CBC encrypt prefixes
`iv` and writes **standard PKCS#7** — always a pad, so a payload of `n` bytes
becomes `32 + 16 * (n / 16)`. The decrypt side's one-block lag and absent
padding validation stay DECRYPT-only quirks (D6): they are the reader's
tolerance for files someone else wrote, and PKCS#7 is inside what they accept.
Per-object key derivation is **shared** with decrypt — including the AESV2
`sAlT` and the AESV3 rule that a 32-byte file key is used verbatim with no
salting — which is what makes the round trip exact. (The C++'s `EncryptContent`
truncates the AESV2 object key to the file key's length where `DecryptStart`
does not; the two agree at the only key length AESV2 ever has, and the C++
would abort at any other, so we implement the shared reading.) An **empty**
payload encrypts to empty, reproducing `CPDF_Encryptor::Encrypt`'s
short-circuit above the cipher.

Randomness stays out of the crate: `Iv` is a caller argument (no global
state per STYLE.md §1, no `getrandom` per DEPS.md). Building an `/Encrypt`
dictionary — `OnCreate`, `AES256_SetPassword`, `AES256_SetPerms` — remains out
of scope: v1 preserves passwords and never sets them.

## 4. `pdfrum-filters`  *(behavior: `core/fxcodec` basic codecs)*

Pure functions on slices; no streaming v1 (C++ also decodes to memory):

```rust
pub enum Filter { Flate, Lzw, AsciiHex, Ascii85, RunLength, CcittFax, Jbig2, Dct, Jpx, Crypt }
impl Filter {
    pub fn from_name(n: &Name) -> Option<Filter>;   // incl. abbreviations /Fl /AHx ...
    pub fn canonical_name(self) -> &'static Name;   // /DCT and /CCF expanded, as the C++ records them
    pub fn is_image_codec(self) -> bool;            // CcittFax | Jbig2 | Dct | Jpx
    pub fn is_chainable(self) -> bool;              // the ValidateDecoderPipeline allowlist
}

pub fn decode(filter: Filter, input: &[u8], params: &Dict, r: &impl Resolve, limits: &Limits, diags: &mut Diagnostics)
    -> Result<DecodeOutput, Error>;
pub enum DecodeOutput { Bytes(Vec<u8>), Image(NeedsImageCodec) }  // Dct/Jpx/Jbig2 punt to pdfrum-page's image path
pub struct NeedsImageCodec { pub filter: Option<Filter>, pub name: Name, pub params: Dict }  // the codec's input is DecodedStream::data
pub fn predictor(data: Vec<u8>, params: PredictorParams) -> Result<Vec<u8>, Error>;  // PNG+TIFF predictors
pub struct PredictorParams { pub kind: PredictorKind, pub colors: u32, pub bits_per_component: u32, pub columns: u32 }
pub enum PredictorKind { None, Tiff, Png }
impl PredictorParams { pub fn from_dict(d: &Dict, r: &impl Resolve) -> Result<Self, Error>; }

// The chain executor and its two halves — the StreamAcc fallback ladder lives
// here so the parser cannot reinvent it (see below).
pub struct DecodedStream { pub data: Vec<u8>, pub image: Option<NeedsImageCodec> }
pub fn decode_chain(s: &Stream, estimated_size: usize, r: &impl Resolve, limits: &Limits, diags: &mut Diagnostics) -> DecodedStream;
pub fn decoder_list(dict: &Dict, r: &impl Resolve) -> Option<Vec<(Name, Dict)>>;   // GetDecoderArray
pub fn validate_pipeline(filters: &Array, r: &impl Resolve) -> bool;               // ValidateDecoderPipeline

// Per-filter entry points, for callers that already know the filter. CCITT is
// reached only from the image path: unlike every other filter it needs the
// image's own /Width and /Height, because /Columns and /Rows default to them.
pub fn decode_flate(input: &[u8], estimated_size: usize, limits: &Limits, diags: &mut Diagnostics) -> Result<Vec<u8>, Error>;
pub fn encode_flate(input: &[u8]) -> Vec<u8>;   // the writer's half; see below
pub fn decode_lzw(input: &[u8], early_change: bool, limits: &Limits, diags: &mut Diagnostics) -> Result<Vec<u8>, Error>;
pub fn decode_run_length(input: &[u8], diags: &mut Diagnostics) -> Result<(Vec<u8>, usize), Error>;
pub fn decode_ascii85(input: &[u8]) -> Result<(Vec<u8>, usize), Error>;
pub fn decode_ascii_hex(input: &[u8]) -> (Vec<u8>, usize);
pub fn decode_ccitt(input: &[u8], p: CcittParams, image_width: u32, image_height: u32, diags: &mut Diagnostics) -> Result<CcittImage, Error>;
pub struct CcittParams { pub k: i64, pub end_of_line: bool, pub encoded_byte_align: bool,
                         pub black_is_1: bool, pub columns: i64, pub rows: i64 }
pub struct CcittImage { pub width: u32, pub height: u32, pub row_bytes: usize, pub bits: Vec<u8> }
pub const RUN_LENGTH_MAX_OUTPUT: u64 = 20 * 1024 * 1024;
```

Flate via `miniz_oxide` with a hard output cap from `Limits`; LZW via `weezl`
(TIFF variant + EarlyChange); CCITT G3/G4 via `hayro-ccitt`; RLE and AHx/A85
written in-crate. Decisions: a chain ending in an image codec returns the
*pre-image* bytes.
Decisions (orchestrator, from the brief's open questions):
`Limits.max_decoded_stream_len` defaults to 1 GiB and exceeding it is an
`Error` plus diagnostic — a deliberate divergence from the C++'s silent
1 GiB truncation (unobservable on real corpora; safety wins where fidelity
is not at stake; conformance will verify no corpus file trips it). The
four-rung StreamAcc fallback ladder (including handing compressed bytes to
the consumer on empty decode) is Tier-A behavior — port it exactly.
The ladder lives in **this** crate, as `decode_chain`, rather than in
`pdfrum-parser` as the earlier "chains applied by the caller" wording had it:
its four fallbacks are inseparable from what each decoder returns, so splitting
them across a crate boundary invites exactly the silent divergence SPEC §0
exists to prevent. `pdfrum-parser`'s stream accessor calls `decode_chain` and
adds nothing. `decode` stays public for a caller holding one known filter.
`NeedsImageCodec` carries no bytes of its own: the codec's input is
`DecodedStream::data`, already the right bytes whether earlier filters produced
them or the codec was the whole chain and the raw stream is what it reads —
which is the C++'s fourth fallback resolved once instead of at every consumer.
`RunLengthDecode`'s 20 MiB cap is a *separate* constant, not a `Limits` field:
it is a rejection the oracle really performs and files depend on, so it is not
configurable. `bytes_consumed` is reported only by the three filters whose
inline-image use needs it (RLE, A85, AHx); Flate and LZW do not report it, per
the brief's Q4.
[spec] 2026-08-29 (added with `pdfrum-edit`, M7): **`encode_flate` is the one
compression entry point, and it lives here.** The writer's stream serializer
(edit brief §1.13) flate-encodes an unfiltered stream, which is the only
compression in the workspace. It belongs beside `decode_flate` — the same
codec, the same `miniz_oxide` dependency, and putting it in `pdfrum-edit`
would make that crate the second place that knows what `/FlateDecode` means.
Level 6 (`Z_DEFAULT_COMPRESSION`, what PDFium's `compress()` uses); the bytes
are not byte-identical to zlib's and are not meant to be — round-trip through
`decode_flate` is the contract.

## 5. `pdfrum-parser`  *(behavior: `core/fpdfapi/parser` — THE fidelity-critical crate)*

Layered, each layer a module with a pure-ish entry point:

```rust
// lexer: zero-copy tokens over the file bytes
pub struct Lexer<'a> { /* pos, bytes */ }
pub enum Token<'a> { Int(i64), Real(f32), Str(Cow<'a,[u8]>), Name(&'a [u8]), Delim(Delim), Kw(&'a [u8]), ... }

// syntax: Token stream -> Object (bounded by Limits.max_object_nesting)
pub fn parse_object(lx: &mut Lexer, limits: &Limits, diags: &mut Diagnostics) -> Result<Object, Error>;

// xref: startxref chase, classic tables + xref streams + hybrid, prev-chains,
//       and the RECOVERY path: full-file scan rebuilding the table (port
//       CPDF_Parser rebuild heuristics faithfully — this is the superpower).
pub struct Xref { /* map ObjRef -> Entry */ }
pub enum Entry { Offset(u64), InObjStream { stream: ObjRef, index: u32 }, Free }
pub fn read_xref(file: &[u8], limits: &Limits, diags: &mut Diagnostics) -> Result<(Xref, Dict /*trailer*/), Error>;

// doc: ties it together; owns the lazy object store (impl Resolve)
pub struct Document { bytes: Arc<[u8]>, xref: Xref, trailer: Dict, security: SecurityHandler,
                      store: ObjectStore /* HashMap<ObjRef, OnceLock<Arc<Object>>> */, pub diags: Diagnostics }
pub struct LoadOptions { pub password: Option<Vec<u8>>, pub limits: Limits }
pub fn load(bytes: Arc<[u8]>, opts: &LoadOptions) -> Result<Document, LoadError>;
pub enum LoadError { NotPdf, WrongPassword, UnsupportedEncryption(..), Broken(..) }  // (abridged)

impl Document {
    pub fn page_count(&self) -> u32;
    pub fn page(&self, i: u32) -> Result<PageDict, Error>;   // pages-tree walk w/ cycle guard + /Kids repair
}
```

Decisions: whole file in memory as `Arc<[u8]>` v1 (progressive/linearized
loading is post-M8). Object streams decoded once, cached in store. Fetch-cycle
guard (an object referencing itself through /Length etc.) via in-progress set.
Every recovery the C++ performs gets a `Diagnostic`. Fuzz targets: lexer,
parse_object, read_xref, load.

## 6. `pdfrum-cmap` + `pdfrum-font`  *(behavior: `fpdfapi/cmaps`, `fpdfapi/font`, `fxge` font half)*

- `pdfrum-cmap`: `build.rs` converts the C++ static tables (`core/fpdfapi/cmaps/*.cpp`)
  into a compact binary embedded with `include_bytes!`; runtime API:
  `predefined(name: &Name) -> Option<CMap>`, `CMap::decode(&self, bytes) -> impl Iterator<Item=(CharCode, Cid)>`,
  plus an embedded-CMap parser (CID ranges). [spec] 2026-08-29 (font brief
  OQ-1): embedded `usecmap` is a NO-OP, matching PDFium exactly; "usecmap"
  support means the static predefined-table `use_offset_` chain only. The
  generated ~615 KiB table blob is COMMITTED with its generator and
  provenance doc (cmap brief OQ-2); conformance re-derives and diffs it when
  the oracle checkout is present.
- `pdfrum-font` core types:

```rust
pub enum Font { Simple(SimpleFont), Type0(Type0Font), Type3(Type3Font) }
pub struct SimpleFont { pub glyphs: GlyphSource, pub encoding: [Option<GlyphName>; 256] /* +Differences */,
                        pub widths: [f32; 256], pub to_unicode: Option<ToUnicode>, /* (abridged) */ }
pub enum GlyphSource { Fontations(FontRef /* skrifa over embedded or substitute bytes */), Type1(Type1Font) }
pub struct Type1Font { /* our PFA/PFB/charstring parser -> outlines; sub-module type1/ */ }

impl Font {
    /// The one text-decoding entry point everything (render, extract) shares:
    pub fn decode(&self, s: &[u8]) -> impl Iterator<Item = CharItem> + '_;
    pub fn glyph_path(&self, gid: Gid) -> Option<BezPath>;   // FontUnits scaled to 1000/em text space
    /// The same space, grid-fitted at a pinned 64 ppem. [spec] wave 7b.
    pub fn hinted_glyph_path(&self, gid: Gid) -> Option<BezPath>;
}
pub struct CharItem { pub code: CharCode, pub cid: Option<Cid>, pub gid: Gid, pub unicode: SmallVec<[char;2]>, pub width: f32 }
```

[spec] 2026-08-29 (`pdfrum-font` implementation). Five shape corrections, made
in the same commit as the code:

1. **`GlyphSource::Fontations` does not hold a `FontRef`.** It holds an owning
   `Face` record over `Arc<[u8]>` that reconstructs the reader per use, because
   a borrowed `FontRef<'static>` needs a self-reference the closed dependency
   set has no `yoke`-style crate for. More consequentially, `Face` is a
   **two-backend** value: `skrifa::FontRef` requires a table directory and
   therefore **cannot open a bare CFF**, which all fourteen Foxit base-14 blobs
   are. The second backend is `read_fonts::ps::cff::CffFontRef`. PDFium's own
   Rust bridge splits exactly the same way (`Sfnt ?? CffFontRef ?? Type1Font`),
   so this is the shape upstream arrived at; the brief's §1.16.3 records the
   bridge but its §3.1 sketch did not carry the split into `GlyphSource`.
   `GlyphSource` also gains a third variant, `None`, for a Type3 font or a
   program no backend could read — the state `IsEmbedded() == false` describes.
2. **`CharItem` gains `has_glyph: bool`.** `Gid` cannot express PDFium's `-1`,
   which is distinct from glyph 0 (`.notdef`) and means "draw nothing"; making
   `gid` an `Option<Gid>` instead would push the unwrap into every render call
   site for a case that is common, so the flag plus a `glyph()` accessor is the
   shape. `vertical_glyph` is present as ruled.
3. **`SimpleFont`'s field list** is as the brief's §3.1 records it — `unicodes:
   [u16; 256]` and `glyph_index: [u16; 256]` are first-class — plus `widths`
   as a `SimpleWidths` record rather than `[f32; 256]`, because the *unset*
   sentinel (`0xFFFF`) is behaviorally distinct from a zero width and the
   all-caps aliasing of §1.4 reads that distinction.
4. **`Font::load` takes a `FontCache` and returns `Option<Font>`**, per the
   brief; `Type0Font` is boxed inside the enum so the three variants do not
   differ in size by an order of magnitude.
5. **Five items are public for `pdfrum-doc`'s appearance generation** (doc
   brief OQ-3): `Font::char_code_from_unicode`, `Font::char_width`,
   `Font::append_char`, `Font::base_font_name` and `Font::load_standard`. All
   five are internals the brief already describes; only their visibility is new.

Also resolved by implementation, with the code as the record: **OQ-3** (both
halves measured against the oracle, not reasoned about — see
`docs/status/pdfrum-font.md`), **OQ-4** (always unhinted, no `tricky` list),
**OQ-6(a)** (`skip_font_enumeration` defaults to `false`, matching the oracle's
enumeration build, with the knob public), **OQ-6(b)** (`SimilarityScore` and
`FindFamilyNameMatch` are ported; `fontdb` is a face enumerator only), and
**OQ-7** (no new `Limits` field — PDFium's own `kOutOfSpecBFLimit` and
`kMaxType3FormLevel` are ported verbatim and `Limits::max_array_len` already
bounds the `/W` and `bfchar` collections).

Substitution/fallback: `fontdb` scan + a port of the C++
`SimilarityScore`/`FindFamilyNameMatch` matcher on top of it, behind a
`SubstitutionOptions` record whose enumeration-mode default is resolved
empirically in M2 against oracle --font-dir runs (font brief OQ-6);
plus embedded Foxit fallback faces
(`core/fxge/fontdata`, BSD) selected by a port of `CFX_FontMapper`'s
name/charset/flags heuristics (brief inventories them). Glyph outline cache:
`GlyphCache` keyed `(font_id, gid, dest_width, weight, italic_angle,
vertical)` — [spec] 2026-08-29 (font brief OQ-8): dest_width alone changes MM
outlines, so the old (font-id, gid, hint-flags) key was insufficient; CharItem
additionally gains `vertical_glyph: bool` (GSUB-substituted vertical forms are
not derivable downstream). pdfrum-type1 is first-party and MUST cover PFB
container + eexec + Type1/MM charstrings incl. MM blending (the Foxit MM
fallback fonts are the terminal substitution rung); read-fonts `ps::type1` may
be used only if the pinned release exposes what is needed. Unpaired-surrogate
divergence D3 accepted: both sides converge on U+FFFD through the harness
transcode, so Tier-A stays byte-exact. Cache owned by the render session,
not global.

[spec] 2026-08-29 (`pdfrum-type1` implementation, resolving font-brief **OQ-2**).
The pinned `read-fonts = "=0.43.3"` **does** ship `ps::type1`, and it covers
more than the brief expected: PFB/PFA/bare sniffing, both eexec ciphers, the
whole Type 1 charstring operator set including `seac` and `flex`, and even the
Multiple-Master blend othersubrs 14–18. What it does **not** expose is the one
thing PDFium's `AdjustVariationParams` needs — instantiation at chosen design
coordinates. `Type1Font::weight_vector` is read-only from the file, there is no
setter, `/BlendAxisTypes`, `/BlendDesignPositions` and `/BlendDesignMap` are
not parsed at all, and the public `CharstringContext` trait cannot be
implemented over their font because the charstring and subroutine bytes have no
public accessor. **OQ-2 therefore resolves to option 1**: `pdfrum-type1` stays
first-party and complete. `read-fonts` is still a dependency, for the two
things it genuinely owns — the Adobe Glyph List (`ps::agl`) and the predefined
Standard/Expert/ISO-Latin-1 encoding tables (`ps::encoding`) — and its
`ps::type1` serves as the crate's **differential test oracle** at the file's own
weight vector. DEPS.md needs no change.

Three shape corrections to §3.6 of the brief, made in the same commit as the code:

1. `Encoding::Custom` carries `Box<[Option<Box<str>>; 256]>`, not
   `Box<[Option<GlyphName>; 256]>`. `GlyphName` is a `pdfrum-font` type and
   depending on it here would be the dependency cycle §3.6 itself identifies;
   `pdfrum-font` maps these names into its own vocabulary at the boundary.
2. `Gid` is defined **by this crate** (`pub struct Gid(pub u16)`) rather than
   imported. Same reason.
3. `outline`/`glyph_bounds` gain a `outline_with_diagnostics` sibling. A
   charstring that aborts mid-glyph still yields its partial outline, and the
   caller needs a way to learn that happened without making the common path
   take a `&mut Diagnostics`.

**Accepted divergence D14 (Tier-B, in our favor).** `read-fonts`/FreeType
quantize every *unscaled* outline coordinate to a whole font unit (multiply by
1/64, discard the low 10 bits, shift back). We keep the charstring's arithmetic
in `f64` and return the unrounded value, because the outline is about to be
multiplied by a text matrix and rasterized at device resolution, where dropping
sub-unit precision is pure loss. Pinned by the differential test, which asserts
the stronger property that their coordinate is exactly ours truncated. We also
emit the zero-length segments FreeType's stem-darkening filter suppresses;
suppressing them is a rasterizer's business, not a font parser's.

## 7. `pdfrum-page`  *(behavior: `core/fpdfapi/page`)*

```rust
pub enum Op { /* all ~73 content operators, typed args: MoveTo(Point), SetFont(Name, f32), ... */ }
pub fn parse_content(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> Vec<Op>;  // infallible: bad op -> diag + skip (C++ behavior)

pub struct Page { pub objects: Vec<PageObject>, pub media_box: Rect, pub crop_box: Rect, pub rotate: Rotation, /* (abridged) */ }
pub enum PageObject { Path(PathObject), Text(TextObject), Image(ImageObject), Shading(ShadingObject), Form(FormObject) }
// Each *Object is a small record: geometry/payload + Arc'd resources + GraphicsState snapshot. No behavior inside.

pub struct GraphicsState { pub ctm: Affine, pub fill: ColorValue, pub stroke: ColorValue, pub stroke_params: StrokeParams,
                           pub clip: ClipStack, pub font: Option<(Arc<Font>, f32)>, pub blend: BlendMode,
                           pub alpha: AlphaPair, pub soft_mask: Option<Arc<SoftMask>>, /* (abridged; heavy members Arc'd */ }

/// The interpreter is a fold: ops + resources -> page objects. Pure w.r.t. inputs.
pub fn build_page(ops: &[Op], res: &Resources, r: &impl Resolve, limits: &Limits, diags: &mut Diagnostics) -> Page;

pub enum ColorSpace { DeviceGray, DeviceRgb, DeviceCmyk, Indexed{..}, IccBased{ profile: Arc<moxcms::..>, n: u8, alt: Box<ColorSpace> },
                      Separation{ alt: Box<ColorSpace>, tint: Arc<Function> }, DeviceN{..}, Lab{..}, CalGray{..}, CalRgb{..}, Pattern{..} }
impl ColorSpace { pub fn to_rgb(&self, comps: &[f32]) -> Rgb; pub fn n_components(&self) -> usize; }

pub enum Function { Sampled(Type0), Exponential(Type2), Stitching(Type3), PostScript(Type4 /* enum-op stack machine, no recursion depth > limit */) }
impl Function { pub fn eval(&self, input: &[f32], out: &mut [f32]) -> Result<(), Error>; }

pub enum Shading { FunctionBased(..), Axial(..), Radial(..), FreeMesh(..), LatticeMesh(..), Coons(..), TensorMesh(..) }
pub struct ImageData { /* decoded pixels: enum over Gray1/8, Rgb8, Cmyk8, Indexed + SMask/ColorKey */ }
pub fn decode_image(xobj: &Stream, r: &impl Resolve, ...) -> Result<ImageData, Error>;  // dispatches to zune-jpeg / pdfrum-jbig2 / pdfrum-jpx / filters
```

Decisions: `q`/`Q` is a `Vec<GraphicsState>` stack with cheap clone (Arc'd
heavy fields). Form XObjects recurse through `build_page` with depth guard.
Inline images (`BI…EI`) handled in `parse_content` with the C++'s EI-scan quirks.
Decoded-image cache: keyed `(ObjRef, RequestedSize)` per render/extract session
([spec] 2026-08-29: a plain ObjRef key cannot express resolution-dependent
invalidation — page brief Q8).
**[spec] 2026-08-30 (M11): the editing half of the page-object model.** Edit
brief E6's proposal is accepted and grown. `Content<T>` gains `dirty: bool`
(false on parse) and `active: bool` (true on parse) beside the
`content_stream: i32` it already carried, and `Page` gains
`dirty_streams: BTreeSet<i32>` and `stream_ctms: BTreeMap<i32, Affine>` — the
`all_ctms_` map the page brief §1.6 said we would carry, now actually carried.
`NO_CONTENT_STREAM = -1` names the streamless sentinel. All five are plain
fields with no behavior; the operations on them are free functions in the new
`mutate` module, per STYLE.md §1.

Three further additions the brief did not anticipate, each because a *parsed*
object cannot otherwise name the resource a *regenerated* stream must refer
to. `ImageObject` and `FormObject` gain `source: Option<ObjRef>` and
`TextObject` gains `font_source: Option<ObjRef>` (mirrored on `TextState`, so
it survives `q`/`Q`): the interpreter already resolved these references for
its caches, and a page object holding decoded pixels or a loaded font has no
other way back to the `/XObject` or `/Font` entry a `Do` or `Tf` has to spell.
`None` means "written inline, or reached from an annotation's `/AP`", and such
an object is dropped when its stream is rewritten — which is what the C++ does
with an inline image too.

`content_stream` is now populated for real. `StreamBounds` records where each
`/Contents` element's operators begin within the joined operator list, and
`build_page_streams` is `build_page_from_dict` plus those boundaries; the
render and text paths keep the cheaper entry point, which pays for neither
the boundaries nor the CTM map.

Decisions (orchestrator, from the page brief's open questions): the three
additive `Limits` fields the brief proposes (colorspace construction depth,
type-3 stitching nesting, names-tree length) are accepted with generous
defaults — C++ uses visited-sets, which remain the primary mechanism; the caps
are unobservable safety nets (Q1). All D1–D22 divergences accepted as written,
including porting the pixel-visible C++ quirks verbatim (shading LUT
off-by-one, sampled-function negative-index wrap) and declining the
crash/aliasing bugs the brief rules pixel-invisible (Q5/Q6).

## 8. `pdfrum-render` + backends  *(behavior: `core/fpdfapi/render`, `core/fxge`)*

```rust
/// The ONLY trait between engine and rasterizers. Object-safe, small, peniko/kurbo vocabulary.
pub trait RenderDevice {
    fn fill_path(&mut self, path: &BezPath, t: Affine, brush: &Brush, rule: FillRule, aa: AntiAlias);
    fn stroke_path(&mut self, path: &BezPath, t: Affine, brush: &Brush, stroke: &Stroke, aa: AntiAlias);
    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32);
    fn push_clip(&mut self, path: &BezPath, rule: FillRule);
    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>);
    fn pop(&mut self);
}
pub trait RasterBackend {  // factory: lets the engine rasterize soft masks & tiles offscreen
    type Device: RenderDevice;
    fn new_target(&self, w: u32, h: u32, clear: Color) -> Self::Device;      // [spec] E10: explicit clear color
    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device;       // [spec] E4: non-isolated groups
    fn snapshot(&self, d: &Self::Device) -> Pixmap;                          // [spec] E3: fill+stroke knockout buffer
    fn finish(&self, d: Self::Device) -> Pixmap;   // RGBA8, premultiplied
}
// [spec] E2 (render brief): RenderDevice additionally gets
//   fn push_clip_rect(&mut self, rect: Rect);   // hard-edged, never antialiased —
// PDFium's axis-aligned rect fills/clips are integer-snapped and aliased
// (shrink-wider-side, ties right/bottom); routing them through fill_path
// would add AA pixels on a large fraction of the corpus.
// Rulings 2026-08-29: Coons/tensor subdivision capped at depth 32 with a
// non-finite control-point check (additive safety, render brief Q2); the
// transfer-function array-reversal question (Q1) resolves in favor of the
// C++ unittest's asserted observable — which, measured at M3, is the
// reversal itself: two of that unittest's expectation constants are
// misnamed, and its TranslateColor pairs say array[2] drives red.
// Backend capability claims for vello_cpu/tiny-skia (brief §5) are
// INFERRED and must be verified against fetched crate sources before any
// backend code is written (brief Q4) — now VERIFIED by probing
// (docs/design/backend-verification.md, binding for backend implementers):
// traits hold unchanged; key invariants — vello image draws with alpha != 1.0
// panic (wrap in an opacity layer); mesh cells are drawn opaque into the
// scratch pixmap with shading alpha applied once at blit; tiny-skia silently
// drops fills/clips thinner than 1/4096 (engine pre-guards degenerates); pin
// vello Level+RenderMode for Tier-C; vello targets are u16-dimensioned
// (engine tiles or clamps above 65535); build luminosity masks from raw
// bytes, never the BT.709 helpers; asymmetric /Extend is engine-emulated via
// Pad + a computed clip.
// [spec] wave 8: `AntiAlias` gains a third variant, `FullCover`, because AGG
// has three modes and not two. `full_cover` keeps the rasterizer's own
// choice of which pixels a span covers and discards their coverage *value*,
// writing every one at the source alpha (`CFX_AggRenderer::GetSrcAlpha`
// against `GetSourceAlpha`, cfx_agg_devicedriver.cpp:481-491) — which is a
// test against zero, not the midpoint threshold `aliased_path` applies.
// Reading it as `Off` put a white pin-hole through every internal seam of a
// subdivided Coons patch, because a pixel two abutting cells each half-cover
// is dropped by both. The engine's integrator expresses it exactly
// (`scanline::Coverage::Full`), vello's aliasing threshold expresses it
// exactly at `Some(1)`, and tiny-skia — which has only a bool — maps it to
// *antialiased*, the nearer of its two: reaching every pixel the oracle
// reaches and writing some light beats not reaching them at all. The scratch
// pixmap and the draw-cells-opaque discipline are unchanged and still
// required (D12, §5.3).

pub struct RenderOptions { pub transform: Affine, pub text_aa: TextAa, pub grayscale: bool, pub subpixel_text_positioning: bool, /* (abridged) */ }
pub fn render_page(page: &Page, opts: &RenderOptions, backend: &impl RasterBackend) -> Pixmap;
// [spec] wave 8, orchestrator-authorized: optional content is a PRE-PASS,
// not a render-time decision.
//   pdfrum_page::page_visibility(&Page, &mut OcContext, &impl Resolve,
//                                &mut Diagnostics) -> Visibility
//   pdfrum_render::render_page_with_visibility(page, opts, backend,
//                                &Visibility, &mut RenderCaches, diags)
// PDFium hangs a CPDF_OCContext off CPDF_RenderOptions and asks it inside
// RenderSingleObject (cpdf_renderstatus.cpp:247), on a form's /OC (:401) and
// on an image's (cpdf_imagerenderer.cpp:197). We do not: deciding visibility
// needs indirect-object lookup and a mutable evaluation cache, and consuming
// the answer needs neither, so threading a Resolve into the render API to
// carry it would put a resolver in front of every rasterizer for one bool per
// object. `Visibility` is plain data shaped like the page — one entry per
// object in each list, a form's children nested — because a page object has
// no id and its position is the only thing that names it. An absent entry is
// visible, so an all-visible page collapses to an empty tree and `render_page`
// is `render_page_with_visibility` with `Visibility::all_visible()`. Sub-graphs
// outside the page's own lists — pattern cells, soft-mask groups, type-3 char
// procs — take all-visible: the tree does not describe them, and the oracle
// asks no OC question inside any of them either.
// Two page-graph additions this needs: `FormObject::oc` and `ImageObject::oc`
// carry the XObject's own `/OC`, which is a second declaration site
// independent of any enclosing marked-content sequence; and
// `ContentMarks::optional_content_all` returns *every* `/OC` mark rather than
// the innermost, because `CheckPageObjectVisible` gives each one a veto.
```

Engine decisions: large text renders as filled glyph `BezPath`s through the
glyph cache and small text as cached alpha bitmaps ([spec] wave 7b below);
render modes fill/stroke/clip per Tr. Axial/radial shadings map to
peniko gradients; function-based and mesh shadings (1,4–7) evaluate to a
`Pixmap` via a pure shading evaluator, then `draw_image`. Tiling patterns:
rasterize one tile offscreen via `RasterBackend`, draw repeated (respect
XStep/YStep quirks). Soft masks: render mask subtree offscreen, convert
(luminosity/alpha + TR function) to `AlphaMask`, pass to `push_layer`.
Isolated groups: engine-level compositing over offscreen layers.
Knockout ([spec] 2026-08-29, page brief Q4): the oracle never parses /K
anywhere in core/ — v1 matches the oracle and does NOT implement knockout;
the compositor's layer model keeps a documented (unused) slot for it as a
post-M8 correctness option. Backends never know about group semantics either
way. `pdfrum-raster-vello` (vello_cpu),
`pdfrum-raster-tinyskia` and `pdfrum-raster-exact` implement the two traits;
conformance Tier C diffs them.

**([spec] 2026-08-29, `pdfrum-render` implementation.)** The trait surface
above — the six `RenderDevice` methods plus `push_clip_rect`, and the four
`RasterBackend` methods including E2/E3/E4/E10 — is implemented **unchanged**
on both backends. Five corrections and one addition, made in the same commit
as the code:

1. **`render_page` returns `Result<Pixmap, Error>`, not `Pixmap`.** A target
   with a zero axis, or one beyond `vello_cpu`'s `u16` dimensions, has no
   output at all; every *other* failure remains a diagnostic and a skipped
   object, per STYLE §3. `Error` has exactly those two variants, and
   `render_page` also takes a `&mut Diagnostics` so the recovery channel
   reaches the caller.
2. **`RenderOptions` is the flag surface, not `grayscale: bool`.** It carries
   `transform`, `color_mode: ColorMode { Normal | Gray | Alpha | Forced }`,
   `text_aa`, the three smoothing/halftone flags PDFium forces on inside
   type-3 procs and tile cells, `convert_fill_to_stroke`, and `background:
   Option<Color>` — the last resolving render brief Q5 as proposed:
   `render_page` follows `pdium_test`'s white-vs-transparent choice from the
   page's own transparency, and the field is an override only.

   **([spec] 2026-08-29, burn-down wave 5.)** It gains one more field,
   `subpixel_text_positioning: bool`, **defaulting to `false`**, which is the
   oracle. Below `|char2device.a| + |char2device.b| > 50` PDFium renders a
   glyph *bitmap* and blits it on a fixed grid — y on whole pixels, x on
   **thirds** of one (`cfx_renderdevice.cpp:1254-1257` snaps and integer-
   floors x; line 1352's `x_subpixel = (int)(device_origin.x * 3) % 3` gives
   the thirds back through the LCD triple). Filling outlines where they truly
   land instead displaced every stem edge by the baseline's fractional part.
   Reproducing the grid is worth **+60 files at SSIM ≥ 0.99** on the corpus.

   The knob is the parity/off-grid tradeoff and it is one knob, not a family:
   `true` restores fractional placement for a caller who wants text where the
   PDF puts it rather than where a golden expects it — smooth animation, a
   non-integer device scale, any use where oracle parity is not the goal.
   Which rounding runs is decided by `FontAntiAliasingMode`, which
   `DrawNormalText` derives itself and which `bClearType` does **not** set:
   the conformance configuration resolves to `kLcd` (thirds in x) and
   `--no-smoothtext` to `kMono` (whole pixels in x, plus `AdjustGlyphSpace`).
   The snap does not apply to a stroked or pattern-coloured run, which
   `ProcessText` sends to `DrawTextPath`, nor above the `> 50` threshold.

   **([spec] 2026-08-29, burn-down wave 7b.)** The sentence above — "text
   renders as filled glyph `BezPath`s through the glyph cache" — is now true
   only *above* the threshold. Below it the engine reproduces the oracle's
   glyph-**bitmap** pipeline, in a new `pdfrum-render::glyph` module:

   - the outline is grid-fitted at a **pinned 64 ppem** for any SFNT face
     (`Font::hinted_glyph_path`, new on `pdfrum-font`; `RenderGlyph` adds
     `FT_LOAD_NO_HINTING` exactly when `!IsTtOt()`), transform applied after;
   - it is rasterized **3× wide** with `ft_lcd_padding`'s 43/64-subpixel
     margin, each span spread over five subpixel columns by FreeType's FIR5
     `{8, 77, 86, 77, 8}` filter;
   - the triples are averaged and passed through `kTextGammaAdjust`, with the
     window shifted by the `x_subpixel` phase above.

   `RenderCaches` gains a second cache, `glyph_bitmaps`, keyed by the existing
   `GlyphKey` plus the device matrix quantised as `(int)(m · 10000)` — the
   oracle's own `UniqueKeyGen` key, phase deliberately excluded because the
   cached bitmap is the 3×-wide one and the phase is a read of it. The blit
   goes through the existing `draw_image` seam at a whole-pixel translation,
   so no trait grows a seventh method.

   `pdfrum-raster-exact`'s `cell` module **moves into the engine** as
   `pdfrum_render::scanline`, unchanged. A glyph bitmap must be identical
   under every backend — the oracle's comes from FreeType, not from whatever
   draws the page's paths — and one integrator in the engine is how that holds
   by construction, the same argument `blend::composite_premultiplied` makes.

   Worth **+23 files at SSIM ≥ 0.99** and +2 byte-exact, with no regressions,
   and 1.5–3.5x on a text page for the two immediate-mode backends.
   `subpixel_text_positioning` still turns the whole thing off.
3. **The walk is generic over the backend, not `dyn`.** `RasterBackend::
   snapshot` needs the concrete device to read pixels back, so `&mut dyn
   RenderDevice` survives only where a device is genuinely swappable — the
   Coons scratch buffer. The `dyn` budget STYLE §2b sets is unchanged.
4. **Q1: the `/TR` array reversal is real, and `pdfrum-page` had it right.**
   `pFuncs[2 - i] = Load(array[i])` is observable: `array[2]` drives red and
   `array[0]` drives blue. `CPDFDocRenderDataTest.TransferFunctionArray`
   reads as if it said the opposite only because two of its expectation
   constants are **misnamed** — `kExpectedType0FunctionSamples` is the type 4
   program's sine ramp (verified: it matches `sin(v/255 · 360°)/2` on all 128
   positive samples) and `kExpectedType4FunctionSamples` is the type 0
   function's flat one. Its ten `TranslateColor` pairs settle it without
   naming a function at all. An earlier reading of that test concluded the
   reverse and had `render::transfer` compensate by reading slot `2 - i`,
   which cancelled a correct parse and swapped red and blue on every rendered
   `/TR` array; both halves are gone. See `pdfrum-page::transfer`'s module doc
   for the arithmetic.
5. **The page-to-device matrix flips y.** `Rotation::display_matrix`
   normalises the crop-box origin and applies `/Rotate` but leaves PDF's
   y-up convention intact, so `render_page` composes an explicit flip.
   Recorded because the symmetric fixtures hide its absence.

   **([spec] 2026-08-29, burn-down wave 5: the flip is about the *device*
   box, not the page box.)** `CPDF_Page::GetDisplayMatrixForRect`
   (`cpdf_page.cpp:216-218`) builds the matrix from an integer `FX_RECT`
   divided by the page's float size, and
   `CPDFSDK_RenderPageWithContext` passes it `FX_RECT(0, 0, size_x, size_y)`
   — the *truncated* bitmap size. So an A4 page 841.89 points tall renders
   into 841 rows at a y scale of `841 / 841.89`: the page is squeezed to fit
   the bitmap its size was truncated into, rather than translated by its
   float height. The difference is up to a device pixel between the top and
   bottom of a page, which was sub-count while glyphs were filled at their
   true position and became a whole row once origins are snapped. Correcting
   it is worth **+74 files at SSIM ≥ 0.99** on top of the snap.
6. **Two render-brief errata**, both pinned by tests. `kColorSqrt` is not any
   closed form of ISO 32000-1 §11.3.5.2's `D(x)`: 35 of its 256 entries differ
   from `round(255·D)` and 102 from the truncating spelling, so the verbatim
   transcription is the authority and brief Q7's "exact for all 256" is wrong.
   And `SoftLight`'s low branch divides by 255 twice rather than once by
   65025, but the two are exhaustively equal there (the numerator is
   non-negative for every `src < 128`), so brief test 33's premise does not
   hold; the spelling is still ported verbatim.

**([spec] 2026-08-29, wave 6: a third backend, and the default splits in
two.)** `pdfrum-raster-exact` is an analytic scanline rasterizer of our own,
implementing the trait surface above **unchanged** and adding **no external
dependency** — it is written against `kurbo` and `peniko` alone, both already
in the closed set.

7. **The conformance default and the facade default are now different
   backends, deliberately.** `pdfrum-tool --use-renderer=` gains `exact`,
   `tiny-skia` and `vello`, and `exact` is what `--png` uses when nothing asks
   otherwise; the `pdfrum` facade keeps `vello_cpu`. The two defaults answer
   different questions and had been sharing one answer.

   A *conformance* run asks whether the engine decided a page's pixels. Any
   rasterizer's own quantisation policy is noise in that measurement, and
   `tiny-skia`'s is not small: it supersamples at four subsamples per axis, so
   a diagonal edge takes one of seventeen coverages and a half-covered pixel
   lands on 8/16 of the range where the oracle writes the exact half. Over the
   corpus that is a persistent few-count spread along every non-axis-aligned
   edge. The analytic backend integrates the same area the oracle integrates,
   on the same 256ths-of-a-pixel grid, through the same measured mapping
   `min(255, floor(cov·256))` (`agg_rasterizer_scanline_aa.h:283-297`), so
   what survives a golden diff is the engine's own work. Worth **+16 files at
   SSIM ≥ 0.99 and +36 byte-exact**, with zero previously-passing files lost.

   An *API user* asks for a fast, well-maintained production rasterizer, which
   is what `vello_cpu` is and what the analytic backend does not try to be.
   Making one backend serve both would either slow the facade down or measure
   conformance through a sampling policy, so the split is the honest shape.

   Tier C keeps `tiny-skia` against `vello_cpu` as its **gating pair**. The
   tier's value is that two *independent* implementations disagree out loud,
   and the analytic backend shares this project's engine-facing arithmetic —
   `blend::composite_premultiplied` is one authority for all three — so
   diffing it against either would test less, not more. It is reported as a
   third column rather than gated.

8. **`draw_image`'s transform maps the image's pixel grid, not its unit
   square**, and SPEC's own comment said otherwise. Every engine call site
   passes a plain translation for a device-sized buffer, because the engine
   has already resampled the image to its device size before the call. The
   unit-square reading collapses a whole-page image onto one pixel, silently
   and totally; the trait doc now states the convention the code has always
   had.

9. **Premultiplying a composited colour rounds; it does not truncate.** The
   truncating `mul255` is the oracle's product wherever the oracle performs
   one, and it stays that everywhere. But storing a straight result into a
   premultiplied buffer is *our* round trip, not one of the oracle's, and its
   inverse (`CFX_DIBitmap::UnPreMultiply`'s `+ alpha / 2`) rounds — so
   truncating on the way in loses a count on most values. Measured: straight
   `145` at alpha `223` premultiplies to `126` truncating and `127` rounding,
   and only `127` comes back as `145`. That count was every pixel of
   `alpha_composite`'s overlap. Proved exhaustively never worse than
   truncating, and strictly better on most of the 65 280 pairs.

Deferred to M4/M5 with the code shaped for them, not stubbed around: tiling
patterns and shading patterns (the pattern is parsed and reaches the object
graph, but `Content::pattern` is not yet drained into a cell render), type-3
glyph procedures and their blue-zone cache, the `/SMask` group's own object
list (a soft mask currently contributes its `/BC` backdrop, which is exact
for a backdrop-only mask), and the `/Matte` and knockout image paths. Each
has its ported decision logic and unit tests in place — `pattern.rs`'s
absence is the one genuine gap, and the rest are wiring.

## 9. `pdfrum-text`  *(behavior: `core/fpdftext`)*

Pure derivation, no rendering dependency:
`pub fn extract(page: &Page, r: &impl Resolve) -> TextPage` where
([spec] 2026-08-29, text brief rulings: `CharBox.unicode` is `u32`, not
`char` — goldens contain U+0000 and lone surrogates, and the --txt emitter
must be able to write oracle-exact UTF-32LE code units. The --txt output is
derived from the UNFILTERED char list, not the search-facing text buffer —
they are different sequences, verified against the oracle. Char-class and
bidi-bucket tables are build-script-generated from the oracle's own ICU and
committed with provenance, cmap-blob precedent — no ICU dependency, and
`unicode-bidi` may end up unused here since PDFium's four-way bucket predates
the UBA; keep it pinned until M2 empirics settle it. Q2 interface gaps are
recomputed locally via `Dict::raw`, no page-crate change.)

**([spec] 2026-08-29, `text-nonempty` aggregate.) The text tier is scored two
ways and the exit criterion reads the second.** Of the 1832 `--txt` dumps a
corpus run compares, **872 are empty** — the oracle wrote a byte-order mark
and no characters, which the store holds as a zero-length file after
transcoding, because those pages carry no text. A tool that emitted nothing
for every page therefore scores 47.6% on a naive pass rate while extracting
nothing at all, and the number would keep flattering it as real extraction
landed. So `scoreboard.json`'s `totals.text` carries `pages`/`matched` **and**
`nonempty`/`nonempty_matched` with both rates, and `conformance run` and
`triage` print both. **M2's "≥ 98% of corpus" exit criterion (PLAN.md §6) is
the `nonempty_rate`**, over the 960 dumps that hold characters. `.annot.txt`
is a different tier and is excluded, as are the three document-level dumps.
`TextPage { chars: Vec<CharBox> /* unicode, bbox, origin, font-size, angle */, runs: … }`
plus `TextPage::find(needle, opts) -> impl Iterator<Item=Range<usize>>` and
`web_links()`. The C++ heuristics (space insertion thresholds, line breaks,
reading order, rotated text, hyphenation) are ported *exactly* — this crate is
Tier-A byte-exact against the oracle, so brief must enumerate every heuristic
constant from `cpdf_textpage.cpp` before implementation.

**([spec] 2026-08-29, implementation.)** Three shapes settled while building
it, all narrowing the sketch above rather than replacing it:

- `TextPage` carries **three** fields, not two: `chars`, `text: Vec<char>` and
  `runs: CharIndex`. `text` is the search-facing string and is a *different
  character sequence* from `chars`; making them separate public fields of
  different types is the only durable defence against a refactor collapsing
  them, which is the single easiest way to get this crate wrong. `runs` is the
  segment table bridging the two index spaces (design brief D2).
- `find` takes `FindOptions` **by value** (it is three booleans and `Copy`),
  and `extract` takes `(&Page, &impl Resolve, &ExtractOptions, &Limits, &mut
  Diagnostics)`. The `Limits` parameter is present and unused: nothing here
  consumes untrusted bytes directly, and no cap of PDFium's applies, but the
  signature stays uniform with every other crate's entry point so that a cap
  added later is not a breaking change.
- `TextPage::to_utf32le()` is public, because `--txt` is a Tier-A contract and
  the encoding — one byte-order mark, four bytes per *unfiltered* character —
  belongs beside the data it encodes rather than in the tool.

**([spec] 2026-08-29, two supporting additions in landed crates.)** Text
extraction needs two things §7 and §6 did not promise, both because a
byte-exact text tier exposes what a perceptual one cannot:

- `pdfrum-page`'s `TextObject` gains `type3_metrics: BTreeMap<u32,
  Type3Metrics>`, and the crate gains a `type3` module. A Type 3 glyph's
  advance and bounding box live inside its `/CharProcs` content stream's
  `d0`/`d1` operator, so no consumer can measure one from the font alone; the
  interpreter already opens those streams and records the answer. Six corpus
  fixtures extracted *nothing at all* without it.
- `pdfrum-page`'s `BuildContext` gains a font-instance cache keyed on the
  `/Font` resource's reference, so every `Tf` naming one resource shares one
  `Arc<Font>`. Duplicate suppression compares fonts by pointer identity
  (design brief §1.7a), and loading a fresh instance per `Tf` silently
  disabled it — a double-drawn page extracted twice.

## 10. `pdfrum-doc`  *(behavior: `core/fpdfdoc`)*

Records + functions over the catalog: `Bookmarks` (outline tree iterator with
cycle guard), `NameTree` lookup, `Link`/`Action`/`Dest` enums, `Annotation`
enum (typed per subtype), AcroForm: `Form { fields: Vec<Field> }`,
`Field { kind: FieldKind /* Text|Check|Radio|Combo|List|Button|Sig */, value, flags, kids }` with
`set_value()` + **appearance-stream generation** (port `cpdfsdk_appstream`/
variable-text layout — a pure `Field -> content-stream bytes` function).
Struct tree reader for `--show-structure` parity.
Rulings 2026-08-29 (doc brief escalations): NO second variable-text engine —
the `cpdfsdk_appstream`/`CPWL_EditImpl` NeedAppearances path is out of scope
(only ~5 corpus pixel files reach it; they become documented waivers); ship
only the checkbox/radio path tables, off by default (E1 — **superseded, see
the 2026-08-29 revision below: there was no second engine, and the three
`SetAs*` producers are now in scope over the `vt` engine this crate already
has**). Dict keys are
SORTED at the two --show-structure emission sites only — storage stays
insertion-order per §2, whose divergence note gains this exception (E2). The
oracle exits nonzero on Unknown/Redact --annot fixtures (verified at golden
generation: no crash-artifact goldens exist) — handle gracefully, waiver
documented (E3). %.3f tie-breaking: implement glibc half-to-even if the M6
corpus-wide test shows it matters (E4). `unicode-bidi` gains pdfrum-doc as a
consumer for variable-text UBA line ordering, validated against the six
pinned orderings before vt layout is written (E5). `Limits` gains
`max_name_tree_depth: u32 = 32` (additive).

**[spec] 2026-08-29 (M6 implementation, three corrections to the rulings
above).**

*E1's scope estimate was wrong, and the ruling still stands.* The brief
scopes the widget appearance generator to the five corpus files that set
`/NeedAppearances`, because the form-wide regeneration path is gated on that
flag. It is — but `CPDFSDK_Widget::OnLoad` calls `ResetAppearance` on any
widget whose appearance is not valid, **ungated**, so the path reaches every
widget in the corpus that lacks a usable `/AP`. It is directly Tier-A visible:
`--annot`'s two colour lines fail whenever an appearance exists, and its
object count reports what the generated stream drew. The decision not to port
`CPWL_EditImpl` is unchanged; what changes is that this crate must build the
*chrome* — background, border, and the checkbox and radio glyph shapes — for
every such widget rather than for five files. The text body remains out of
scope and is what the residual `--annot` gap is made of. (**Superseded for the
body**: the revision below puts it in scope. The decision not to port
`CPWL_EditImpl` still holds — it turned out not to be what the body needs.)

*E3 is confirmed and becomes a harness rule.* The golden store **does**
contain crash artifacts: `redact_annot`'s manifest records
`oracle_failures: ["Annot"]` beside a zero-byte dump. Rather than waive by
file, `conformance run` now skips any artifact whose own oracle pass failed —
a golden written by an aborting process is not an answer, and comparing
against one pins the crash as the contract. `Pass::owns_artifact` maps an
artifact name back to the pass that wrote it.

*E4 resolves to "it matters".* `--annot`'s `%.3f` and the structure tree's
`%f` both go through a hand-written round-half-to-even fixed-point formatter
rather than Rust's `{:.N}`, which rounds half away from zero.

**E1's cost is now measured, and the decision is open.** Burn-down wave 9
classified every document still below SSIM 0.99 and found the widget text body
to be the **largest single named cluster: 16 of 83**, from `listbox_form` at
0.933 to `text_form_color` at 0.989. Only **2 of the 16** set
`/NeedAppearances`; the rest are ordinary files with an unadorned `/Tx` or
`/Ch` field. It is Tier-A visible too — `--annot` reports 0 objects where the
golden reports 1, 7 and 27 on `bug_983867`, `bug_477200528` and
`scrollable_widgets1`.

Two things the original ruling could not have known:

- The producer is **not** `GenerateFormAP`, which really is gated on
  `NeedAppearances` (`cpdf_annotlist.cpp:209`). It is
  `CPDFSDK_AppStream::SetAsTextField`/`SetAsListBox`/`SetAsComboBox`
  (`cpdfsdk_appstream.cpp:1686`, `:1604`, `:1532`), reached ungated from
  `cpdfsdk_widget.cpp:1109-1111`. `scrollable_widgets1` distinguishes them: it
  has no `/I`, so `GenerateListBoxAP` would highlight nothing, and the golden
  highlights its selection — `SetAsListBox` reads `/V` through
  `GetSelectedIndex` (`cpdfsdk_appstream.cpp:1631-1636`).
- **The second engine the ruling declined already exists.** `pdfrum-doc`'s
  `vt` module is a complete variable-text layout engine, and `vt::Config`
  already carries `multi_line`, `auto_return`, `sub_word`, `limit_char` and
  `char_array` — the knobs the text-field case sets. `ap/freetext.rs` is a
  working consumer of it. The remaining work is wiring, not a port:
  `widget::generate` never receives a `TextFont`, and `ap`'s text-bearing
  dispatch answers only for `FreeText`.

**[spec] 2026-08-29 — E1 IS REVISED. The widget text body is IN scope; the
`CPWL_EditImpl` port stays declined.** The ruling declined "a second
variable-text engine". Wave 9 established that there is no second engine to
write: `CPWL_EditImpl` is a shell over `CPVT_VariableText`, which `pdfrum-doc`'s
`vt` module already is in full, and the only thing the shell adds that a
generated appearance can observe is a **vertical alignment offset** — the same
argument `ap/freetext.rs` has always passed. What the ruling was protecting
against does not exist, so the ruling no longer protects anything.

What is authorized: the three producers `SetAsTextField`, `SetAsComboBox` and
`SetAsListBox`, implemented over the existing `vt` engine as
`ap::field_body`, reached from `widget::generate_with_text`. What stays
declined, unchanged: porting `CPWL_EditImpl` itself, and everything else in
`fpdfsdk/pwl` — the editing widgets, the caret, the scroll bars, the focus
machinery. None of it is reachable from a generated appearance.

Two further behaviors this makes necessary, both recorded here because they
are contracts rather than implementation:

- **A generated appearance is measured with the face its own `/DA` names**,
  loaded from the form's `/DR /Font` through the same substitution the page
  uses — not with one stock face for the whole document. The ascent and
  descent the layout stacks lines by come from the *substituted face*, and
  they differ from the base-14 metric tables enough to change the row count of
  a list box (718/−219 against 905/−211 for the hermetic corpus's Helvetica,
  which is 11.24 units of row pitch against 13.39).
- **A widget whose field type is not one of the six the builder dispatches on
  gets no appearance at all** — an intermediate `/Kids` node with no `/FT`,
  and a signature. This is Tier-A visible: `--annot`'s two colour lines report
  a colour exactly when no appearance stream exists.

The M6 waiver shrinks accordingly: **76 `--annot` artifacts to 13** (72 files
to 13), which clears M6's exit criterion outright — 99.4% — so the exclusion
is retired rather than recounted. PLAN.md §M6 is restated to say so.

**[spec] 2026-08-29 (burn-down wave 11) — the appearance-validity rules, and
`13 → 4`.** Three of the remaining thirteen turned on questions this section
had left approximate, and they are contracts because each is Tier-A visible.

- **Regeneration is gated on `!!GetDictFor("AP")` and nothing deeper**
  (`cpdfsdk_baannot.cpp:85-87`). A widget with any `/AP` dictionary is never
  given a new appearance, however unusable that dictionary is — so a radio
  button whose `/AP /N` lists only its on-state while `/AS` reads `Off` keeps
  having no drawable appearance. The earlier text called this "deliberately
  left loose"; it is not loose, it is the rule, and it only became payable
  alongside the next two.
- **A checkbox or radio button whose `/AP /N /<AS>` does not resolve to a
  stream is outlined**, hairline, in `0xFFAAAAAA`, over its normalized `/Rect`
  (`cpdfsdk_widget.cpp:364-406`, `:969`). This is a *second, deeper* validity
  test on a different code path from the one above, and the state is read from
  `/AS` alone — no `/V` or `/Parent` fallback. The fill argb it is drawn with
  is **0**: `EvenOddOptions()` there names a rule for a fill that never
  happens.
- **`/NeedAppearances` regenerates unconditionally but is usually invisible.**
  `NewAnnot` calls `ResetAppearance` whenever the flag is set
  (`cpdfsdk_pageview.cpp:108-113`), consulting no `/AP` — but `SetAsCheckBox`
  and `SetAsRadioButton` write only `/AP /N /<GetCheckedAPState()>` and
  `/AP /N /Off`, while every reader resolves `/AP /N /<AS>`. When `/AS` names
  neither, the new streams land in keys nothing looks up. `GetCheckedAPState`
  answers the first non-`Off` key **unless the field carries `/Opt`**, in which
  case it answers the widget's control index as a decimal string
  (`cpdf_formcontrol.cpp:78-89`). Honouring the flag without that rule costs
  more than it earns; with it, it is free.

Also in scope and landed: `SetAsPushButton`'s **caption** (its icon half and
six of its seven `/TP` arrangements stay declined — no corpus file carries a
`/MK /I`, and with no icon every arrangement collapses to the caption-only
box); the `/Hide` **document open action**, which is the only one of the
eighteen action types a rendered page or a dump can observe without a user or
a script engine, and which must run before either reads `/F`; and the rule
that **two `/Fields` entries sharing a `/T` are one field with two controls**,
so the value both show is the first dictionary's while their geometry stays
their own.

`--annot` artifacts: **13 → 4**, all four now the annotation font map's
per-character fallback to a second face (`CPDF_BAFontMap`, charset-driven —
*not* the two-slot `CPVT_FontMap`, which that path never enters).

## 11. `pdfrum-edit`  *(behavior: `core/fpdfapi/edit`)*

`pub fn save(doc: &Document, mode: SaveMode, out: &mut impl io::Write) -> Result<(), Error>`
with `enum SaveMode { Full, Incremental }`. Serializer writes objects
deterministically (stable dict order = insertion order; floats via `ryu`
shortest, matching dragonbox behavior). Incremental save appends
changed-object sections + new xref. Page import/reorganization as functions
`Document::import_pages(...)`.
Rulings 2026-08-29 (edit brief escalations): `Document` additionally exposes
`last_xref_offset`, `main_xref_is_stream`, and the raw `/Encrypt` dict —
additive §5 changes landed with the M7 work (E1/E2). Font subsetting is
**CID fonts only** and the contract is
`fn subset(font_bytes, gids) -> (Vec<u8>, GidMap)` — the `subsetter` crate
renumbers GIDs and strips cmap (unlike HarfBuzz RETAIN_GIDS), so /W,
ToUnicode, and Identity-H content streams are re-keyed through the returned
map (E4). The four import-path bugs the brief identifies are FIXED, not
ported — each divergence pinned by its own test (E10). M7's "oracle reopens
our files" exit is a two-step harness check: pdfrum saves, the oracle
reopens + renders (E7; pdfium_test has no save flag). Round-trip property
tests own this crate's correctness.

**Ruling 2026-08-30 (M10): E3 is SUPERSEDED — save preserves encryption.**
E3 said v1 would save encrypted documents decrypted. It no longer does. A
document loaded with a password saves **encrypted under the same handler and
the same file key**, so the output opens with that same password; the writer
re-enciphers every string and stream through §3's new `encrypt`.
`SaveOptions::remove_security` stays as the explicit opt-out and keeps its old
behavior (no `/Encrypt`, plaintext body, forced full save), so
`Error::EncryptedSaveUnsupported` now means only "this `/Encrypt` names a
cipher we hold no key for" — an `/Identity` filter, or a handler this reader
answered with `SecurityHandler::Identity`.

Password-preserving only: there is no API to change a document's password or
its encryption mode, which is what keeps `/O`, `/U`, `/OE`, `/UE` and `/Perms`
copyable rather than derivable. Four rules the writer honours:

- The `/Encrypt` dictionary is never enciphered (ISO 32000-1 §7.6.1), and is
  written **from the plaintext copy the trailer lookup found** rather than
  through the deciphering object store — which has no exemption for it and
  would otherwise hand back an `/O` and `/U` run through a cipher keyed by the
  very material they carry. A trailer holding it inline gets it promoted to a
  fresh object number, since `/Encrypt` must be a reference.
- A signature's `/Contents` and an XMP metadata payload keep their existing
  exemptions.
- `/P` is copied, not recomputed.
- The M7 `security_changed_` / `/ID`-rekey / full-save interlock is unchanged
  and still authoritative: the R2/R3 rekey forces a full save, and so does
  `remove_security`.

`Document` gains `security_handler(&self) -> &SecurityHandler` (additive §5)
so the writer can reach the file key. `pdfrum-tool` gains `--save-decrypted`
for the old behavior; plain `--save` now preserves encryption.

One deliberate divergence (edit brief **D17**): the C++ writer skips the cipher for a
metadata stream *unconditionally* — `CPDF_Stream::WriteTo` never consults
`IsMetadataEncrypted()`, whose only callers are in the parser. That leaves a
file whose `/Encrypt` claims enciphered metadata and whose metadata is
plaintext, so PDFium's own reader deciphers it into rubbish on the way back
in. We follow the flag instead: `/EncryptMetadata true` writes an enciphered
packet, `false` writes a plaintext one. The oracle's *reader* honours the
flag, so both writers' output opens in both readers — ours simply still has
its metadata.

**Ruling 2026-08-30 (M11): E6 is RESOLVED and content regeneration is
wired.** The emitters were already byte-pinned; what landed is the holder half
around them. `regenerate(&Page, &Dict, &impl Resolve) -> Option<PageRewrite>`
is the entry point, and the `None` is load-bearing: a page nothing dirtied is
not rewritten at all, so an ordinary save stays byte-identical.
`apply_rewrite` puts a `PageRewrite` into an `EditDoc` — replacing the
elements that survive, adding the ones that are new, reshaping `/Contents`,
repointing `/Resources` — and `shared_objects` is the sweep that decides
between rewriting an element and copying it, because an element two pages
point at cannot be edited in place.

Four rules the writer honours, all ported from `CPDF_PageContentManager`:

- **`/Contents` shape transitions are not symmetric.** Absent gaining an
  element becomes a lone stream at index 0; a lone stream gaining a second
  becomes an array `[old new]` and the new one is index **1**; an array
  gaining one appends at `len - 1`. A lone stream losing index 0 loses the
  `/Contents` **key** rather than becoming an empty stream. An array losing
  elements stays an array — down to one element and down to none.
- **Removal renumbers every object, and an unmapped index collapses to 0.**
  That is the C++'s default-inserting `std::map` read literally, and it is
  deliberate: an object whose element was removed was not written by this
  regeneration and its recorded index has to point somewhere.
- **An empty regenerated buffer is a deletion, unless the stream still moves
  the transform.** A stream that drew nothing but changes the CTM keeps its
  whole frame, because the streams after it are relying on the move.
- **A regenerated stream carries no filter.** The bytes are the operators,
  uncompressed, and any `/Filter` the element had is dropped with its
  `/DecodeParms` — keeping the key over plaintext describes a stream nothing
  can read.

The resource sweep maintains exactly `/ExtGState`, `/Font` and `/XObject`,
names entries `FX{E,F,X}{n}` counting from 1 on **every** call, and parks
rather than drops what it removes, so a parked name stays reserved. Usage is
recorded over **every active object**, not only those in the streams being
written: an object in a clean stream still names its font, and sweeping that
font away would break a stream nobody asked to change. An object in a clean
stream can therefore only *record* a name, never mint one.

E8's per-holder dedup caches are a `ResourceTable` created fresh per call, as
proposed — which changes only which `FX{n}` a resource gets across repeated
regenerations of one page, and is unobservable after re-parse. E9's nested
form regeneration is **not** implemented: a form object is written as the
`Do` that draws it, so its own stream is never rewritten, and the recursion
the guard was for does not arise.

## 12. JBIG2 / JPX integration

Decoding rides on `hayro-jbig2` and `hayro-jpeg2000` (see DEPS.md), but
**always behind our own thin entry points** in `pdfrum-page`'s image path — the
dependency never leaks into any signature:

```rust
pub fn decode_jbig2(globals: Option<&[u8]>, data: &[u8], w: u32, h: u32, limits: &Limits) -> Result<BitImage, Error>;
pub fn decode_jpx(data: &[u8], limits: &Limits) -> Result<JpxImage, Error>;   // codestream + JP2 wrapper, SMaskInData
```

Oracle = decoded bytes from `--save-images`/`--save-rendered-images` across
the full corpus; a conformance gap in a hayro crate gets an upstream issue
plus, if load-bearing, a first-party `pdfrum-jbig2`/`pdfrum-jpx` port dropped in
behind the same two functions (that port follows the original spec: enums for
segment/marker types, arithmetic decoder as small struct + pure functions, no
recursion on untrusted counts). Both entry points fuzzed from day one with
upstream fuzzer corpora regardless of which implementation sits behind them.

## 13. `pdfrum` (facade) + `pdfrum-tool`

Facade wraps the crates into the user API; it contains **no logic**, only
composition and ergonomics:

```rust
let doc = pdfrum::Document::open("f.pdf")?;                    // or from_bytes(Arc<[u8]>) / with_password
for page in doc.pages() {                                     // lazy, cached
    let pix = page.render(&RenderOptions::default())?;        // rayon-parallel across pages Just Works
    let text = page.text()?.to_string();
}
```

**[spec] 2026-08-30 (M11): the facade's editing surface.** `Page::edit()`
returns an owned `PageEdit` holding that page's object graph;
`Document::save_pages(path, &[PageEdit], &SaveOptions)` and
`write_pages_to` turn the changes into replacement objects on the way out.
The document is never mutated, which is the same shape form filling already
had and is what keeps `Document` `Sync` and editing two pages concurrent.

`PageEdit` offers `objects`/`object_mut`/`push`/`insert`/`remove`/
`set_visible`/`transform`/`is_modified`, plus `graph`/`graph_mut` as the
documented escape hatch onto `pdfrum-page`. Taking `object_mut` *is* the edit
— the object is marked dirty on the way out rather than leaving the caller to
remember — so a caller that only reads uses `objects`. Three plain config
structs build objects to add: `PathBuilder` (with a `rect` constructor),
`TextBuilder` and `ImageBuilder`, each with a `build() -> PageObject`, per
STYLE §4's preference for struct-update syntax over builder ladders.
`PageObject` is re-exported, being the currency of the whole surface.

`SaveOptions` gains `remove_security: bool`, defaulting **false** — the M10
ruling reached the facade, which had been forcing it true. An encrypted
document therefore saves encrypted through this path as through every other,
and an edited page's regenerated streams go through the same cipher as the
rest of the body because they are written the same way.

`pdfrum-tool` gains `--mutate=<add-rect|remove-first|touch-all>`, which
implies `--save` and has **no oracle counterpart** for the same reason
`--save` has none. It exists so the harness can run M11's exit check:
`conformance mutate-round-trip` has pdfrum mutate page 0 and save, has the
*oracle* reopen and render the result, and compares that render against ours
of the same file at SSIM >= 0.99.

`pdfrum-tool` mirrors `pdfium_test` flags/outputs byte-for-byte where Tier A
demands (`--png --md5 --txt --annot --show-metadata --show-pageinfo
--show-structure --save-images --pages --scale --password --time --font-dir`),
including UTF-32LE text output and the `MD5:<path>:<hash>` stdout format, so
the harness diffs like-for-like.

**([spec] 2026-08-29, tool structure and staged flags.)** The tool grows one
output format per crate, so it is built to accept the whole flag surface from
the start and implement what it can: a flag whose crate does not exist yet is
*recognized*, noted once on stderr, and produces no output, because the
harness runs one fixed set of passes against every candidate and a tool that
rejected those flags would tag rendering and text as a **tool error** rather
than as unimplemented — and would lose the page count the same invocation
reports on stderr. Modules: `options` (the oracle's own hand-written parse,
`--pages=` stream-extraction semantics included), `run` (the per-file
sequence, exit conventions and page-count accounting), and one module per
dump. `run::dump_page` is the single dispatch point a new format plugs into.

Three behaviors the dumps depend on that are *not* in the C++'s dump code and
are therefore easy to miss: `Unsupported feature: <name>.` notices go to
**stdout** and so land inside the metadata and pageinfo dumps, in positions
fixed by which C++ call site raises them; `--show-pageinfo` reads the five
boxes **off the page's own dictionary**, without inheritance, normalization,
or the letter-sized default; and `--show-metadata`'s `(N bytes)` figure is the
UTF-16LE encoding's length *including* its terminator, so an absent key still
prints a line reading `(2 bytes)` while a document with no usable `/Info`
prints nothing at all.

---

## 14. Design-brief template (required sections)

Every `docs/design/<crate>.md` must contain, in order: **Behavior inventory**
(what the C++ does, incl. every limit constant and recovery heuristic, with
C++ file references); **Divergences** (where we deliberately differ and why);
**Module plan** (files, types beyond this spec, data flow); **Test plan**
(which C++ unittest assertions port, snapshot/fuzz/conformance clusters);
**Open questions** (resolved before implementation or escalated to the user).
A brief that proposes changing this spec triggers §0.

Orchestrator affirmation 2026-08-30: pdfrum-edit divergence D17 accepted — metadata encryption follows /EncryptMetadata; the C++'s unconditional skip is a destructive upstream writer bug and is not reproduced.
