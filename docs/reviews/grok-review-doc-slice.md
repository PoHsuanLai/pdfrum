<!-- Durable copy of the M14 review record. Source: session
     ede35de1-5746-41fa-ab05-706565ff2804 scratchpad/grok-review-doc-slice.md, 2026-09-01. Verbatim. -->

# Cross-vendor review: `crates/pdfrum-doc` M14 slice

**Reviewer:** Grok (read-only). **Repo:** `/mnt/data2/pdfium/pdfrum`, branch `main`.
**Range reviewed:** `567f0b9^..6da06a3` (`git show` of each hash; `git diff 567f0b9^..6da06a3 -- crates/pdfrum-doc`).
**Working tree:** dirty `crates/pdfrum-doc/src/form/field.rs` (uncommitted `/I`-first interaction reader, +175 lines). Not reviewed. Verdicts are on committed trees only.

**Commits in range**

| Hash | Subject |
|---|---|
| `567f0b9` | `FieldFlags` combo-edit / multi-select / do-not-scroll |
| `e172d63` + `5b4af15` | `Highlight` overlay + unfocused-body golden |
| `30d03c0` + `a43928c` | `vt/hit.rs` queries, `Place::start`/`Hash` |
| `68bfd46` | `overlay_with` + `Appearance` merge by raw `/Annots` index |
| `6c672a9` | `LiveState` + `generate_with_live` |
| `6da06a3` | `Focus`/`FocusBox`; no tint on the focused widget |

**Later `crates/pdfrum-doc` commits (noted, no verdict)**

- `7249c98` — rustdoc: stop linking the public `overlay_with` docs at private `focus_rect`.
- `dae5bbd` — hover pop-up card (`ap::popup`, `set_hover`, `push_open_popup`). `is_visible` no longer drops every pop-up.
- `26f3706` — live list box reserves `CPWL_ScrollBar::kWidth` (12) on the client; stored `/AP` path does not. Golden file untouched.

`cargo nextest run -p pdfrum-doc --test-threads 30`: **399 passed** (HEAD + dirty `field.rs`, not the `6da06a3` tree). `none_is_byte_identical_on_every_existing_widget_fixture` passed. C++ oracle was read, not built.

Line numbers below are the `6da06a3` tree.

---

## Findings

### 1. blocker — `word_index_of_place` drops a wrapped line-header onto index 0

**File:** `crates/pdfrum-doc/src/vt/hit.rs:427-446` (and the skip at `hit.rs:770-776`).

**What the code does.** `word_index_of_place` never looks at `place.line`. For `word == -1` it does `saturating_add(1) → 0` and adds that to the preceding sections' counts. A caret at the header of line 1 of section 0 — `Place { section: 0, line: 1, word: -1 }`, which is exactly what `place_at_point` returns for a click left of that line's first midpoint — therefore reports index **0**.

`every_place_survives_the_index_round_trip` skips `word < 0 && l > 0` and claims “the same collapse upstream performs.” Upstream does collapse, but **not to 0**.

**What C++ / the contract say.** `CPVT_VariableText::WordPlaceToWordIndex` (`core/fpdfdoc/cpvt_variabletext.cpp:361-379`) calls `UpdateWordPlace` first (`:347-358`), which runs `PrevLineHeaderPlace` (`:715-720`):

```cpp
if (place.nWordIndex < 0 && place.nLineIndex > 0)
    return GetPrevWordPlace(place);
```

That is the previous line's `GetEndWordPlace` (`cpvt_section.cpp:267-290`). For a section whose first line holds words `0..=4`, the header of line 1 becomes `{line: 0, word: 4}` and the index is **5**, not 0. `place_at_point` itself is allowed to name the line header (`SearchWordPlaceImpl` seeds `nWordIndex = -1` and `SearchWordPlace` then writes `nLineIndex = nMid`, `cpvt_section.cpp:395-367`); the conversion is what folds it.

This is load-bearing for the consumer the queries were added for. `pdfrum-form/src/edit/ops.rs:13-16` converts every mutation through the flat index; `insert_char` (`ops.rs:419`) calls `caret_index()` → `word_index_of_place`. A click at the start of a wrapped line, then a keystroke, inserts at the start of the field.

**Concrete fix.** At the top of `word_index_of_place`, apply the same fold:

```rust
if place.word < 0 && place.line > 0 {
    if let Some(begin) = layout.sections.get(place.section as usize)
        .and_then(|s| s.lines.get(place.line as usize))
        .map(|line| line.begin)
    {
        let word = begin.saturating_sub(1); // -1 if this line starts at 0
        return word_index_of_place(layout, Place { word, line: place.line - 1, ..place });
    }
}
```

