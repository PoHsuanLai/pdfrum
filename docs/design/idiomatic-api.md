# Making pdfrum a fully idiomatic Rust library

**Status:** proposal. Not a `[spec]` change yet — each work package below
becomes one when it lands, per SPEC.md §0.
**Date:** 2026-09-02. **Amended 2026-09-02 — see §A, which supersedes §1's
governing principle and §7's sequence, revises §3's audience table, and
withdraws one sentence of §5's preamble. §2, §4 and §6 stand.**
**Scope:** ~~the public API of `pdfrum` and, secondarily, the curated public
surface of the member crates it composes.~~ **Widened by §A:** the public API
of `pdfrum` *and of every published member crate*, each on its own terms.
Behaviour against the oracle does not move.

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

## A. Amendment, 2026-09-02 — every crate is idiomatic, not just the facade

**This section supersedes §1's governing principle and §7's ordering, revises
§3's table, and withdraws one sentence of §5's preamble. §2, §4 and §6 stand
unchanged; §A.8 confirms each line by line.** Nothing below is deleted from
§§1–9 — the superseded text is quoted where it is replaced, per the house rule
that a claim a later ruling overturns is withdrawn in place.

Reading order, if you only want the ruling: §A.2 is the new principle, §A.3 is
the distinction it depends on, §A.10 is the new sequence. Everything else is
the evidence for those three.

### A.1 What §1 said, and why it is withdrawn

§1 governed with:

> **Idiomatic at the facade. Oracle-faithful below.**
>
> The C++ is an oracle for *behaviour*, not a vocabulary for *types*. A page
> index that is `u32` in the file, an `/Ff` word whose bits change meaning by
> field type, a Win32 `VK_PRIOR`, a form-layout midpoint computed as
> `(top + bottom) / 2` in `f32` — all of that stays in the crate that has to
> match a golden. **The facade translates.**

The first two sentences are right and are kept. The third — *the facade
translates* — is withdrawn. It smuggles in a premise the rest of this
workspace does not hold: that the facade is the product and a member crate is
an implementation detail that may stay ugly because something downstream will
launder it.

Three things in the tree contradict that premise.

1. **Modularity is a stated selling point, and every member crate but two is
   `publish = true`.** `pdfrum-raster-vello` and `pdfrum-script` are
   `publish = false`; the other seventeen ship to crates.io as public APIs
   with their own rustdoc, their own semver, and their own `cargo add`
   audience. A caller who wants text extraction and nothing else adds
   `pdfrum-text` — that is the whole point of the split, and PLAN.md §3
   describes each crate as a thing that *replaces* a PDFium module, not as a
   private sub-unit of a facade.

2. **The facade's own crate documentation already promises the opposite of
   §1.** `crates/pdfrum/src/lib.rs` says, under *"This crate composes, it does
   not compute"*:

   > Everything here is a thin, ergonomic surface over a stack of member
   > crates, **each of which is a public API in its own right and none of
   > which this crate hides.** […] `pdfrum-parser`, `pdfrum-page`,
   > `pdfrum-render`, `pdfrum-text`, `pdfrum-doc` and `pdfrum-edit` are all
   > **there to be used directly.**

   A crate cannot be "a public API in its own right […] there to be used
   directly" and also be the place the C-style vocabulary is allowed to live.
   §1 and the shipped crate docs cannot both be true. The crate docs are the
   promise made to users, so §1 is the one that gives way.

3. **There is often no facade layer to translate in.** This is the decisive
   one, and it is measurable rather than rhetorical. The facade does not wrap
   the offending types; it re-exports them verbatim:

   | Facade line | What it re-exports | Translation performed |
   |---|---|---|
   | `crates/pdfrum/src/lib.rs:258` | `pdfrum_text::{CharBox, FindOptions, TextPage, WebLink}` | none |
   | `crates/pdfrum/src/annotation.rs:3` | `pdfrum_doc::{AnnotFlags, Subtype}` | none |
   | `crates/pdfrum/src/form.rs:8` | `pdfrum_doc::form::{FieldFlags, FieldKind}` | none |
   | `crates/pdfrum/src/lib.rs:202` | `pdfrum_form::tab::Rect as FormRect` | a rename |

   So WP8's `page_text(start, count)` and `to_utf32le()` are not "one layer
   down" from an idiomatic facade — they *are* the facade, because
   `pdfrum::TextPage` **is** `pdfrum_text::TextPage`. §1 promised a
   translating layer that, for these types, does not exist and was never
   going to be written: writing it would mean the facade wraps `TextPage` in
   a new type, which nobody has proposed and §2 would call a regression.

**The ruling (user, 2026-09-02):** *"since we want to make the crates
modular, should we, not only make the facade idiomatic, but make the crate
itself be? then the ugly c style can just be tested in the crate."*

### A.2 The replacement principle

> **Every published crate is idiomatic in its types and its naming. Oracle
> fidelity lives in function bodies and private representation — never in a
> public signature.**

The `(top + bottom) / 2.0` midpoint stays, computed inside `pdfrum-form`,
strict comparisons and all. What goes away is `pdfrum-form` *advertising* an
`f32` rect and an `f32` point in its own public API. The user's phrase for
this — "the ugly c style can just be tested in the crate" — is the operative
half: oracle-shaped behaviour keeps its tests at the crate level, where the
golden lives and where the `#![allow(clippy::float_cmp)]` that licenses it is
already written down. The crate's *surface* stops advertising it.

### A.3 The distinction that keeps the goldens green

This amendment is only survivable because two things that look alike are not.
Getting this line wrong in either direction breaks something: put behaviour on
the vocabulary side and goldens move; put vocabulary on the behaviour side and
nothing is ever cleaned up.

**C-style *vocabulary* — transliteration with no reason to exist at any
level. Eliminate it in the crate; do not translate it at the facade.**

