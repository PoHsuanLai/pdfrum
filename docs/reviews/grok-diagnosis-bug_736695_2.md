<!-- Durable copy of the M14 review record. Source: session
     ede35de1-5746-41fa-ab05-706565ff2804 scratchpad/grok-diagnosis-bug_736695_2.md, 2026-09-01. Verbatim. -->

# Diagnosis: `bug_736695_2.in#form-events` (M14)

Measurement-only. No source edits. 2026-09-01.
Row SSIM 0.979073, max_channel_diff 250, unmoved by today's form-interaction fixes.

This is **Residue 2 (the open combo list window)**, previously named only for `bug_1372651`. M14.md:1730–1732 left this row undiagnosed. It is the same mechanism, now measured.

---

## 1. What the `.evt` does, and what the golden shows

Fixture PDF (byte-identical for `_2/_3/_4`, golden key `54a8f12a521c255c`):
`/mnt/data2/pdfium/pdfium-c++/testing/resources/pixel/bug_736695_4.pdf`

One widget, obj 9 0:

| key | value |
|---|---|
| `/FT` | `/Ch` |
| `/Ff` | `393216` = Combo (`1<<17`) + Edit (`1<<18`) |
| `/Opt` | `Spain`, `Sweden` (UTF-16BE) |
| `/Rect` | `[165.7 315.9 315.7 330.1]` |
| `/T` | `(Country Box)` |
| `/V` `/I` `/AS` `/MK` | **absent** |

Page `/MediaBox [0 0 595 342]`. Page content paints a white `re B*` at `[163.1 313.1 155.3 19.9]` around the widget.

`bug_736695_2.evt` (sha8 `fe1feead`) — three lines, then stop:

1. `mousemove,312,324`
2. `mousedown,left,312,324`
3. `mouseup,left,312,324`

`(312,324)` is inside the drop button (right 13 units of `/Rect`: x `302.7–315.7`, y `315.9–330.1`). PNG: col 312, row `342-324=18`.

Oracle golden `conformance/goldens/54a8f12a521c255c/input.pdf.0.events-fe1feead.png` (MD5 `dc22ed3fd9688de025c00df81548a652`, 455 unique colours):

- combo focused (white fill, form-field tint gone, caret at the left of the empty edit)
- **dropdown open downward**, listing `Spain` then `Sweden` in a second window under the widget

Siblings, same PDF, different scripts:

| row | script | oracle result | pdfrum | SSIM |
|---|---|---|---|---|
| `_2` | open, leave open | popup visible | focused closed combo, **no list** | **0.979073 fail** |
| `_3` | open, click `(312,310)` (inside the popup, PDF y 310 < 315.9) | selects `Spain`, closes list, stays focused | miss → `kill_focus` → unfocused empty (MD5 = `_4`) | 0.997003 pass |
| `_4` | open, hover, click `(6,6)` off | dismiss, unfocused = plain | unfocused empty | 0.999998 pass |

`_3` passes the 0.99 floor without ever selecting: disagreement is only the 150×15 widget (`Spain`+navy band vs tinted empty). `_2` fails because the oracle's list covers a 151×29 band the closed combo does not.

---

## 2. Pixel diff (pdfrum `--send-events` vs golden `events-fe1feead`)

Page 595×342. Differing **1987 / 203490 (0.976%)**. Bbox **`(163,8)–(318,54)` (156×47)**. maxch **250**.

| band | diffs | maxch | what |
|---|---:|---:|---|
| rows 8–9, cols 163–318 | 312 | 19 | page-content stroke AA (`126` vs `127`). Same class as passing `_4`. |
| rows 11–25 (widget `/Rect`) | **124** | 154 | caret 24 px at cols 166–167 + drop-button AA. Focused white fill **agrees**. |
| rows 26–54, cols 165–315 | **1551 (78%)** | **250** | **the list window** |
| rows 55–end | 0 | — | — |

Popup-band modals:

- **pdfrum:** white `(255,255,255)` ×1246 (page), greys `(101,101,101)`/`(153,153,153)` ×151 (bottom edge of the content `re B*`)
- **oracle:** glyph darks `(24,24,24)`/`(29,29,29)` ×149 each (`Spain`/`Sweden`), LCD/hover greys `(230,230,230)`/`(225,225,225)` ×147, plus ClearType fringes `(248,255,255)`, `(255,249,199)`

Oracle `_2` vs plain: 3798 px, bbox `(165,11)–(315,54)` — events change the widget (untint) **and** paint the list. pdfrum `_2` vs pdfrum `_4`: 2265 px, bbox `(165,11)–(315,25)` — **widget only**. We take focus; we never grow a list.

Crops: `/tmp/bug736695_2/ora2_crop.png` (Spain/Sweden under the box) vs `ours2_crop.png` (closed focused combo).

---

## 3. Mechanism (C++ vs pdfrum)

**C++ — drop-button click opens a second PWL window, then FFLDraw paints it.**