Add a wrapping fixture (narrow plate, `"abcdef"` that breaks) that clicks the second line's header and asserts the index equals the first line's length, not 0. Replace the skip in `every_place_survives_the_index_round_trip` with an assertion that the collapsed spelling is the previous line's end.

---

### 2. should-fix — a text-field selection recolors the whole `BT` run white

**File:** `crates/pdfrum-doc/src/ap/field_body.rs:549-554` (`wrap_text`).

**What the code does.** If `Highlight.selection` is non-empty, the fill for the entire body run is `Color::Gray(1.0)`. The bands are painted first as `0 51/255 113/255` rectangles; unselected glyphs are still emitted in the same `BT`/`ET` in white. A partial selection of `"ABCDEFGH"` covering `"CD"` draws AB and EFGH white on the un-tinted field background.

List boxes are fine: they color per row (`field_body.rs:868-889`). The bug is the text-field / combo path that goes through `wrap_text`.

**What C++ says.** `CPWL_EditImpl::DrawEdit` (`fpdfsdk/pwl/cpwl_edit_impl.cpp:650-689`) sets `bSelect` per word (`place > BeginPos && place <= EndPos`), fills `crSelBK = ArgbEncode(255, 0, 51, 113)` only for those words, and splits the show buffer when `crOldFill != crCurFill`. Unselected text stays `crTextFill`.

`Highlight` only carries rectangles, so `wrap_text` cannot recover the selected span. The M14-doc sentence “the text over it is set white” describes the shortcut, not DrawEdit.

**Concrete fix.** Carry the selected character range on `Highlight` (or a sibling) and emit three runs inside one `BT`: DA-color prefix, white selected span, DA-color suffix — the same split DrawEdit does at `:678`. Painting the original run twice (DA color, then white clipped to the bands) also works and keeps the rectangle-only API.

---

### 3. should-fix — `Place::default()` is `(0, 0, 0)`, not the line header

**File:** `crates/pdfrum-doc/src/vt/hit.rs:82-88`, `105-110`.

**What the code does.** `#[derive(Default)]` on `Place` zeros all three fields, so `Default` is “after the first character.” `Place::start()` is `(0, 0, -1)`.

**What C++ / the brief say.** `CPVT_WordPlace` defaults to `(-1, -1, -1)` (`cpvt_wordplace.h`). `GetBeginWordPlace` is `(0, 0, -1)` (`cpvt_variabletext.cpp:413-415`). Brief §1.20.1: `word == -1` is before the first word; `SetText` inserts at `(0, 0, -1)`.

`a43928c` added `start()` specifically so `Selection::empty` does not need a layout. `Default` still hands a different, valid-looking place to any `..Default::default()` / `BTreeMap` hole.

**Concrete fix.** Implement `Default` as `Place::start()` (drop the derive), or `#[derive(Default)]` on a newtype that cannot be a caret. Grep `Place::default` / `..Default::default()` in `pdfrum-form` after.

---

### 4. nit — caret is a filled `re f`, C++ strokes a 0.4-wide line

**File:** `crates/pdfrum-doc/src/ap/field_body.rs:559-566`; `vt/hit.rs:357-372`.

**What the code does.** `CARET_WIDTH = 0.4`. `wrap_text` fills the rectangle `caret_rect` returns, after `ET`, in `0 g`. Coverage is `[x, x+0.4]` at the character's trailing edge.

**What C++ says.** `CPWL_Caret::width_ = 0.4f` (`cpwl_caret.h:41`). `DrawThisAppearance` (`cpwl_caret.cpp:33-55`) strokes a vertical line at `rcRect.left + width_ * 0.5f` with `line_width = width_`, fill argb 0, opaque black. With butt caps that also covers `[x, x+0.4]`. Same width, same place, different primitive. At 1× a 0.4-unit stroke vs fill can differ by a hair on the caps.

**Concrete fix.** Stroke a 0.4-wide vertical segment at `left + 0.2` with fill-none, matching `:46-55`, if a focused-field golden shows a one-pixel disagreement on the caret column.

---

### 5. nit — `FocusBox` docs claim a read-only combo is `Inflated`; C++ combo is always empty

**File:** `crates/pdfrum-doc/src/ap/mod.rs:96-107` (the `FocusBox` module docs). `annot_render.rs:276-285` has the correct table.

**What the code does.** The enum does not encode field type; the caller supplies the box. The `ap/mod.rs` docs say an editable combo is `None` and a read-only combo / checkbox / radio is `Inflated`.

