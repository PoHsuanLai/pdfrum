# `pdfrum` (facade) status

**Updated:** 2026-09-03 · **State:** implemented. The user-facing API over the
whole stack — open, page, render, text, annotations, form, save, import —
with runnable doctests on real fixtures, four examples, and integration tests
over every facade path. The workspace `README.md` is written.

Contract: SPEC.md §13 (facade = composition and ergonomics, **no logic**);
style: STYLE.md, especially §4 (this crate *is* the API surface).

## The API surface

Everything is re-exported from one `lib.rs` block a reviewer reads in one
screen (STYLE.md §4). Options are plain config structs with `Default` and
struct-update syntax; there are no builder ladders and no generics in the
facade's own types.

**Opening.** `Document::open(path)`, `open_with_password(path, &[u8])`,
`open_with(path, &OpenOptions)`, `from_bytes(Arc<[u8]>)`,
`from_bytes_with(…)`. `OpenOptions { password, limits }`.

**Document-level.** `page_count`, `page(i) -> Result<Page>`, `pages()`
(lazy iterator, skipping pages that will not load), `page_label(i)`,
`outline() -> Outline`, `metadata() -> Metadata`, `xmp_metadata()`,
`form() -> Option<Form>`, `attachments() -> Vec<Attachment>`,
`diagnostics()` (load-time) and `all_diagnostics()` (running total),
`version`, `is_encrypted`, `permissions`, `owner_permissions`,
`xref_was_rebuilt`, `bytes`, and the escape hatch `parser()`.

**Page.** `index`, `width`/`height` (rotation applied), `media_box`,
`crop_box`, `rotation() -> Rotation`, `render(&RenderOptions) -> Pixmap`,
`render_on(&B, &RenderOptions, &mut RenderSession)`, `text() -> TextPage`,
`text_on(&mut RenderSession)`, `annotations() -> Vec<Annotation>`, `links()`,
`edit() -> PageEdit`, and the escape hatch `objects()`.

*Collapsed 2026-09-03 (§A.10 step 10, WP3).* This list used to hold nine
methods: `render` / `render_on` / `render_with` / `render_with_on` /
`render_session` / `render_session_on` and `text` / `text_with` /
`text_session` — a grid of "which rasterizer x which caches" with one real
body underneath. `RenderSession` already carries both caches and its `build`
field *is* a `BuildContext`, so every `_with` form was `_session` with one
half of a session and every backend-less form was the default backend. Four
methods now: `render` and `text` for the one-page case, `render_on` and
`text_on` for a run. `FormSession::with_context` still takes a `BuildContext`
directly, because a session that will never render still needs fonts.

**Editing** (M11). `Page::edit()` hands back an owned `PageEdit` over that
page's object graph; `Document::save_pages(path, &[PageEdit], &SaveOptions)`
and `write_pages_to` turn the changes into replacement objects on the way out.
Nothing is mutated before the save — the same shape form filling has, and for
the same reason: `Document` is a `Sync` shared reader, so editing a page must
not need `&mut` on it.

`PageEdit` offers `len`/`is_empty`/`objects`/`object_mut`/`push`/`insert`/
`remove`/`show`/`hide`/`is_visible`/`transform`/`is_modified`, plus
`font_of`/`image_of` to name a resource an existing object already uses, and
`graph`/`graph_mut` as the escape hatch onto `pdfrum-page`. `insert` / `show` /
`hide` / `transform` return `Result<(), IndexOutOfRange>` (WP9, 2026-09-03);
`remove` already returned `Option<PageObject>`. **Taking
`object_mut` is the edit**: the object is marked dirty on the way out, so a
caller that only wants to look uses `objects`.

*Corrected 2026-09-03 (WP9).* `Form::set` / `Form::set_checked` return
`Result<(), UnknownField>` — an unknown name is an error, not a silent ignore.
`FormSession::replace_selection` and `set_index_selected` keep `bool` because
the bool is the answer. `Outline`'s `IntoIterator` walks in place through
`OutlineIter`; it no longer `collect`s into a `Vec` to satisfy the trait.

Three plain config structs build objects to add — `PathBuilder` (with a `rect`
constructor), `TextBuilder`, `ImageBuilder` — each with `build() -> PageObject`,
and `PageObject` is re-exported because it is the currency of the whole
surface.

