# Making pdfrum a fully idiomatic Rust library

**Status:** proposal. Not a `[spec]` change yet — each work package below
becomes one when it lands, per SPEC.md §0.
**Date:** 2026-09-02.
**Scope:** the public API of `pdfrum` and, secondarily, the curated public
surface of the member crates it composes. Behaviour against the oracle does
not move.

PLAN.md locked the API as “pure idiomatic Rust” on day one. STYLE.md §4
points at the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
and already names the bar: no `get_` prefixes, config structs not builder
ladders, one-screen `lib.rs` re-export, `Send + Sync`, `Debug` on everything
public. This document is the gap analysis and the work to close it.

The engine is not the problem. The core open / page / render / text / save
path is already a Rust library. The remaining work is to make the rest of the
surface match that core, without smuggling PDFium or Win32 names, packed
integers, or types a `cargo add pdfrum` caller cannot name.

---

## 0. Verdict

`pdfrum` is idiomatic in the sense that matters for a first `cargo add`:
types over C status codes, `Result` over `FPDF_BOOL`, borrows over
`RetainPtr`, a trait at the one real seam, no `unsafe`, no panics, `Send +
Sync`. It is not idiomatic in the stricter library sense: newtypes for
indices and versions, no boolean mode arguments, one method not a cartesian
product, one geometry vocabulary, an `Event` enum instead of
`on_mouse_down(page, x, y, …)`, no Windows virtual-key codes, and a facade
whose signatures only name types the facade re-exports.

Do this as a breaking pass on `0.1.0`. There are no downstream crates to
keep compatible, and a pile of `#[deprecated]` aliases would freeze the
unrusty names into the docs. When a package lands, SPEC.md and the facade
doctests move in the same commit.

---

## 1. Governing principle

**Idiomatic at the facade. Oracle-faithful below.**

The C++ is an oracle for *behaviour*, not a vocabulary for *types*. A page
index that is `u32` in the file, an `/Ff` word whose bits change meaning by
field type, a Win32 `VK_PRIOR`, a form-layout midpoint computed as
`(top + bottom) / 2` in `f32` — all of that stays in the crate that has to
match a golden. The facade translates.

Two tests for every change:

1. A caller who has never seen PDFium, `fpdf_formfill.h`, or Win32 can use
   the type without reading a comment that names those things.
2. `conformance` and the `.evt` goldens do not move. Internal `f32` midpoints,
   raw `/Annots` indices, packed permission bits, UTF-32LE dumps — those
   remain, one layer down.

STYLE.md’s transliteration test applies to the public surface too: *would
this signature look the same if the author had never seen the C++?*

---

## 2. What is already idiomatic — do not undo

These are load-bearing and correct. A pass that “cleans up” any of them is
a regression.

- **Ownership.** `Document` owns the bytes. `Page<'a>`, `Form<'a>`,
  `Annotation<'a>` borrow it. Edits and form fills buffer on a side object
  and apply at save, so `Document` stays `Sync`. No `Rc<RefCell<_>>` graph.
- **Config structs with `Default` + struct-update syntax.** `OpenOptions`,
  `RenderOptions`, `SaveOptions`, `SessionConfig`, `PathBuilder`. STYLE.md
  §4 forbids builder ladders; keep that.
- **Enums for closed sets.** `Rotation`, `ColorMode`, `TextAa`, `FieldKind`,
  `Subtype`, `Update`, `UpdateKind`, `Placement`.
- **One `Error`, domain variants, `thiserror`, `#[non_exhaustive]`.** Inner
  errors reachable via `source`. Damage is `Diagnostics`, not `Err`.
- **The rasterizer seam.** `Page::render_on<B: RasterBackend>` is the right
  shape. The withdrawn `Backend` enum was the unrusty one; do not bring it
  back.
- **kurbo / peniko as the 2D vocabulary**, at least on the render path.
- **Values out, not callbacks.** `Response` / `AppearanceUpdate` /
  `PopupView` / `ScrollView`. The 2026-09-01 ruling against a `FormChrome`
  trait stands.
- **Escape hatches, documented as such.** `Document::parser`, `Page::objects`,
  `Annotation::dict`, `PageEdit::graph`. Power users reach past the facade;
  the facade does not pretend to be total.
- **`Send + Sync` as a tested property**, rayon as the caller’s dependency.

The happy path stays:

```rust
let doc = Document::open("report.pdf")?;
for page in doc.pages() {
    let pixmap = page.render(&RenderOptions::scaled(2.0))?;
    let text = page.text().all_text();
}
```

Everything below is about making the *rest* of the crate feel like that.

---

## 3. Two audiences