**What C++ says.** `CPWL_ComboBox::GetFocusRect` (`cpwl_combo_box.cpp:321-323`) returns `CFX_FloatRect()` with no editable check. Same as `CPWL_Edit` (`cpwl_edit.cpp:313-315`). Check/radio use `CPWL_Wnd::GetFocusRect` inflate-by-1 (`cpwl_wnd.cpp:713-719`). The `annot_render.rs` table and `docs/status/M14-doc.md` §6 match C++.

**Concrete fix.** Copy the `annot_render.rs` table into the `FocusBox` docs so `pdfrum-form` does not inflate a combo.

---

### 6. nit — `field_body::generate` grew two required parameters

**File:** `crates/pdfrum-doc/src/ap/field_body.rs:343-354` at `6da06a3`.

**What the code does.** Public `generate` went from `(dict, catalog, font, r)` to also requiring `caret_and_selection` and `live`. `widget::generate_with_text` kept its signature and now forwards two `None`s (`widget.rs:240-247`).

**What the contract says.** SPEC §10 M14 clause and brief §2b: additive; “none changes an existing signature's meaning.” The widget sibling is the additive pattern; `generate` itself is a Rust-breaking change. The only in-tree caller was updated. Output of `None, None` is the old stream.

**Concrete fix.** Optional: `generate(...)` stays the four-argument function; `generate_with_live` lives next to it the way `widget` already does. Not a behavior bug.

---

## Verified correct

1. **`FieldFlags` bits** (`form/field.rs:138-164`) match `constants/form_flags.h`: `kChoiceEdit = 1<<18`, `kChoiceMultiSelect = 1<<21`, `kTextDoNotScroll = 1<<23`. Neighbouring-bit tests (`1<<17` is combo, not edit; `1<<22` is do-not-spell-check, not do-not-scroll) are right. 1-based “Bit N” comments match the existing `is_combo` style.

2. **Horizontal tie-break** (`hit.rs:289-310`) is raw `x > word.x + word_width(...) * 0.5` with no epsilon. `word_width` includes `word.tail` (comb `fWordTail`, `cpvt_variabletext.cpp:645-657`). Vertical section/line searches use `geom::is_float_bigger` / `is_float_smaller` (0.0001). Asymmetry matches `SearchWordPlaceImpl` (`cpvt_section.cpp:413, 423`) vs `SearchWordPlace` (`:346-360`) / `CPVT_VariableText::SearchWordPlace` (`cpvt_variabletext.cpp:476-490`). Linear scan ≡ the C++ binary search on increasing `fWordX`.

3. **Acceptance geometry.** Embeddertest `InsertTextInPopulatedTextFieldMiddle` (`fpdf_formfill_embeddertest.cpp:2232-2241`) clicks `RegularFormAtX(134.0)` at `kRegularFormY = 115` (`:271`) on **`text_form_multiple.pdf`** (`:210-216`), expects `ABCDHelloEFGH`. Unit test `the_embeddertests_middle_click_lands_after_four_characters` reproduces that through real Helvetica 12. Neighbour clicks at 102 and 166, exact-midpoint strictness, comb-cell midpoints, and the explicit `offset` all match the status doc. Brief U2 is closed. `SearchLineWord` does not exist.

4. **`offset` sign.** `vertical_offset` is `(0, (content_h - plate_h) * 0.5)` = `−fPadding`. `place_at_point` undoes it on the way in; `set_text` adds it on the way out. Matches `EditToVT`/`VTToEdit` (`cpwl_edit_impl.cpp:1105-1129`) with scroll at the plate seed.

5. **`LiveState::scroll` is the delta from the seed.** `shift()` negates both components (`field_body.rs:240-244`). `scrolling_shifts_the_drawn_text_by_the_scroll_offset` asserts Td moves by `−scroll`. Matches `VTToEdit` subtracting `(scroll_pos_point_ − (plate.left, plate.top))`. `(0, 0)` is unscrolled and needs no plate. Push buttons pass `(0, 0)` outright. List-box bands move with their rows.

6. **Caret 0.4 after the text, selection rgb(0, 51, 113).** `CARET_WIDTH = 0.4`; caret `re f` after `ET`; bands before `BT` with `0 0.2 0.443137 rg` (51/255, 113/255). A live selection suppresses the caret (brief §1.20.5 rule 7; `SetCaret` `cpwl_edit.cpp:704-706`). Finding 2 is only the unselected-glyph color.