| Item | Where it lives today | Verified at |
|---|---|---|
| `page_text(start, count)`, `rects(start, count: Option<usize>)` | `pdfrum-text` | `crates/pdfrum-text/src/lib.rs:214,269`; `src/select.rs:20` |
| `permissions(owner: bool) -> u32` | `pdfrum-crypt`, `pdfrum-parser` | `crates/pdfrum-crypt/src/lib.rs:552`; `crates/pdfrum-parser/src/doc.rs:797` |
| `version() -> u8`, packed `major × 10 + minor` | `pdfrum-parser` | `crates/pdfrum-parser/src/doc.rs:695` — the doc comment spells the encoding out |
| `Key(pub u16)`, `Modifiers(pub u32)` | `pdfrum-form` | `crates/pdfrum-form/src/event.rs:63,119` |
| `FontFlags(pub u32)`, `AnnotFlags(pub i64)`, `FieldFlags(pub i64)` — public inner fields | `pdfrum-font`, `pdfrum-doc` | `crates/pdfrum-font/src/ids.rs:87`; `crates/pdfrum-doc/src/annot/mod.rs:169`; `crates/pdfrum-doc/src/form/field.rs:102` |
| `to_utf32le()`, the `annot_dump` module | `pdfrum-text`, `pdfrum-doc` | `crates/pdfrum-text/src/lib.rs:318`; `crates/pdfrum-doc/src/lib.rs:28` |
| `pub fn is_float_zero(f32)` — **defined twice, publicly, in two crates** | `pdfrum-doc`, `pdfrum-text` | `crates/pdfrum-doc/src/geom.rs:45`; `crates/pdfrum-text/src/charinfo.rs:168` |
| `pub fn peniko_mix`, `pub const LCD_FIR5`, `LCD_PADDING_26_6` | `pdfrum-render` | `crates/pdfrum-render/src/device.rs:251`; `src/glyph.rs:69,77` — `26_6` is a FreeType fixed-point encoding spelled into the identifier |
| `pub const NO_CONTENT_STREAM: i32 = -1` — a `-1` sentinel **promoted into the curated re-export block** | `pdfrum-page` | `crates/pdfrum-page/src/mutate.rs:44`, re-exported at `src/lib.rs:80`. `Option<usize>` is the shape. |
| `as_c_int`, `as_c_float`, `real_as_c_int` — C in the name, in the curated block | `pdfrum-object` | `crates/pdfrum-object/src/lib.rs:62`. The *behaviour* is oracle-required (`FX_Number::GetSigned`); the *name* is not. |
| `PDFIUM_TEST_CLOCK_SECS`, `PDFIUM_TEST_TZ_OFFSET_SECS`, `PDFIUM_TEST_FX_LOCALTIME_OFFSET_SECS` | `pdfrum-form` | `crates/pdfrum-form/src/script/mod.rs:72,80,111` — the **only** `PDFIUM`/`FX_` strings in any public identifier in the workspace, and there are three of them. |

None of these is load-bearing for a golden. Every one of them is a shape a
Rust author who had never opened `fpdftext/` or `fpdf_formfill.h` would not
have written, and every one of them is in a **member** crate, which is why §1's
"the facade translates" never reached them.

**C-style *behaviour* — this cannot move. It is not ugly code awaiting
cleanup; it is the oracle contract.**

| Behaviour | Where | Why it stays |
|---|---|---|
| `(top + bottom) / 2.0`, sum-then-halve, not `f32::midpoint` | `crates/pdfrum-form/src/tab.rs:76,84` | The banding comparisons are strict; a midpoint differing in the last bit moves an annotation between Tab-order bands. The file carries `#![allow(clippy::manual_midpoint, clippy::float_cmp)]` with that reason. |
| Widget geometry compared in `f32` after narrowing from `kurbo::Rect` | `crates/pdfrum-form/src/page.rs:415`; `src/hit.rs:157` | The oracle's `CFX_PointF`/`CFX_FloatRect` are `float`; hit tests are inclusive on every edge and a test pins it. |
| `/Ff` bits whose meaning depends on `/FT` | `pdfrum-doc` | §6's table: bit 26 is "file select" on text and "sort" on choice. A flag *set* would pretend they compose. |
| `page_index_of` returning `-1` | `crates/pdfrum-doc/src/nav/dest.rs:203,219` | An `impl Fn(u32) -> i32` callback matching the oracle's destination resolution. |
| UTF-32LE `--txt` dump bytes | `pdfrum-text` | It is the harness's diff format. |

The rule that separates them, and the one to apply when a new case arises:

> **Does the C shape change the answer, or only the spelling?** If a golden
> moves when you change it, it is behaviour and it belongs in a body or a
> private field. If only the call site changes, it is vocabulary and it
> belongs nowhere.

Two borderline cases, resolved by that rule and recorded so they are not
re-argued:

- **`AnnotFlags::names()` returning flags in a fixed bit order**
  (`crates/pdfrum-doc/src/annot/mod.rs:213`). The *order* is behaviour — it is
  the `--annot` dump's order, and §6 already says so. The *location* is
  vocabulary: it is the tool's output format sitting on a library type, and it
  moves with `annot_dump`. Split the item, do not exempt it.
- **`pdfrum_object::as_c_int`** (`lib.rs:62`). The truncation semantics are
  behaviour and are pinned by tests; the name is vocabulary. Rename, keep the
  body, keep the tests, keep the doc comment's citation of
  `FX_Number::GetSigned` — a doc comment may name the C++, an identifier may
  not. That is the general form: **provenance belongs in rustdoc, never in a
  signature.**

### A.4 Two cross-cutting findings

Both surfaced when the surface was inventoried per crate rather than through
the facade, which is itself evidence for §A.7's revised audience table.

**One: `pub mod` count is the best single predictor of leaked surface, and
STYLE.md §4 is currently satisfied in letter and broken in spirit.** §4 asks
that a crate's public API "fits in one `lib.rs` re-export block a reviewer can
read in one screen." The four crates with the *shortest* `lib.rs` —
`pdfrum-render` (103 lines), `pdfrum-doc` (73), `pdfrum-form` (68),
`pdfrum-page` (348) — are the four with the *largest* leaked surface, because
they satisfy "one screen" by listing `pub mod`s instead of re-exporting a
curated set. `pdfrum-form` is the extreme: **sixteen `pub mod`s and zero
private modules.** Every module in the crate is public.

This yields a mechanical criterion for WP13, sharper than the current one:

> A member crate's `pub mod` count is justified module by module in its own
> design doc, **and every type appearing in a public signature is nameable
> from the crate root.**

`pdfrum_form::tab::Rect` fails the second half today — it appears in ten
public signatures inside its crate and is not in `lib.rs`'s re-export block at
all, so a caller must spell a module path the crate never advertises.

**Two: four geometry vocabularies coexist.** §4 (WP4) named three rectangles
and two points *on the facade*. Per crate there are four, and a value moving
from `pdfrum-page` to `pdfrum-form` is converted twice:

| Vocabulary | Where | Scalar |
|---|---|---|
| `kurbo::Rect` / `Point` — the workspace default, re-exported by `pdfrum-common` | everywhere below form | `f64` |
| `pdfrum_doc::geom` — 20+ free functions re-implementing kurbo's own API *over* `kurbo::Rect` (`left`, `right`, `width`, `union`, `intersect`, `contains`, `inflate`, `normalize`, `is_empty`) | `pdfrum-doc` | `f32` accessors over `f64` |
| `pdfrum_form::tab::Rect` — own struct, four `f32` fields, not re-exported | `pdfrum-form` | `f32` |
| `pdfrum_form::event::Point` — own struct, two `f32` fields | `pdfrum-form` | `f32` |

