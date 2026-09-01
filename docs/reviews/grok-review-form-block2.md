<!-- Durable copy of the M14 review record. Source: session
     ede35de1-5746-41fa-ab05-706565ff2804 scratchpad/grok-review-form-block2.md, 2026-09-01. Verbatim. -->

# Grok Build review: M14 block 2 (pdfrum-form / facade / tool)

Cross-vendor (non-Claude) reading review of committed `e530b1f..HEAD` restricted to
`crates/pdfrum-form`, `crates/pdfrum`, `crates/pdfrum-tool`. Report-only; no tree
edits; no checkout/restore/stash. C++ oracle at `/mnt/data2/pdfium/pdfium-c++`
read only.

**HEAD reviewed:** `10d3f7bfcc7841b8a0a4d49a683e3d214c8e010a` (`main`)
**Range:** 19 commits (`e6be169` … `f22414b`). Working-tree edits by other agents
were ignored; every citation is `git show HEAD:path`.

**Contract:** PLAN.md §M14 + “Rulings 2026-09-01”; SPEC.md §15; docs/design/pdfrum-form.md
§1 / §2 D1–D14 / §3.5; STYLE.md.

**nextest:** `cargo nextest run -p pdfrum-form` — 268 passed, 0 failed (2.3s compile + 0.093s run).
Facade tests in `crates/pdfrum` were **not** run (dispatch allowed `-p pdfrum-form` only).

**Verdict:** three blockers. Do not treat this slice as oracle-faithful until Tab
order, list-box click coordinates, and `/I` selection are fixed and the inverted
ported assertion is un-inverted.

---

## Findings

### 1. blocker — Tab ignores page `/Tabs`; always Structure

**Where:** `crates/pdfrum-form/src/route.rs:897-905`

**What the code does:** `focus_ring` filters subtypes then always calls
`tab::FocusRing::build(&focusables, tab::TabOrder::Structure)`. `PageForm`
never stores `/Tabs`. The ring builder and `TabOrder::from_tabs` in
`crates/pdfrum-form/src/tab.rs:110-117` are unused on the live path.

**What C++ / contract say:** `CPDFSDK_AnnotIterator::GetTabOrder`
(`cpdfsdk_annotiterator.cpp:111-122`) reads the page dict’s `/Tabs` (`R` → row,
`C` → column, else structure). `CPDFSDK_PageView::OnKeyDown`
(`cpdfsdk_pageview.cpp:533-548`) with nothing focused uses
`GetFirstFocusableAnnot` / `GetLastFocusableAnnot` from that iterator.

`annotiter.pdf` page 0 is `/Tabs /R`, page 1 `/C`, page 2 `/S`. Upstream
`TEST_F(FPDFFormFillEmbedderTest, FormFillFirstTab)`
(`fpdf_formfill_embeddertest.cpp:788-801`) asserts first Tab lands on **annot
index 1**. `FormFillFirstShiftTab` (`:804-818`) asserts Shift+Tab lands on
**index 0**. The crate’s own unit tests already encode that
(`crates/pdfrum-form/tests/tab_order.rs:44-56`: row ring `first() == 1`,
`last() == 0`; continuous visit `1,2,3,0`).

The facade port (`crates/pdfrum/tests/form_tab.rs:41-50`) only asserts
`focused_annot().is_some()`, so Structure-order landing on annot 0 stays green.

**Fix:** store `TabOrder` on `PageForm` in `page::read` via
`TabOrder::from_tabs(page_dict.byte_string(/Tabs))`. Pass that into
`FocusRing::build`. Pin `form_tab.rs` `FormFillFirstTab` / `FormFillFirstShiftTab`
to annot indices 1 and 0 on page 0, and assert the four-Tab walk is `1,2,3,0`.

---

### 2. blocker — `/I` vs `/V`: interaction uses the appearance reader, and the ported test asserts the wrong rows

**Where:** `crates/pdfrum-form/src/page.rs:122-124` (`WidgetInfo::selected` →
`ap::field_body::selected_indices`); `crates/pdfrum/tests/form_listbox.rs:69-99`.

