# Production rustdoc — trim pass

**Status:** scoped, not started. Not a `[spec]` change: no signatures move.
**Date:** 2026-09-02. **Updated 2026-09-03:** names and counts re-measured
after the idiomatic-API pass landed (`docs/design/idiomatic-api.md`), which
renamed or removed most of the methods the first draft cited.
**Audience:** whoever edits `///` / `//!` in library crates. STYLE.md §6 stays
in force; this file says what that section means for a caller reading
`docs.rs/pdfrum`.

The docs are written for agents and for the next person to match PDFium.
They are too long for a host who ran `cargo add pdfrum` and opened the crate
page. This pass makes rustdoc production-ready **without** deleting the
design briefs, without turning off `missing_docs`, and without touching
implementation `//` comments.

---

## 0. Verdict

Trim, do not strip.

- **Crate page and ~12 facade types** are essays. That is what rustdoc
  users see. Cut those.
- **Every public item still has a doc.** First sentence stays. `# Errors`
  stays. At most one example per *type* or primary method.
- **`//` next to code is out of scope.** It is not cargo docs (8.9k lines,
  0.08 per code line).
- **Design briefs and `docs/status/` keep the diary.** Dates, “we considered
  const generics,” `foo.cpp:123` — those already have a home. They do not
  belong in `///`.

Measured 2026-09-03, library crates, `#[cfg(test)]` modules excluded:

| Kind | Lines | In `cargo doc`? |
|---|---|---|
| `//!` module docs | 7.3k | yes |
| `///` item docs | 26.1k | yes |
| `//` implementation | 6.0k | no |

Rustdoc/code is **0.41** workspace-wide (81.7k code lines). The facade is
**1.24** (2.5k rustdoc lines over 2.0k of code). That asymmetry is the
problem: callers read `pdfrum`, not `pdfrum-page`. The ratios rose since the
first draft (0.28 / 1.11) because the API pass deleted code and the docs
stayed.

---

## 1. What success looks like

A host opens `docs.rs/pdfrum` and can, in one screen, see what the crate
does, copy one example, and click through to `Document`, `Page`, `Form`,
`FormSession`. A type page is a summary, a short invariant if one exists,
`# Errors` if it returns `Result`, and at most one `# Examples` block.
Sibling methods (`render` / `render_on`, `text` / `text_on`) do not each
repeat the crop-box story.

`cargo test --doc -p pdfrum` still passes. `missing_docs = warn` still
warns. No public item becomes undocumented.

---

## 2. Non-goals

- No API changes. This is not `docs/design/idiomatic-api.md`.
- Do not delete doctests that are the *only* worked example of a type.
  Collapse duplicates; keep one.
- Do not set `#![allow(missing_docs)]`. Do not weaken workspace lints.
- Do not rewrite design briefs (`docs/design/pdfrum-*.md`) or STYLE.md
  beyond the one paragraph in §8.
- Do not touch `//` implementation comments, `#[cfg(test)]` module docs,
  or `pdfrum-tool`.
- Do not hide public items with `#[doc(hidden)]` just to shorten rustdoc.
  Hidden is for sibling-crate plumbing that was never a host API
  (`debug_runs`, the fuzz-only link checkers). The hygiene pass that decided
  what is hidden (idiomatic-api.md WP11, landed 2026-09-03) is done; this
  pass inherits its answers and adds none.

---

## 3. The bar — std-shaped rustdoc

Match the *shape* of `std` / `serde` / `tokio` item docs, not their brevity
at all costs. PDF has invariants std does not (`y`-up page space, damage
vs `Err`, one-level resolve). Those stay. Diaries go.

**Every public item**

1. First sentence: what it is, in PDF terms. ISO 32000 §x.y welcome.
2. Optional second paragraph: the one thing a caller gets wrong if we stay
   silent (coordinate space, “not an error”, buffered until save).
3. `# Errors` if the signature is `Result`.
4. `# Examples` only if this is the canonical place for that example
   (see §4).
5. Nothing else.

**Crate / module `//!`**

- Crate root: what it is, one snippet, where to start (links), the two
  scope notes that prevent a wrong purchase (no XFA; JS off by default).
  Target **≤ 50 lines** including the snippet.
- Other modules: one to four sentences. A module is not a design brief.

**Hard caps (review blockers if exceeded without a listed exception)**

| Surface | Cap | Exception |
|---|---|---|
| Crate `//!` (`pdfrum`) | 50 lines | none |
| Other module `//!` | 12 lines | `pdfrum-form` crate root, `pdfrum-page` crate root — 25 |
| Type / trait / enum | 20 lines excluding example | `FormSession`, `Document`, `Page`, `PageEdit` — 30 including a *single* example |
| Method / function | 12 lines excluding `# Errors` and example | the *primary* method of a type may have one example; siblings may not |
| Example on a type or method | one fenced block | crate root may have two: basic loop, and rayon *or* form, not both plus a third |