One asymmetry worth knowing: `ImageBuilder` *places* an `/XObject` the
document already holds rather than encoding new pixels, because that is what an
image page object is. Its `build()` fills in a one-by-one placeholder bitmap
that the regenerated stream never looks at, so rendering the edited graph
before saving shows the placeholder — render the *saved* file to see the
image.

**Saving.** `save`, `save_with`, `save_incremental`, `write_to`, `save_form`,
`write_form_to`, `save_pages`, `write_pages_to`, `import_pages`.
`SaveOptions { update, version, remove_security }` — `remove_security`
defaults **false** since M11, which is when the M10 ruling reached this crate:
`write_edit` had been forcing it true, so an encrypted document saved
decrypted through every facade path. It now saves encrypted under the handler
its password opened, and an edited page's regenerated streams go through that
same cipher because they are written the same way as the rest of the body.

**Cache reuse.** `RenderSession { build: BuildContext, caches: RenderCaches }`
— a plain record of the two caches that live in different crates, added in M8
so a run over many pages shares its *glyph outlines* as well as its fonts.

**Rendering.** `RenderOptions { transform, color_mode, text_aa, smooth_paths,
interpolate_images, background, annotations }` plus the two constructors
callers otherwise write by hand: `RenderOptions::scaled(scale)` and
`RenderOptions::fit(w, h, max_w, max_h)`.

*Corrected 2026-09-02 (WP10).* The two smoothing flags used to be
`no_path_smooth` / `no_image_smooth`, PDFium's inverted flag-word names with
`false` defaults. They are now positive and default to `true`, which is what a
viewer does. The engine's `pdfrum_render::RenderOptions` is a **different
type** and keeps all seven of `CPDF_RenderOptions::Options`' names, so the port
stays reviewable against `cpdf_renderoptions.h`; the two are joined only at
`RenderOptions::to_inner`, which is where the `!` lives (§A.8, §B).

*Corrected 2026-09-02.* This paragraph used to list a `backend` field and a
`Backend { Vello, TinySkia }` enum. **Both were withdrawn**: naming three
rasterizers in one enum meant `cargo add pdfrum` compiled all three whatever
the caller used, so the backend became an *argument* — `Page::render_on` takes
an `impl RasterBackend` — and the facade carries
one rasterizer, `vello_cpu`, per DEPS.md's "primary rasterizer". The other two
are dev-dependencies of this crate and a normal dependency of any caller who
names one. `pdfrum-tool` still defaults to tiny-skia on purpose, because
conformance wants the determinism baseline.

**Text.** `TextPage` is re-exported from `pdfrum-text` unchanged — wrapping
it would only have hidden `Display`, `slice`, `find`, `rects`, `index_at`,
`text_in_rect`, `web_links` and `char` behind delegating methods with nothing
to add. *Updated 2026-09-02 by WP8*, which changed those signatures in the
crate rather than at the facade: `page_text(start, count)` became
`slice(range)`, `rects(start, count)` became `rects(range)`, `all_text()`
became `Display`, `char_at` became `char`, and the two index spaces became
`CharIndex` and `TextIndex` — re-exported here alongside `IndexMap`, the table
that converts between them. The verbatim re-export is why: with no facade
layer to translate in, the crate's shape *is* the facade's shape.

**Form.** `Form { fields(), field(name), field_count, need_appearances,
set(name, value), set_checked(name, bool), edits() }` and `Field { name,
kind, flags, index, value, stored_value, default_value, is_checked,
is_read_only, is_required, states, options, tooltip, widget_count }`.
Values are buffered on the `Form` and applied at save time, so filling a form
never needs a `&mut Document` and the document stays shareable.

**Saving.** `save(path)`, `save_with(path, &SaveOptions)`,
`save_incremental(path)`, `write_to(&mut impl Write, …)`,
`save_form(path, &Form, …)`, `write_form_to(…)`,
`import_pages(path, &source, pages, at)`, where `pages` and `at` both take
anything that converts into a `PageIndex`.
`SaveOptions { update: Update::{Rewrite, Incremental}, version }`.

