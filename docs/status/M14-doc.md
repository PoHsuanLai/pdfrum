# M14 — the `pdfrum-doc` half

The three additive changes SPEC §10's "[spec] 2026-09-01 (M14)" clause
authorizes, plus one the dispatch wiring asked for afterwards. Nothing
existing changed shape; `crates/pdfrum-doc/**` is the only tree touched.

| Change | Where | Commit |
|---|---|---|
| `ap::field_body::generate` gains `caret_and_selection: Option<&Highlight>` | `src/ap/field_body.rs` | `e172d63`, `5b4af15` |
| `vt::hit` — the four layout queries | `src/vt/hit.rs` | `30d03c0` |
| `Place::start()` and `Hash`, for the consumer | `src/vt/hit.rs` | `a43928c` |
| `annot_render::overlay_with` and `ap::Appearance` | `src/annot_render.rs`, `src/ap/mod.rs` | `68bfd46` |

~~`FieldFlags`' three new predicates — the first item on §2b's list — were not
part of this slice and are not in these commits.~~ **Withdrawn 2026-09-01 by
the orchestrator:** they were landed earlier the same day by the first (Grok)
session of this slice as `567f0b9` (`src/form/field.rs`), with tests. All
three of §2b's `pdfrum-doc` items are on main.

---

## 1. The tie-break rule, transcribed

All C++ paths are relative to `/mnt/data2/pdfium/pdfium-c++/`.

### The rule in one sentence

**A click lands *after* a character only when it falls strictly past that
character's horizontal midpoint** — so a click exactly on a midpoint lands
*before* the character, and the midpoint is half the character's full advance
(comb tail included), not half its inked extent.

### Where it is written

`core/fpdfdoc/cpvt_section.cpp:392-428`, `CPVT_Section::SearchWordPlaceImpl`.
The predicate appears twice, identically — once inside the binary search's
narrowing step and once in the post-loop check that decides between the
landed index and the line header:

```cpp
412      CPVT_WordInfo* pWord = word_array_[nMid].get();
413      if (fx > pWord->fWordX + vt_->GetWordWidth(*pWord) * 0.5f) {
414        nLeft = nMid;
```

```cpp
421    if (fxcrt::IndexInBounds(word_array_, nMid)) {
422      CPVT_WordInfo* pWord = word_array_[nMid].get();
423      if (fx > pWord->fWordX + vt_->GetWordWidth(*pWord) * 0.5f) {
424        wordplace.nWordIndex = nMid;
425      }
426    }
```

Three details that decide whether a port is right or merely close:

- **The comparison is a raw `>`, with no epsilon.** This is the one float
  comparison in the whole hit-test path that does *not* go through
  `FXSYS_IsFloatBigger`. Those macros, at `core/fxcrt/fx_system.h:36-41`,
  carry a `0.0001` tolerance:

  ```cpp
  36  #define FXSYS_IsFloatZero(f) ((f) < 0.0001 && (f) > -0.0001)
  37  #define FXSYS_IsFloatBigger(fa, fb) \
  38    ((fa) > (fb) && !FXSYS_IsFloatZero((fa) - (fb)))
  ```

  The vertical searches — which section (`cpvt_variabletext.cpp:476-490`) and
  then which line (`cpvt_section.cpp:346-360`) — use them. The horizontal one
  does not. That asymmetry is upstream's, it is load-bearing, and softening
  the horizontal test moves carets.

- **The width is the advance, not the glyph.**
  `CPVT_VariableText::GetWordWidth` at `cpvt_variabletext.cpp:645-657` is
  `GetCharWidth(...) * fFontSize * kFontScale + fWordTail`, and `fWordTail`
  is what a comb cell adds. So in a comb field the midpoints are the *cells'*
  midpoints, well right of the glyphs' own centres.