A “line” is a rustdoc source line (`/// …`), not a rendered paragraph.

---

## 4. Keep / cut

### Keep

- The first sentence.
- Caller-visible invariants:
  - damage is [`Diagnostics`], not [`Error`]
  - `all_diagnostics` is a running total; read it after the work
  - mouse coordinates are page space, `y`-up, crop-box origin
  - `Form::set` is buffered until save
  - `Page::render` applies crop and `/Rotate` for you
  - `Resolve` is one hop; a ref-to-ref is absence
- `# Errors` naming the facade variant (`Error::Render`, `Error::Open`).
- One example that a copy-paste compiles:
  - crate root: open → render → text
  - `FormSession`: click, type, blur (this is the one that earns a long
    example; it lives on the **type**, not on `mouse_down` or `apply`)
  - `Page::render`: default + scaled
  - `Document::from_bytes`: the `Arc<[u8]>` contract
- ISO section citations (`ISO 32000-1 §12.7`).
- Links to the sibling method instead of restating it.

### Cut (move, do not invent a new home if one exists)

| Cut from rustdoc | Goes to |
|---|---|
| “Added 2026-09-02, replacing `Backend`…” | git / `docs/status/pdfrum-facade.md` |
| “Const generics were considered and rejected…” | `docs/design/` of that crate |
| `cpdf_*.cpp:309`, `pdfium_test paints`, `FFLDraw` | `docs/design/pdfrum-*.md` (already there) |
| STYLE.md section numbers in a public item | STYLE.md |
| The rayon doctest on the crate page and the one on `RenderSession` | keep **one** of the two (the `par_iter` + one-session-per-worker form); the crate page's `BuildContext` variant is already gone |
| Essay on every `render_*` / `with_config*` twin | one sentence + `see Page::render` / `see FormSession::new` |
| “This crate composes, it does not compute” plus the member-crate tour | README “The facade”; crate page keeps one sentence and the escape-hatch links |
| M15 fixture counts, `this.getField` not built, PLAN.md §M15 | `docs/status/M15.md`; crate page: “`script` is off by default; the `Doc`/`Field` object model is incomplete” |
| Why a `Mutex` rather than a `RefCell` on `Document` | a `//` on the field — it is an implementation constraint, not a host contract |

### The sibling rule

If method B is “method A with an extra argument”:

```rust
/// [`Page::render`] on a rasterizer you name.
///
/// # Errors
///
/// As [`Page::render`].
pub fn render_on<B: RasterBackend>(...) -> Result<Pixmap>
```

That is the whole doc. No second example that asserts `(200, 200)` again. No
history of the `Backend` enum. The example lives on `render` (or on
`RenderOptions`), once.

Apply to: `render` / `render_on`; `text` / `text_on`; `FormSession::new` /
`with_config` / `with_context` / `with_config_in` / `with_cascade` /
`with_scripts`; `mouse_move` / `mouse_down` / `mouse_up` / `double_click` /
`mouse_wheel` / `focus_at` / `key_down` / `character` (each is
`apply(Event::…)` — one sentence and a link to [`FormSession::apply`]);
`save` / `save_with` / `write_to` / `save_incremental`; `save_form` /
`write_form_to`; `Document::save_pages` / `write_pages_to` and
`DocEdit::save_pages` / `write_pages_to` / `save` / `write_to` (the
`Document` forms delegate to `DocEdit`; say so in one sentence and link).

### Examples: one canonical site

The facade currently has **51** doctest blocks. After the pass, target
**~20**, all still compiling.

| Example | Lives on | Deleted from |
|---|---|---|
| open + render + text | crate `//!` | — |
| parallel render with `RenderSession` | crate `//!` *or* `RenderSession` (pick one) | the other, and `BuildContext` rayon snippet |
| `from_bytes(Arc<[u8]>)` | `Document::from_bytes` | — |
| `page.render` default and 2× | `Page::render` | `render_on`, `RenderOptions` if it only repeats size |
| click / type / undo / blur | `FormSession` (the type) | `with_config_in`, `with_context`, `apply`, the `mouse_*` wrappers |
| `Form::set` then `save_form` | `Document::save_form` | `Form::set` keeps a three-line snippet *or* a link |
| `Page::edit` + `remove` | `Page::edit` | `save_pages` can stay as “and then save” with no second walkthrough |
| embed a font, write text in it, save | `DocEdit::embed_font` | `standard_font`, `TextBuilder` (link) |

Doctests that exist only to re-assert `page_count() == 1` on
`hello_world.pdf` go. The unit tests already own that.

---

## 5. Before / after

