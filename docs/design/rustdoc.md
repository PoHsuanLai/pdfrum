# Production rustdoc — trim pass

**Status:** WP1–WP6 landed 2026-09-03, WP5 included: every library crate is at
the §3 caps. §8 is ticked but for the one box a human has to tick. Not a `[spec]` change: no signatures move.
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

### WP6 — STYLE.md one paragraph — **landed 2026-09-03**

Appended to §6, after “Every public item has a doc comment…”. The landed
text adds the internal-reference clause and names the gate, neither of which
existed when this was drafted:

> Public rustdoc is for a caller of the crate, not for the next agent. First
> sentence, then the invariant they can get wrong, then `# Errors`, then at
> most one example per type. Design history, C++ paths and class names,
> internal milestone and work-package numbers, internal document names, and
> rejected alternatives belong in `docs/design/` and in `//` on the
> implementation — a reader on docs.rs has none of those and can follow none
> of them. Caps and the sibling rule: `docs/design/rustdoc.md`.

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

WP1–WP6 are landed, and so is every crate WP5 named: the ten that were open
(`pdfrum-page`, `pdfrum-parser`, `pdfrum-cmap`, `pdfrum-crypt`,
`pdfrum-filters`, `pdfrum-type1` and the four `pdfrum-raster-*`) landed
2026-09-03. Every library crate is now at the §3 caps.

WP1–3 are the production-ready bar for `docs.rs/pdfrum`. WP4–5 are the
same bar for anyone who clicks through to a member crate. WP6 freezes it.