The `pdfrum-doc` one deserves a note: `crates/pdfrum-doc/src/geom.rs` is a
`util` module under a domain-sounding name, which STYLE.md §4 bans by
intent if not by that spelling. `kurbo::Rect` already has `width()`,
`height()`, `union()`, `intersect()`, `contains()`, `inflate()`, `is_empty()`
and `abs()`. What is *not* redundant is the epsilon comparison
(`EPSILON = 1e-4`, compared in `f64` even for `f32` inputs) — that is oracle
behaviour and stays, as a few private functions. The rest is duplication.
Note that `pdfrum-form`'s `geom` is a *different* thing entirely — a real
page↔plate transform — so the two modules do not even mean the same thing.

### A.5 Two tests for every change

Adapted from §1's pair, with the audience widened from "the facade" to "the
crate the change is in":

1. **A caller who has never seen PDFium, `fpdftext/`, `fpdf_formfill.h`, or
   Win32 can use the type without reading a comment that names those things —
   and they are entitled to that from `cargo add pdfrum-text` as much as from
   `cargo add pdfrum`.** The escape-hatch exemption in §4 survives: a method
   documented in its first sentence as an escape hatch may name a
   sibling-crate type, because naming it is the point.
2. **`conformance` and the `.evt` goldens do not move.** Internal `f32`
   midpoints, `f32` hit tests, raw `/Annots` indices, packed permission bits,
   UTF-32LE dumps — those remain, one layer down *inside the crate that owns
   them*, which is where they already are. If a proposed change cannot keep
   test 2 while satisfying test 1, test 2 wins and the item is recorded as
   behaviour in §A.3's second table rather than fixed.

STYLE.md §7's transliteration test now applies to **every** crate's public
surface, not only the facade's: *would this signature look the same if the
author had never seen the C++?*

### A.6 What it costs, measured

§5's preamble says: *"Internal types may keep the old shape behind `From` /
`Into` at the crate boundary."* Under this amendment that conversion layer
largely **disappears**, which is a simplification — but it moves two costs
onto the crates, and one of the two claims made for it in the original
framing does not survive contact with the code.

**Claim checked: "`pdfrum-tool`'s `.evt` replay must convert instead."**
Verified against the replay path; **the claim is true but materially
misleading, and the reason is worth recording** because it removes the main
objection to this amendment.

- The `.evt` parser does not produce `f32` and never did. It produces `i32`,
  via a hand-rolled C `atoi` — `crates/pdfrum-tool/src/events.rs:340`, with
  `pub enum Event { MouseDown { x: i32, y: i32, .. }, .. }`. An `.evt` script
  **cannot express a fractional coordinate at all**: `atoi` stops at the `.`.
  There is no `parse::<f32>()` anywhere in the path.
- The widening to `f32` is *invented* by the bridge, in one four-line
  function — `crates/pdfrum-tool/src/dispatch.rs:287`:

  ```rust
  #[expect(clippy::cast_precision_loss,
      reason = "page coordinates; exact below 2^24 and the C++ widens to double here too")]
  fn at(value: i32) -> f32 { value as f32 }
  ```

  Under this amendment that becomes `f64::from(value)`, and the mirror
  `Call` enum's two field types change. **That is the entire cost to
  `pdfrum-tool`.** The corpus is integer coordinates of at most four digits,
  exact in both `f32` and `f64`, so no replay result can move.
- The `expect` reason above already concedes the point: *"the C++ widens to
  double here too."* Confirmed at the oracle, read-only:
  `public/fpdf_formfill.h:1202-1206` declares `FORM_OnMouseMove(…, double
  page_x, double page_y)`, and `fpdfsdk/fpdf_formfill.cpp:435-444` narrows
  with `CFX_PointF(page_x, page_y)` **at the entry point, before any
  comparison**. So `pdfrum_form::event::Point`'s `f32` is not oracle
  fidelity — it is *stricter than the oracle's own API*, and its own doc
  comment at `crates/pdfrum-form/src/event.rs:19-22` says why: *"the entry
  points take doubles."* Taking `kurbo::Point` publicly and narrowing on
  entry reproduces the oracle's boundary **more** exactly than today's
  signature does.
- `pdfrum-form` already depends on `kurbo` (`crates/pdfrum-form/Cargo.toml:30`)
  and already narrows `kurbo::Rect` to its own `f32` rect at
  `crates/pdfrum-form/src/page.rs:415`. The point conversion is the symmetric
  half of a boundary the crate already has. **No new dependency, and DEPS.md
  is not touched.**

**The real cost, which the original framing pointed at the wrong artefact.**
It is not in `pdfrum-tool`; it is one layer inside `pdfrum-form`, and it is
a genuine hazard that the implementing WP must handle explicitly:

- `crates/pdfrum-form/src/hit.rs:157` — `contains(rect, x: f32, y: f32)`
  compares the point against rect edges that were **already rounded to `f32`**
  by `page.rs:415`. An `f64` point compared against an `f32`-rounded edge
  changes inclusive-edge behaviour, which `hit.rs`'s
  `containment_includes_every_edge` test pins.
- `crates/pdfrum-form/src/route.rs:2279` — `to_plate` subtracts in `f32`
  (`Plate::to_widget`: `at.x - rect.left`, plus the rotation table) and only
  then widens to `kurbo::Point`. Six call sites depend on it: caret
  placement, the double-click client-rect test, `row_at`, `on_drop_button`,
  `drag_to`, and `popup_hit`. Widening the *public* point without narrowing
  it at entry would make that a mixed-precision subtraction and could move a
  caret across a glyph boundary or a list click across a row.

  **The prescription follows directly, and is the amendment's own rule:**
  narrow to the private `f32` `Point` *in the entry function*, exactly as
  `to_rect` already narrows the rect and exactly as
  `fpdf_formfill.cpp:443` narrows to `CFX_PointF`. The interior does not
  change at all, so neither does any golden. What must **not** happen is a
  half-migration that leaves an `f64` point meeting an `f32` rect.

- **The `(top + bottom) / 2` midpoint is not part of this cost at all**, and
  §1 citing it alongside the event point was a category error worth
  correcting. `tab::Rect::center_y` is fed only by `Focusable::rect` — widget
  `/Rect`s — in the Tab-order banding at `tab.rs:286-291`. Tab traversal is
  driven by a key event, which carries **no coordinates**. No event point
  ever reaches the midpoint. It is an argument for keeping `tab::Rect`
  private and `f32`; it is not an argument about `Point`.

**Other instances of the same cost — where dropping the boundary layer moves
work into a crate rather than removing it.**