**What the code does:** session `ChoiceState` is seeded from
`selected_indices`. At this HEAD that function is `/V` first, `/I` only when
`/V` is absent, and `/I` compared as **text**, so an integer index matches
nothing (`git show HEAD:crates/pdfrum-doc/src/ap/field_body.rs` around the
`selected_indices` doc: “`/V` decides when it is present”).

`form_listbox.rs::an_index_array_alone_currently_selects_nothing` names
`CheckIfMultipleSelectedIndices` and **asserts the empty set**, documenting the
inversion as a pdfrum-doc problem.

**What C++ / contract say:** `CPDF_FormField::IsItemSelected`
(`cpdf_formfield.cpp:546-554`) consults `/I` first when
`use_selected_indices_` is set by `UseSelectedIndicesObject` (`:863-935`):
usable if there is no `/V`, or `/I` and `/V` have the same count, every index
in range, and the multiset of option **values** those indices name equals `/V`.
`FORM_IsIndexSelected` is that function.

`TEST_F(FPDFFormFillListBoxFormEmbedderTest, CheckIfMultipleSelectedIndices)`
(`fpdf_formfill_embeddertest.cpp:3175-3182`) expects rows **1 and 3**.
`CheckIfMultipleSelectedValues` (`:3185-3192`) expects **2 and 4**. The latter
Rust port (`form_listbox.rs:53-68`) only checks `rows.len() >= 2`.

Mismatch (`:3195-3202`) correctly expects 0 and 2 — both rules agree there.

**Fix:** do not reuse the appearance-generation reader for session state.
Implement `IsItemSelected` / `UseSelectedIndicesObject` in the form crate (or
call a reader that is `/I`-first with the usable test). Change
`an_index_array_alone_currently_selects_nothing` to assert `[1, 3]`. Tighten
the `/V`-only row to `[2, 4]`. Fixture constants in `form_listbox.rs:21-28`
already match `kFormBeginX` / `kMultiFormMultiple*YFirstVisibleOption`
(`fpdf_formfill_embeddertest.cpp:588-598`).

---

### 3. blocker — list-box click row is computed in mixed coordinate spaces

**Where:** `crates/pdfrum-form/src/route.rs:738-765` (`row_at`)

**What the code does:**
```
let client = ap::field_body::client_rect(...);  // appearance space, origin at widget
let offset = (((client.y1 as f32) - at.y) / height) as usize;  // at is page space
```
`client_rect` (`pdfrum-doc` `field_body.rs:291-292`) is
`deflate(rotated_rect, border)` — `rotated_rect` (`widget.rs:457-468`) is
`(0,0,w,h)` after `/MK /R`. `at` is PDF user space. For a list whose box sits
at y≈371, `client.y1 - at.y` is largely negative; the `cast_sign_loss` expect
turns that into a huge `usize`; `checked_add` / range check then returns
`None`. The click still **focuses** the widget (`mouse_down` focuses before
`choice_click`), so `a_single_select_list_selects_the_row_clicked` can pass on
the file’s pre-existing single selection without the click ever choosing a row.

**What C++ says:** `CFFL_FormField::OnLButtonDown` (`cffl_formfield.cpp:103`)
passes `FFLtoPWL(point)` into the list. `GetCurMatrix` for 0° is translation
by `(left, bottom)` (`:442-464`); inverse is subtract origin, still y-up —
the same space `client_rect` lives in.

**Fix:** convert `at` with the same origin subtract already in `to_plate`
(`route.rs:1219-1221`) before subtracting from `client.y1`. Then the existing
`choice_click` shift/ctrl/plain table is reachable. Add an assertion that
clicking the first visible row of the single-select list selects **that**
index, not merely `len()==1`.

---

### 4. should-fix — live `to_plate` drops rotation; `geom::Plate::to_plate` is unused

**Where:** `crates/pdfrum-form/src/route.rs:1219-1221`;
`crates/pdfrum-form/src/geom.rs:126-150`.

**What the code does:** routing’s `to_plate` is `Point::new(at.x - origin.x0,
at.y - origin.y0)`. `geom::Plate::to_plate` implements the four-quadrant
map plus y-down flip and is only reached from `geom` unit tests and
`never_panics.rs`.