- **The default answer is the line header.** `wordplace` is seeded from the
  range's begin with `nWordIndex` forced to `-1` (`cpvt_section.cpp:395-396`),
  so a point left of every midpoint on the line yields the position *before*
  the first character rather than no answer.

### The surrounding control flow

- `CPVT_VariableText::SearchWordPlace` (`cpvt_variabletext.cpp:462-504`)
  picks a section by y, then delegates. A point above every section returns
  `GetBeginWordPlace()`; below every section, `GetEndWordPlace()`. Neither is
  a failure — that is what lets a drag leave the widget and keep extending a
  selection.
- `CPVT_Section::SearchWordPlace` (`cpvt_section.cpp:334-376`) picks a line
  the same way, with `fTop = fLineY - fLineAscent - GetLineLeading()` and
  `fBottom = fLineY - fLineDescent`. `GetLineLeading()` is always zero here,
  which `vt/mod.rs`'s module docs already record.
- There is **no** `CPVT_Section::SearchLineWord`. The brief and the task
  description both name it; the line-selection-by-y is inlined in
  `SearchWordPlace` above. Noted so the next reader does not go looking.
- `WordPlaceToWordIndex` / `WordIndexToWordPlace`
  (`cpvt_variabletext.cpp:361-411`) count `kReturnLength = 1` per section
  break, which is why `word_index_of_place` adds one per paragraph boundary.

### The coordinate spaces

`OutToIn` at `cpvt_variabletext.cpp:975-977` is
`(x - plate.left, plate.top - y)`: the caller's point is PDF user space,
y-up; the layout's is y-down from the plate's top-left. Before that,
`CPWL_EditImpl::EditToVT` (`fpdfsdk/pwl/cpwl_edit_impl.cpp:1109-1129`) adds
the scroll position and the **vertical alignment padding**. See §3.

---

## 2. The tests that pin it

All in `crates/pdfrum-doc/src/vt/hit.rs`.

| Test | What it pins |
|---|---|
| `the_embeddertests_middle_click_lands_after_four_characters` | The acceptance geometry, below. |
| `the_same_field_clicked_at_either_end_gives_the_two_extremes` | The same field at x = 102 and x = 166, the two neighbouring embeddertests. |
| `a_click_exactly_on_a_midpoint_lands_before_that_character` | The strictness itself, on a plate where midpoints are whole numbers: exactly on lands before, a thousandth past lands after. |
| `points_outside_the_content_clamp_to_its_ends` | Left, right, above and below. None fails. |
| `an_empty_field_answers_its_only_place_from_anywhere` | An empty field has one place and every click finds it. |
| `a_blank_line_between_paragraphs_is_its_own_place` | An empty paragraph between two full ones is reachable. |
| `every_place_survives_the_index_round_trip` | Brief §4.4's P6, over six texts. |
| `comb_cells_hit_test_on_the_cell_rather_than_the_glyph` | The `fWordTail` term. |
| `a_caret_clicked_at_its_own_position_does_not_move` | `place_at_point` and `point_at_place` agree at every position. |
| `a_place_past_the_layout_still_answers_a_point` | No panic on a caret that outlived an edit. |

### The acceptance geometry, computed by hand

`fpdfsdk/fpdf_formfill_embeddertest.cpp:2232-2248` types `ABCDEFGH`, clicks
at `RegularFormAtX(134.0)` — which is `(134, 115)`, from `kRegularFormY` at
`:271` — and expects `ABCDHelloEFGH`: the caret after exactly four
characters. The field's `/Rect` is `[100 100 200 130]` and its `/DA` is
`(0 0 0 rg /F1 12 Tf)` over `/BaseFont /Helvetica`. With the default one-unit
border the plate starts at x = 101, and Helvetica 12 gives:

```
  A  101.000 .. 109.004    midpoint 105.002
  B  109.004 .. 117.008    midpoint 113.006
  C  117.008 .. 125.672    midpoint 121.340
  D  125.672 .. 134.336    midpoint 130.004
  E  134.336 .. 142.340    midpoint 138.338
```

