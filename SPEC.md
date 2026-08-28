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
pub struct Diagnostics { entries: Vec<Diagnostic>, limit: usize }
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

No string types, no stream traits, no "utils". If something feels like it
belongs here, it probably belongs in the crate that uses it.

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
pub struct ObjRef { pub num: u32, pub gen: u16 }

pub struct PdfString { pub bytes: Box<[u8]>, pub hex: bool }  // raw bytes + source spelling (hex vs literal — C++ round-trips it, CPDF_String::is_hex; the writer needs it). Helpers: as_text() -> Cow<str> (PDFDoc/UTF-16BE/UTF-8 detection)
pub struct Name(Box<[u8]>);               // helpers: as_str(); pub mod names { pub const LENGTH: &Name; ... } for constants/
pub struct Array(pub Vec<Object>);
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
    // typed accessors used everywhere; all resolve refs internally:
    pub fn get<'a>(&'a self, key: &Name, r: &impl Resolve) -> Option<Resolved<'a>>;
    pub fn int(&self, key: &Name, r: &impl Resolve) -> Option<i64>;
    pub fn number(&self, ...) -> Option<f32>;     // Int|Real coercion, as C++ GetNumber
    pub fn name(&self, ...) -> Option<&Name>;     /* + str_, array, dict, stream, rect, matrix */
}
```

Decisions: recursion into `Object` is bounded by `Limits.max_object_nesting`
at *parse* time, so access code may recurse freely. `Object` is `Send + Sync`.
Equality is structural; no interning v1 (revisit with benchmarks only).
Dict order: C++ stores dicts in a sorted `std::map` (so its writer emits keys
sorted); we deliberately keep insertion order in storage *and* serialization —
written-file byte layout is not an oracle target (round-trip fidelity is
semantic: reparse + re-render), so this divergence is accepted and permanent.
Integer accessor semantics are tri-state (see the resolution matrix and
`FX_Number` inventory in `docs/design/pdfrum-object.md`): `Int(i64)` stores the
parsed value; the accessor layer provides the C-int wrapping view
(`as_c_int() -> i32`) that Tier-A behaviors observe.

## 3. `pdfrum-crypt`  *(behavior: `core/fdrm`, parser security handlers)*

Closed enum, not a trait: `pub enum SecurityHandler { Rc4V2 {..}, AesV4 {..}, AesV5 {..}, Identity }` with
`SecurityHandler::from_encrypt_dict(dict, file_id, password) -> Result<Self, Error>`
(handles /R 2..6, user+owner password verification, returns `WrongPassword` variant distinctly) and
`fn decrypt(&self, obj: ObjRef, class: CryptClass, data: &[u8]) -> Vec<u8>` where
`enum CryptClass { Stream, String, Embedded }` (per-class crypt filters /StmF /StrF).
Primitives from RustCrypto (`aes`+`cbc`, `md5`, `sha1`, `sha2`); RC4 written
in-crate (~30 lines, no dep).

## 4. `pdfrum-filters`  *(behavior: `core/fxcodec` basic codecs)*

Pure functions on slices; no streaming v1 (C++ also decodes to memory):

```rust
pub enum Filter { Flate, Lzw, AsciiHex, Ascii85, RunLength, CcittFax, Jbig2, Dct, Jpx, Crypt }
impl Filter { pub fn from_name(n: &Name) -> Option<Filter>; }  // incl. abbreviations /Fl /AHx ...

pub fn decode(filter: Filter, input: &[u8], params: &Dict, r: &impl Resolve, limits: &Limits, diags: &mut Diagnostics)
    -> Result<DecodeOutput, Error>;
pub enum DecodeOutput { Bytes(Vec<u8>), Image(NeedsImageCodec) }  // Dct/Jpx/Jbig2 punt to pdfrum-page's image path
pub fn predictor(data: Vec<u8>, params: PredictorParams) -> Result<Vec<u8>, Error>;  // PNG+TIFF predictors
```

Flate via `miniz_oxide` with a hard output cap from `Limits`; LZW via `weezl`
(TIFF variant + EarlyChange); CCITT G3/G4 via `hayro-ccitt`; RLE and AHx/A85
written in-crate. Decisions: filter chains applied left-to-right by the
caller; a chain ending in an image codec returns the *pre-image* bytes.

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
  plus an embedded-CMap parser (CID ranges, usecmap).
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

Substitution/fallback: `fontdb` scan + embedded Foxit fallback faces
(`core/fxge/fontdata`, BSD) selected by a port of `CFX_FontMapper`'s
name/charset/flags heuristics (brief inventories them). Glyph outline cache:
`GlyphCache` keyed `(font-id, gid, hint-flags)` owned by the render session,
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
Decoded-image cache: `HashMap<ObjRef, Arc<ImageData>>` per render/extract session.

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
Isolated/knockout groups: engine-level compositing over offscreen layers —
backends never know about knockout. `pdfrum-raster-vello` (vello_cpu) and
`pdfrum-raster-tinyskia` implement the two traits; conformance Tier C diffs them.

## 9. `pdfrum-text`  *(behavior: `core/fpdftext`)*

Pure derivation, no rendering dependency:
`pub fn extract(page: &Page, r: &impl Resolve) -> TextPage` where
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