> **WP1–WP4 landed 2026-09-03** as `2685dd0`, `45d4b5c`, `dc7a810`,
> `76f72a5` (Grok Build's crate-page trim, finished and landed by a Claude
> agent after Grok's balance ran out). §8 before → after: crate `//!`
> 201 → **50**; facade items over the cap 24 → **0** with no exception
> needed; doctest blocks 49 → **24** (§4 and §6 said 51 — two of the 98
> fence lines were ```` ```text ```` and not doctests); provenance hits
> 22 → **0**; `cargo test --doc -p pdfrum` 48 → 23 (24 with `script`);
> no non-doc line changed; the API snapshot unmoved. Three drifts in this
> file at the code: §6's WP2 table listed three *private* functions
> (`rgba_of`, `install_calculation_order`, `install_page_scripts`) which
> never reach the rustdoc index and so are outside the caps; and §4's rayon
> row assumed the crate page still had a `BuildContext` snippet — WP1 had
> already removed both, so `RenderSession` keeps the one example and the
> crate page links to it. One rule for WP5: an intra-doc link to a
> feature-gated re-export (`[`ScriptCascade`](crate::ScriptCascade)`)
> breaks the default-feature doc build — spell it as a bare code span, and
> run `RUSTDOCFLAGS="-D warnings" cargo doc` in **both** feature states.
> WP6 is open.

> **WP5 `pdfrum-text` landed 2026-09-03.** Crate `//!` 48 → **18** (8 lines
> of prose plus the one `no_run` open→extract fence); provenance hits in
> `///` / `//!` 33 → **0**; doctest blocks 3 → **3**; no non-doc line
> changed (the only non-comment additions are blank separators between a
> `//!` and the `//` block beneath it); the API snapshot unmoved. Reading
> recorded for §3's cap: the crate-root fence does **not** count toward the
> prose cap — the cap wording is ambiguous, and §3 requires the root to
> carry one snippet, so a root of ≤ 12 prose lines plus a single fence is
> in bounds. The first pass at this WP failed review: it **deleted** the
> module essays instead of moving them, so a doc-only diff that started
> with zero items over the cap still removed ~470 lines of design
> reasoning. Fourteen sites lost ≥ 10 doc lines and only 12 `//` lines were
> added. Corrected here: every essay is back verbatim as a `//` block on the
> body it explains (`pipeline` `SOFT_HYPHEN` / `UNMAPPABLE`, `unicode`
> `normalize_space` and the RLE table, `dedup`'s five-object rule, and the
> module essays of `links`, `object`, `pipeline`, `line`, `unicode`,
> `find`, `index`, `orientation`), which is exactly what §4's Cut table
> prescribes. Three contract sentences also had to be **re-derived from the
> code** because the summarised versions stated the contract wrong:
> `TextPage::slice` widens its bounds onto real text (`index.rs`
> `text_index_at_or_after` / `text_index_end`) rather than "skipping
> stripped characters" inside the range; `TextPage::rects` skips generated
> and sub-`0.01` boxes and pushes a box **unconditionally at the end**
> (`select.rs`), so an all-skipped run yields one all-zero rect rather than
> being "clamped"; and `CharType::Hyphen` forces its `unicode` to `0x0002`
> while `search_text` carries `U+00AD` (`pipeline.rs:889-894`) — the one
> place the two outputs provably disagree. The lesson for the remaining
> WP5 crates: a doc-only pass moves essays, it does not summarise them, and
> a summary of a contract must be checked against the implementation before
> it ships.

> **WP5 `pdfrum-object` landed 2026-09-03.** Crate `//!` 39 → **27** (13
> prose plus the one open-a-dict fence); module `//!` over the 12-line cap
> 3 → **0** (`dict` 34 → 11, `number` 19 → 10, `resolve` 14 → 10); indexed
> items over the cap 1 → **0** (`Object::clone_direct` 44 → 12 prose);
> provenance hits in `///` / `//!` 12 → **0**; doctest blocks 26 → **26**;
> 195 doc lines removed against 107 added and **70 `//` lines added**, so
> every essay moved rather than vanishing. No non-doc line changed; the API
> snapshot unmoved. The one-hop resolve story is now stated once, on the
> `Resolve` trait, with `Resolved::as_direct`, the crate root and `dict.rs`
> linking to it. Contracts re-derived from the code before being
> summarised: `clone_direct`'s cut edge **disappears** — `clone_flattened`
> `filter_map`s the key or element away rather than writing null
> (`object.rs`), and an unresolvable reference takes the same path, so the
> two are indistinguishable; a flattened stream keeps `s.data.clone()`,
> i.e. its raw still-encoded bytes with `/Filter` intact. `Dict::name` is
> `self.raw(key)?.as_name()` — no resolution at all — so "the type check
> happens before any resolution, so `/Type 5 0 R` never names a type" is
> exact. `Dict::text` is the one resolving accessor that does *not* route
> through `as_direct`, but `Object::to_text` returns `""` for `Ref`, so the
> one-hop rule still holds observationally and the wording on `Resolve`
> ("reads as missing") covers both spellings.

> **WP5 `pdfrum-render` landed 2026-09-03.** Reading recorded first: the
> caps apply to the crate's *rustdoc index*, which for this crate is the
> `pub use` block plus the four public backend-seam modules (`blend`,
> `glyph`, `pixmap`, `scanline`) and `walkprofile` under its feature —
> `walk.rs`, `options.rs`, `color.rs`, `ctx.rs`, `text.rs`, `path.rs` and
> the rest are private modules whose `//!` never reaches docs.rs, and a
> `pub(crate)` item inside a public module (`glyph::LcdBitmap`,
> `scanline::coverage_to_alpha`) is not indexed either. Measured against
> that set: indexed items over the cap 7 → **0**; indexed module `//!` over
> the 12-line cap 5 → **0** (`glyph` 64 → 12, `walkprofile` 61 → 12,
> `scanline` 60 → 12, `blend` 20 → 12, `pixmap` 14 → 11); crate `//!` 58
> lines, 18 prose, unchanged and inside the 50 cap; doctest blocks 1 → **1**;
> 1023 doc lines removed against 477 added and **196 `//` lines added**. No
> non-doc line changed; the API snapshot unmoved. `RenderOptions`'
> `struct_excessive_bools` expectation and its "these are the oracle's flag
> names one for one" reason are untouched — they are an attribute, not a
> doc line, so editing them would have broken the non-doc gate.
>
> **The provenance sweep was widened** at the reviewer's instruction, and
> the wider pattern is the one the queued workspace-wide sweep and CI gate
> should inherit. Beyond `Added 20…` / `cpdf_` / `CPDF_` / `FPDF_` /
> `pdfium_test` / `.cpp:`, rustdoc must not name our internal phases or
> internal documents either — on docs.rs nobody knows what `M12`, `WP7`,
> `§A.11`, `docs/status/M12.md`, `PLAN.md §M15` or `SPEC.md §8` mean. The
> grep, over `///` / `//!` lines only:
>
> ```
> \bM[0-9]{1,2}[a-z]?\b|\bWP[0-9]+\b|§[A-Z]\.[0-9]|docs/(status|design|upstream)|PLAN\.md|SPEC\.md|STYLE\.md|DEPS\.md
> ```
>
> On main that matched 37 lines in `pdfrum-render` and 3 in `pdfrum-object`;
> both are **0** now, as are the C++-path hits (172 → 0 in render, 9 → 0 in
> object, `#[cfg(test)]` excluded). The rule applied was **trim, do not
> relocate by default**: for each citing sentence, ask whether it is useful
> at all. Two kinds were kept — an invariant a caller can get wrong, which
> stays in rustdoc rewritten without the pointer; and a measured fact that
> changes how the *code* must be read, which becomes a short `//` stating
> the fact rather than "see M12 §1.6". Everything else — which pass decided
> what, which corpus document it was measured on, which ruling superseded
> which — was **deleted**, because git and `docs/` already hold it. The
> split: of 218 citing `///` lines, ~40 survive as `//` measured facts (the
> four-stage small-glyph bitmap pipeline, the cell rasterizer's cover/area
> representation, the opaque-Normal fast-path derivation, the
> straight-vs-premultiplied quantisation, the backdrop double-count), ~65
> were rewritten in place as rustdoc with the pointer dropped, and the
> remainder was retired.
>
> Contracts re-derived from the code before being summarised.
> `needs_alpha_background`: the match arm lists exactly Screen through
> Luminosity — every mode *above* `Multiply` — and recurses into a
> `PageObject::Form`, so "any `/ExtGState` on the page, **or in a form it
> draws**, names a blend mode above `Multiply`" is exact where "declares a
> `/Group`" would be wrong. `takes_bitmap_path` returns `true` at
> `|a| + |b| <= 50`, so the doc says "the threshold is `<= 50`; above it the
> outline is filled", not the C++'s inverted `> 50` spelling.
> `composite_solid` is the entry point for a *straight* colour and
> `composite_premultiplied` for an already-premultiplied one; both docs now
> name the other so a backend author cannot pick the quantising path by
> accident.
>
> Two traps for the next crate. `cargo doc --features profiling` was
> **already failing on main**: `walkprofile::phase_start` linked
> `[`Started::end`]`, a private item, and `rustdoc::private_intra_doc_links`
> is `-D warnings`. Same class as §7's feature-gate note above — a doc link
> to something not in the index breaks the build — so the fix is the same:
> a bare code span. It is fixed in this commit because the gate had to be
> green. And a mechanical rewrapper that strips a trailing parenthetical
> **must not touch a Markdown list**: flattening a bullet's continuation
> indent from two spaces to none silently turns the list into a paragraph in
> the rendered page. Strip inside list blocks line by line, or re-indent
> afterwards and diff the result.

> **WP5 `pdfrum-form` landed 2026-09-03.** The crate carrying the most
> `cpdf_`/`cffl_`/`cpwl_` citations in the workspace. Crate `//!` 21 (no
> snippet) → **26**, of which 20 are prose and six a `text` fence, inside §3's
> 25-line exception; indexed items over the 20-line cap 15 → **0**; indexed
> module `//!` over the 12-line cap 11 → **0**; provenance hits in `///`/`//!`
> 207 → **0**; internal phase and document references 21 → **0**; doctest
> blocks 4 → **4**. 1370 doc lines removed against 693 added and **22 `//`
> lines added**; five essays moved rather than being cut. No non-doc line
> changed; the API snapshot unmoved.
>
> The rule the reviewer set was applied as stated: **lean toward deleting**.
> Most citing sentences were diary — which pass, which fixture, which ruling —
> and git and `docs/` hold that already. What survived is an invariant a caller
> can get wrong, rewritten without the pointer, or a measured fact that changes
> how the code reads, as a short `//`. Contracts re-derived from the code
> before being summarised: `Keystroke::applied` — an out-of-range
> `selection_start` yields an **empty prefix** while an out-of-range
> `selection_end` yields an empty suffix, the two halves genuinely disagreeing;
> `CommitOutcome::formats` — a `None` from a commit that *ran* erases while a
> `None` from one that never ran means nothing; `is_text_overflow` — the check
> is made **before** the character, and overflow is a comb field's property
> alone; `page::selected` — the interaction reader takes `/I` first where the
> appearance reader takes `/V`, agreeing except on `/I` present with `/V`
> absent. The feature-gate trap fired again in its private-item form:
> `model.rs` linked `[`super::field`]`, a private module, so
> `cargo doc --features script` failed where the default build passed. A bare
> code span, as §7 already recorded.

> **WP5 `pdfrum-script` landed 2026-09-03.** Nearly clean already: provenance
> hits 7 → **0**, internal references 0 → 0, indexed items over the cap 0 → 0,
> indexed module `//!` over the cap 2 → **0** (`af` 20 → 13, `error` 20 → 12),
> crate `//!` 41 (29 prose plus two fences) unchanged and inside the 50 cap,
> doctest blocks 2 → 2. All seven citations were pointers, so each states what
> it pointed at instead.

> **WP5's remaining ten crates landed 2026-09-03**, closing WP5. None has a
> Cargo feature, so each was measured and gated in its one feature state, and
> the facade was rebuilt in both of its. Measured from the crate's own rustdoc
> JSON rather than by grep, which is what settled two counting questions the
> earlier notes left open: an item's cap is checked against the *index*, so a
> private helper carrying a 40-line essay is out of scope however long it is,
> and §3's "excluding `# Errors` and one example" means the `# Errors` section
> is subtracted from an item's prose the way a fence already was.
>
> | Crate | Root prose | Items over cap | Doctests | doc − / doc + / `//` + |
> |---|---|---|---|---|
> | `pdfrum-page` | 29 → **15** (25 cap) | 8 → **0** | 5 → 4 | 143 / 49 / 49 |
> | `pdfrum-parser` | 27 → **9** | 2 → **0** | 9 → 9 | 48 / 17 / 17 |
> | `pdfrum-cmap` | 42 → **12** | 1 → **0** | 13 → 13 | 53 / 15 / 12 |
> | `pdfrum-crypt` | 43 → **12** | 2 → **0** | 9 → 9 | 67 / 19 / 25 |
> | `pdfrum-filters` | 30 → **12** | 3 → **0** | 15 → 15 | 51 / 23 / 12 |
> | `pdfrum-type1` | 44 → **12** | 2 → **0** | 4 → 4 | 73 / 21 / 22 |
> | `pdfrum-raster-agg` | 55 → **11** | 0 → 0 | 1 → 1 | 52 / 8 / 30 |
> | `pdfrum-raster-tinyskia` | 22 → **12** | 0 → 0 | 1 → 1 | 16 / 6 / 9 |
> | `pdfrum-raster-vello-cpu` | 32 → **12** | 0 → 0 | 1 → 1 | 29 / 9 / 17 |
> | `pdfrum-raster-vello` | 55 → **12** | 2 → **0** | 1 → 1 | 110 / 31 / 44 |
>
> No non-doc line changed in any of the ten; the API snapshot unmoved;
> `cargo test --doc` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` green
> per crate and for `pdfrum` in both feature states. `pdfrum-raster-vello` is
> `publish = false` and was included under §6's rule anyway: its root was 55
> against a cap of 12 and two indexed items were over, which is grossly over on
> both halves.
>
> Contracts re-derived from the code and corrected before shipping.
> `Pixels::sample_bytes` returns `[0, 0, 0]` for a pixel whose index does not
> compute — the doc had never said what an out-of-range read does.
> `decode_image` searches `form_resources` *before* `page_resources`; the old
> text said only that inline images get it, which is the interpreter's
> convention, not this function's rule. `RequestedSize::for_device` falls back
> to `Self::Full` on a non-finite or sub-one axis, which "no reduction" left
> ambiguous. `Document::lazy_diagnostics` clones rather than draining, and a
> poisoned lock yields empty rather than panicking. `inherit_from` returns the
> *child unchanged* past the depth limit — the parent is dropped, so "refused
> with a diagnostic" left a caller unable to tell which CMap survives.
> `font_file` puts the **whole** blob in `/Length1` when there is no `eexec` at
> all, a third case beside the PFB and PFA ones the doc described.
> `decoder_list`'s `/DecodeParms` rule is one rule, not three cases: only the
> two shapes that match `/Filter` ever yield parameters and every other
> combination lands on the same empty dictionary. `VelloCpuDevice::draw_image`
> clamps its alpha to `0.0..=1.0` before the strict `< 1.0` that takes the
> opacity layer.
>
> Two traps of §7's kind fired again. `request_adapter` said "see the module
> documentation for why it leaks", but `adapter` is a **private** module whose
> `//!` never reaches docs.rs — the same class as the feature-gated and
> private-item link traps, in its third spelling: a pointer into the index from
> outside it. And three crate roots carried a `*Renamed 2026-09-02, was …*`
> line, which is exactly WP4's `Added YYYY-MM-DD` in another tense.
>
> One finding the brief did not anticipate: **the C++-provenance half of WP4
> was never swept in these crates.** The phase-and-document half had been done,
> but `cpdf_`/`CPDF_`/`FPDF_`/`pdfium_test`/`.cpp:` in `///` and `//!` was a
> facade-only sweep. On main
> these ten carried 96 such lines (`pdfrum-page` 77, `pdfrum-crypt` 12,
> `pdfrum-parser` 3, `pdfrum-raster-agg` 2, `pdfrum-cmap` 1, `pdfrum-type1` 1).
> This pass cleared them only where they sat inside an item it was already
> rewriting, taking the total to **83** (`pdfrum-page` 66, `pdfrum-crypt` 11,
> `pdfrum-parser` 3, `pdfrum-raster-agg` 2, `pdfrum-cmap` 1); every survivor
> is on an item that is inside its cap. That is a queue item of its own.

> **A CI gate for this existed between 2026-09-03 and 2026-09-03** as
> `scripts/check-no-internal-refs.nu`, and was **removed the same day** — see
> the note at the end of this section. What it learned while it ran is worth
> keeping, because it is what a future reader would otherwise rediscover by
> grep. Two shapes are easy to get wrong. `SPEC\.md` **under-matches**: the
> spelling the tree carries is the unsuffixed `(SPEC §15.8)`. And a bare
> `§[A-Z]\.[0-9]` **over-matches** `ISO/IEC 15444-1 §A.4.1`, which is an annex
> section of a published standard and exactly the kind of citation a reader
> *can* follow. Running the pattern workspace-wide rather than per crate found
> 19 lines across seven crates that two earlier passes had missed.

> **The C++-provenance sweep landed 2026-09-03, workspace-wide.** WP4's
> `cpdf_`/`CPDF_`/`FPDF_`/`pdfium_test`/`.cpp:` half had only ever run on the
> facade. The note above left it as a queue item and estimated 83 lines across
> the ten crates WP5 had just finished; measured with the wider pattern and
> over every crate rather than ten, it was **416**. Two things the estimate
> could not see: `CFX_` and `CJS_` were never in WP4's pattern at all, and four
> crates — `pdfrum-doc`, `pdfrum-tool`, `pdfrum-edit`, `pdfrum-font`, 302 lines
> between them — had been in no rustdoc work package of any kind. The 83 is
> reproducible and was right for what it measured: WP4's five alternatives over
> those ten crates is 84 today.
>
> Citing lines before and after, then the shape of the diff: doc lines
> removed, doc lines added, and `//` lines added — the last being the essays
> and derivations that **moved** rather than going.
>
> | Crate | Before | After | doc − | doc + | `//` + |
> |---|---|---|---|---|---|
> | `pdfrum-doc` | 105 | **0** | 452 | 317 | 47 |
> | `pdfrum-tool` | 76 | **0** | 218 | 165 | 63 |
> | `pdfrum-page` | 70 | **0** | 288 | 171 | 76 |
> | `pdfrum-edit` | 61 | **0** | 218 | 143 | 89 |
> | `pdfrum-font` | 60 | **0** | 167 | 132 | 55 |
> | `pdfrum-crypt` | 11 | **0** | 48 | 31 | 26 |
> | `pdfrum-form` | 7 | **0** | 23 | 18 | 0 |
> | `pdfrum-text` | 6 | **0** | 9 | 6 | 0 |
> | `pdfrum-script` | 6 | **0** | 8 | 14 | 0 |
> | `pdfrum-raster-agg` | 4 | **0** | 24 | 20 | 0 |
> | `pdfrum-render` | 3 | **0** | 11 | 7 | 4 |
> | `pdfrum-parser` | 3 | **0** | 8 | 5 | 7 |
> | `pdfrum-common` | 3 | **0** | 12 | 8 | 3 |
> | `pdfrum-cmap` | 1 | **0** | 3 | 1 | 1 |
>
> The four crates with no `//` added are the ones whose citations were all
> pure appendix — `pdfrum-text`'s six geometry helpers each stated the
> behaviour completely and then named the `CFX_` class that also has it — and
> `pdfrum-script`, where every citation was a *title* naming the C++ symbol
> for a function the caller knows by its JavaScript name (`util.printx`,
> `util.scand`), so the docs got longer rather than shorter.
>
> The rule was §4's, and the lean was to delete: a citation that only says
> where a sentence came from goes, because the sentence already carries the
> fact; a sentence whose *content* is an invariant a caller can get wrong keeps
> the content and loses the pointer; and a measured fact that changes how the
> **code** must be read becomes a `//` on the body, where the file and line may
> stay in full. No non-doc line changed in any crate and the API snapshot is
> unmoved.
>
> **Every `[oracle-bug]` site keeps both citations.** That is the one place
> nothing may be dropped, because the record of where PDFium is wrong is the
> whole reason we diverge from it. The `///` states what *we* do and why the
> specification permits it; a `//` beside it carries PDFium's defect and
> pdf.js's agreement, file and line intact — `cpdf_security_handler.cpp:305`
> and `crypto.js:1116-1120` on the crypt-filter pair, `cpdf_docrenderdata.cpp`
> and `evaluator.js:944-959` on the transfer-function array,
> `cpdfsdk_widget.cpp:1029` and `annotation.js` on widget rotation,
> `cpvt_section.cpp:84`'s `<=`-for-`==` typo and `annotation.js:3107-3134` on
> punctuation classification, `fpdf_edittext.cpp:488-491` and
> `evaluator.js:4103-4116` on `/CIDToGIDMap`.
>
> **`pdfrum-tool` needed no exception, which was the surprise.** It is a CLI
> whose stated job is to mirror `pdfium_test`'s flags, so naming that tool
> looked like its own contract rather than provenance, and the first design
> here was a per-line opt-out marker for it. In the event no line wanted one:
> the crate already writes "the oracle" in almost every sentence, and
> substituting that reads better, not worse — "the oracle cannot save a
> document at all" says everything about `--save` that the binary's name did.
> What was left after the substitution was file-and-line provenance, and it
> split the ordinary way. `events.rs` is the one that changes shape, because it
> documents a file format the harness writes: the verb grammar is the contract
> and the eight `— event.cc:NN-NN` suffixes were decoration on it.
>
> Three contracts were found **stated wrong** and corrected against the code,
> which is the argument for reading rather than pattern-matching.
> `pdfrum-edit`'s `content/text.rs` said the text state the emitter loses "is
> the C++'s behavior and matching it is the requirement" — its own module
> three headings away had already established the opposite, so the loss is a
> limit of the emitter and now says so. `write/reach.rs` named a
> `seen_ref_objects` filter that does not exist; the function is
> `count_reference` and the set is `seen_sources`, and the old name was the
> C++'s. And `pdfrum-render`'s `to_straight_bgra` explained its `opaque` flag
> by naming the oracle's bitmap format instead of saying what a caller gets.
>
> One thing left open, and it is a live regression risk:
> `crates/pdfrum-font/src/encoding/tables.rs` is `@generated`, and
> `scripts/extract-font-tables.py` still emits the `/// \`kFoo\` (path.cpp).`
> form the sweep replaced. Re-running the extractor reverts fifteen lines.
> Nothing in `scripts/ci.nu` regenerates the file, so this is a trap rather
> than a break; the extractor needs the same edit, and that is a queue item.
>
> **The gate that was going to hold this is not being written.** A first draft
> widened `scripts/check-no-internal-refs.nu` with the C++ pattern as a second
> class, and the shape of what it grew is the argument against it: nine
> alternatives, an ISO exemption, a per-line opt-out for the identifiers that
> are legitimately ours, and a fourth planted-line control to prove the opt-out
> both fires and does not over-fire. Each of those exists because the one
> before it was too blunt, and none of them helps anyone write a better doc
> comment. The rule is one sentence in STYLE.md §6 and a person can follow it;
> a citation that comes back comes back in review, where a human can tell an
> `[oracle-bug]` record that must keep its citation from a diary entry that
> must not — a distinction no pattern could make. The existing script went with
> it, in the commit before these.

Do not combine WP1 with WP5. The crate page is a writing task; the inner
crates are a grind. Mixing them produces an unreviewable diff.

---

## 8. Definition of done

Verified 2026-09-03 unless noted.

- [x] `pdfrum` crate `//!` ≤ 50 lines. **50.**
- [x] No facade item doc over the §3 cap except the listed exceptions, and
      those exceptions still have a single example. **0 over**, counting prose
      lines and excluding the fence, which is what §3's "excluding example"
      means; the four named exceptions each keep one.
- [x] Sibling methods have no second example and no diary.
- [x] `rg -n 'Added 20|considered and rejected|\.cpp:' crates/pdfrum/src --glob '*.rs'`
      is empty in `///` / `//!` lines ( `//` may still match). **0**, and the
      wider pattern is clear across every crate rather than the facade alone.
- [x] `cargo test --doc -p pdfrum` passes. **25 passed.**
- [x] `cargo doc -p pdfrum --no-deps` builds with
      `rustdoc::broken-intra-doc-links` clean. **Under
      `RUSTDOCFLAGS="-D warnings"`, in both feature states** — which is the
      stronger claim, and the one the feature-gate and private-item traps of
      §7 make necessary.
- [x] Facade doctest block count is in the 15–25 range (51 today). **26** —
      one over the stated range, and left there: the range was written when
      the count was 51 and every surviving fence is the canonical site for its
      type under §4's table. Trimming one to reach 25 would delete a worked
      example to satisfy a number.
- [x] STYLE.md §6 has the paragraph in WP6.
- [ ] A human opened `target/doc/pdfrum/index.html` and the `FormSession`
      / `Page` / `Document` pages and could see the summary above the fold.
      **The one box no gate can tick.**

- [x] Inner crates at the cap. **All of them**, as of 2026-09-03:
      `pdfrum-text`, `pdfrum-object`, `pdfrum-render`, `pdfrum-form` and
      `pdfrum-script` first, then `pdfrum-page`, `pdfrum-parser`,
      `pdfrum-cmap`, `pdfrum-crypt`, `pdfrum-filters`, `pdfrum-type1` and the
      four rasterizer crates.

- [x] The C++-provenance sweep, workspace-wide. **416 → 0** across fourteen
      crates, on `cpdf_`, `CPDF_`, `FPDF_`, `CFX_`, `CJS_`, `cpdfsdk_`,
      `pdfium_test`, `.cpp:` and `.h:`. See §7's note for the per-crate table,
      the three contracts it corrected, and why no CI gate holds it.

Not in done: comment ratio as a number — we are not optimizing 0.28.

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