7. **Byte-identity test is not tautological.** `none_is_byte_identical_on_every_existing_widget_fixture` (`field_body.rs:1690-1711` at `6da06a3`) `include_str!`s `tests/data/unfocused_field_bodies.txt` (captured in `5b4af15`, unchanged through `6da06a3` and HEAD) and compares `generate(..., None, None)` against that snapshot — eight fixtures, 1048 bytes, including the two `<none>` rows. Parent `wrap_text` (`e172d63^:386-399`) is the `overlay == None` branch of the new `wrap_text` by construction. `widget::generate_with_text` still has its old signature. I did not recompile `e172d63^` to prove the snapshot was taken from the parent binary rather than from a post-change `None` call; either capture would match.

8. **`overlay_with` merge key.** Both generated and supplied overlays are addressed by `list.source_indices[slot]` (raw `/Annots` index). `Appearance::{Untouched, Generated, Suppressed}` is the three-state the merge needs. `overlay` is `overlay_with(..., None)`. `set`/`get` still hide `Suppressed` as `None`; `appearance()` distinguishes them. Focus survives a merge past the overlay's end; an *entry* past the end is dropped.

9. **Focus / tint.** `push_chrome` (`annot_render.rs:240-258`): if `focus.annot == index`, append `focus_rect` (often nothing) and return; else `highlight`. Matches `CFFL_InteractiveFormFiller::OnDraw` (`cffl_interactiveformfiller.cpp:68-94`) given `KillFocusForAnnot` → `EscapeFiller` sets `valid_ = false` (`cffl_formfield.cpp:629-631`), so a previously focused widget returns to the tint branch. `FocusBox::{None, Inflated, Rect}` matches Edit/Combo empty, Wnd inflate-1, ListBox caret-item ∩ client, PushButton deflate. `focus_rect` strokes opaque black, width 1, dash `[1.0]`, phase 0, `FillRule::None` (`cfx_drawutils.cpp:16-39`, `cfx_graphstatedata.h:52-55`). Page-box `Contains` (`cffl_formfield.cpp:487-488`) is deliberately not applied here.

10. **Additive on the frozen callers.** `annot_render::overlay`, `widget::generate_with_text`, `FieldFlags` existing predicates, `vt` layout — signatures and `None` output unchanged. `#![forbid(unsafe_code)]`. Hit-test uses `get` / saturating arithmetic; no library `unwrap` on input. `Place` is a record plus free functions, not a widget object.

11. **M14-doc claims that hold.** Tie-break transcription, comb tail, `offset` correction to brief §3.2, U2 closed, `SearchLineWord` does not exist, fixture name `text_form_multiple.pdf`, LiveState shape, focus-table vs the brief's GetViewBBox mistake, golden not a None-vs-None tautology.

---

## Could not verify

- Parent-binary capture of `unfocused_field_bodies.txt` (would need `git checkout` of `e172d63^`, which this review is forbidden to do). Structural identity of the `None` path is verified; the bytes were not regenerated from that tree.
- Pixel identity of filled-caret vs stroked-caret, and of all-white partial selection, against `form_textfield_focused_*` goldens. No focused `Highlight` is supplied from this crate at `6da06a3` (producer is `pdfrum-form`; `13959c3` later sets focus — outside this slice).
- Page-box containment of a hanging `FocusBox::Rect`. Unimplemented by design; no corpus file exercises it.
- `GetLineLeading != 0`. Engine documents it as always zero (`vt/mod.rs:10-13`). The C++ first-line `fTop` subtracts it (`cpvt_section.cpp:343-344`); a non-zero leading would need a matching subtract in `find_line`.
- Conformance scoreboard / the eighteen `form-events` rows. Status doc's 1636/69 split was not re-run.
- Uncommitted `field.rs` (`/I`-first interaction reader) and any metric/selection work landing in the working tree during the review.
- C++ oracle behavior beyond what the sources say — tree is read-only, not built.

---

## Status-doc vs code (short)

`docs/status/M14-doc.md` is accurate on the tie-break, the `offset` argument, comb tails, caret width, selection rgb, LiveState scroll-as-delta, focus-vs-tint control flow, and the golden not being a tautology. Two overstatements:

- §2's table lists the round-trip test as pinning brief §4.4 P6 “over six texts.” The test skips every later-line header, which is exactly the `PrevLineHeaderPlace` case finding 1 is about.
- “Nothing existing changed shape” (`M14-doc.md` lead). `field_body::generate`'s public signature grew two arguments (finding 6). Frozen callers (`generate_with_text`, `overlay`) did not.