134 is past D's midpoint and short of E's, so the caret lands after D. The
test reproduces this through the real layout with real Helvetica metrics, not
against these numbers as constants.

**One correction to the brief.** §4.3.1 and the task both name
`testing/resources/text_form.pdf` as the fixture. The test's own
`GetDocumentName()` (`:210-216`) returns **`text_form_multiple.pdf`**. It does
not change the arithmetic — that file's "Text Box" field carries an identical
`/Rect` and `/DA` — but the fixture named in the brief is not the one loaded.

---

## 3. What the brief got wrong

**§3.2's signatures are incomplete: the queries need the vertical alignment
offset.** The brief lists `place_at_point` as taking a layout and a point. A
single-line text field is drawn *vertically centred* in its plate — the
`fPadding` of `CPWL_EditImpl::EditToVT`, which `ap::field_body::vertical_offset`
already computes for the appearance path — so its text sits below where the
layout placed it. Without that argument, a click at the acceptance geometry's
y = 115 falls below the content box entirely and clamps to the end place. The
first draft of the acceptance test failed exactly that way, returning word 7
instead of 3.

The fix is not a heuristic: `offset` is now an explicit `(f32, f32)` parameter
on all three geometric queries, the same pair `vt::edit_ap::generate` already
takes. It is also the *only* thing the editor shell adds over the layout,
which is what SPEC §10's E1 revision says, so taking it as an argument keeps
that ruling's premise intact rather than quietly reintroducing editor state
into `vt`.

**§5's U2 is now closed.** It recorded the tie-break as unread and recommended
reading `SearchWordPlace` / `SearchLineWord` before implementing. Done, and
the rule is §1 above. `SearchLineWord` does not exist.

---

## 4. Byte-identity evidence for the `Highlight` `None` path

The claim: passing `None` for the new `caret_and_selection` parameter produces
the stream `generate` produced before the parameter existed, byte for byte.

**How it was established.** The pre-change sources were checked out from the
parent commit into a scratch copy, a dump harness was compiled against *them*,
and its output was recorded. The working-tree sources were then restored, the
same harness compiled against those, and the two outputs diffed. They are
identical: **1048 bytes over eight fixtures**, `diff` empty.

The eight fixtures are the six the existing body tests exercise (a text field,
a comb text field, a combo box, a list box selected by `/V`, one by `/TI`, one
by `/I`) plus the two that correctly generate nothing at all (an empty text
field, a caption-only push button). The `<none>` rows are part of the identity:
they prove the overlay did not cause a body to be emitted where none was.

**How it stays established.** That captured output is checked in as
`crates/pdfrum-doc/tests/data/unfocused_field_bodies.txt` and compared by
`ap::field_body::tests::none_is_byte_identical_on_every_existing_widget_fixture`.

The prior session's version of that test compared a `None` call against a
second `None` call and asserted they matched. That is a tautology — it detects
only a non-deterministic overlay, not a changed one — and it was replaced.

Second, independent evidence from the other side: `src/ap/widget.rs` is
unchanged apart from the literal `None` at its one call site, so its fifteen
stream-asserting tests are running against the new code and passing.

---

## 5. The fourth change: a caller-supplied overlay