| Audience | Crate they depend on | What they may see |
|---|---|---|
| Almost everyone | `pdfrum` only | The facade re-export block. Every type that appears in a `pdfrum` signature is named in that block. |
| Engine / conformance / a fourth rasterizer | a member crate | A curated `lib.rs` re-export. Helper functions, oracle dump formats, and scan-conversion internals are crate-private or `#[doc(hidden)]`. |

The dual-crate story (“this crate composes, it does not hide”) is a feature.
It is not a licence to put `pdfrum_doc::ap::Focus` on a facade return type
without re-exporting it, or to ship `pdfrum_text::pipeline::is_float_zero`
as public API.

---

## 4. Non-goals

- **No behaviour change.** Goldens, `.evt` scripts, SSIM thresholds, the
  diagnostics channel, damage-tolerant open — untouched.
- **No `bitflags` crate.** Closed decision; see §6.
- **No fourth trait seam.** `RenderDevice`, `Resolve`, `Cascade` stay the
  list. Chrome stays values the host pulls.
- **No builder ladders** for options. Struct update is the style.
- **No hiding escape hatches.** `parser()` / `objects()` / `dict()` /
  `graph()` stay. They become *complete*: every type they mention is
  re-exported, or they are clearly marked as “you now depend on crate X”.
- **No C ABI, no `prelude` module, no `get_` prefixes.**
- **Member crates stay published.** They do not become `publish = false`
  implementation details. Their *surface* shrinks; their existence does not.

---

## 5. Work packages

Each package is one `[spec]` commit (or a tight stack of them), independently
reviewable, with doctests updated in the same change. Internal types may keep
the old shape behind `From` / `Into` at the crate boundary.

### WP1 — Types instead of encodings

Packed integers and boolean-mode arguments are the most visible C residue.

**`PdfVersion`.** `Document::version() -> u8` where `17` means 1.7, and
`SaveOptions.version: Option<u8>` with the same encoding.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PdfVersion {
    pub major: u8,
    pub minor: u8,
}

impl PdfVersion {
    pub const PDF_1_4: Self = Self { major: 1, minor: 4 };
    pub const PDF_1_7: Self = Self { major: 1, minor: 7 };
    pub const PDF_2_0: Self = Self { major: 2, minor: 0 };
}

impl Document {
    pub fn version(&self) -> PdfVersion { /* from the header */ }
}
```

The packed `u8` stays as a private conversion next to the parser. The facade
never shows it. `SaveOptions.version` becomes `Option<PdfVersion>`.

**`PageIndex`.** STYLE.md §2 already lists this newtype. It does not exist.
Page numbers are `u32` on `Document::page`, `FormSession` mouse methods,
`AnnotId.page`, `import_pages`, and `i32` internally for destination
resolution (`page_index_of` returns `-1`).

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageIndex(u32);

impl PageIndex {
    pub const fn new(n: u32) -> Self { Self(n) }
    pub const fn get(self) -> u32 { self.0 }
}

impl From<u32> for PageIndex { /* */ }
impl Document {
    pub fn page(&self, index: impl Into<PageIndex>) -> Result<Page<'_>> { /* */ }
}
```

`impl Into<PageIndex>` keeps `doc.page(0)` working. Destinations that cannot
name a page stay `Option<PageIndex>`, never `-1`.

**`Permissions`.** `Document::permissions(owner: bool) -> u32` is a boolean
mode argument plus a raw ISO bitfield. `FormSession` then tests
`bits & 0x100`. Callers ask questions, not bit numbers.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub print: bool,
    pub modify: bool,
    pub copy: bool,
    pub annotate: bool,
    pub fill_form: bool,
    pub extract: bool,
    pub assemble: bool,
    pub print_high_quality: bool,
}

impl Permissions {
    pub const ALL: Self = /* every field true */;
    pub fn from_bits(bits: u32) -> Self { /* ISO 32000-1 table 22 */ }
    pub fn bits(self) -> u32 { /* for a writer that must emit the word */ }
}