**What C++ / contract say:** every mouse entry applies `FFLtoPWL`
(`cffl_formfield.cpp:499-500` = inverse of `GetCurMatrix` `:442-464`):

| rot | page → PWL |
|---|---|
| 0° | `(x-left, y-bottom)` |
| 90° | `(y-bottom, W-(x-left))` |
| 180° | `(W-(x-left), H-(y-bottom))` |
| 270° | `(H-(y-bottom), x-left)` |

Brief D3 / §1.14: one affine, computed once. Design data-flow step 4 is
`geom::page_to_plate(field).transform(at)`.

For **0°**, origin-subtract is the C++ PWL point. `vt::hit::place_at_point`
(`pdfrum-doc/src/vt/hit.rs:179-181`) then does `y = plate.top - point.y`, so
text caret placement on unrotated widgets can still be right. `/MK /R` 90/180/270
sends the click into the wrong plate axis. Wiring `geom::Plate::to_plate` **and**
`place_at_point` would double-flip y — use one or the other.

**Fix:** keep routing’s PWL-space (y-up, origin at widget) convention, since that
is what `client_rect` / `place_at_point` consume, and apply the `GetCurMatrix`
inverse including rotation. Leave `geom.rs` as the y-down plate map only if a
caller actually wants y-down, or delete the dead module so D3 has one home.

---

### 5. should-fix — `focus_of` FocusBox table mismatches C++ on combo and push-button

**Where:** `crates/pdfrum-form/src/route.rs:1361-1385`

| kind | code | C++ | match |
|---|---|---|---|
| text | `FocusBox::None` | `CPWL_Edit::GetFocusRect` empty (`cpwl_edit.cpp:313-315`) | yes |
| **any combo** | `None` only if `editable`; else `Inflated` | `CPWL_ComboBox::GetFocusRect` empty **always** (`cpwl_combo_box.cpp:321-323`) | **no** (gated combo) |
| multi-select list | `caret_row_box` = caret item ∩ client, origin added back (`:1389-1435`) | `CPWL_ListBox::GetFocusRect` (`cpwl_list_box.cpp:227-234`) then `PWLtoFFL` | yes for 0° |
| single-select list / check / radio | `Inflated` | `CPWL_Wnd::GetFocusRect` Inflate(1,1) (`cpwl_wnd.cpp:713-719`) | yes |
| **push button** | `Inflated` (folded into Toggle/Button arm `:1384`) | `CPWL_PushButton::GetFocusRect` **deflate by border** (`cpwl_special_button.cpp:21-24`) | **no** |

`pdfrum-doc` `annot_render.rs` (committed table at the `focus_rect` docs) already
names combo → none and push-button → `FocusBox::Rect` (deflated). `focus_of`
does not follow that table.

Tint skip **is** wired: `FormSession::focus_for_page` → `focus_of` →
`pdfrum-tool` `session_overlay` sets overlay focus → `push_chrome` returns
before `DrawShadow` when `focus.annot == index`
(`cffl_interactiveformfiller.cpp:59-95`: live valid control skips tint; after
`KillFocusForAnnot` `valid_` is false so unfocused widgets tint again).
Focused-only skip matches the practical C++ lifetime.

**Fix:** `Choice` with `config.combo` → `FocusBox::None` regardless of
`editable`. `FieldState::Button` → `FocusBox::Rect` of the window deflated by
the widget border (same numbers `widget_border` already reads). Keep
`Inflated` for check/radio and single-select **list**.

---

### 6. should-fix — list-box wheel drops Shift/Ctrl; combo is treated as a list

**Where:** `crates/pdfrum-form/src/route.rs:282-316`, `807-834`

**What the code does:** any `Choice` under the pointer, focused or not, gets
`scroll_choice` → `move_selection(±1)` then `scroll_into_view`. Modifiers on
the `Event::MouseWheel` are discarded at `apply` (`:114`). Combo and list share
the arm.

**What C++ says:** `CPWL_ListBox::OnMouseWheel` (`cpwl_list_box.cpp:357-368`)
calls `OnVK_DOWN` / `OnVK_UP` with `IsSHIFTKeyDown` / `IsCTRLKeyDown`. Those
flags change multi-select behaviour (`cpwl_list_ctrl.cpp:242-255`: Ctrl no-op
on the index, Shift range-select, else single-select). Combo has **no**
`OnMouseWheel` override; `CPWL_Wnd::OnMouseWheel` (`cpwl_wnd.cpp:412-429`)
returns false unless a child holds keyboard capture (closed combo: typically
nothing moves).

