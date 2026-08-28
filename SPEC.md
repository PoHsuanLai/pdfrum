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
pub struct Array(Vec<Object>);            // private field + `Array::of(values)`; the invariant (no direct streams) is enforced in push()
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

Decisions: recursion into `Object` is bounded by `Limits.max_object_nesting`
at *parse* time, so access code may recurse freely. `Object` is `Send + Sync`.
Equality is structural and `PartialEq`-only (`Real(f32)` has no total
equality); `ObjRef`, `Name` and `PdfString` are additionally `Eq + Hash`.
No interning v1 (revisit with benchmarks only).
Dict order: C++ stores dicts in a sorted `std::map` (so its writer emits keys
sorted); we deliberately keep insertion order in storage *and* serialization —
written-file byte layout is not an oracle target (round-trip fidelity is
semantic: reparse + re-render), so this divergence is accepted and permanent.
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
}
pub struct CharItem { pub code: CharCode, pub cid: Option<Cid>, pub gid: Gid, pub unicode: SmallVec<[char;2]>, pub width: f32 }
```

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
    fn new_target(&self, w: u32, h: u32) -> Self::Device;
    fn finish(&self, d: Self::Device) -> Pixmap;   // RGBA8, premultiplied
}

pub struct RenderOptions { pub transform: Affine, pub text_aa: TextAa, pub grayscale: bool, /* (abridged) */ }
pub fn render_page(page: &Page, opts: &RenderOptions, backend: &impl RasterBackend) -> Pixmap;
```

Engine decisions: text renders as filled glyph `BezPath`s through the glyph
cache (render modes fill/stroke/clip per Tr). Axial/radial shadings map to
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
way. `pdfrum-raster-vello` (vello_cpu) and
`pdfrum-raster-tinyskia` implement the two traits; conformance Tier C diffs them.

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
`TextPage { chars: Vec<CharBox> /* unicode, bbox, origin, font-size, angle */, runs: … }`
plus `TextPage::find(needle, opts) -> impl Iterator<Item=Range<usize>>` and
`web_links()`. The C++ heuristics (space insertion thresholds, line breaks,
reading order, rotated text, hyphenation) are ported *exactly* — this crate is
Tier-A byte-exact against the oracle, so brief must enumerate every heuristic
constant from `cpdf_textpage.cpp` before implementation.

## 10. `pdfrum-doc`  *(behavior: `core/fpdfdoc`)*

Records + functions over the catalog: `Bookmarks` (outline tree iterator with
cycle guard), `NameTree` lookup, `Link`/`Action`/`Dest` enums, `Annotation`
enum (typed per subtype), AcroForm: `Form { fields: Vec<Field> }`,
`Field { kind: FieldKind /* Text|Check|Radio|Combo|List|Button|Sig */, value, flags, kids }` with
`set_value()` + **appearance-stream generation** (port `cpdfsdk_appstream`/
variable-text layout — a pure `Field -> content-stream bytes` function).
Struct tree reader for `--show-structure` parity.

## 11. `pdfrum-edit`  *(behavior: `core/fpdfapi/edit`)*

`pub fn save(doc: &Document, mode: SaveMode, out: &mut impl io::Write) -> Result<(), Error>`
with `enum SaveMode { Full, Incremental }`. Serializer writes objects
deterministically (stable dict order = insertion order; floats via `ryu`
shortest, matching dragonbox behavior). Incremental save appends
changed-object sections + new xref. Page import/reorganization as functions
`Document::import_pages(...)`. Font subsetting via `subsetter` crate behind
`fn subset(font_bytes, gids) -> Vec<u8>`. Round-trip property tests own this
crate's correctness.

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

`pdfrum-tool` mirrors `pdfium_test` flags/outputs byte-for-byte where Tier A
demands (`--png --md5 --txt --annot --show-metadata --show-pageinfo
--show-structure --save-images --pages --scale --password --time --font-dir`),
including UTF-32LE text output and the `MD5:<path>:<hash>` stdout format, so
the harness diffs like-for-like.

---

## 14. Design-brief template (required sections)

Every `docs/design/<crate>.md` must contain, in order: **Behavior inventory**
(what the C++ does, incl. every limit constant and recovery heuristic, with
C++ file references); **Divergences** (where we deliberately differ and why);
**Module plan** (files, types beyond this spec, data flow); **Test plan**
(which C++ unittest assertions port, snapshot/fuzz/conformance clusters);
**Open questions** (resolved before implementation or escalated to the user).
A brief that proposes changing this spec triggers §0.