1. **Permissions, and this one moves work *out* of the facade.**
   `crates/pdfrum/src/form_session.rs:767-778` does the bit-twiddling today:

   ```rust
   let bits = self.doc.permissions(false);
   pdfrum_form::Permissions { fill_form: bits & 0x100 != 0,
                              modify_annotation: bits & 0x20 != 0 }
   ```

   `pdfrum-form` **already has** the idiomatic struct-of-booleans WP1 wants
   (`crates/pdfrum-form/src/hit.rs:118`, with `ALL`, `NONE`, `may_interact`)
   — a member crate arrived at the right shape for its *own* API's sake, with
   no facade involved. The raw `u32` survives only in `pdfrum-crypt` and
   `pdfrum-parser`. Under this amendment the ISO-table-22 decode moves *into*
   `pdfrum-crypt`, next to the `/P` word it decodes, and the facade's hand-
   written mask disappears. Net: less code, and the magic numbers `0x100` and
   `0x20` stop being spelled in a crate that has no business knowing them.

2. **`FormRect` disappears from the facade, and its two remaining callers are
   in the tool.** `pdfrum::FormRect` appears outside `pdfrum-form` in exactly
   two places, both `crates/pdfrum-tool/src/chrome.rs:322,324`. `tab::Rect`
   is not even in `pdfrum-form`'s own curated re-export block
   (`lib.rs:67` exports `tab::{FocusRing, Focusable, TabOrder}`, not `Rect`);
   callers reach it as `pdfrum_form::tab::Rect`. Making it private and
   returning `kurbo::Rect` from `PopupView`/`PopupGeometry` costs those two
   lines and nothing else.

3. **`RenderOptions` exists twice and the two copies are not the same
   problem.** `crates/pdfrum/src/render.rs:77-79` and
   `crates/pdfrum-render/src/options.rs:127-131` both carry `no_path_smooth`
   / `no_image_smooth`. WP10 flips the facade's pair to positive defaults;
   whether the engine's pair follows is **not** obvious, and §A.8 records it
   as the one place this amendment does not settle.

4. **`pdfrum-text` has a name collision WP8 did not notice.** WP8 proposes
   `pub struct CharIndex(usize)`. `pdfrum_text::CharIndex` **already exists**
   (`crates/pdfrum-text/src/index.rs:27`) and is the *segment table* mapping
   the two index spaces — `TextPage.runs: CharIndex`, a public field. The
   newtype needs a different name, or the table does. This is visible only
   from inside the crate, which is itself an argument for this amendment: the
   facade view of `pdfrum-text` never showed it.

### A.7 What is owed to each audience — §3 revised

§3's table said:

> | Engine / conformance / a fourth rasterizer | a member crate | A curated `lib.rs` re-export. Helper functions, oracle dump formats, and scan-conversion internals are crate-private or `#[doc(hidden)]`. |

The *entitlement* named there is right and is kept. The **audience** is wrong:
"engine / conformance / a fourth rasterizer" describes people who work on this
repository, and it is what licensed §1's "the facade translates". The people
who actually depend on a member crate are strangers who ran `cargo add
pdfrum-text`. Replace the table with:

| Audience | Crate they depend on | What they are owed |
|---|---|---|
| Almost everyone | `pdfrum` only | The facade re-export block. Every type that appears in a `pdfrum` signature is named in that block. |
| **Anyone who adds a single member crate** — a text-extraction pipeline that adds `pdfrum-text`, a viewer that adds `pdfrum-form`, a linter that adds `pdfrum-parser` | one member crate | **The same idiomatic standard the facade is held to, in that crate's own vocabulary.** A curated `lib.rs` re-export block; ranges not `(start, count)`; enums not packed integers; questions not bit words; no Win32 or `FPDF_` names in rustdoc. Helper functions, oracle dump formats and scan-conversion internals are crate-private or `#[doc(hidden)]`. |
| This repository — engine, conformance, a fourth rasterizer | several member crates | Everything above, plus the `#[doc(hidden)]` items and the escape hatches. This audience is served *by* the second row, not instead of it. |

The dual-crate story ("this crate composes, it does not hide") is still a
feature, and it is still not a licence to ship `pdfrum_text::pipeline::
is_float_zero` as public API. What changes is that it is now also not a
licence to leave `pdfrum_text::TextPage::page_text(start, count)` C-shaped on
the grounds that a facade caller will never see it. A `pdfrum-text` caller
sees nothing else.

### A.8 What this does **not** change

Confirmed line by line against the revised principle.

**§2 (already idiomatic — do not undo): stands in full.** Every item on that
list is a property of the *engine and its member crates*, not of the facade,
so the amendment strengthens it rather than straining it. Ownership,
config-structs-with-`Default`, enums for closed sets, one `Error` per crate,
the `RasterBackend` seam, kurbo/peniko, values-not-callbacks, documented
escape hatches, `Send + Sync` — all already hold at the crate level and are
precisely what the second audience is being promised. The happy-path snippet
is unchanged.

**§4 (non-goals): stands, with one clarification and one strengthened item.**

- *No behaviour change* — stands, and §A.3/§A.5 make it sharper by naming the
  test.
- *No `bitflags` crate* — stands; see §6 below.
- *No fourth trait seam* — stands. Nothing here proposes one.
- *No builder ladders* — stands.
- *No hiding escape hatches* — stands, and is **clarified**: an escape hatch
  may name a sibling-crate type, and §A.5's test 1 exempts it. The amendment
  narrows the exemption to methods documented as escape hatches in their first
  sentence; it is not a general licence.
- *No C ABI, no `prelude`, no `get_` prefixes* — stands, and is nearly
  satisfied already: a sweep finds three `get_`-prefixed public functions
  workspace-wide, of which two (`get_or_render`, `get_or_insert`) are the
  sanctioned `get_or_*` idiom and one (`pdfrum_page::image::scanline::
  get_bits`, `crates/pdfrum-page/src/image/scanline.rs:23`) should be private.
- *Member crates stay published* — **this item is now load-bearing rather than
  incidental.** §4 already refused to make them `publish = false`
  implementation details. This amendment supplies the reason: they are
  published *because* they are meant to be depended on individually, and a
  published crate that is only usable through the facade is not modular. The
  sentence "their *surface* shrinks; their existence does not" is kept and
  extended: their surface also gets *renamed*.

**§6 (do not add `bitflags`): stands in full, and the amendment strengthens
its central case.** §6's argument is that PDF flag words are a poor fit for a
flag-set crate because unknown bits must round-trip, dump order is fixed, and
`/Ff`'s meaning depends on `/FT`. Every one of those reasons is a *behaviour*
reason in §A.3's second table, so none of them is touched. What §6 asks for —
a hand-rolled newtype with typed constants, `BitOr`, `contains`, and a
`from_bits` that retains unknown bits — is exactly "idiomatic types, oracle
fidelity in the body and the private field", i.e. §A.2 restated for flags.
Two consequences the amendment adds rather than removes:

- §6's "field private" in the `FontFlags` sketch is now **mandatory, in the
  crate**. All three flag newtypes ship a `pub` inner field today
  (`FontFlags(pub u32)`, `AnnotFlags(pub i64)`, `FieldFlags(pub i64)`), and
  all three live in member crates that the facade re-exports verbatim. There
  is no facade in which to fix them.
- §6's ruling that `FieldFlags` stays predicates rather than a set is
  reinforced: the `/FT`-dependence is behaviour, and behaviour does not
  migrate.

**The one thing that does not survive cleanly: `pdfrum_render::RenderOptions`
and WP10.** Recorded as an open question rather than papered over. The
engine's option struct carries a deliberate, documented
`#[expect(clippy::struct_excessive_bools)]` at
`crates/pdfrum-render/src/options.rs:92-98`:

> "these are `CPDF_RenderOptions::Options`' independent bit flags one for one;
> grouping them into sub-structs or an enum would hide which upstream flag
> each is and **make the port unreviewable**"

That is a claim that in *this* struct the oracle's flag *names* are
load-bearing — reviewability of the port, not just spelling — which is the one
place where "vocabulary" and "behaviour" genuinely blur. §13's clippy gate
already grandfathers it. Two readings, and this document does not choose:

- **Facade only.** WP10 flips `pdfrum`'s copy to `smooth_paths` /
  `interpolate_images` with `true` defaults, and `pdfrum-render` keeps the
  oracle's names as a reviewable one-for-one port. Cost: the amendment admits
  one documented exception, and `pdfrum-render`'s direct callers keep an
  inverted flag word.
- **Both.** `pdfrum-render` renames too and the one-for-one mapping moves into
  a comment table. Cost: the port review argument, which was accepted when the
  crate was written and has not been re-examined since.

The rest of this amendment does not depend on which is chosen. It is flagged
here because it is the only item where the ruling and an existing, reasoned,
in-tree decision actually collide.

### A.9 Which work packages collapse into per-crate work

§7 ordered WP11 (member-crate surface hygiene) **twelfth of thirteen**, with
the rationale *"Largest diff, least user-facing."* Under this amendment the
second half of that sentence is false: WP11 is least-user-facing **only if the
facade is the product**. For the second audience in §A.7, WP11 *is* the
product, and much of it is not a separate package at all — it is the same edit
as the WPs that were ordered ahead of it.

The test for collapsing: **is the fix the same edit whether it is done for the
facade or for a direct dependant?** If the offending item lives in a member
crate and the facade re-exports it or forwards to it unchanged, then yes —
there is one edit, and splitting it across two work packages means touching
the same file twice.