The “moves the selection, not the view” rule for **list boxes** is otherwise
right, including `scroll_into_view` only when the caret would leave the box.

**Fix:** pass wheel modifiers into `move_selection` / a VK-shaped helper.
Do not call `scroll_choice` for `config.combo` (consume or ignore the way
`CPWL_Wnd` does).

---

### 7. should-fix — `force_kill_focus` does not go through `route::kill_focus`

**Where:** `crates/pdfrum/src/form_session.rs:296-313`

**What the code does:** calls `focus::kill` and returns a single
`UpdateKind::FocusChanged`. No `Regenerated` / `LiveEdit` appearance.

**What the contract says:** SPEC §15.6 / brief §3.3 step 6: dropping focus
commits the outgoing field and switches it from live editor state to a
generated stream. `route::kill_focus` (`route.rs:943-958`) does that redraw.
A left-click miss uses it; the public `FORM_ForceToKillFocus` analogue does
not.

The doctest only checks `updates` non-empty, which `FocusChanged` satisfies.

**Fix:** assemble a `Context` for the focused page (already in `pages` /
`read_page`) and call `route`’s kill path, or share that helper.

---

### 8. should-fix — several TEST_F ports drop the fixture constants they name

Named-after-TEST_F is the slice contract. Gaps:

- `FormFillFirstTab` / `FormFillFirstShiftTab` — see finding 1. Unit tests in
  `tab_order.rs` have the constants; `form_tab.rs` does not.
- `CheckIfMultipleSelectedValues` — C++ expects indices 2 and 4; Rust only
  `len() >= 2` (`form_listbox.rs:53-68`).
- `CheckIfMultipleSelectedIndices` — inverted expected set (finding 2).
- `DoubleClickInTextField` (`fpdf_formfill_embeddertest.cpp:2765-2778`) —
  constants and “Hello World” match; behaviour is SelectAll vs line (finding 9).

Correctly pinned: combo `BEGIN_X/END_X/EDITABLE_Y/NON_EDITABLE_Y/READ_ONLY_Y`
(`form_choice.rs:27-33` vs embeddertest `:274-385`); list-box Y constants
(`form_listbox.rs:21-28` vs `:588-598`); drag `FORM_BEGIN_X=102`,
`FORM_END_X=195`, `FORM_Y=115` (`form_routing.rs:198-201`); action modifiers
`[0,2,1,3]`; `ButtonActionInvokeTest` crbug.com/1028991 reproduced
(`form_actions.rs:157-175`); pwl_edit fifty-character run and signed
selection table; special-button EnterOn{Check,Radio}{,ReadOnly}.

**Fix:** assert the C++ expected annot / row indices in the facade tests that
claim those TEST_F names.

---

### 9. nit — double-click is `SelectAll` upstream, `select_line_at` here

**Where:** `crates/pdfrum-form/src/route.rs:253-272`; `edit/ops.rs:714-728`

C++ `CPWL_Edit::OnLButtonDblClk` (`cpwl_edit.cpp:637-644`) calls
`edit_impl_->SelectAll()` after `ClientHitTest`. The embeddertest comment
says “entire line”; the body is whole-field select. On a single-line field
(the ported “Hello World” case) they coincide. A multiline field would
diverge.

**Fix:** call `select_all` (already a text action) instead of `select_line_at`.

---

### 10. nit — SPEC §15.1 still says `apply` has not landed

**Where:** SPEC.md §15.1 (HEAD). This range **did** land
`pdfrum_form::apply` + `Context` (`route.rs:54-117`) and 80c1164 updated §15.8
(`set_page_in_view`, `selected_text`, `replace_selection`) but left §15.1
claiming “Neither `apply` nor `FormContext` has landed” and “currently report
every event unconsumed”.

**Fix:** rewrite §15.1 to the actual `apply(session, ctx, event) -> Response`
(cascade still not threaded; that remaining gap can stay named).

---