### Crate root (`pdfrum/src/lib.rs`)

Today: 201 lines. Target: ≤ 50.

Keep: one-paragraph identity, the open/render/text snippet, “Where to start”
bullets, JS-off-by-default + `script` feature in five lines, damage in four
lines, one parallel-render snippet **or** a link to `RenderSession`.
Cut: the PDFium-oracle manifesto (README already has it), the three-way
rayon tutorial, the member-crate tour beyond one sentence, M15 fixture
arithmetic.

### `Page::render_on`

Today: 60 lines — a catalogue of every rasterizer crate and which one
`render` picks, the session-and-caches story restated from `RenderSession`,
`# Errors`, and an example that repeats `render`'s size assertion.

After:

```text
[`Page::render`] on a rasterizer you name.

# Errors

As [`Page::render`].
```

### `Document::all_diagnostics`

Today: 35 lines on lazy parse, three sinks, STYLE.md, a snippet that only
checks `len() >= at_open`.

After: what it is (load + every lazy read since); that
`diagnostics()` is the load-time snapshot; read it after the work. No
snippet, or a two-line `assert!(doc.all_diagnostics().len() >= n)` on
`diagnostics()` instead — one of the two methods, not both.

### `FormSession` the type

Today: 117 lines including the full tutorial and the `apply` / wrapper
story that step 8 of the API pass added.

After: three short sections — events in / values out; page-space
coordinates; focus vs appearance — then **one** example (click, type,
blur). Cap 30 lines plus that example. The substitution-options warning
stays on `with_context` as two sentences, not on the type.

---

## 6. Work packages

Each WP is one reviewable diff. Doctests green at every step
(`cargo test --doc -p pdfrum`). No signature changes, so no SPEC.md
edit unless a doc link bitrots because we renamed a heading.

### WP1 — Crate page (`pdfrum/src/lib.rs`)

Rewrite the `//!` to the §3 cap. Move manifesto / member-crate tour /
M15 counts to README if they are not already there (they mostly are).
Keep two snippets max.

**Files:** `crates/pdfrum/src/lib.rs`, `crates/pdfrum/README.md` only if a
paragraph is missing there.

### WP2 — Facade types over the cap

Rewrite, in this order (current line counts of the worst offenders):

| Item | File | Now |
|---|---|---|
| `FormSession` | `form_session.rs` | 117 |
| `Page::render_on` | `page.rs` | 60 |
| `FormSession::with_scripts` | `form_session.rs` | 52 |
| `PageEdit` | `edit.rs` | 52 |
| `RenderSession` | `session.rs` | 41 |
| `Page::render` | `page.rs` | 38 |
| `RenderOptions` | `render.rs` | 36 |
| `Document::save_pages` / `save_form` / `import_pages` / `save` | `save.rs` | 35 / 30 / 29 / 21 |
| `Document::all_diagnostics` / `open_with_password` | `document.rs` | 35 / 25 |
| the `Argb` and `LoadError` re-export comments | `lib.rs` | 35 / 23 |
| `FormSession::with_config_in` / `with_cascade` / `with_context` / `popup_for_page` / `apply` | `form_session.rs` | 34 / 33 / 32 / 21 / 21 |
| `DocEdit::embed_font` | `save.rs` | 29 |
| `Form` / `Form::set` | `form.rs` | 28 / 26 |
| `Outline` | `outline.rs` | 24 |

(Measured 2026-09-03 as consecutive `///` lines above each `pub` item;
everything over the 20-line cap is listed.)

Apply the sibling rule while touching those files so `render_on` and
friends shrink in the same diff as `render`.

**Files:** `form_session.rs`, `edit.rs`, `session.rs`, `render.rs`,
`document.rs`, `page.rs`, `save.rs`, `form.rs`, `outline.rs`. That is
almost the whole facade; do it as two PRs if needed (session+form, then
page+save+edit).

### WP3 — Facade examples collapse

After WP2, grep ` ``` ` in `crates/pdfrum/src`. Delete fences that only
repeat `(200, 200)` or `page_count() == 1`. Target ~20 fences. Run
`cargo test --doc -p pdfrum` and `cargo test --doc --workspace` if a
re-export’s example moved.

### WP4 — Provenance sweep (facade, then inner *public* items)

Mechanical. Delete from `///` / `//!`:

- `Added YYYY-MM-DD`
- `cpdf_`, `CPDF_`, `FPDF_`, `cffl_`, `cpwl_`, `pdfium_test`, `.cpp:`
- `STYLE.md`, `SPEC.md`, `PLAN.md` as citations (ISO is fine)
- “considered and rejected”, “withdrawn”, “replacing `Backend`”

If the sentence is the *only* explanation of a caller-visible invariant,
rewrite it without the citation (`FastGetDirect` → “one hop; a
reference to a reference is absent”).