impl Document {
    pub fn permissions(&self) -> Permissions { /* the password that opened it */ }
    pub fn owner_permissions(&self) -> Permissions { /* owner view */ }
}
```

This is a struct of booleans, **not** a bitflags newtype. See §6.

**`Error::WrongPassword`.** The docs already say this is the one variant
worth matching to prompt again, and then they send the caller into
`pdfrum_parser::LoadError` through `Error::Open`. Lift it:

```rust
pub enum Error {
    #[error("wrong password")]
    WrongPassword,
    Open(#[from] pdfrum_parser::LoadError),
    // ...
}
```

`open_with_password` maps `LoadError::WrongPassword` here so a facade-only
caller never names the parser crate.

### WP2 — Flag newtypes, without the `bitflags` crate

Covered in full in §6. Short version: do not add `bitflags`. Give
`Modifiers`, `FontFlags`, and `AnnotFlags` the same typed-constant + `BitOr`
+ `contains` algebra. Leave `FieldFlags` as predicates. Replace the
permissions `u32` with WP1’s struct.

### WP3 — Collapse the render / text method grid

Six methods for “which rasterizer × which cache”:

```text
render / render_on
render_with / render_with_on
render_session / render_session_on
```

and three for text (`text` / `text_with` / `text_session`). The engine
already has one generic `render_page_with_caches`. The facade should too.

```rust
impl Page<'_> {
    pub fn render(&self, options: &RenderOptions) -> Result<Pixmap> {
        self.render_on(&VelloCpuBackend::new(), options, &mut RenderSession::new())
    }

    pub fn render_on<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
        session: &mut RenderSession,
    ) -> Result<Pixmap> { /* the one body */ }

    pub fn text(&self) -> TextPage {
        self.text_on(&mut RenderSession::new())
    }

    pub fn text_on(&self, session: &mut RenderSession) -> TextPage { /* */ }
}
```

Two methods each, not six and three. `RenderSession` already carries both
caches; `BuildContext` remains reachable as `session.build` for the
substitution-options case. `FormSession::with_context` keeps taking a
`BuildContext` because a session that will not render still needs fonts.

`RenderSession.caches` is currently `pdfrum_render::RenderCaches` and that
type is **not** re-exported. Either re-export it, or make the field private
and offer `session.build_mut()`. Public fields of unexported types are how
the “you must depend on a second crate” leak starts.

### WP4 — One geometry vocabulary on the facade

The facade currently speaks three rectangles and two points:

| Type | Crate | Scalar | Shape |
|---|---|---|---|
| `kurbo::Rect` / `Point` | re-exported whole crate | `f64` | `{x0,y0,x1,y1}` |
| `pdfrum_form::tab::Rect` as `FormRect` | form | `f32` | `{left,bottom,right,top}` |
| `pdfrum_form::Point` | form, not re-exported | `f32` | `{x,y}` |
| mouse methods | facade | `f32` | bare `x, y` arguments |

A host converting a click from a window to a page already has a `kurbo::Point`
from the same space `Page::crop_box` uses. Making them type `f32 x, y` and
then a second `Rect` is a PDFium bridge.

**Facade signatures take kurbo.** Convert to the form crate’s `f32` types at
the crate boundary. The `(top + bottom) / 2` midpoint, the strict banding
comparisons, the oracle’s `f32` event script — all of that stays inside
`pdfrum-form`. Drop `FormRect`. Drop the flattened `x, y` arguments.

```rust
impl FormSession<'_> {
    pub fn apply(&mut self, event: Event) -> Response { /* */ }

    pub fn mouse_move(
        &mut self,
        page: impl Into<PageIndex>,
        at: kurbo::Point,
        modifiers: Modifiers,
    ) -> Response { /* Event::MouseMove */ }
}
```

Convenience methods are allowed; they take `kurbo::Point`, not four scalars.
`PopupGeometry.rect` on the way *out* is also `kurbo::Rect`. The form crate
may keep its own `Rect` privately.

Re-export **the types used in signatures**, not the whole crates:

```rust
pub use kurbo::{Affine, BezPath, Point, Rect, Size, Vec2};
pub use peniko::{BlendMode, Color};
```

`pub use kurbo;` dumps kurbo’s entire public API into `pdfrum::kurbo::`. A
caller who wants a specialised kurbo type adds `kurbo` themselves. That is
the ordinary Rust rule.

### WP5 — Form session: `Event` in, Rust names

`pdfrum-form` already has a decent `Event` enum. The facade unwraps it into
Win32-shaped methods and then SPEC §15.8 says they are “named to the Rust
API guidelines”. They are named to `FORM_OnMouseMove`.

Target surface:

```rust
impl FormSession<'_> {
    pub fn apply(&mut self, event: Event) -> Response;

    // thin wrappers over apply, no `on_` prefix
    pub fn mouse_move(&mut self, page: impl Into<PageIndex>, at: Point, modifiers: Modifiers) -> Response;
    pub fn mouse_down(&mut self, page: impl Into<PageIndex>, at: Point, modifiers: Modifiers) -> Response;
    pub fn mouse_up(&mut self, page: impl Into<PageIndex>, at: Point, modifiers: Modifiers) -> Response;
    pub fn mouse_wheel(&mut self, page: impl Into<PageIndex>, at: Point, delta: (i32, i32), modifiers: Modifiers) -> Response;
    pub fn double_click(&mut self, page: impl Into<PageIndex>, at: Point, modifiers: Modifiers) -> Response;
    pub fn key_down(&mut self, key: Key, modifiers: Modifiers) -> Response;
    pub fn character(&mut self, ch: char, modifiers: Modifiers) -> Response;

    pub fn blur(&mut self) -> Response;          // was force_kill_focus
    pub fn focused_annot(&self) -> Option<AnnotId>;
    pub fn focused_text(&self) -> Option<String>;
    pub fn selected_text(&self) -> Option<String>;
    pub fn replace_selection(&mut self, text: &str) -> bool; // see WP9
}
```

Rename:

| Today | Target | Why |
|---|---|---|
| `force_kill_focus` | `blur` | `CPDFSDK_InterForm::ForceKillFocus` is not a Rust method name. `commit_focus` is acceptable if `blur` feels too DOM. |
| `VirtualKey` | `Key` | Drop the alias. |
| `EventModifiers` | `Modifiers` | Drop the alias. |
| `EventResponse` | `Response` | Drop the alias. |
| `MouseButton` | `Button` or just use `Button` | Drop the alias. |
| `on_button(..., down: bool, ...)` | `Event::MouseDown { button: Button::Right, .. }` via `apply` | A boolean that means “up or down” is an enum you already have. |
| `set_page_in_view` | `set_viewed_page` | Same job, no PDFium “page in view” calque. Keep the Tab-from-nothing behaviour. |
| `FormSession::inner()` | delete from the facade, or `#[doc(hidden)]` | An escape hatch onto a type with the same name in another crate. Power users depend on `pdfrum-form` and build a session there. |

