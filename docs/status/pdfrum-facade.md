# `pdfrum` (facade) status

**Updated:** 2026-08-29 · **State:** implemented. The user-facing API over the
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
`version`, `is_encrypted`, `permissions(owner)`,
`xref_was_rebuilt`, `bytes`, and the escape hatch `parser()`.

**Page.** `index`, `width`/`height` (rotation applied), `media_box`,
`crop_box`, `rotation() -> Rotation`, `render(&RenderOptions) -> Pixmap`,
`render_with(…, &mut BuildContext)`, `render_session(…, &mut RenderSession)`,
`text() -> TextPage`, `text_with(…)`, `text_session(…)`,
`annotations() -> Vec<Annotation>`, `links()`, and the escape hatch
`objects()`.

**Cache reuse.** `RenderSession { build: BuildContext, caches: RenderCaches }`
— a plain record of the two caches that live in different crates, added in M8
so a run over many pages shares its *glyph outlines* as well as its fonts.

**Rendering.** `RenderOptions { transform, backend, color_mode, text_aa,
no_path_smooth, no_image_smooth, background, annotations }` plus the two
constructors callers otherwise write by hand: `RenderOptions::scaled(scale)`
and `RenderOptions::fit(w, h, max_w, max_h)`. `Backend { Vello, TinySkia }`
— `vello_cpu` is the facade default, per DEPS.md's "primary rasterizer"
(note this differs from `pdfrum-tool`, which defaults to tiny-skia on purpose
because conformance wants the determinism baseline).

**Text.** `TextPage` is re-exported from `pdfrum-text` unchanged — it was
already the right shape, and wrapping it would only have hidden `all_text`,
`find`, `rects`, `index_at`, `text_in_rect`, `web_links` and `char_at` behind
delegating methods with nothing to add.

**Form.** `Form { fields(), field(name), field_count, need_appearances,
set(name, value), set_checked(name, bool), edits() }` and `Field { name,
kind, flags, index, value, stored_value, default_value, is_checked,
is_read_only, is_required, states, options, tooltip, widget_count }`.
Values are buffered on the `Form` and applied at save time, so filling a form
never needs a `&mut Document` and the document stays shareable.

**Saving.** `save(path)`, `save_with(path, &SaveOptions)`,
`save_incremental(path)`, `write_to(&mut impl Write, …)`,
`save_form(path, &Form, …)`, `write_form_to(…)`,
`import_pages(path, &source, &[u32], at)`.
`SaveOptions { update: Update::{Rewrite, Incremental}, version }`.

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
   `Page::render_session` / `Page::text_session`. Measured at 2.1x on a
   two-page document and 3.1x on a four-page one; see `docs/status/M8.md` §1,
   including the type-3 snapping caveat that keeps `render_page` the
   byte-identical baseline.

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