**Collapse into per-crate work** (the WP survives as a heading inside the
crate's package, not as a package of its own):

| WP | Owning crate(s) | Why it collapses |
|---|---|---|
| **WP8** `TextPage` indices | `pdfrum-text` | The clearest case, as expected. `pdfrum::TextPage` **is** `pdfrum_text::TextPage` (`lib.rs:258`, verbatim re-export). Every offending item — `page_text(start, count)`, `rects(start, count)`, the two index spaces, `all_text`, `to_utf32le`, the `text: Vec<char>` field colliding with a `text` method — is in `crates/pdfrum-text/src/lib.rs`. There is no facade edit at all beyond the re-export line. Add the `CharIndex` collision from §A.6. |
| **WP2** flag algebra | `pdfrum-font`, `pdfrum-doc` | All three newtypes are defined in member crates with `pub` inner fields and re-exported verbatim (`annotation.rs:3`, `form.rs:8`). Identical edit either way. |
| **WP6** `Key` as an enum | `pdfrum-form` | `Key(pub u16)` is `crates/pdfrum-form/src/event.rs:63`. The facade only aliases it (`Key as VirtualKey`). The enum, the table and `from_virtual`/`virtual_code` are all `pdfrum-form` edits; the facade's share is deleting the alias. |
| **WP1** *in part* — `PdfVersion`, `Permissions` | `pdfrum-parser`, `pdfrum-crypt` | The packed `u8` and the `(owner: bool) -> u32` pair are member-crate signatures with member-crate doctests demonstrating the raw encoding. The facade forwards them (`document.rs:394,413`) and hand-decodes bits at `form_session.rs:771-777`. Fixing the crate deletes the facade's copy. |
| **WP12** *in part* — the JavaScript sentence | `pdfrum-form`, facade | The false claim is in the facade's crate docs, but the seam it misdescribes (`Cascade`, the `script` feature) is `pdfrum-form`'s. A `pdfrum-form` caller reads a different, correct story today; fixing the facade's text is a facade edit, but deciding whether to expose the seam is a `pdfrum-form` API question. |

**Stay separate** (genuinely facade-shaped, or genuinely cross-cutting):

| WP | Why it does not collapse |
|---|---|
| **WP3** collapse render/text method grid | The six-way grid exists **only** in `crates/pdfrum/src/page.rs:147-391`. `pdfrum-render` has one generic entry point already. Pure facade work. |
| **WP5** form session names + `Event` | The facade's `FormSession` is its **own type** (`crates/pdfrum/src/form_session.rs`), not a re-export — `pdfrum_form::FormSession` is a state record and `route::apply` is the entry point. The `on_*` names, `force_kill_focus`, `inner()` and the flattened `x, y` arguments are facade inventions. Stays a facade package — but note `route::apply` already takes an `Event`, so a `pdfrum-form` caller is *ahead* of the facade here. |
| **WP7** facade signatures name only re-exported types | Facade-only by definition. Its *pressure* changes, though: several rows resolve by making the member type idiomatic rather than by re-exporting it as-is. |
| **WP9** `Option`/`Result` on mutation | The `PageEdit` bool-returners are facade-only (`crates/pdfrum/src/edit.rs:110,124,143`). `Form::set`'s silent ignore has a `pdfrum-doc` twin (`form/field.rs:612`) — that row alone collapses; the rest do not. |
| **WP13** mechanical gates | Cross-cutting by construction, and it **grows**: the snapshot and the doc-link gate now apply per published crate, not to `pdfrum` alone. |
| **WP1** *in part* — `PageIndex`, `Error::WrongPassword` | `PageIndex` threads through the facade, `pdfrum-doc` and `pdfrum-form` together; it is a workspace-wide newtype, not one crate's. `WrongPassword` is a facade error-lifting change. |
| **WP4** geometry vocabulary | Straddles. The *facade* half (drop `FormRect`, drop flattened `x, y`) is facade work; the *crate* half (make `tab::Rect` private, take `kurbo::Point` and narrow at entry, return `kurbo::Rect` from `PopupView`) is `pdfrum-form` work and carries the §A.6 hazard. Ordered as one package but landed crate-first. |
| **WP10** `RenderOptions` | Straddles, and is the §A.8 open question. |

**WP11 does not survive as a package.** It becomes the *shape* of the
per-crate packages: each crate's package carries its own `lib.rs` curation
(`mod` + `pub use`), its own dump-format eviction, and its own
`cargo public-api` snapshot. What is left of WP11 as written — the four rules
in its body — is promoted to the standing rule in §8's STYLE.md amendment.

### A.10 Revised sequence

Ordering principle, changed by this amendment: **order by blast radius within
a crate, then by how many other crates wait on the result** — not by
"user-facing first, hygiene last", which presupposed the facade is the user.
Two things stay true from §7: every step is a `[spec]` commit that leaves the
goldens green, and WP7's re-exports are purely additive and unblock
everything.

| Order | Package | Why this position | Touches |
|---|---|---|---|
| 1 | **WP7 re-exports** (unchanged from §7) | Purely additive, breaks nothing, and makes every later signature change expressible. Keeps its old first place. | `crates/pdfrum/src/lib.rs` |
| 2 | **`pdfrum-common` + `pdfrum-object` curation** | Bottom of the DAG, smallest surface (18 and 122 public items), nothing above them can be curated while they still leak. Cheap, and it establishes the pattern the other sixteen follow. | those two crates |
| 3 | **`pdfrum-text` — WP8 entire** | The most C-shaped published surface, the one with no facade translation whatsoever, and it depends on nothing above it. Highest ratio of idiomatic gain to risk in the workspace, which is why it moves from §7's tenth place to here. Resolve the `CharIndex` collision first. | `pdfrum-text`, facade re-export line, `pdfrum-tool` (`to_utf32le` moves) |
| 4 | **`pdfrum-parser` + `pdfrum-crypt` — WP1's `PdfVersion` and `Permissions`** | Newtypes must exist before anything above can name them, and these two crates own the encodings. Deletes the facade's hand-written bit masks as a side effect. | parser, crypt, facade |
| 5 | **WP1's `PageIndex`** | Workspace-wide newtype; needs step 4's crates settled and must precede every form and navigation signature. Keeps §7's reasoning, one place later. | facade, doc, form, edit |
| 6 | **`pdfrum-font` + `pdfrum-doc` — WP2 flag algebra, and `annot_dump` out of the library surface** | The flag newtypes are here, `pub` fields and all. Bundled with evicting `annot_dump` (used only by `pdfrum-tool`) because both are the same crate's surface and both are mechanical. `FontFlags` constants changing from `u32` to `Self` remains the risky bit — many tests construct them. | font, doc, tool |
| 7 | **`pdfrum-form` — WP6 `Key`, then WP4's crate half** | `Key` first (self-contained table), then the geometry: `tab::Rect` private, `kurbo::Point` in with narrowing **at the entry function**, `kurbo::Rect` out of `PopupView`. The §A.6 hazard lives entirely here; landing it as one crate's package is what keeps it reviewable. `pdfrum-tool`'s `at()` becomes `f64::from`. | form, tool, facade |
| 8 | **WP5 form session names + `Event` on the facade** | After 7, so the new signatures are the final ones — §7's reasoning, preserved. Now genuinely facade-only, because the crate half landed in 7. | facade, SPEC §15.8, doctests |
| 9 | **`pdfrum-page` + `pdfrum-render` curation, and WP10** | The two noisiest surfaces (12 and 24 `pub mod`s; 407 and 317 public items) and the largest mechanical diff, so late — but *before* the gates, not after them. Carries the §A.8 `RenderOptions` decision, which must be made here. | page, render, facade, tool, examples |
| 10 | **WP3 collapse render/text method grid** | Facade-only, and reads best once `RenderOptions` has settled in 9. | facade, examples, docs |
| 11 | **WP9 `Option`/`Result` on mutation** | Mechanical, facade-shaped, independent. Unchanged from §7's neighbourhood. | facade edit + form, `pdfrum-doc`'s `Form::set` twin |
| 12 | **`pdfrum-edit` + the raster crates + `pdfrum-cmap`/`filters`/`type1` curation** | Independent of everything above; can run in parallel from step 2 onward. Listed here only because nothing waits on it. | those crates |
| 13 | **WP12 docs** | After the surface is true, per §7. Now covers every crate's rustdoc, not only the facade's. | rustdoc, README, `docs/status/pdfrum-facade.md` |
| 14 | **WP13 gates, per published crate** | Last, and larger than §7's version: `cargo public-api` snapshot and the broken-intra-doc-links gate for each of the seventeen published crates, plus STYLE.md and clippy. | CI, STYLE.md, clippy.toml |

What moved and why, in one line each:

- **`pdfrum-text` 10 → 3.** It has no facade layer, so it was never "one layer
  down"; it was the surface all along.
- **WP11 12 → dissolved across 2, 6, 9, 12.** Its rationale ("least
  user-facing") was the §1 premise this amendment withdraws.
- **WP1 split 2 → 4 and 5.** The crate-owned half (`PdfVersion`,
  `Permissions`) is a different package from the workspace-wide half
  (`PageIndex`).
- **WP4 split 6 → 7 and 8.** Its crate half carries a numerical hazard that
  deserves its own review; its facade half does not.
- **WP2 3 → 6, WP6 7 → 7.** Neither moved far, but both are now filed under
  the crate that owns the type rather than standing alone — which is what
  makes them one commit instead of two.
- **WP3 5 → 10, WP9 9 → 11.** Both are facade-only and nothing waits on them,
  so they yield their early slots to crate work that does block others.
- **WP12/WP13 11,13 → 13,14.** Unchanged in spirit; both now per-crate.

Net effect on the shape of the plan: §7 had thirteen packages of which one
(WP11) was a monolith deferred to the end. This has fourteen steps of which
**seven are named crates** and seven are cross-cutting or facade work. The
monolith is gone, and the two steps that were hardest to review — WP11's
"largest diff" and WP4's precision hazard — are each split across the crates
that own them.

Parallelism: step 12's crates depend on nothing in the chain and can start at
step 2. Steps 3 and 6 do not depend on each other. Step 7 must follow 5.

### A.11 Per-crate scope

**Basis, stated because it is not `cargo public-api`.** `docs/status/api-baseline/`
does not exist at the time of writing and the `chore/api-baseline` branch is
still at `9b8f74b` with no snapshots, so **these numbers are estimated, not
measured by `cargo public-api`.** They come from two counts run over the
worktree: `pub mod` declarations in each `lib.rs`, and a grep for
`^\s*pub (fn|struct|enum|trait|type|const|union)` across each crate's `src/`.
That over-counts slightly (items inside private modules are counted even
though they are not reachable) and under-counts re-exports, so treat them as
an order-of-magnitude signal for *sequencing*, not as an API contract. The
counts were produced twice, independently, and agree to the item; the named
offenders in the last column were each opened and read at the cited line.
When the snapshots land, this table should be re-derived from them — the
expected direction of change is *downward*, since `cargo public-api` will not
count items behind private modules.

| Crate | `pub mod` | pub items | pub fn | What its surface needs, concretely | Size |
|---|---|---|---|---|---|
| `pdfrum-common` | 0 | 18 | 9 | Already the model: no `pub mod`, tiny, re-exports `kurbo`. | trivial |
| `pdfrum-object` | 2 | 122 | 100 | `pub mod names` is a legitimate documented namespace; `pub mod number` is not. `as_c_int` / `as_c_float` / `real_as_c_int` renamed (`lib.rs:62`) — bodies and tests unchanged. Otherwise disciplined. | small |
| `pdfrum-crypt` | 0 | 28 | 16 | One item: `SecurityHandler::permissions(owner: bool) -> u32` (`lib.rs:552`) → two methods returning a `Permissions` struct, with the ISO-table-22 decode moving here from the facade. Its doctest asserts `0xFFFF_FFFF` and must change with it. | small |
| `pdfrum-filters` | 0 | 30 | 20 | Clean. | trivial |
| `pdfrum-cmap` | 1 | 32 | 24 | `pub mod lexer` should be private. `CharCode(pub u32)` / `Cid(pub u16)` are genuine identifier newtypes, not bit words — they stay. | trivial |
| `pdfrum-type1` | 0 | 56 | 40 | Clean within itself. One cross-crate note: it defines `pub struct Gid(pub u16)` (`lib.rs:105`) duplicating `pdfrum-font`'s `Gid` (`ids.rs:14`) — one name, two distinct types, two crates. A coherence question for step 12, not an idiom one. | trivial |
| `pdfrum-font` | 3 | 282 | 195 | `FontFlags(pub u32)` → private field + typed `Self` constants (`ids.rs:87`). Note *why* this one matters beyond tidiness: the constants are `u32`, not `FontFlags`, so **`flags.has(7)` compiles today** — the type exists but types nothing. Also `WIDTH_UNSET: u16 = 0xffff` (`widths.rs:16`) and `FaceHandle(pub usize)` (`subst/db.rs:19`, a public raw index into a private table). `pub mod encoding` / `subst` / `tounicode` curated. The crate also holds the workspace's best *positive* example — `has_glyph: bool` documented as "PDFium's `-1`, which is distinct from glyph 0" (`lib.rs:139`) — which `Option<Gid>` would make unrepresentable rather than merely documented. | medium |
| `pdfrum-parser` | 0 | 82 | 63 | The best-shaped large crate in the workspace: a 60-line `lib.rs`, zero `pub mod`, 82 deliberately re-exported items. `version() -> u8` packed (`doc.rs:695`) → `PdfVersion`; `permissions(owner: bool) -> u32` (`doc.rs:797`) → forward crypt's struct. Two `keyword: bool` modes at `lexer.rs:644,664`. | small |
| `pdfrum-page` | 12 | 407 | 283 | Largest public surface in the workspace. Twelve `pub mod`s (`color`, `function`, `image`, `inline_image`, `mutate`, `optional`, `pattern`, `shading`, `state`, `transfer`, `transparency`, `type3`) → curated re-export. **The clearest single offender in the workspace is here:** `pub const NO_CONTENT_STREAM: i32 = -1` (`mutate.rs:44`) with `pub fn content_stream(&self) -> i32` (`:69`), *re-exported at `lib.rs:80`* — a `-1` sentinel in the curated block. Also `image::scanline::get_bits` (`image/scanline.rs:23`), `Page::color(stroking: bool)` (`page.rs:394`), `is_valid_page_dict(.., strict: bool, ..)` (`page.rs:548`, also re-exported), and **eight** sites carrying `std_conversion: bool` across `color/` — one two-variant enum fixes all eight and is the highest-leverage rename in the crate. Most of the rest become private rather than renamed. | large |
| `pdfrum-render` | **24** | 317 | 224 | The worst `pub mod` count in the workspace — 24 against **one** private module. Named offenders: `pub fn peniko_mix` (`device.rs:251`, returns a `peniko` type the crate does not re-export), `LCD_FIR5` / `LCD_PADDING_26_6` (`glyph.rs:69,77`), the `SUBPIXEL_*` constants and `to_subpixel` (`scanline.rs:69-153`), `EMPTY_CLIP_RECT: Rect = Rect::new(-1.0, -1.0, 0.0, 0.0)` (`clip.rs:28` — an inverted rect standing for "nothing", i.e. `Option<Rect>`), and the `render_page` / `_with_visibility` / `_with_caches` ladder (`walk.rs:55,79,110`). `pub mod walkprofile` (`lib.rs:88`) is **not** on this list: it looks like a STYLE.md §1 global-state violation (`pub fn take() -> Profile` over a `thread_local!`), but it is behind a default-off `walk-profile` feature and its module doc argues the §1 point explicitly at `walkprofile.rs:38-48` — checked, and the reasoning stands. It needs curation like its neighbours, nothing more. Carries the §A.8 `RenderOptions` decision. | large |
| `pdfrum-raster-vello-cpu` | 0 | 13 | 10 | Clean — a backend crate should look like this. | trivial |
| `pdfrum-raster-tinyskia` | 0 | 11 | 9 | Clean. | trivial |
| `pdfrum-raster-agg` | 2 | 20 | 15 | `pub mod image` / `target` → curated; its two sibling backends have zero `pub mod`, so bring it in line and it is trivial. | trivial |
| `pdfrum-text` | 4 | 122 | 97 | **The headline crate** — smallest of the six noisy ones, and the one with the least excuse, since no facade stands between it and its callers. `page_text(start, count)` and `rects(start, count)` → ranges (`lib.rs:214,269`, `select.rs:20`); two unnamed index spaces → newtypes, renaming around the existing `CharIndex` table (§A.6); `all_text` → `as_str`/`Display`; `to_utf32le` → `pdfrum-tool` (`lib.rs:318`); `text: Vec<char>` field renamed off the `text` method; `pub fn is_float_zero` (`charinfo.rs:168`) private; `pub mod bidi`/`index`/`links`/`unicode` curated. Two of WP11's claims about this crate are **stale and withdrawn**: `debug_runs` is already `#[doc(hidden)]` (`lib.rs:116`), and `pipeline` is already a private `mod` (`lib.rs:64`) — only the `pub fn`s inside it need demoting. `find` already returns a `Range` and `char_at` already returns `Result`, not a sentinel. | medium |
| `pdfrum-doc` | 13 | 398 | 322 | Second-largest, and 322 public fns is the most in the workspace. `AnnotFlags(pub i64)` and `FieldFlags(pub i64)` → private fields (`annot/mod.rs:169`, `form/field.rs:102`); `pub mod annot_dump` — the `--annot` output format, used only by `pdfrum-tool` (`crates/pdfrum-tool/src/annot.rs:13,60`) — evicted or `#[doc(hidden)]`, along with its `three_places` / `six_places` float formatters (`annot_dump.rs:276,282`); `pub mod geom` reduced to the epsilon comparisons (§A.4) with the kurbo-duplicating half deleted; thirteen `pub mod`s curated. **`pub mod ap` alone carries ~139 public items** — the whole of appearance generation's internals, where the interface is `generate_appearances`. `page_index_of`'s `-1` stays: behaviour. | large |
| `pdfrum-form` | 16 | 317 | 205 | `Key(pub u16)` → enum, and note the Win32 names are in the *constants*, not just the type: `PRIOR` and `NEXT` are `VK_PRIOR`/`VK_NEXT` for PageUp/PageDown (`event.rs:63ff`). `Modifiers(pub u32)` → private field (`event.rs:119`) — `contains`/`union`/`without` already exist, so the field need never have been public. `event::Point` → `kurbo::Point` in, narrow at entry; `tab::Rect` private (10 public-signature appearances inside the crate, 2 callers outside, both in `pdfrum-tool`); `PopupView`/`PopupGeometry` return `kurbo::Rect`; `Rotation::degrees() -> i32` / `from_degrees` re-open a four-variant enum (`geom.rs:69`); the three `PDFIUM_TEST_*` constants renamed (`script/mod.rs:72,80,111`). Sixteen `pub mod`s and **zero private ones** curated — it already has a good `pub use` block, it just also exports everything a second way. The midpoint, the strict comparisons and the `f32` hit tests all stay. | medium–large |
| `pdfrum-edit` | 5 | 154 | 118 | `pub mod content`/`encrypt`/`font`/`import`/`write` → curated; today each is `pub mod` *and* selectively re-exported, so the same items are reachable two ways. One nice illustration of the vocabulary rule in a single signature: `paint_operator(fill: FillRule, stroke: bool)` (`content/path.rs:35`) — one argument is properly an enum, its neighbour is a bool. | medium |
| `pdfrum` (facade) | 1 | 175 | 153 | Drop `pub mod edit` (§7 WP7). The rest is WP3/WP5/WP7/WP9/WP10 as written. | medium |

Two crates are excluded because they are `publish = false` and therefore have
no external audience: `pdfrum-raster-vello` and `pdfrum-script`. They are held
to STYLE.md like anything else, but nothing in this amendment applies to them.

**Where the work actually is.** Four crates — `pdfrum-page` (407),
`pdfrum-doc` (398), `pdfrum-render` (317), `pdfrum-form` (317) — hold 1439 of
the workspace's 2584 public items and 65 of its 83 `pub mod`s. Seven crates
need essentially nothing. That distribution is why §A.10 sequences by
crate rather than by work package: the packages were a poor unit because they
cut across a distribution this lopsided.

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

> **Superseded 2026-09-02 by §A.** Kept as written, per the house rule that a
> claim a later ruling overturns is withdrawn in place rather than deleted.
> **What survives:** the first two sentences, and both numbered tests in
> substance. **What is withdrawn:** *"The facade translates"* — and with it
> this section's scoping of the whole principle to the facade. §A.1 gives the
> three reasons, one of which is that for several of the types named just
> below there is no facade layer to translate in. §A.2 states the replacement
> principle; §A.3 draws the vocabulary/behaviour line the replacement depends
> on; §A.5 restates the two tests with the audience widened from the facade to
> every published crate.

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

> **Stands unchanged under the 2026-09-02 amendment** (§A.8). Every item here
> is a property of the engine and its member crates rather than of the facade,
> so widening the principle to every crate strengthens this list instead of
> straining it — these are precisely what §A.7's second audience is promised.

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

> **Revised 2026-09-02 by §A.7.** The *entitlement* in the second row is
> right and is kept. The *audience* is wrong: "Engine / conformance / a fourth
> rasterizer" names people who work on this repository, and naming them is
> what licensed §1's "the facade translates". The people who actually depend
> on a member crate are strangers who ran `cargo add pdfrum-text`. §A.7 has
> the replacement table, with three rows rather than two.

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

> **Stands under the 2026-09-02 amendment** (§A.8), with one clarification and
> one item promoted. Clarified: *no hiding escape hatches* — a method
> documented as an escape hatch in its first sentence may name a sibling-crate
> type, and §A.5's first test exempts it. Promoted: *member crates stay
> published* is now load-bearing rather than incidental — §A.1 supplies the
> reason this document previously left implicit.

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

> **Amended 2026-09-02 by §A.** The packages below stand as descriptions of
> *what* must change; §A.9 revises *how they are grouped* — several are not
> separate packages at all but the same edit inside one crate — and §A.10
> revises the order. One sentence in this section's preamble is withdrawn:
> *"Internal types may keep the old shape behind `From` / `Into` at the crate
> boundary."* Under §A.2 that conversion layer largely disappears; §A.6
> measures what its loss actually costs, and corrects two claims about it
> that do not survive contact with the code.

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

> **Stands in full under the 2026-09-02 amendment** (§A.8), which strengthens
> its central case rather than touching it: every reason given below for
> rejecting `bitflags` — unknown bits must round-trip, the dump order is
> fixed, `/Ff`'s meaning depends on `/FT` — is a *behaviour* reason in §A.3's
> second table, and behaviour does not migrate. Two consequences the amendment
> adds: the "field private" in the `FontFlags` sketch below becomes mandatory
> and becomes **member-crate** work (all three newtypes ship `pub` inner
> fields today, in crates the facade re-exports verbatim), and the ruling that
> `FieldFlags` stays predicates is reinforced.

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

> **Superseded 2026-09-02 by §A.10.** The table below is kept as written. Its
> ordering principle — user-facing first, hygiene last — presupposed that the
> facade is the product, which §A.1 withdraws. The most visible consequence:
> **WP11 is ordered twelfth of thirteen here, described as "Largest diff,
> least user-facing."** That description is false for anyone who depends on a
> single member crate, and §A.9 finds that WP11 does not survive as a package
> at all — it is the *shape* of the per-crate packages. §A.10 has the replacement
> sequence, fourteen steps ordered by crate, and closes with a line-by-line
> account of what moved and why.

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

> **Extended 2026-09-02 by §A.** Two of the bullets below are widened from the
> facade to every published crate — the one on signatures naming only
> re-exported types, and the one on packed encodings. What remains of WP11
> after §A.9 dissolves it is promoted here: the four rules in WP11's body
> become the standing per-crate rule, together with §A.4's sharper criterion
> — *every type appearing in a public signature is nameable from the crate
> root*, which `pdfrum_form::tab::Rect` fails today. SPEC.md §9's `TextPage`
> row and §15.5's `Key` row become `pdfrum-text` and `pdfrum-form` changes
> rather than facade ones.

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