Keep `choose` / `close_popup` / `popup_for_page` / `scroll_view` — those are
already the value-not-callback design. Re-export `Event` from the facade so
`apply` is usable without a second crate.

The four constructors (`new`, `with_config`, `with_context`,
`with_config_in`) stay. The pair taking a `BuildContext` is load-bearing
(SPEC §15.8); do not collapse them.

### WP6 — `Key` as an enum

SPEC §15.5 defends `Key(pub u16)` because the wire format admits any integer
and the ported assertions send codes the form layer does not handle. That
argument is why the *parser* of `.evt` files stores a `u16`. It is not why
the *library* surface should.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    Backspace,   // was BACK,  0x08
    Tab,
    Return,      // 0x0D; keep distinct from Newline (0x0A)
    Escape,
    Space,
    PageUp,      // was PRIOR, 0x21
    PageDown,    // was NEXT,  0x22
    End,
    Home,
    Left,
    Up,
    Right,
    Down,
    Insert,
    Delete,
    A, Y, Z,     // accelerator letters the form layer branches on
    Shift,       // reported as a key; never consumed
    Control,
    Other(u16),  // F1, digits, anything the .evt corpus sends
}

impl Key {
    pub fn from_virtual(code: u16) -> Self { /* the table, remainder Other */ }
    pub fn virtual_code(self) -> u16 { /* inverse */ }
}
```

`Other` is the point, not the objection. Exhaustive matches on our own keys
fail to compile when a variant is added; `Other` is the arm that means “the
form layer does not decide on this”. Win32 names (`PRIOR`, `NEXT`, `BACK`,
`VirtualKey`) do not appear in rustdoc.

The `.evt` parser keeps talking `u16` and converts at the boundary.

### WP7 — Facade signatures only name re-exported types

A `cargo add pdfrum` caller must be able to write a type annotation for
every value they receive. Today they cannot:

| Signature | Unexported type |
|---|---|
| `FormSession::focus_for_page` | `pdfrum_doc::ap::Focus` (and `FocusBox`) |
| `UpdateKind::Regenerated` / `LiveEdit` | `pdfrum_doc::GeneratedAp` |
| `TextBuilder.font` / `ImageBuilder.source` | `pdfrum_object::ObjRef` |
| `TextBuilder.render_mode` | `pdfrum_page::TextRenderMode` |
| `Document::parser` | `pdfrum_parser::Document` |
| `Page::objects` / `PageEdit::graph` | `pdfrum_page::Page` (name collision with facade `Page`) |
| `RenderSession.caches` | `pdfrum_render::RenderCaches` |
| `Annotation::dict` | `pdfrum_object::Dict` |

Two allowed resolutions, pick per type:

1. **Re-export it**, with a facade-level doc comment, if a normal caller
   matches on it or stores it. `Focus`, `FocusBox`, `GeneratedAp`,
   `ObjRef`, `TextRenderMode` belong here.
2. **Mark the method an escape hatch** and say so in the first sentence of
   its rustdoc: “Requires `pdfrum-parser`.” `parser()`, `graph()`,
   `graph_mut()`, `dict()` are this. The return type may stay namespaced
   (`pdfrum_parser::Document`) so the collision with facade `Document` /
   `Page` is obvious.

Do not mix the two. `focus_for_page` is not an escape hatch; it is what a
renderer asks every frame (SPEC §15.8). `GeneratedAp` is the payload of the
ordinary `Response`. Those must be in the `lib.rs` block.

Drop `pub mod edit`. The types are already re-exported at the crate root.
Two paths (`pdfrum::PageEdit` and `pdfrum::edit::PageEdit`) for one type is
noise.

### WP8 — Text extraction: index spaces and ranges

`TextPage` is re-exported unchanged from `pdfrum-text`, and that crate still
speaks the C API:

- `page_text(start, count)` is `FPDFText_GetText`. Rust wants a range.
- `rects(start, count: Option<usize>)` uses `None` as “to the end”.
- `find` returns `Range<usize>` in **text** index space; `web_links` uses
  **character-list** indices. Two spaces, no newtype. STYLE.md §2 asked for
  this and did not get it.
- `all_text()` exists because the public field is already named `text:
  Vec<char>`.
- `to_utf32le()` is an oracle dump format on a library type.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharIndex(usize);   // TextPage::chars
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextIndex(usize);   // TextPage search-facing text

impl TextPage {
    pub fn chars(&self) -> &[CharBox];
    pub fn char(&self, index: CharIndex) -> Result<&CharBox, Error>;
    pub fn as_str(&self) -> String;                 // was all_text; or Display
    pub fn slice(&self, range: impl RangeBounds<TextIndex>) -> String;
    pub fn find<'a>(&'a self, needle: &str, options: FindOptions)
        -> impl Iterator<Item = Range<TextIndex>> + 'a;
    pub fn web_links(&self) -> Vec<WebLink>;        // ranges are CharIndex
    pub fn rects(&self, range: impl RangeBounds<CharIndex>) -> Vec<kurbo::Rect>;
}
```