**Form interaction** (M14). `FormSession` is the facade's *own* type over
`pdfrum-form`'s state record: `new`, `with_config`, `with_context`,
`with_config_in`, `apply(Event)` and the thin wrappers over it
(`mouse_move`, `mouse_down`, `mouse_up`, `double_click`, `mouse_wheel`,
`focus_at`, `key_down`, `character`), `blur`,
`focused_annot`/`focused_text`, `focus_for_page`, `popup_for_page`,
`scroll_view`, `choose`, `close_popup`. There is **no** escape hatch: `inner()`
is gone (§WP5), because a power user wants a session of their own and
`pdfrum-form` builds one.

**JavaScript, behind the `script` feature** (M15 step 1, WP12). The facade has
one cargo feature and it is off: `script = ["pdfrum-form/script"]`. Off, the
crate compiles no engine — `scripts/check-no-boa.nu` asserts that a default
`pdfrum` tree contains no `boa_*` crate, and that this feature does reach one,
so neither half can pass vacuously. On, it re-exports `ScriptCascade`,
`ScriptConfig`, `TranscriptLine`, `ScriptBuildError`, `ScriptFailure`,
`ScriptStop` and `FieldActions`, and adds two entry points:

- `FormSession::with_cascade(doc, impl Cascade + 'static)` — **unconditional**,
  because `Cascade` and `NoScripts` are. This is the seam a host puts its own
  commit rules on, and the facade re-exports `Cascade`, `FieldRef`,
  `FieldWrites`, `Keystroke`, `KeystrokeOutcome` and `NoScripts` with no
  feature gate so a caller can write one from `pdfrum::*` alone.
- `FormSession::with_scripts(doc, &ScriptConfig)` — feature-gated, and the one
  to use for scripting. It builds a `ScriptCascade` *and* installs what the
  cascade cannot read for itself: each page's `/AA /K`, `/AA /V`, `/AA /C` and
  `/AA /F` sources, each field's fully qualified name and stored value, and the
  form's `/AcroForm /CO` calculation order. The install is **per page, as a
  page is first read**, because a `FieldRef`'s index is a page-local field id
  allocated while that page's `/Annots` are walked — it does not exist before
  the page does. `FormSession::scripts()` hands back the cascade so the host
  can read the transcript (`app.alert`, `Doc.submitForm`, `app.launchURL` come
  back as values, never as I/O) and the stops.

What a script reaches is M15 step 2's surface: the `AF*` library, `util`,
`app.alert`, `event`, and the `Doc`/`Field` object model — `getField`,
`getNthFieldName`, `numFields`, `numPages`, the metadata properties,
`getAnnot(s)`, `gotoNamedDest`, `resetForm`, `calculateNow`, and `Field`'s 52
properties and 26 methods. **23 of the oracle's 47 JavaScript fixtures
byte-exact**, up from 11. `docs/status/M15.md` §2 has the per-name table and
the per-fixture accounting.

*Both WP12 defects are closed* (2026-09-03, `9f3046e`). A format script's
output now reaches the regenerated appearance — `FormSession::formatted`
carries it, keyed by field, because `ResetFieldAppearance` gives every control
of the field the same string and this crate hands appearances back rather than
painting them. And `FieldRef::index` is now unambiguously a **document-wide**
`/Fields` position: `WidgetInfo::field_index` carries it beside the page-local
`FieldId`, and `PageForm::field_of_index` converts back, so a calculation on a
multi-page form writes the field `/CO` named rather than the one that happened
to sit at the same position on the page.

**Errors.** One `Error` enum whose variants name **domains** rather than
crates — `Open`, `Read`, `Render`, `Doc`, `Save`, `Text`, `Io` — each
wrapping the member crate's own error so `source()` still reaches the detail.
Damage a document survives is not represented here at all: it is a
`Diagnostics` entry on the value that came back (STYLE.md §3).

## Parallelism

`Document` is `Send + Sync` with an internally synchronized object store, and
every output type (`Pixmap`, `TextPage`, `Page`, `Annotation`, …) is
`Send + Sync`. `rayon` is a dependency of this crate alone, per DEPS.md.
Both patterns are documented with runnable doctests and pinned by tests:
plain `par_iter().map(render)`, and `map_init(BuildContext::new, …)` for the
long-document case where per-thread font and image caches pay.

`BuildContext` is `Send` but reached through `&mut`, so it is per-worker
rather than shared — asserted in `lib.rs`'s `send_sync` test, because a
member crate that grew an `Rc` would otherwise break every rayon caller at
*their* compile time rather than ours.

## Tests