Do **not** strip those strings from `//` or from `docs/design/`.

### WP5 — Inner crates, public API only

Same caps, applied to items that appear in that crate’s rustdoc index
(`pub use` in its `lib.rs`). Do not walk every `pub fn` in
`pdfrum-render::text` on this pass unless it is re-exported.

Priority order (host-reachable first):

1. `pdfrum-text` (`TextPage`, `FindOptions`, `CharBox`, `WebLink`) — re-exported by the facade
2. `pdfrum-form` crate root + `FormSession` (inner) + `Event` + `Response` + `PopupView`
3. `pdfrum-render` (`Pixmap`, `ColorMode`, `TextAa`, `RasterBackend`)
4. `pdfrum-object` (`Resolve`, `Object`, `Dict` accessors) — the one-level story stays, shortened
5. The rest (`pdfrum-page`, `pdfrum-doc`, `pdfrum-parser`, …) only if a type page is > 30 lines

Known offenders from the first draft (`pdfrum-render/src/text.rs`
`place_glyphs` at 65, `pdfrum-form/src/edit/ops.rs` scroll-to-caret at 56,
`pdfrum-doc` widget/annot-render essays at 47–52) were mostly **privatised
by the curation pass** since; a private item's essay becomes `//` on the
function body, and only what a crate still re-exports gets a 12-line `///`.
Re-measure per crate with the same script as WP2 before starting.

### WP6 — STYLE.md one paragraph

Append to §6, after “Every public item has a doc comment…”:

> Public rustdoc is for a caller of the crate, not for the next agent.
> First sentence, then the invariant they can get wrong, then `# Errors`,
> then at most one example per type. Design history, C++ paths, and
> rejected alternatives belong in `docs/design/` and in `//` on the
> implementation. Caps and the sibling rule: `docs/design/rustdoc.md`.

No other STYLE.md edits.

---

## 7. Sequence and size

| Order | WP | Why this position | Approx. files |
|---|---|---|---|
| 1 | WP1 crate page | Highest leverage; what docs.rs opens | 1–2 |
| 2 | WP2 facade types | The 12 essays | ~8 |
| 3 | WP3 examples | Depends on WP2 so we do not delete an example and then need it | facade src |
| 4 | WP4 provenance | Mechanical, can run parallel to WP3 once WP2 has rewritten the essays | facade, then inner `lib.rs` re-exports |
| 5 | WP5 inner crates | After the facade is the template | per crate |
| 6 | WP6 STYLE.md | Last, so it describes the pass that landed | 1 |

WP1–3 are the production-ready bar for `docs.rs/pdfrum`. WP4–5 are the
same bar for anyone who clicks through to a member crate. WP6 freezes it.

Do not combine WP1 with WP5. The crate page is a writing task; the inner
crates are a grind. Mixing them produces an unreviewable diff.

---

## 8. Definition of done

- [ ] `pdfrum` crate `//!` ≤ 50 lines.
- [ ] No facade item doc over the §3 cap except the listed exceptions, and
      those exceptions still have a single example.
- [ ] Sibling methods have no second example and no diary.
- [ ] `rg -n 'Added 20|considered and rejected|\.cpp:' crates/pdfrum/src --glob '*.rs'`
      is empty in `///` / `//!` lines ( `//` may still match).
- [ ] `cargo test --doc -p pdfrum` passes.
- [ ] `cargo doc -p pdfrum --no-deps` builds with
      `rustdoc::broken-intra-doc-links` clean.
- [ ] Facade doctest block count is in the 15–25 range (51 today).
- [ ] STYLE.md §6 has the paragraph in WP6.
- [ ] A human opened `target/doc/pdfrum/index.html` and the `FormSession`
      / `Page` / `Document` pages and could see the summary above the fold.

Not in done: inner crates at the cap (WP5 may lag). Not in done: comment
ratio as a number — we are not optimizing 0.28.

---

## 9. Reviewer checklist (per file)

1. First sentence still true if you delete everything after it?
2. Would this paragraph still be written if the author had never seen the
   C++? (STYLE.md §7, applied to rustdoc.)
3. Is there an example, and is this the canonical site for it?
4. Does a sibling method restate this? If yes, replace with a link.
5. Any date, `.cpp`, “rejected”, or STYLE/SPEC/PLAN citation? Cut or
   rewrite.
6. Cap.

---

## 10. Relationship to other docs

- **STYLE.md §6** is the policy. This file is the cap and the queue.
- **`docs/design/idiomatic-api.md`** is signatures. Do not mix a rustdoc
  trim into an API rename; rustdoc PRs should be doc-only so
  `cargo test --doc` is the whole gate.
- **README** remains the manifesto (oracle, pure Rust, crate map). The
  crate page stops competing with it.