### 11. nit — `clear_siblings` is a no-op

**Where:** `crates/pdfrum-form/src/route.rs:972-990`

Radio activation comments that sibling widgets of the same field go Off, then
`let _ = (session, field);`. One shared `ToggleState` per `FieldId` cannot
express per-widget `/AS`. Observable once a radio group has two kids.

Not in the requested checklist; recorded so it is not mistaken for a port.

---

## Verified correct

- **Tab with nothing focused + `set_page_in_view`.** Facade
  `dispatch_keyboard` (`form_session.rs:534-557`) is the one keyboard path that
  may run with `focus == None`, and only for `Key::TAB`, routed to
  `page_in_view` (default 0). `pdfrum-tool` `dispatch.rs:72` sets it per replay
  page. Matches `FORM_OnKeyDown` taking the page in view
  (`fpdf_formfill.cpp:558-564`). Test `a_tab_from_nothing_enters_the_ring_on_the_page_in_view`.
  (Order on that page is still finding 1.)

- **Drag.** `mouse_down` drops `caret_anchor`; `mouse_move` / `mouse_up` call
  `drag_to` **before** clearing the anchor (`route.rs:132-140`, `:205-213`).
  That is the `SelectTextWithMouse` shape (press, move, release). Facade
  constants 102/195/115 match. Either direction selects the same run.

- **List-box wheel (selection, not view), unrotated, no modifiers.**
  `scroll_choice` (`:807-834`) is `OnVK_DOWN`/`UP` ±1 plus
  `scroll_into_view` only on overshoot — `cpwl_list_box.cpp:357-368` +
  `cpwl_list_ctrl.cpp:263-265`. Text wheel is a separate scroll-offset path.

- **Tint skip for the focused widget.** `focus_of` always returns an index
  even when the box is `None`. Tool overlay carries that Focus.
  `push_chrome` skips `DrawShadow` for that index
  (`cffl_interactiveformfiller.cpp:68-83` live-control branch never reaches
  `:92-94`).

- **Text FocusBox empty; multi-select caret∩client (0°); single-select /
  check / radio Inflate(1,1).** See table in finding 5.

- **`UpdateKind::ActionRequested`.** Boxed action + modifiers; `Response::actions`
  order; link Return fires, Space/Shift/Ctrl do not; modifiers `0,2,1,3`
  (`update.rs:89-95`, `form_actions.rs:40-82`). D2: no callback.

- **Editable vs gated combo.** Typed text vs type-ahead (`form_choice.rs`);
  combo four-item undo group lives on `ChoiceState::edit`.

- **Read-only widgets** fail the hit-test gate and never take focus (combo,
  list, checkbox ports).

- **Tab modifiers** other than Shift refused (`route.rs:859-866` vs
  `cpdfsdk_pageview.cpp:538-541`). Ring does not wrap.

- **D1 / D2 / STYLE.** Session record + free functions; no callback table;
  `unwrap`/`expect`/`panic` on the file path are `unwrap_or` / `get()` (the
  one `panic!` in `field/text.rs:400` is inside `#[cfg(test)]`). `Context` is
  a borrowed view, not a god object.

- **0° text click → plate.** Origin subtract + `place_at_point` y-flip is the
  C++ 0° `FFLtoPWL` path. Commit `c5aa00a` is the right bugfix for unrotated
  widgets.

- **`pdfrum-form` nextest:** 268/268 passed at this HEAD.

---

## Could not verify

- Rotated-widget caret / list-row / focus-box geometry (no `/MK /R` fixture in
  this slice; finding 4 is from the C++ matrix, not a failing test).
- `crates/pdfrum` facade tests and `.evt` pixel goldens (not in the allowed
  nextest invocation; other agents mutate the tree).
- `GetFocusBox` MediaBox containment drop (`cffl_formfield.cpp:487-488`) —
  `annot_render` documents it as the rectangle producer’s job; `caret_row_box`
  does not clip to the page box.
- Combo popup open/close (not in this block).
- Whether a later uncommitted `pdfrum-doc` `/I`-first rewrite of
  `selected_indices` would turn finding 2’s inverted test red (working tree
  was not the review subject).
- C++ binary behaviour (oracle is read-only; file:line citations only).