| | |
|---|---:|
| Integration tests (`tests/facade.rs`, real fixtures) | **59** |
| Doctests (every public item, runnable) | **35** |
| `pdfrum-doc` unit tests added for the form model | **20** |
| Examples (compiled by the gate, all run clean) | 4 |

Fixtures are three PDFs copied unmodified from the oracle checkout's
`testing/resources` into `tests/fixtures/`, with `PROVENANCE.md` recording
origin, commit, licence and what each exercises: `hello_world.pdf` (render,
text, a stream with no `/Length`), `text_form.pdf` (AcroForm, widget
annotation), `bookmarks.pdf` (outline two levels deep, two pages).

Gate: `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`,
`cargo nextest run` (2640 tests), `cargo test --doc` (149 doctests),
`cargo deny check`, pure-Rust tree check — `scripts/ci.sh` green.

`cargo doc --no-deps` builds, but is **not** warning-free and was wrongly
recorded as such here: it emits 34 rustdoc warnings across the workspace
(redundant explicit link targets, and public docs linking private items) in
member crates, none of them in the facade. `cargo doc` is not part of
`scripts/ci.sh`, which is why the discrepancy went unnoticed. Cleaning them up
and adding the check to the gate is unclaimed work.

## Two things moved *down* the stack

STYLE.md forbids logic in the facade, and twice the honest reading was that a
member crate's API was incomplete rather than that the facade needed code.

**1. The AcroForm data model did not exist** (`pdfrum-doc`, new
`form::field`). SPEC.md §10 specifies `Form { fields: Vec<Field> }` and
`Field { kind, value, flags, kids }` with `set_value()`, but M6 shipped only
`field_attr` and `full_name` — the two attribute helpers the `--annot` dump
needed — and nothing that enumerates, classifies, reads or writes a field.
The conformance harness never noticed because no oracle dump enumerates
fields. Implementing this in the facade would have put the field-tree walk,
the terminal-field rule, the `/Ff` bit decoding and the value-writing logic in
the one crate that is supposed to contain none, so it went where SPEC.md §10
already said it lives: `pdfrum_doc::form::{Form, Field, FieldKind,
FieldFlags, Widget, FieldValues, FieldEdit, apply}`, 20 unit tests.

**2. Annotation-appearance overlay was stranded in the binary**
(`pdfrum-tool` → `pdfrum-doc::annot_render`). Painting a page's annotation
appearances into its object graph is what makes `FPDF_ANNOT` renders match,
and it encodes two genuinely non-obvious behaviours — the two upstream passes
disagree about the `kInvisible` flag, and an appearance is *fitted* into its
rectangle rather than translated. It lived in `pdfrum-tool`, a binary, so the
facade could not call it and would have had to duplicate it. Moved verbatim
(imports retargeted) into `pdfrum-doc`, which already owns annotations and
appearance generation and already depends on `pdfrum-page`; the tool now
calls the shared function. Its tests moved with it.

Neither is a SPEC change: the first implements a contract that was already
written and unimplemented, the second relocates code without altering it.

## Candidate M8 polish items

Things a member crate's API made awkward. None blocks the facade; each is a
sharp edge a caller would eventually hit.

1. ~~**`render_page` allocates its own `RenderCaches` per call.**~~ **Done
   (M8).** `pdfrum_render::render_page_with_caches` takes the caches,
   `render_page` delegates to it with a fresh set, and `target_size` is now
   public. The facade pairs the two caches in `RenderSession`, reached through
   `Page::render_on` / `Page::text_on` (WP3, 2026-09-03; they were
   `render_session` / `text_session` when this was written). Measured at 2.1x
   on a two-page document and 3.1x on a four-page one; see
   `docs/status/M8.md` §1, including the type-3 snapping caveat that keeps
   `render_page` the byte-identical baseline.

2. **Two `Resolve` calling conventions coexist.** `pdfrum-object` and
   `pdfrum-parser` take `r: &impl Resolve`; `pdfrum-doc` takes
   `<R: Resolve>(…, r: &R, …)`. Both work, but the inconsistency is visible
   in every facade call site and would be worth settling on one before 1.0.

3. **`Limits` and `&mut Diagnostics` are threaded through nearly every
   `pdfrum-doc` read**, including ones that cannot diagnose anything
   (`Bookmark::title`, `Link::rect`). The facade constructs a throwaway
   `Diagnostics` at a dozen call sites purely to satisfy signatures, and
   discards it. Worth auditing which reads actually need the sink.