`page_text` and `to_utf32le` move to `pdfrum-tool` (or stay on `TextPage`
as `#[doc(hidden)]` methods the tool uses). They are not library API.

Public fields `chars` / `text` / `runs` can stay — STYLE.md §1 wants data,
not objects — but `text: Vec<char>` should not collide with a method named
`text`. Rename the field `search_text` or wrap it.

### WP9 — Mutation returns `Option` / `Result`, not `bool`

C APIs return `FPDF_BOOL`. These methods still do:

| Method | `false` means | Target |
|---|---|---|
| `PageEdit::insert` | index past the end | `Result<(), IndexOutOfRange>` |
| `PageEdit::transform` | no such object | `Result<(), IndexOutOfRange>` |
| `PageEdit::set_visible` | no such object | `Result<(), IndexOutOfRange>` |
| `FormSession::replace_selection` | nothing focused | `bool` is actually the answer (“did it change”); keep, document |
| `FormSession::set_index_selected` | refused | keep `bool` (“accepted”), or an enum `{ Accepted, Refused }` if we grow a third outcome |
| `Form::set` | unknown name is **ignored** | `Result<(), UnknownField>` — silent ignore is the C move |

`set_visible(index, visible: bool)` additionally takes a boolean mode.
Split it:

```rust
impl PageEdit {
    pub fn show(&mut self, index: usize) -> Result<(), IndexOutOfRange>;
    pub fn hide(&mut self, index: usize) -> Result<(), IndexOutOfRange>;
}
```

`set_checked(name, checked: bool)` is the one boolean that is idiomatic —
the value *is* a bool. Keep it. `set_index_selected(index, selected: bool)`
is the same shape and can stay once the “no such row” path is `false`
rather than a panic, which it already is.

`PageEdit::remove` already returns `Option<PageObject>`. That is the
pattern.

`Outline`’s `IntoIterator` currently `collect`s into a `Vec` and iterates
that. Implement it over the existing iterator, or drop `IntoIterator` and
keep `iter()`. Do not allocate to satisfy a trait.

### WP10 — Options and colour: drop inverted flags, use peniko

`RenderOptions { no_path_smooth, no_image_smooth }` are PDFium flag names
(`FPDF_RENDER_NO_SMOOTHTEXT` and friends) with inverted defaults. Rust
options are positive and default to the common case:

```rust
pub struct RenderOptions {
    pub transform: Affine,
    pub color_mode: ColorMode,
    pub text_aa: TextAa,
    pub smooth_paths: bool,          // default true
    pub interpolate_images: bool,    // default true
    pub background: Option<Color>,
    pub annotations: bool,           // default true
}
```

`PathBuilder.fill: Option<[f32; 3]>` and `TextBuilder.fill: [f32; 3]` sit
next to a crate that already re-exports `peniko::Color`. Use `Color` (or
keep a small `Rgb { r: f32, g: f32, b: f32 }` if the 0..=1 RGB-only
constraint is the point). Do not make a caller who has a `peniko::Color`
unpack it into a triple to draw a rectangle.

`FieldFlags::do_not_scroll` is a spec-bit name. A predicate `scrolls(self)
-> bool` (inverting the bit) is the Rust spelling; keep `do_not_scroll` as
a synonym only if a reader of ISO 32000 is expected to search for it —
then `#[doc(alias = "DoNotScroll")]` on `scrolls` is enough.

### WP11 — Member-crate surface hygiene

STYLE.md §4: “Public API of every crate fits in one `lib.rs` re-export
block a reviewer can read in one screen.”

True of `pdfrum`. False of `pdfrum-page`, `pdfrum-render`, `pdfrum-form`,
`pdfrum-font`, `pdfrum-text`. Those export `pub mod` trees (`scanline`,
`glyph`, `pipeline`, `route`, `hit`, `inline_image`) and helper functions
(`is_float_zero`, `peniko_mix`, `debug_runs`, `LCD_FIR5`).

Rule, applied per crate:

1. `lib.rs` has `mod foo;` (private) and `pub use foo::{The, Types};`.
2. A type another **workspace crate** needs is `pub`. A function only this
   crate uses is not. Cross-crate helpers cannot be `pub(crate)` (that is
   per-crate); they are `pub` and either in the curated re-export or
   `#[doc(hidden)]`.
3. Oracle dump formats (`to_utf32le`, annot flag `names()` in dump order,
   `--txt` sentinels) live with the tool or behind `#[doc(hidden)]`.
4. `cargo public-api -p <crate>` is snapshotted. A new public item in a
   member crate is a review question: facade-facing, sibling-crate, or
   should have been private?

This is mechanical and large. Do it crate by crate, starting with
`pdfrum-text` and `pdfrum-render` (the noisiest), not as one diff.

### WP12 — Docs that match the crate

The crate-level rustdoc still says **“No JavaScript. … a permanent scope
decision.”** `pdfrum-form` has a default-off `script` feature, `Cascade` is
the third seam, and the facade hardcodes `NoScripts`. The sentence is
false, and a host who wants scripts cannot reach the seam without depending
on `pdfrum-form` directly.

Either:

- expose the seam on the facade (`FormSession::with_cascade`, or a `script`
  feature on `pdfrum` that re-exports `ScriptCascade`), and rewrite the
  crate docs to “off by default, on behind `script`”, or
- keep the facade script-free and rewrite the crate docs to say so
  precisely: “this crate never runs scripts; `pdfrum-form`’s `script`
  feature does.”

Stale docs are an API bug. Same pass: `docs/status/pdfrum-facade.md` still
lists `RenderOptions::backend` and `Backend { Vello, TinySkia }`, which
were withdrawn 2026-09-02.

`Attachment.name` is a public field; almost every peer is a getter. Pick
one style. Public fields for plain data (`Metadata`, `OpenOptions`,
`RenderOptions`, `AnnotId`) and getters for computed or borrowed values
(`Bookmark::title`, `Field::value`) is the existing, correct split —
`Attachment.name` is data and may stay a field; document it.

### WP13 — Mechanical gates

Once the surface is the intended one, stop it drifting.

- **`cargo public-api -p pdfrum`** snapshotted in CI. Diff is the review.
- **`cargo doc -p pdfrum --no-deps`** with
  `rustdoc::broken-intra-doc-links = deny`. Unexported types in signatures
  become a build failure the moment rustdoc cannot link them — or they
  must be re-exported (WP7).
- **A unit test that the re-export block compiles as a caller would write
  it:** every public signature’s types are named from `pdfrum::*` only.
  The existing `Send + Sync` test is the model.
- **Clippy:** `fn_params_excessive_bools = deny` on the facade crate.
  `struct_excessive_bools` stays allowed on engine option structs that
  mirror oracle flag words (already documented on
  `pdfrum_render::RenderOptions`).
- **STYLE.md §4 gains three sentences**, see §8.

---

## 6. Flags: do not add `bitflags`

Raised separately; recorded here so the next agent does not reopen it.