Not one of §2b's three. `annot_render::overlay` built its `AnnotOverlay`
internally and accepted none, so appearances a form session produced after an
event could not reach the page. `overlay_with` takes an
`Option<&ap::AnnotOverlay>` and merges it over what it generates; `overlay` is
now a call to it with `None`, so its one existing caller
(`crates/pdfrum/src/page.rs`, outside this slice's ownership) is untouched and
its output is unchanged.

Both overlays are keyed by the **raw** `/Annots` index —
`list.source_indices[slot]`, which recovers it after the loaded list has
dropped and reordered entries. The merge keeps that key.

`AnnotOverlay`'s slot became `ap::Appearance`, a three-state enum, because two
states cannot say what the merge needs to say:

| State | Meaning |
|---|---|
| `Untouched` | Nothing to say; the file's own `/AP` is used. |
| `Generated(GeneratedAp)` | Draw this instead. |
| `Suppressed` | Draw nothing at all, even if the file has an `/AP`. |

`Suppressed` is the "positive suppress marker" the wiring asked for. **Nothing
sets it yet.** It exists as a value rather than as an absence so a cleared
appearance is distinguishable from an ungenerated one, which absence could not
express. The draw loop honours it already (only the widget highlight survives,
since that is painted after the appearance and independently of it), so the
later caller needs no further change here.

`set` and `get` keep their signatures and meanings; a suppressed entry reads
as `None` from `get`, and `appearance()` is what tells the two apart.

---

## 6. Open items

- ~~**`FieldFlags`' three predicates** (`is_editable_combo`, `is_multi_select`,
  `do_not_scroll`) — the first of §2b's three `pdfrum-doc` items — are not in
  this slice.~~ Withdrawn — landed as `567f0b9` (see §1 note).
- **`Suppressed` has no producer.** By design, but it means the draw-loop
  branch that honours it is currently exercised only by unit tests.
- **U3 remains open** — `/NeedAppearances` interacting with a live edit is
  untouched here and still has no corpus fixture.

---

## 7. The fifth change: the live-state override (`6c672a9`)

The seam the eighteen `form-events` rows were waiting on, and the one thing
§5's overlay could not carry: **what a focused field is showing**, when that
is not what the file stores.

### What was wrong

Every path into the generators read the value out of the dictionary and
offered no way past it. `text_field` called `field_value(valued, r)`,
`combo_box` called `selected_indices` and then `field_value`, `list_box`
called `selected_indices` and read `/TI` — and `ap::widget::generate_with_text`
wrapped all three with a literal `None` at its one call site. So a field the
user had typed into rendered its stored `/V`; an empty one rendered empty,
with §5's caret sitting correctly at position zero of a string nobody was
editing. The caret half worked and the text half did not, which made the
overlay's own correctness invisible.

### The shape

One optional parameter, a record of the four displaced reads:

```rust
pub struct LiveState<'a> {
    pub text: &'a str,
    pub selected: &'a [usize],
    pub top_visible: usize,
    pub scroll: (f32, f32),
}
```

`ap::field_body::generate` takes it after `caret_and_selection`; `BodyInput`
carries it as `live: Option<&'a LiveState<'a>>`; the builders read it in place
of the dictionary. `ap::widget` gains a sibling entry point rather than a
sixth argument on the existing one:

```rust
pub fn generate_with_live<R: Resolve>(
    dict: &Dict, catalog: &Dict, font: &TextFont<'_>, r: &R,
    caret_and_selection: Option<&field_body::Highlight>,
    live: Option<&field_body::LiveState<'_>>,
) -> Option<GeneratedAp>;
```

`generate_with_text` keeps its signature and its two existing callers
(`ap::mod`'s dispatch and `pdfrum-form`'s `route.rs`) untouched; it is now
`generate_with_live` with two `None`s. That is why the change is additive on
both sides of the seam rather than only on ours.