4. ~~**The facade cannot surface a document-wide diagnostics view.**~~ **Done
   (M8).** `Document::all_diagnostics()` returns a running total over three
   sinks: the load-time snapshot, the object store's lazy repairs (newly
   reachable via `pdfrum_parser::Document::lazy_diagnostics`), and this
   crate's own reads, which now fold their sinks into a `Mutex<Diagnostics>`
   on the document instead of dropping them. `diagnostics()` is unchanged and
   still answers the load-time question. No member-crate signature changed.
   See `docs/status/M8.md` §2.

5. **`Field::value` needs the resolver *and* an optional edit buffer**
   (`value(Option<&FieldValues>, &R)`). The facade hides this by cloning the
   buffer into each `Field` handle it hands out, which is correct but copies
   per field. A borrowed view would be tidier.

6. **`--annot`'s widget text body remains the standing waiver** (SPEC §10
   E1). It is visible from the facade as: filling a text field stores the
   value and regenerates the widget's *chrome*, but draws no text, so a
   viewer that does not lay out field text itself shows the field empty. This
   is documented on `Form` and pinned by a test that states the split rather
   than wishing it away — but it is the one place where "fill a form and save
   it" does not fully deliver what a user expects.

7. **A formatting script's output is computed and then dropped.** Found while
   wiring WP12's `script` feature, and it is a `pdfrum-form` defect rather
   than a facade one, so it is recorded here rather than fixed here.

   `commit::run` returns a `CommitOutcome` carrying `display: Option<String>`
   — the string `/AA /F` produced, which by design does *not* change the
   stored value — and `route.rs`'s `commit_field` reads `outcome.reverted` and
   `outcome.writes` and **nothing else**. `grep -rn '\.display' crates/` finds
   no reader outside `commit.rs`'s own tests. So the format hook runs, the
   engine answers, and the answer goes nowhere: `UpdateKind` has no variant
   that can carry it, and a caller applying the returned updates sees the raw
   value.

   Measured on `testing/resources/pixel/bug_113910.pdf`, whose one field's
   `/AA /F` is `AFNumber_Format(0, 1, 0, 0, "", false)`: typing `1234` and
   committing regenerates an appearance whose content stream is `(1234) Tj`.
   That is PLAN.md §M15's own worked example of what a JavaScript-off renderer
   gets wrong — "a field with `AFNumber_Format` shows raw text where Acrobat
   shows `$1,234.00`" — reproduced with the engine *on*. The engine is not the
   missing piece; the wire from `CommitOutcome::display` to `UpdateKind` is.

   It is why `tests/form_scripts.rs` proves the seam through the **transcript**
   rather than through a formatted field value: the transcript is the one
   script-produced value that currently reaches a caller. Closing this needs a
   `pdfrum-form` change (a new `UpdateKind` variant, or `display` on
   `AppearanceUpdate`) and belongs to M15 step 2, not to WP12.

8. **`FieldRef::index` means two different things.** Also found wiring WP12,
   also a `pdfrum-form` question rather than a facade one.

   `pdfrum_form::page::read` allocates a `FieldId` **per page**, in first-seen
   order as that page's `/Annots` are walked (`field_id_of`), and
   `route::field_ref` hands that number to a cascade as `FieldRef::index`. But
   `pdfrum_doc::form::Form::calculation_order` — the `/CO` walk M15 step 1
   added — answers positions in the document's whole `Form::fields` list, and
   `route::commit_field` spends a calculation's writes as `FieldId(index)`
   under the comment *"A calculation names fields by their index in the form's
   list, which is what `FieldId` is."* **It is not.** The two spaces coincide
   for a single-page form whose widgets appear in `/Fields` order — which is
   every `/CO`-bearing fixture in the oracle's corpus, so nothing red is
   visible today — and diverge for a form spread over pages, or one whose
   `/Annots` order differs from its `/Fields` order.

   `FormSession::with_scripts` inherits the mismatch rather than introducing
   it, and says so where it installs the order. Fixing it in one place and not
   the other would make them disagree in a *new* way, so it belongs to M15
   step 2, where the `Doc`/`Field` object model has to settle what a script's
   field identity is in the first place.