The `bitflags` crate is the wrong fix. DEPS.md is closed (STYLE.md §5);
adding it is a `[spec]` change it does not earn. The decision is already
written down on `Modifiers`, `FontFlags`, `AnnotFlags`, and in SPEC
§15.5. PDF flag words are also a poor fit for the crate:

| Type | What it is | `bitflags` fit |
|---|---|---|
| `Modifiers` | Nine independent input bits a host *builds* | Yes — and the 40-line version already exists (`contains`, `union`, `BitOr`) |
| `FontFlags` | ISO table 123 plus `USE_EXTERN_ATTR`. Files set reserved bits. `SYMBOLIC` and `NON_SYMBOLIC` co-occur | Poor. Unknown bits must round-trip (`from_bits_retain` at every parse) |
| `AnnotFlags` | `/F` word. The dump needs a **fixed name order**, including a bit with no printed name | Poor. The crate’s `Debug`/iter is the wrong order |
| `FieldFlags` | `/Ff` word whose bit meaning **depends on `/FT`**. Bit 26 is “file select” on text and “sort” on choice | No. A set would pretend those bits compose |
| permissions | ISO table 22, 1-indexed from bit 3, with reserved holes | No. Callers ask “may I print?”, not “is bit 3 set?” |

`bitflags::from_bits` returning `None` on reserved bits is a footgun
against damaged files. A newtype over the integer already does the right
thing: keep the word, expose named tests.

### The actual gap

The four types do not share an algebra, and permissions is not a type:

```rust
Modifiers::SHIFT | Modifiers::CONTROL           // typed constants, BitOr
FontFlags(FontFlags::SERIF | FontFlags::ITALIC) // constants are u32
FieldFlags(pub i64)                              // predicates only
AnnotFlags(pub i64)                              // a few predicates, no constants
doc.permissions(false) -> u32                    // not a type
```

### One hand-rolled pattern, used where the bits are actually a set

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FontFlags(u32); // field private

impl FontFlags {
    pub const SERIF: Self = Self(1 << 1);
    pub const ITALIC: Self = Self(1 << 6);
    // ...

    pub const fn bits(self) -> u32 { self.0 }
    pub const fn from_bits(bits: u32) -> Self { Self(bits) } // retain unknown
    pub const fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    pub const fn union(self, other: Self) -> Self { Self(self.0 | other.0) }
    pub const fn with(self, other: Self) -> Self { self.union(other) }
    pub const fn without(self, other: Self) -> Self { Self(self.0 & !other.0) }
}