Ownership: `field_value` returns an owned `String` and `live.text` is
borrowed, so the two meet in a `Cow<'_, str>` — a `BodyInput::text` helper for
the text field, and inline in the combo box, whose stored branch has a second
owned source (an option's label) the live branch does not. The list box's
selection is a `Cow<'_, [usize]>` for the same reason.

### Where the scroll enters the geometry

`CPWL_EditImpl::VTToEdit` — `fpdfsdk/pwl/cpwl_edit_impl.cpp:1087-1107`:

```cpp
1105  return CFX_PointF(point.x - (scroll_pos_point_.x - rcPlate.left),
1106                    point.y - (scroll_pos_point_.y + fPadding - rcPlate.top));
```

That is the only transform between the layout and the drawn point, and every
drawn thing goes through it: `Iterator::GetWord` rewrites each word's location
with it (`:75`), `GetLine` each line's (`:85`), and the caret and the
selection rects are published through it too (`:1430-1431`).

Three consequences the port turns on:

- **`scroll_pos_point_` is a position, not a delta**, and `SetPlateRect`
  (`:746`) seeds it at `(rcPlate.left, rcPlate.top)`. So at rest the two
  bracketed terms cancel and only `-fPadding` survives — which is exactly the
  `vertical_offset` this module already had, and why the unscrolled stream was
  right without ever naming a scroll.
- **What a reader can observe is therefore the difference from that seed
  alone.** `LiveState::scroll` carries the difference rather than the
  position, which is what makes `(0.0, 0.0)` mean "unscrolled" without the
  record having to know a plate to interpret itself against. `shift()` negates
  both components, matching the subtraction above.
- It reaches the emitter through **the same `offset` argument the vertical
  alignment already travelled on** — `set_text` adds the two and passes one
  pair to `vt::edit_ap::generate`. No new plumbing, and adding zero writes the
  identical `Td` operators.

The list box does not go through `set_text` (its rows are laid out one at a
time into a zero-height plate), so it applies the shift itself — to the row
origin **and** to the selection band, which must stay under the row it belongs
to rather than where that row rested. `1.20.9` of the brief is what says the
`scrollable_widgets{1,2}` fixtures can observe a scroll *only* through which
words are drawn and where, and this is the "where".

### What is deliberately not overridden

A **push button** takes `(0.0, 0.0)` outright, named at the call site: its
text is a `/MK /CA` caption, never a value, so nothing a session holds can
displace it and it never scrolls.

A **focused combo box** ignores the option-label lookup entirely and draws
`live.text` — an editable one is being typed into, and a read-only one has had
its text set from the row the user picked, so the session has already resolved
the label either way.

### Byte identity

Unchanged and still enforced. §4's golden
(`tests/data/unfocused_field_bodies.txt`) and its test were not touched, and
they pass against the new code, which is the evidence that `None` for both
optional parameters is the stream that existed before either. Two new tests
approach it from the live side:
`an_unscrolled_live_edit_draws_where_the_stored_value_would` asserts a live
edit holding the same text at zero scroll is byte-equal to the stored path,
and `none_and_a_default_live_state_agree_on_every_widget_fixture` asserts the
same across the fixtures whose dictionaries store nothing to override
(`ch_list_i`, `tx_empty`, `btn`). The other five fixtures legitimately differ
under a default `LiveState` — an empty field with nothing selected at row zero
is a different appearance from `Hello` — and are excluded by name rather than
by silence.

### Gates

`fmt`, `clippy -D warnings`, `nextest -p pdfrum-doc` (378 pass, up from 367 — ten new tests in
`field_body`, one in `widget`),
`cargo test --doc -p pdfrum-doc`, `cargo build -p pdfrum-form` (clean),
`cargo nextest run --workspace` (3385 pass). Conformance
`run --check-regressions`: **no regressions** — 1705 files, 1636 pass, 69
fail, the same 18 `form-events` / 2 `page-count` / 42 `pixel-fail` / 9
`tierA-mismatch` split as before.

### Open item

~~**Nothing constructs a `LiveState` yet.**~~ **Closed the same day.** It was
written as an open item — the producer is `pdfrum-form`'s change, not this
one — and that crate's routing commit (`8e1fca1`) landed on top of this seam
within the hour, calling `generate_with_live` and building a `LiveState` from
both `FieldState::Text` and `FieldState::Choice`. The seam is consumed end to
end; unlike §5's `Suppressed`, it did not have to wait.

**The eighteen `form-events` rows did not move.** A second conformance run
*after* `8e1fca1` — so with the seam built, consumed, and a `LiveState`
actually reaching the generators — reports the identical 1636/69 split and the
identical eighteen. So the override was necessary and is not sufficient:
something further down the event pipeline still gates those rows, and finding
it is `pdfrum-form`'s work rather than this crate's. Recorded here so the next
reader does not re-derive the seam looking for the cause; the evidence that
*this* half is right is the unit tests above, not the scoreboard.

---

## 6. Focus: the tint the edited field does not get (`6da06a3`)

`src/annot_render.rs`, `src/ap/mod.rs`, `src/lib.rs`. Additive; the `None`
path is the pass as it was.

### The rule, and where it is written

`CFFL_InteractiveFormFiller::OnDraw` (`fpdfsdk/formfiller/cffl_interactiveformfiller.cpp:59-95`)
is **one `if`/`else` over whether the widget has a live form-field control**,
not a switch on focus:

```cpp
68  CFFL_FormField* pFormField = GetFormField(pWidget);
69  if (pFormField && pFormField->IsValid()) {
70    pFormField->OnDraw(pPageView, pWidget, pDevice, mtUser2Device);
71    if (callback_iface_->GetFocusAnnot() != pWidget) {
72      return;                                    // exit 1
73    }
75    CFX_FloatRect rcFocus = pFormField->GetFocusBox(pPageView);
76    if (rcFocus.IsEmpty()) {
77      return;                                    // exit 2
78    }
80    CFX_DrawUtils::DrawFocusRect(pDevice, mtUser2Device, rcFocus);
82    return;                                      // exit 3
83  }
85  if (pFormField) { pFormField->OnDrawDeactive(...); }
86  else            { pWidget->DrawAppearance(...); }
89  if (!IsReadOnly(pWidget) && IsFillingAllowed(pWidget)) {
90    pWidget->DrawShadow(pDevice, pPageView);      // the tint
91  }
```

**All three exits are inside the live-control branch, and none of them
reaches `DrawShadow`.** So the tint is not "suppressed when focused" — it is
simply on the other side of a branch the edited widget never takes. That is
one fact with two consequences, and the second is the one that was easy to
miss: an empty focus box (exit 2) still costs the tint.

### The focus box has three answers, and two of them are nothing

`CFFL_FormField::GetFocusBox` (`fpdfsdk/formfiller/cffl_formfield.cpp:480-489`)
asks the live PWL control for `GetFocusRect` and then discards the result
unless the page box *contains* it. The overrides disagree, and the table is
the design:

| control | `GetFocusRect` | our `FocusBox` |
|---|---|---|
| `CPWL_Edit` — text field (`fpdfsdk/pwl/cpwl_edit.cpp:313-315`) | **empty** | `None` |
| `CPWL_ComboBox` (`fpdfsdk/pwl/cpwl_combo_box.cpp:321-323`) | **empty** | `None` |
| `CPWL_ListBox`, multi-select (`fpdfsdk/pwl/cpwl_list_box.cpp:227-234`) | the **caret item's** rect ∩ client rect | `Rect(..)` |
| `CPWL_ListBox` single-select, check box, radio (`fpdfsdk/pwl/cpwl_wnd.cpp:713-719`) | window rect `Inflate(1,1)` | `Inflated` |
| `CPWL_PushButton` (`fpdfsdk/pwl/cpwl_special_button.cpp:21-24`) | window rect deflated by the border width | `Rect(..)` |

**The task brief was wrong about the text field.** It specified the widget
rect via `GetViewBBox`; `GetViewBBox` (`cffl_formfield.cpp:38-53`) is the
*invalidation* rectangle — it unions the focus box with the annot rect and
inflates by one — and is never what `OnDraw` strokes. `OnDraw` strokes
`GetFocusBox`, and for a text field that is empty. **A focused text field
draws no outline at all.**

### How it was confirmed against the goldens

Two files, read directly out of `conformance/goldens`.

**`form_textfield_focused_ltr` — the tint, and the absence of an outline.**
`7c9ffc0dafcfa42f`, `/Rect [50 40 150 70]` on a 200x100 page, so device rows
30..59 and columns 50..149 — 3000 pixels.

| artifact | tinted `(241,244,255)` pixels | dashed outline |
|---|---|---|
| `input.pdf.0.png` (no events) | **3000** | none |
| `input.pdf.0.events-{6e0b1e47,a631b3ae,de654ec5,ea28932b}.png` | **0** each | none |

All four events goldens carry glyphs, a selection band or a caret (the
one-pixel column at x=100 in `a631b3ae`, rows 39..50) over plain white. Not a
single tinted pixel, and not a single dash. That is exits 1 and 2 of the
branch above.

**`scrollable_widgets1` — the one focus rectangle in the corpus.**
`1b74251ab464ae5e`, a **multi-select list box** (`/FT /Ch`, `/Ff 2097152` =
bit 22) at `/Rect [100 400 200 430]` on a 300x600 page, so the widget covers
device rows 170..199 and columns 100..199. Both events goldens are untinted,
and both stroke a dashed near-black rectangle at **alternating x, step 2** —
the `{1.0f}` dash array at width 1:

| golden | dashed rows | dashed columns |
|---|---|---|
| `events-6128e0d0` | 185 and 198, with vertical dashes at x=101 and x=186 | 101..186 |
| `events-70812be1` | 171 and 184, same columns | 101..186 |

**14 device rows tall and 85 columns wide, entirely inside a widget that is
30 by 100** — and the two goldens differ only in *where* the band sits, which
is the scroll position their `.evt` files set. That is
`CPWL_ListBox::GetFocusRect`'s caret item clipped to the client area, not the
widget's edges and not the widget's edges inflated. The brief's "one pixel
outside the widget's bottom edge, device row 199" does not describe either
golden.

### The shape, and why it is a field rather than a parameter

Option (b). `ap::AnnotOverlay` gains

```rust
pub struct Focus { pub annot: usize, pub box_: FocusBox }
pub enum FocusBox { None, Inflated, Rect(kurbo::Rect) }
```

with `AnnotOverlay::set_focus(Focus)` / `focus() -> Option<Focus>`.
`overlay_with`'s signature is **unchanged**; `pdfrum-tool`, `pdfrum` and
`pdfrum-form` all compile untouched. Three reasons this beat a ninth
parameter:

- The focus is set by the same session that sets the appearances, from the
  same raw `/Annots` index space, and travels with them through the same
  `merge_over`.
- `overlay_with` already carries an `#[expect(clippy::too_many_arguments)]`.
  Adding a ninth argument to a function that needs a waiver to have eight is
  the wrong direction.
- The focus needs to survive the merge, and a parameter would not: a session
  overlay sized for the one appearance it produced can still name a focused
  annotation past its end, which `merge_over` now carries across explicitly
  (an *entry* past the end is still dropped — it names a slot; a *focus* is
  not — it names an annotation).

`FocusBox` is three-valued rather than an `Option<Rect>` because `Inflated`
is derivable from the annotation alone while `Rect` is not, and collapsing
them would force every caller to know the inflation rule. `None` is the
default and is not a failure: a focused entry that names it still loses the
tint.

### In the pass

The four `highlight(...)` call sites became one `push_chrome(...)`, which is
the branch transcribed: when `focus.annot == index` it appends `focus_rect`'s
output (often nothing) and returns; otherwise it appends `highlight`'s. The
two are exclusive by construction, which is what makes the `Suppressed`
interaction fall out rather than need a rule — a suppressed *focused* widget
gets neither the appearance nor the tint, and still gets its outline, because
the focus rect is chrome painted after the appearance and independently of
it, exactly as the tint was.

`focus_rect` itself transcribes `CFX_DrawUtils::DrawFocusRect`
(`core/fxge/cfx_drawutils.cpp:16-39`): the four corners as a closed path,
stroked opaque black with dash array `{1.0}`, phase 0, and the rest of
`CFX_GraphStateData`'s defaults (`core/fxge/cfx_graphstatedata.h:52-55`) —
width **1.0**, butt caps, miter joins. Fill argb is **0**, so the
`EvenOddOptions()` beside it names a rule for a fill that never happens;
that is `FillRule::None` here, the same spelling `invalid_outline` already
uses for the same reason.

The page-box containment test at `cffl_formfield.cpp:487-488` is deliberately
**not** applied here. It is a `Contains`, not an intersection — a box hanging
one unit off the page is discarded whole — and it needs the page box, so it
belongs to whoever computes the rectangle.

### Tests

Nine, all in `src/annot_render.rs`.

| Test | What it pins |
|---|---|
| `the_focused_annotation_loses_its_tint` | The pixel claim: one object unfocused, none focused. |
| `a_focus_box_of_none_strokes_nothing_but_still_suppresses_the_tint` | Exit 2 — the text-field case. |
| `a_focused_widget_strokes_a_dashed_black_hairline_over_its_focus_box` | Colour, width 1, dash `[1.0]`, phase 0, `FillRule::None`, identity matrix, exact bounds, `dirty: false`. |
| `the_list_box_focus_box_is_the_caret_item_not_the_widget_rect` | `scrollable_widgets1`'s geometry: 14 rows tall, narrower than the widget. |
| `an_inflated_focus_box_grows_the_annotation_rect_by_one_unit` | `CPWL_Wnd::GetFocusRect`, including a corner-first `/Rect` that normalizes before inflating. |
| `an_empty_focus_box_strokes_nothing_and_does_not_bring_the_tint_back` | A degenerate box on either axis. |
| `every_index_but_the_focused_one_is_unchanged` | Six widgets, focus on index 1: every other index compares equal by value. |
| `no_focus_is_the_pass_as_it_was` | `None` over eight annotations at three indices each equals `highlight` alone. |
| `focus_merges_over_and_is_not_bounded_by_the_overlay` | Merge semantics, and a focus past the overlay's end. |

`tests/data/unfocused_field_bodies.txt` and its test are untouched.

### Gates

`cargo fmt -p pdfrum-doc -- --check`; `cargo clippy -p pdfrum-doc
--all-targets -- -D warnings`; `cargo nextest run -p pdfrum-doc` (387 pass);
`cargo test --doc -p pdfrum-doc` (3 pass); `cargo build -p pdfrum-form -p
pdfrum -p pdfrum-tool` (clean, unchanged); `cargo nextest run --workspace`
(3430 pass, 1 skipped). Conformance `run --check-regressions`: **no
regressions** — 1705 files, 1636 pass, 69 fail, and a per-file diff of the two
scoreboards shows **0 rows differing**. Nothing supplies a focus yet, so the
change is inert by construction.

### Open items

- **Nothing sets a focus yet.** The producer is `pdfrum-form`'s: it holds the
  focused `AnnotId` and must call `AnnotOverlay::set_focus` beside the
  appearances it already sets. Until it does, the eighteen `form-events` rows
  keep their tint and stay where they are — the tint is the whole of the
  `form_textfield_focused_*` delta, so those rows should move on the first
  commit that sets it.
- **Only `FocusBox::None` is reachable from a text field, which is all four
  `form_textfield_focused_*` rows need.** `Rect` needs the list control's
  scroll and caret state, which lives in `pdfrum-form`; `scrollable_widgets1`
  stays failing until that crate computes and supplies it.
- **The page-box containment test is unimplemented by design** (above). A
  caller supplying a `Rect` must apply it, or a focus box hanging off the page
  will be stroked where the oracle drops it. No corpus file exercises this.
