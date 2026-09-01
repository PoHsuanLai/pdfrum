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

`FieldFlags`' three new predicates — the first item on §2b's list — were not
part of this slice and are not in these commits.

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

- **`FieldFlags`' three predicates** (`is_editable_combo`, `is_multi_select`,
  `do_not_scroll`) — the first of §2b's three `pdfrum-doc` items — are not in
  this slice.
- **`Suppressed` has no producer.** By design, but it means the draw-loop
  branch that honours it is currently exercised only by unit tests.
- **U3 remains open** — `/NeedAppearances` interacting with a live edit is
  untouched here and still has no corpus fixture.