impl std::ops::BitOr for FontFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { self.union(rhs) }
}
```

Apply this to `Modifiers` (already close), `FontFlags` (constants become
`Self`, drop `has(u32)`), and `AnnotFlags` (named constants plus the
predicates). `from_bits` always retains unknown bits.

**Do not** put `FieldFlags` on that algebra. The predicates (`is_read_only`,
`is_combo`, `is_multiline`) are the API. `FieldFlags::COMBO |
FieldFlags::MULTILINE` would be a lie. Keep the newtype, keep a `bits()` /
`from_bits()` pair for round-trip, keep the type-specific accessors.

**Permissions** is WP1’s struct of questions, not a third bitflags-like
newtype. `from_bits` / `bits` exist for the writer and for the form
session, which today writes `bits & 0x100`.

When `bitflags` *would* be the answer: a flags argument callers compose in
application code, with no unknown bits, no spec table, and no dump order.
`Modifiers` is the only one that looks like that, and it is already
implemented. Replacing forty lines with a crate, against a closed
manifest, is the opposite of STYLE.md §5.

---

## 7. Sequence

Do not attempt this as one milestone. Each WP is a `[spec]` commit that
leaves the goldens green.

| Order | Package | Why this position | Touches |
|---|---|---|---|
| 1 | WP7 re-exports | Purely additive. Unblocks every later signature change. | `pdfrum/src/lib.rs` |
| 2 | WP1 types (`PdfVersion`, `PageIndex`, `Permissions`, `WrongPassword`) | Newtypes first, so later method signatures can use them. | facade, parser (From impls), form session (permission bits) |
| 3 | WP2 flag algebra | Local to the flag types; `FontFlags` constants changing from `u32` to `Self` is the risky bit (many tests construct them). | font, doc, form |
| 4 | WP10 `RenderOptions` positive flags | Small, isolated, every render call site. | facade, render, tool, examples |
| 5 | WP3 collapse render/text methods | Depends on `RenderSession` being the one cache object (already true). | facade, examples, docs |
| 6 | WP4 geometry on the facade | Form session mouse methods change shape; do it just before WP5. | facade, form (From at boundary) |
| 7 | WP6 `Key` enum | Internal conversion table; `.evt` parser stays on `u16`. | form, facade, tool replay |
| 8 | WP5 form session names + `Event` | The visible break. Do after Key and geometry so the new signatures are the final ones. | facade, SPEC §15.8, doctests |
| 9 | WP9 `Option`/`Result` on mutation | Mechanical. | facade edit + form |
| 10 | WP8 `TextPage` indices | Independent; can run parallel to 6–9. Tool keeps `to_utf32le`. | text, facade, tool |
| 11 | WP12 docs | After the surface is true. Includes the JavaScript sentence and the stale facade status doc. | rustdoc, README, `docs/status/pdfrum-facade.md` |
| 12 | WP11 member-crate hygiene | Largest diff, least user-facing. Crate by crate, snapshot `cargo public-api`. | each member crate |
| 13 | WP13 gates | Last. Snapshot the surface we meant to keep. | CI, STYLE.md, clippy.toml |

WP4 and WP5 are the ones a host rewriting a viewer will feel. Everything
before them is types a compiler will point at. Everything after is
hygiene.

Parallelism: WP8 (text) does not depend on the form work. WP11 on
`pdfrum-cmap` / `pdfrum-filters` / `pdfrum-crypt` does not depend on the
facade at all.

---

## 8. STYLE.md and SPEC.md amendments

When the first WP lands, STYLE.md §4 gains:

- Facade signatures name only types this crate re-exports, plus `std`. An
  escape hatch that returns a member-crate type says so in the first
  sentence of its rustdoc.
- Packed encodings (`major * 10 + minor`, ISO permission bits, Win32 key
  codes) are internal to the crate that matches the oracle. The facade
  exposes a type.
- Boolean parameters that mean “which mode” are enums. Boolean parameters
  that *are* the value (`set_checked(name, true)`) stay.
- Flag words: hand-rolled newtype, typed constants, `BitOr` / `contains`,
  `from_bits` retains unknown bits. No `bitflags` crate. A word whose bits
  change meaning by context (`FieldFlags`) is predicates, not a set.
- The public API of every crate is the `lib.rs` `pub use` block. `pub mod`
  is for a documented sub-namespace a caller is expected to open, not for
  the implementation.

SPEC.md:

- §13: drop `render_with` / `render_session` / `render_with_on` /
  `render_session_on` from the facade sketch; `render` and `render_on`
  remain. `version` and `permissions` become the types in WP1.
  `SaveOptions.version: Option<PdfVersion>`.
- §15.5: `Key` becomes the enum; `from_virtual` / `virtual_code` are the
  wire conversion. `Modifiers` stays a hand-rolled newtype (already
  specified).
- §15.8: rewrite the method list to WP5. Record the rename
  `force_kill_focus` → `blur`. Record that the facade takes `kurbo::Point`
  and converts. Drop “one method per `FORM_*` entry … named to the Rust
  API guidelines” — that sentence is what produced `on_mouse_move`.
- §8 `RenderOptions`: `smooth_paths` / `interpolate_images`, defaults
  true.
- §9 `TextPage`: `CharIndex` / `TextIndex`; `page_text` / `to_utf32le`
  leave the library surface.

`docs/status/pdfrum-facade.md` is rewritten to the new block, in the same
commit as WP12. It still lists `Backend { Vello, TinySkia }` today.

---

## 9. What success looks like

A host who has never seen PDFium can write, with only `pdfrum` in
`Cargo.toml`:

```rust
use pdfrum::{
    Document, Event, Key, Modifiers, PageIndex, PdfVersion, Permissions,
    Point, RenderOptions, RenderSession,
};

let doc = Document::open("form.pdf")?;
assert_eq!(doc.version(), PdfVersion::PDF_1_7);
assert!(doc.permissions().fill_form);

let mut session = pdfrum::FormSession::new(&doc);
session.apply(Event::MouseDown {
    button: pdfrum::Button::Left,
    at: Point::new(120.0, 115.0),
    modifiers: Modifiers::NONE,
});
session.apply(Event::Char { ch: 'H', modifiers: Modifiers::NONE });
session.key_down(Key::A, Modifiers::CONTROL);
let _ = session.blur();

let page = doc.page(0)?;
let pixmap = page.render_on(
    &pdfrum::VelloCpuBackend::new(),
    &RenderOptions::scaled(2.0),
    &mut RenderSession::new(),
)?;
```

Every type in that snippet is in `pdfrum`’s rustdoc index. None of them is
a Win32 name, a packed `u8`, a raw permission word, or a type from a crate
the host did not add. The goldens are the same colours they are today.

That is the library PLAN.md described. The work above is what is left
between here and there.