1. `CPWL_CBButton::OnLButtonDown` (`fpdfsdk/pwl/cpwl_cbbutton.cpp:65–75`) captures and `GetParentWindow()->NotifyLButtonDown`.
2. `CPWL_ComboBox::NotifyLButtonDown` (`cpwl_combo_box.cpp:497–503`) `SetPopup(!is_popup_)` when the child is the button.
3. `SetPopup(true)` (`:325–377`): require `list_->GetContentRect().Height()>0`; `QueryWherePopup` (`cffl_interactiveformfiller.cpp:670–729`) with `fPopupMin=0` (count 2 ≯ 3), `kMaxListBoxHeight=140`. Here `fBottom=315.9` ≫ `fTop=11.9` → `bBottom=true`, window grows down by list height + 2×border (~29 device rows).
4. `RepositionChildWnd` (`cpwl_combo_box.cpp:238–283`) `list_->SetVisible(true)` and `Move`s it into the grown rect. `CreateListBox` (`:205–236`) is a `CPWL_CBListBox` with `kListboxHoverSel` + `kWindowVScroll`.
5. `CPWL_Wnd::DrawAppearance` (`cpwl_wnd.cpp:252–257`) draws this + children. `pdfium_test` composites it in `FPDF_FFLDraw`, **not** in the widget `/AP`. Kill-focus always `SetPopup(false)` (`:52–58`). List-item mouse-up selects, `SetSelectText`, closes (`:505–516`).

**pdfrum — focus only; no popup state; widget `/AP` cannot overflow `/Rect`.**

- `ChoiceState` (`crates/pdfrum-form/src/field/mod.rs:165–200`) has options/selection/`edit_text`. **No `is_popup`.** `pdfrum-form` has zero `SetPopup`/`popup_open`/`drop_button` hits.
- `route.rs:809–812`: *"A combo box's list is not drawn, so a click in the box selects nothing by row."* `choice_click` returns `None`; `mouse_down` still `take_focus` + `redraw` (`:179–204`). Clicking the button and clicking the edit are the same.
- `ap::field_body::combo_box` (`crates/pdfrum-doc/src/ap/field_body.rs:864–923`) emits one line + `shapes::drop_button` (`DROP_BUTTON_WIDTH=13`, `:121–122`). No list.
- Tool FFLDraw analogue (`crates/pdfrum-tool/src/render.rs:208–227`) overlays **widget** `GeneratedAp`s only. Those are placed on `/Rect`, so they cannot paint rows 26–54.
- Secondary, not SSIM-driving: `highlight_of` (`route.rs:1770–1776`) returns `None` unless `FieldState::Text`, so the focused empty editable combo has no caret (24 px at cols 166–167).

Design `pdfrum-form.md` §1.20.10 claimed `SetPopup` was ported. It is not.

Same named residue as `bug_1372651` (M14.md:777–785, 1510–1514, 1726–1728): that fixture is also "open and leave open", with a larger list (SSIM 0.9105). This row is the two-option instance.

---

## 4. Owning crate and fix proposal

**Owner: `pdfrum-form`** (missing `SetPopup` machine and drop-button routing). Paint of a window **outside `/Rect`** cannot be `field_body::combo_box`; it needs a second overlay in **`pdfrum-doc` `annot_render`** (same pattern as `push_open_popup` for note cards), invoked from the tool's session overlay.

Concrete fix, do not do it here:

1. Add `popup_open: bool` to `ChoiceState`. Toggle it on left-down in the right 13-unit strip (`cpwl_cbbutton.cpp:65` + `cpwl_combo_box.cpp:497`). Close on `kill_focus` (`:52–58`), on list-item mouse-up (`:505–516`), and on click-away.
2. When opening, run the `QueryWherePopup` clamp (`cffl_interactiveformfiller.cpp:670–729`): this fixture opens **down**, height = two row heights + 2×border ≈ 29 pt, origin just below `/Rect`.
3. While open, hit-test that popup rect (so `_3`'s `(312,310)` selects `Spain` instead of missing). Reuse `field::choice::select_only`.
4. Emit a second `GeneratedAp` (or overlay slot) whose bbox is the popup, body = `ap::field_body::list_box` of the two options, composited after the widget. Do **not** fold it into the widget `/AP` — that form is mapped onto `/Rect` and would scale, not overflow.
5. Optionally give a focused editable combo a caret through `highlight_of` (24 px here). After (4), LCD on the list glyphs is M14 ruling (i), out of scope.

---

## 5. Could not determine

- Exact item-height in PDF units (oracle popup is 29 device rows; `(ascent-descent)*12/1000` × 2 + border is consistent, not re-measured from `CPWL_ListCtrl::GetItemHeight`).
- Whether the list's `(230,230,230)` band is hover-sel (`kListboxHoverSel`) or ClearType around the labels; both are present in the unique-colour set.
- Whether a live AP with a bbox larger than `/Rect` would already overflow if we cheated — not tried; the annot matrix maps `/AP` onto `/Rect`, so it should not.
- Jobs tmp `/home/r13921098/.claude/jobs/89471cbc/tmp/` was permission-denied (mode 0775, uid 1002). Scratch PNGs went to `/tmp/bug736695_2/`.
- Did not re-run the conformance harness; compared `pdfrum-tool` / `pdfium_test` PNGs to the stored goldens. Oracle `_2` MD5 matched `events-fe1feead`.
