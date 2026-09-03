# Production rustdoc — trim pass

**Status:** WP1–WP6 landed 2026-09-03; §8 is ticked but for the one box a
human has to tick, and WP5's remaining crates are open (see §8's "not in
done"). Not a `[spec]` change: no signatures move.
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
> most one example per type. Design history, C++ paths, internal milestone and
> work-package numbers, and rejected alternatives belong in `docs/design/` and
> in `//` on the implementation; `scripts/check-no-internal-refs.nu` is the
> gate. Caps and the sibling rule: `docs/design/rustdoc.md`.

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

WP1–WP6 are landed. WP5's remaining crates (`pdfrum-page`, `pdfrum-parser`,
`pdfrum-cmap`, `pdfrum-crypt`, `pdfrum-filters`, `pdfrum-type1`,
`pdfrum-raster-*`) are not in the definition of done and stay open; the CI gate
holds the provenance half of the bar across all of them today.

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
> Two traps for the next crate. `cargo doc --features walk-profile` was
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

> **The CI gate landed 2026-09-03** as `scripts/check-no-internal-refs.nu`,
> wired into `scripts/ci.nu`. It scans every `///` and `//!` under
> `crates/*/src` for the §7 pattern, with two corrections found by running it
> over the whole workspace rather than one crate. `SPEC\.md` **under-matched**:
> the spelling the tree carries is the unsuffixed `(SPEC §15.8)`, so the
> alternative is now `\b(SPEC|PLAN|STYLE|DEPS)(\.md)?\b`, word-bounded to keep
> `SPECIAL` and `PLANE` out. `§[A-Z]\.[0-9]` **over-matched**: `ISO/IEC
> 15444-1 §A.4.1` is an annex section of a published standard a reader can
> follow, so a line matching `(ISO|RFC)…§X.N` is spared — for that alternative
> only, so a line carrying both a standard's annex and a milestone still fails.
> The widening found **19 lines** across seven crates that passes A and B-1
> missed, all swept in the gate's own commit.
>
> The non-vacuity control earned its place on the first run. The draft used
> `-- 'crates/*/src'`, and under git's default pathspec matching a bare `*`
> does not cross a `/` — so that spec matches **no file at all** and the scan
> reported a clean tree by having looked at nothing. Only the planted line
> caught it; `:(glob)crates/*/src/**` is the fix, recorded in the script. The
> gate carries a third assertion besides, that the ISO exemption spares an
> annex citation and still fails a milestone on the line beside it.
> `\bM[0-9]{1,2}` has no false positive today and could acquire one — a matrix
> element, an OpenType tag — so it is kept with the risk documented and a
> per-line opt-out reserved for the day a real one appears.

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
      is empty in `///` / `//!` lines ( `//` may still match). **0**, and
      `scripts/check-no-internal-refs.nu` now holds the wider pattern across
      every crate rather than the facade alone.
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

Not in done: inner crates at the cap (WP5 may lag) — `pdfrum-text`,
`pdfrum-object`, `pdfrum-render`, `pdfrum-form` and `pdfrum-script` are at it;
`pdfrum-page`, `pdfrum-parser`, `pdfrum-cmap`, `pdfrum-crypt`,
`pdfrum-filters`, `pdfrum-type1` and the rasterizer crates are not, and the CI
gate holds only the provenance half of the bar for them. Not in done: comment
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
