# Design brief — form interaction (M14)

Behavior source: `fpdfsdk/formfiller/` (the per-widget interaction state),
`fpdfsdk/pwl/` (the edit control and its siblings), `fpdfsdk/cpdfsdk_widget.cpp`,
`cpdfsdk_pageview.cpp`, `cpdfsdk_interactiveform.cpp`,
`cpdfsdk_formfillenvironment.cpp`, `fpdfsdk/fpdf_formfill.cpp`, and the public
surface `public/fpdf_formfill.h` + `public/fpdf_fwlevent.h`. The executable
event spec is `testing/pdfium_test/event.cc`. All C++ paths are relative to
`/mnt/data2/pdfium/pdfium-c++/`.

Shape contract: PLAN.md §M14 (binding), SPEC.md §10 (`pdfrum-doc`'s existing
AcroForm + appearance-generation contract, which this milestone extends but
does not rewrite). This brief proposes one `[spec]` change — a new crate — and
argues it in §2.

---

## 0. What this milestone is, and what already exists

Phase 1 drew a scope line at "no interactive widget UI" on the premise that it
"only matters with JS". The Phase 3 survey refuted that premise: **137 of the
139 tests in `fpdfsdk/fpdf_formfill_embeddertest.cpp` run with V8 compiled
out**, and `pdfium_test --send-events` drives 59 checked-in `.evt` scripts
against the same V8-off binary. Form interaction is a measurable feature with a
corpus, which is this phase's only admission criterion.

The work splits cleanly in two, and only one half is new:

- **Layout — already done.** `pdfrum-doc/src/vt/` is a complete port of
  `CPVT_VariableText`: section splitting, bidi, line breaking, comb cells,
  password substitution, auto-sizing, placement, and the `Td`/`Tf`/`Tj`
  emitter (`vt/edit_ap.rs`). `ap/field_body.rs` (1213 lines) is a working
  consumer that already reproduces `CPDFSDK_AppStream::SetAsTextField`,
  `SetAsComboBox` and `SetAsListBox`. SPEC §10's E1 revision of 2026-08-29
  established that `CPWL_EditImpl` is *a shell over* `CPVT_VariableText` and
  that the only thing the shell adds which a **generated appearance** can
  observe is a vertical alignment offset.
- **Editing — new, and this brief's subject.** What the shell adds that an
  *interacting user* can observe: a caret, a selection, an undo stack,
  character insertion and deletion, scroll offset, keyboard navigation, focus
  ownership, hit testing, and the commit path that turns edited text back into
  a field value and a regenerated appearance stream.

SPEC §10's standing sentence — "What stays declined, unchanged: porting
`CPWL_EditImpl` itself, and everything else in `fpdfsdk/pwl` — the editing
widgets, the caret, the scroll bars, the focus machinery. None of it is
reachable from a generated appearance" — was correct **for M6's scope** and is
exactly what M14 reopens, on PLAN.md §Phase 3's authority. The sentence's own
justification names the condition under which it lapses: it is scoped to what
a generated appearance can reach. An event API reaches all of it. §5.E1 records
this as the formal escalation.

### 0.1 The three coordinate spaces, named once

Every ordering claim below depends on knowing which space a number is in, so
the vocabulary is fixed here.

- **Device space** — pixels in the bitmap the embedder is drawing into,
  y-down, origin at the top-left of the `start_x, start_y, size_x, size_y`
  rectangle the caller passed to `FPDF_RenderPageBitmap`/`FPDF_FFLDraw`.
- **Page space** — PDF user space of the page, y-**up**, origin at the
  page's `/CropBox` (as normalized by the page's own box resolution).
  `FORM_On*` mouse entry points take `page_x`/`page_y` in **page space** as
  `double`s; `pdfium_test` converts from its `.evt` device coordinates before
  the call.
- **Plate space (vt-internal)** — y-**down** from the client rectangle's
  top-left, which is what `pdfrum-doc/src/vt/` already works in and what
  `Layout::to_pdf` flips back out of. The caret and selection geometry live
  here, and only cross into page space at the appearance-emission boundary.

The C++ adds a fourth, **PWL/window space**, which exists only because
`CPWL_Wnd` is a widget-toolkit window that wants its own origin.
`CFFL_FormField::PWLtoFFL`/`FFLtoPWL` convert between it and page space, and
the conversion is a pure affine that folds the widget's `/Rect` origin and its
`/MK /R` rotation. Divergence D3 erases that space: pdfrum keeps page space and
plate space and computes the rotation into the plate transform once.

---

## 1. Behavior inventory

### 1.1 The public event surface — `public/fpdf_formfill.h`

This is the API the 137 embeddertests and all 59 `.evt` scripts drive, so it
is the contract the Rust facade must be able to express. Every entry takes a
`FPDF_FORMHANDLE` (the form-fill environment) and, except the document-level
ones, an `FPDF_PAGE`.

| API | `fpdf_formfill.h:LINE` | Arguments beyond handle/page | Space | Returns |
|---|---|---|---|---|
| `FORM_OnMouseMove` | :1202 | `int modifier, double page_x, double page_y` | page | `FPDF_BOOL` |
| `FORM_OnMouseWheel` | :1231 | `int modifier, const FS_POINTF* page_coord, int delta_x, int delta_y` | page | `FPDF_BOOL` |
| `FORM_OnFocus` | :1254 | `int modifier, double page_x, double page_y` | page | true **iff** an annot is at the point *and* it took focus |
| `FORM_OnLButtonDown` | :1274 | `int modifier, double page_x, double page_y` | page | `FPDF_BOOL` |
| `FORM_OnRButtonDown` | :1285 | same | page | no effect outside XFA builds (its own comment, :1282) |
| `FORM_OnLButtonUp` | :1302 | same | page | `FPDF_BOOL` |
| `FORM_OnRButtonUp` | :1313 | same | page | no effect outside XFA builds |
| `FORM_OnLButtonDoubleClick` | :1333 | same | page | `FPDF_BOOL` |
| `FORM_OnKeyDown` | :1352 | `int nKeyCode, int modifier` | — | `FPDF_BOOL` |
| `FORM_OnKeyUp` | :1372 | `int nKeyCode, int modifier` | — | **"Currently unimplemented and always returns false"** (:1368-1370) |
| `FORM_OnChar` | :1389 | `int nChar, int modifier` | — | `FPDF_BOOL` |
| `FORM_GetFocusedText` | :1409 | `void* buffer, unsigned long buflen` | — | byte length, UTF-16LE, **including the terminator** |
| `FORM_GetSelectedText` | :1430 | `void* buffer, unsigned long buflen` | — | same |
| `FORM_ReplaceAndKeepSelection` | :1451 | `FPDF_WIDESTRING wsText` | — | `void` |
| `FORM_ReplaceSelection` | :1470 | `FPDF_WIDESTRING wsText` | — | `void` |
| `FORM_SelectAllText` | :1484 | — | — | `FPDF_BOOL` |
| `FORM_CanUndo` / `FORM_CanRedo` | :1496 / :1508 | — | — | `FPDF_BOOL` |
| `FORM_Undo` / `FORM_Redo` | :1519 / :1530 | — | — | `FPDF_BOOL` |
| `FORM_ForceToKillFocus` | :1542 | *(no page)* | — | `FPDF_BOOL` |
| `FORM_GetFocusedAnnot` | :1565 | `int* page_index, FPDF_ANNOTATION* annot` | — | `FPDF_BOOL` |
| `FORM_SetFocusedAnnot` | :1582 | `FPDF_ANNOTATION annot` | — | `FPDF_BOOL` |
| `FORM_SetIndexSelected` | :1801 | `int index, FPDF_BOOL selected` | — | `FPDF_BOOL` |
| `FORM_IsIndexSelected` | :1824 | `int index` | — | `FPDF_BOOL` |
| `FORM_OnAfterLoadPage` | :1093 | — | — | `void` |
| `FORM_OnBeforeClosePage` | :1105 | — | — | `void` |
| `FORM_DoDocumentJSAction` | :1120 | *(no page)* | — | `void` |
| `FORM_DoDocumentOpenAction` | :1134 | *(no page)* | — | `void` |
| `FORM_DoDocumentAAction` | :1162 | `int aaType` | — | `void` |
| `FORM_DoPageAAction` | :1185 | `int aaType` | — | `void` |
| `FPDF_FFLDraw` | :1747 | bitmap + `start_x,start_y,size_x,size_y,rotate,flags` | device | `void` |
| `FPDF_GetFormType` | :1778 | *(document, not handle)* | — | `FORMTYPE_*` |
| `FPDFPage_HasFormFieldAtPoint` | :1636 | `double page_x, double page_y` | page | `FPDF_FORMFIELD_*`, `-1` on bad args |
| `FPDFPage_FormFieldZOrderAtPoint` | :1653 | `double page_x, double page_y` | page | z-order index, `-1` if none |
| `FPDF_SetFormFieldHighlightColor` | :1679 | `int fieldType, unsigned long color` | — | `void` |
| `FPDF_SetFormFieldHighlightAlpha` | :1696 | `unsigned char alpha` | — | `void` |
| `FPDF_RemoveFormFieldHighlight` | :1709 | — | — | `void` |

Three things in this table are contracts rather than trivia:

- **`FORM_OnLButtonUp`'s doc comment is wrong.** Lines :1298-1299 say
  "in device", while `FORM_OnLButtonDown` (:1270-1271), `DoubleClick`
  (:1327-1328) and `OnMouseMove` (:1196-1197) all say "in PDF user space".
  The implementation treats all of them identically — page space. This is an
  upstream comment bug, not a second coordinate convention; recorded so a
  future reader of the header does not "fix" our API to match it.
- **`FORM_OnKeyUp` is a permanent no-op that returns false.** It is not
  unimplemented-in-our-port; it is unimplemented upstream and documented as
  such. pdfrum reproduces the *return value*, and the `Event` enum below
  carries no `KeyUp` variant on that basis (§4.2).
- **`FORM_OnRButtonDown`/`Up` do nothing outside XFA builds**, per their own
  comments (:1282-1284, :1310-1312). XFA is declined by PLAN.md, so these two
  are `false`-returning no-ops in pdfrum by *derivation*, not by omission.

**Field-type constants** (`fpdf_formfill.h:1588-1595`), which
`FPDFPage_HasFormFieldAtPoint` returns and which key the highlight-colour
table:

```
FPDF_FORMFIELD_UNKNOWN      0
FPDF_FORMFIELD_PUSHBUTTON   1
FPDF_FORMFIELD_CHECKBOX     2
FPDF_FORMFIELD_RADIOBUTTON  3
FPDF_FORMFIELD_COMBOBOX     4
FPDF_FORMFIELD_LISTBOX      5
FPDF_FORMFIELD_TEXTFIELD    6
FPDF_FORMFIELD_SIGNATURE    7
```
`FPDF_FORMFIELD_COUNT` is **8** without XFA and 16 with (:1608 / :1610); the
eight XFA types (:1597-1603) are out of scope. `pdfrum-doc`'s existing
`FieldKind` (`form/field.rs:34`) is exactly this closed set minus `Unknown`,
which it expresses as `Option<FieldKind>` from `FieldKind::classify` — so no
new enum is needed for the field-type axis.

**Modifier flags** (`public/fpdf_fwlevent.h:19-28`) — the complete set, and
every one of them appears in either the embeddertests or the `.evt` grammar:

```
FWL_EVENTFLAG_ShiftKey          1 << 0   (0x01)
FWL_EVENTFLAG_ControlKey        1 << 1   (0x02)
FWL_EVENTFLAG_AltKey            1 << 2   (0x04)
FWL_EVENTFLAG_MetaKey           1 << 3   (0x08)
FWL_EVENTFLAG_KeyPad            1 << 4   (0x10)
FWL_EVENTFLAG_AutoRepeat        1 << 5   (0x20)
FWL_EVENTFLAG_LeftButtonDown    1 << 6   (0x40)
FWL_EVENTFLAG_MiddleButtonDown  1 << 7   (0x80)
FWL_EVENTFLAG_RightButtonDown   1 << 8   (0x100)
```

**Virtual key codes** (`fpdf_fwlevent.h:31-197`) are the Windows VK set
verbatim. The subset the form layer actually decides on — every other code
falls through to "not a navigation key" — is:

```
FWL_VKEY_Back      0x08     FWL_VKEY_Tab    0x09    FWL_VKEY_NewLine 0x0A
FWL_VKEY_Clear     0x0C     FWL_VKEY_Return 0x0D    FWL_VKEY_Escape  0x1B
FWL_VKEY_Space     0x20     FWL_VKEY_Prior  0x21    FWL_VKEY_Next    0x22
FWL_VKEY_End       0x23     FWL_VKEY_Home   0x24
FWL_VKEY_Left      0x25     FWL_VKEY_Up     0x26
FWL_VKEY_Right     0x27     FWL_VKEY_Down   0x28
FWL_VKEY_Insert    0x2D     FWL_VKEY_Delete 0x2E
FWL_VKEY_A 0x41 … FWL_VKEY_Z 0x5A          FWL_VKEY_Unknown 0
```
`FWL_VKEY_Prior`/`Next` are Page-Up/Page-Down. The letter codes matter because
the Ctrl-modified shortcuts (`Ctrl+A`, `Ctrl+Z`, `Ctrl+Y`, `Ctrl+X`, `Ctrl+C`,
`Ctrl+V`) are decided on the **`OnKeyDown`** path, not the `OnChar` path — see
§1.6, which is one of the two trickiest orderings in this milestone.

### 1.2 The `.evt` grammar — `testing/pdfium_test/event.cc`, verbatim

This 195-line file is the executable spec for the conformance corpus. It is
short enough to transcribe in full where it decides anything, and it decides
several things that a "reasonable" reimplementation would get wrong.

**The entry point** (`event.h:14-17`):

```cpp
void SendPageEvents(FPDF_FORMHANDLE form,
                    FPDF_PAGE page,
                    const std::string& events,
                    const std::function<void()>& idler);
```

The whole file is one `std::string`, replayed **once per page** — see 1.2.6.

**The dispatch chain** (`event.cc:163-195`), which is the grammar:

```cpp
  auto lines = StringSplit(events, '\n');
  for (const auto& line : lines) {
    auto command = StringSplit(line, '#');
    if (command[0].empty()) {
      continue;
    }
    auto tokens = StringSplit(command[0], ',');
    if (tokens[0] == "charcode") {          SendCharCodeEvent(form, page, tokens);
    } else if (tokens[0] == "keycode") {    SendKeyCodeEvent(form, page, tokens);
    } else if (tokens[0] == "mousedown") {  SendMouseDownEvent(form, page, tokens);
    } else if (tokens[0] == "mouseup") {    SendMouseUpEvent(form, page, tokens);
    } else if (tokens[0] == "mousedoubleclick") {
                                            SendMouseDoubleClickEvent(form, page, tokens);
    } else if (tokens[0] == "mousemove") {  SendMouseMoveEvent(form, page, tokens);
    } else if (tokens[0] == "mousewheel") { SendMouseWheelEvent(form, page, tokens);
    } else if (tokens[0] == "focus") {      SendFocusEvent(form, page, tokens);
    } else {
      fprintf(stderr, "Unrecognized event: %s\n", tokens[0].c_str());
    }
    idler();
  }
```

An if/else chain, not a table. The verb match is exact, case-sensitive,
byte-for-byte `==` on `tokens[0]`, with **no trimming**.

#### 1.2.1 The lexer, and why it matters

`StringSplit` (`testing/fx_string_testhelpers.cpp:27-47`) is a plain
find-and-substr split that **keeps empty fields and never returns an empty
vector**: splitting `""` yields `[""]`. So `command[0]` and `tokens[0]` are
always safe to index and the arity guards are the only thing protecting
`tokens[1..]`. There is no trimming, no quoting, and no escape processing at
any of the three levels.

The lex is three-deep: whole file split on `'\n'`; each line split on `'#'`;
`command[0]` (everything before the first `#`) split on `','`.

#### 1.2.2 Lines, comments, and the `'\r'` trap

- **The line delimiter is `'\n'` alone. `'\r'` is not stripped.** A CRLF file
  leaves `\r` glued to the last token: `mousemove,1,2\r` gives `tokens[2] ==
  "2\r"`, which `atoi` tolerates, but a verb-only line `focus\r` gives
  `tokens[0] == "focus\r"`, which matches nothing → "Unrecognized event".
  `.evt` files are LF, and a Rust parser that is lenient about `\r` is
  *diverging*, not being helpful (D8).
- **`#` starts a comment anywhere on a line**, not only at column zero — but
  only the text *before* it is kept, and **trailing whitespace survives**.
  `mousemove,1,2 # note` yields `"mousemove,1,2 "`, whose last token is `"2 "`
  (harmless to `atoi`); a trailing space after a bare verb would break the
  verb match. No corpus fixture has a trailing comment, so this is
  unexercised — but it is the grammar.
- **A blank or comment-only line `continue`s, which also skips `idler()`.**
  Blank lines are entirely inert; they do not pump the loop. A
  whitespace-only line is *not* empty, so it reaches the else branch,
  prints `Unrecognized event: ` and **does** call `idler()`.
- Because the final `substr` always appends, a file with no trailing newline
  still yields its last line; one with a trailing newline yields a final empty
  element that the empty check skips. **Both forms appear in the corpus** —
  eleven fixtures end without a newline.

#### 1.2.3 The eight verbs

Every number is parsed with `atoi`: optional leading whitespace, optional
sign, decimal digits, stop at the first non-digit, no error signalling,
`0` for anything unparseable, **leading zeros decimal not octal**
(`keycode,09` in `bug_1060549.evt` is 9). No float is ever parsed — even
`mousewheel`'s point is `atoi` then `static_cast<float>`.

| Verb | Syntax | Arity guard | Maps to |
|---|---|---|---|
| `charcode` | `charcode,<int>` | `size()!=2` → error | `FORM_OnChar(form, page, charcode, 0)` — **modifier hardcoded 0** |
| `keycode` | `keycode,<int>[,<modifiers>]` | `<2 \|\| >3` → error | `FORM_OnKeyDown(...)` **then** `FORM_OnKeyUp(...)`, same code + modifiers |
| `mousedown` | `mousedown,<left\|right>,<x>,<y>[,<modifiers>]` | dead, see 1.2.4 | `left` → `FORM_OnLButtonDown(form, page, modifiers, x, y)`; `right` → `FORM_OnRButtonDown`; else `mousedown: bad button name` |
| `mouseup` | `mouseup,<left\|right>,<x>,<y>[,<modifiers>]` | dead | `FORM_OnLButtonUp` / `FORM_OnRButtonUp` / bad button name |
| `mousedoubleclick` | `mousedoubleclick,left,<x>,<y>[,<modifiers>]` | dead | `FORM_OnLButtonDoubleClick`. **`right` is rejected** (`!= "left"` → error, return) |
| `mousemove` | `mousemove,<x>,<y>` | `size()!=3` → error | `FORM_OnMouseMove(form, page, 0, x, y)` — **no modifier field exists** |
| `mousewheel` | `mousewheel,<x>,<y>,<delta_x>,<delta_y>[,<modifiers>]` | dead | `FORM_OnMouseWheel(form, page, modifiers, &point, delta_x, delta_y)` |
| `focus` | `focus,<x>,<y>` | `size()!=3` → error | `FORM_OnFocus(form, page, 0, x, y)` — modifier hardcoded 0 |

An argument-order asymmetry a port must preserve: the `.evt` line orders mouse
arguments `(button, x, y, modifiers)`, while the `FORM_*` call takes
`(modifiers, x, y)`.

#### 1.2.4 The dead arity guard — reproduced, not fixed

The four mouse verbs use `&&` where `||` was meant (`event.cc:63-66`):

```cpp
  if (tokens.size() < 4 && tokens.size() > 5) {
    fprintf(stderr, "mousedown: bad args\n");
    return;
  }
```

`size() < 4 && size() > 5` is unsatisfiable; the guard never fires. Identical
at `:84` (`mouseup`), `:104` (`mousedoubleclick`), and `:135` (`mousewheel`,
`< 5 && > 6`). Consequence: `mousedown,left` reads `tokens[2]`/`tokens[3]`
**out of bounds** — undefined behavior in the oracle — while extra trailing
arguments are silently ignored. `charcode`, `keycode`, `mousemove` and `focus`
use correct guards and do reject.

pdfrum cannot reproduce UB and must not try. The port's rule (D8): a mouse
line with fewer than the required fields is **skipped with a `Diagnostic`**,
and extra trailing fields are ignored exactly as upstream ignores them. No
corpus fixture contains a short mouse line, so the choice is unobservable
against all 59 — which is precisely why it may be made on safety grounds.

#### 1.2.5 Modifiers — substring matching, three keywords

`GetModifiers` (`event.cc:19-32`) verbatim in behavior:

```cpp
  if (modifiers_string.find("shift")   != npos) modifiers |= FWL_EVENTFLAG_ShiftKey;
  if (modifiers_string.find("control") != npos) modifiers |= FWL_EVENTFLAG_ControlKey;
  if (modifiers_string.find("alt")     != npos) modifiers |= FWL_EVENTFLAG_AltKey;
```

The complete keyword set is **`shift`, `control`, `alt`**, lowercase, matched
by **substring search rather than equality or tokenization**. So the modifier
field is one comma-free token in which any of the three words may appear
anywhere: `shiftcontrol`, `shift-control` and `xxaltxx` all work; `Shift` does
not. There is no way to express Meta, KeyPad, AutoRepeat or the button-down
flags from a `.evt` file. Only `shift` occurs in the corpus, in three files.

#### 1.2.6 `idler()`, replay, and file discovery

- `idler()` runs **after every non-skipped line** (`event.cc:193`), including
  lines that errored. It is the caller's message-loop pump. In a V8-off build
  it has no observable effect on form state; it matters for M15's timers, and
  the `Event` enum in §4.2 therefore carries no `Idle` variant while the
  *stream* semantics ("a blank line does not pump") are recorded here for M15.
- **Discovery** (`pdfium_test.cc:2151-2170`): the input PDF's name has its
  **first occurrence of the substring `".pdf"`** replaced with `".evt"` —
  `find`, not a suffix test — then `access(R_OK)`. A path containing `.pdf`
  mid-string is rewritten in the wrong place. A missing, unreadable or empty
  event file is silently fine.
- **Replay site** (`pdfium_test.cc:1480-1482`, in `PdfProcessor::ProcessPage`):
  `SendPageEvents` runs at the **top of processing for every page**, before
  any image is saved. A multi-page document therefore replays the identical
  stream once per page with the same page-space coordinates each time. This is
  a real contract for the harness, not an implementation detail.
- The Python runner passes `--send-events` **unconditionally**
  (`testing/tools/test_runner.py:678-681` text, `:732-735` pixel), so a test is
  event-driven purely by the existence of a sibling `.evt`; it copies the
  `.evt` next to the generated PDF first (`:658-661`) because the C++ derives
  the path from the PDF's. Goldens use the **same naming as non-event runs** —
  `<base>_expected.pdf.<page>.png`, with `_expected_mac` / `_win` / `_skia*`
  variants — there is no event-specific convention. For a text test with no
  `_expected.txt`, the runner asserts the output is **empty**
  (`_VerifyEmptyText`, `test_runner.py:709-720`); those are real assertions,
  not unverified fixtures.

#### 1.2.7 The corpus: 59 files, of which 27 are ours

`find testing -name '*.evt'` gives **59**. **32 are under an `xfa` path** and
are out of scope by PLAN.md's XFA decline. The **27 in scope**:

| Location | Count | Kind |
|---|---|---|
| `testing/corpus/pdfium/annots/annotation_highlight_*.evt` | 6 | pixel; each is the single line `mousemove,128,713` — hover a highlight annotation to raise its popup |
| `testing/resources/javascript/{bug_1445426,bug_1447268,mouse_events,public_methods}.evt` | 4 | text; **JS-dependent → M15**, listed here because the streams themselves are M14's parser test |
| `testing/resources/pixel/*.evt` | 17 | pixel; the actual interaction corpus |

The 17 pixel fixtures, which are M14's conformance target:

| Fixture | Stream | What it pins |
|---|---|---|
| `bug_113910` | click (150,120), `charcode,49..52` ("1234"), `charcode,13` | typing digits then Enter |
| `bug_1372651` | move+click (140,145) | dropdown open |
| `bug_40643646` | `mousemove,20,250` alone | text-annotation popup on hover |
| `bug_477200528` | click (150,429) | list-box option select, focused render |
| `bug_736695_2` | open dropdown (312,324) | combo popup geometry |
| `bug_736695_3` | open dropdown, select (312,310) | combo item pick |
| `bug_736695_4` | open dropdown, hover item, click far off (6,6) | dismissal without selection |
| `checkbox_radiobutton` | check (145,220), 2× `keycode,9`, `charcode,13` | checkbox toggle, Tab order, radio via Return |
| `combobox_form` | click (102,410), Enter to open, Space | popup survives Space |
| `form_textfield_focused_ltr` | click (100,50), types `abc...` + space + `def` | **live `CPWL_EditImpl` render while focused** |
| `form_textfield_focused_rtl` | same, Hebrew `charcode,1489/1495/1512`, `1488/1490/1514` | bidi in the live editor |
| `form_textfield_selected_ltr` | type, then `mousedoubleclick,left,100,50` | **selection highlight geometry** |
| `form_textfield_selected_rtl` | same, RTL | selection highlight under bidi |
| `password` | "tigerssss" into two fields at (102,102) and (102,162) | password substitution live |
| `reset_button` | check a box, Tab to reset, `charcode,13` | form reset action |
| `scrollable_widgets1` | click (150,415) **with no mouseup**, then 25× `mousewheel,150,415,0,-1` | scroll offset in a multiline field |
| `scrollable_widgets2` | 25 down then 5× `mousewheel,150,415,0,1` | scroll back up, clamping |

`form_textfield_focused_ltr.evt`'s own header comment is the clearest
statement anywhere of the contract this milestone adds, and is quoted verbatim
because it is *the* dividing line between M6 and M14:

```
# This test explicitly tests the interactive UI rendering (CPWL_EditImpl).
# Must leave the text field focused at the end of the test. No clicking
# outside. If the field loses focus, it will fall back to generating a static
# Appearance Stream.
```

That is: **a focused field renders from live editor state; an unfocused field
renders from a generated appearance stream.** M6 built the second path in
full. M14 builds the first, and must reproduce the *switch* between them.

Corpus-derived coverage facts a port needs:

- Verb frequency: `mousedown`/`mouseup` everywhere; `charcode` and `mousemove`
  very common; `keycode` common; `mousedoubleclick` in 4 files;
  **`mousewheel` in exactly 2** (`scrollable_widgets1`/`2`); **`focus` in
  exactly 1** (`mouse_events`, which is JS/M15).
- Modifiers appear in 3 files, always `shift`, never `control` or `alt`; and
  the only in-scope-adjacent use of a 5-token mouse line is XFA's
  `dynamic_list_box_allow_multiple_selection.evt`. **No in-scope pixel fixture
  exercises a mouse modifier at all** — shift-click range selection is pinned
  by embeddertests, not by the `.evt` corpus.
- Unbalanced mouse events (a `mousedown` with no `mouseup`) are common and
  **intentional** — `scrollable_widgets1` depends on it. A parser that
  "repairs" them is wrong.
- No fixture uses negative coordinates or a non-numeric field.

### 1.3 The event model: ordering, routing, and return values

#### 1.3.1 The stack the events traverse

Four C++ layers sit between `FORM_OnLButtonDown` and a character landing in a
string. Named here because every ordering claim below refers to one of them,
and because §2 collapses all four:

| Layer | Class | Job |
|---|---|---|
| 1 | `CPDFSDK_FormFillEnvironment` | owns the focused annotation for the whole document; owns the embedder callback table |
| 2 | `CPDFSDK_PageView` | per page: hit-testing, mouse-enter/exit bookkeeping, routing an event to one annot |
| 3 | `CFFL_InteractiveFormFiller` + `CFFL_*` | per widget: creates and owns the interaction state, converts coordinates, runs the action cascade |
| 4 | `CPWL_*` (`CPWL_Edit` over `CPWL_EditImpl`) | the actual caret/selection/undo/scroll machine |

#### 1.3.2 The composite gestures the tests rely on

The embeddertests never call one entry point in isolation; they call fixed
sequences, and those sequences *are* the observable contract because a
different order produces different state. From
`fpdf_formfill_embeddertest.cpp:84-147`:

- **A click** (`ClickOnFormFieldAtPoint`, :84) is exactly three calls:
  `FORM_OnMouseMove(0, x, y)` → `FORM_OnLButtonDown(0, x, y)` →
  `FORM_OnLButtonUp(0, x, y)`. The mouse-move first is not decoration: it sets
  the "mouse is over this widget" state that the button-down path consults.
- **A double click** (`DoubleClickOnFormFieldAtPoint`, :91) is
  `FORM_OnMouseMove` → `FORM_OnLButtonDoubleClick` — with **no preceding
  single click**, and no button-up.
- **A mouse drag selection** (`SelectTextWithMouse`, :137) is
  `OnMouseMove(start)` → `OnLButtonDown(start)` → `OnMouseMove(end)` →
  `OnLButtonUp(end)`. It `DCHECK`s `start.y == end.y`, so every ported
  assertion is a same-line drag.
- **A keyboard shift-selection** (`SelectTextWithKeyboard`, :119) is
  `OnKeyDown(FWL_VKEY_Shift, 0)`, then n× `{OnKeyDown(arrow,
  FWL_EVENTFLAG_ShiftKey); OnKeyUp(arrow, FWL_EVENTFLAG_ShiftKey)}`, then
  `OnKeyUp(FWL_VKEY_Shift, 0)`. Note that `FORM_OnKeyUp` is the documented
  no-op — **the shift-key-down/up bracketing has no effect whatsoever**; the
  selection is driven entirely by the `FWL_EVENTFLAG_ShiftKey` bit on each
  arrow's `OnKeyDown`. A port that models a "shift is held" latch is
  reproducing a fiction.
- **Typing** (`TypeTextIntoTextField`, :97) asserts the field type at the
  point, clicks, then sends `FORM_OnChar('A' + i, 0)` per character. **No
  `OnKeyDown` accompanies a typed character.** Characters arrive on `OnChar`
  alone; `OnKeyDown` carries navigation and shortcuts alone. This split is the
  single most load-bearing fact in the event model (§1.6).

#### 1.3.3 Return values

The oracle's `FPDF_BOOL` returns mean "this event was consumed", not "this
event succeeded". The tests treat them as consumption:

- `FORM_OnKeyDown(Tab, 0)` returns **true** while there is a next focusable
  annotation and **false** when there is not (`FormFillContinuousTab`,
  :821-840: four tabs true, the fifth false). Tab does **not** wrap.
- `FORM_OnKeyDown(Tab, <any modifier other than bare-or-Shift>)` returns
  **false** — `TabWithModifiers` (:866-890) checks Control, Alt, Meta,
  Control|Shift, Alt|Shift, Meta|Shift: all six false.
- With no focused annotation, every non-Tab key returns **false** and creates
  no focus (`KeyPressWithNoFocusedAnnot`, :892-918, over
  `{NewLine, Return, Space, Delete, 0, 9, A, Z, F1}`).
- `FORM_OnFocus` returns true **iff** there is an annotation at the point and
  it took focus.
- A **read-only** widget still *consumes* the event: `CheckReadOnlyInCheckbox`
  (:1063-1096) has both `FORM_OnChar(kReturn)` and `FORM_OnChar(kSpace)`
  return **true** while `FPDFAnnot_IsChecked` stays unchanged. Consumption and
  effect are independent, and the port must return `true` here.

### 1.4 Focus semantics

Focus is owned **per document**, not per page: `CPDFSDK_FormFillEnvironment`
holds one `m_pFocusAnnot`. Its observable rules, each pinned by a test:

1. **Focus survives a right-click outside the field.** `FormText`
   (:1363-1413) renders `focused_text_form_with_abc`, right-clicks at
   (15, 15) far outside, renders again — and the golden is *still*
   `focused_text_form_with_abc` (:1397, comment :1393). Then a **left**-click
   at the same point renders `unfocused_text_form_with_abc` (:1406).
2. **Clicking a non-form point drops focus.** `FocusChanges` (:2803) uses
   `kNonFormPoint = (1, 1)`: after clicking it, `FORM_GetFocusedText` is `""`,
   and clicking it again is idempotent.
3. **`FORM_ForceToKillFocus` drops it** and commits the edit
   (`FocusChanges` step 15, :2803-2834; `Bug1302455Edit*Form`, :1447-1531,
   which call it before rendering so the field renders from its *generated*
   appearance).
4. **Each field keeps its own edit state across focus changes.**
   `FocusChanges` step 6 clicks back to the char-limit field and reads
   `"ABElephant"` — the text typed into it earlier, still there — while the
   regular field independently holds `"ABCDE"`.
5. **Moving the caret within the same field changes nothing.** Steps 10, 12
   and 13 re-click inside an already-focused field and the text is unchanged.
6. **Tab order is the annotation order the iterator produces**, and it does
   not wrap: `FormFillFirstTab` lands on annot index **1**, and
   `FormFillFirstShiftTab` lands on index **0** (:788-819) — the forward and
   backward "first tab" land on *different* annotations, because the cursor
   starts between them rather than before them. The forward sequence is
   1, 2, 3, 0 then stop; backward is 0, 3, 2, 1 then stop (:821-864).
7. **Focus change is reported to the embedder only under form-fill-info
   version 2 (or an XFA build).** `FocusAnnotationUpdateToEmbedder` (:2781)
   expects `OnFocusChange` **0 times** on a non-XFA v1 environment; the
   `…Version2` fixture (:2794) expects it **once**, unconditionally.
   pdfrum has no callback table (§2, D2) — this is recorded because it
   determines that "focus changed" must be an *observable in the returned
   update record*, not a callback.

### 1.5 The undo model — exact granularity

This is the sharpest behavioural contract in the milestone, and the
embeddertests pin it precisely enough that no C++ reading is needed to state
it (though §1.7 confirms it against `cpwl_edit_impl.cpp`):

| Operation | Undo items pushed |
|---|---|
| One typed character (`FORM_OnChar`) | **one item per character** |
| `FORM_ReplaceSelection(text)` — any length, including a 3-char paste | **exactly one item** |
| `FORM_ReplaceAndKeepSelection(text)` | **exactly one item** |
| `FORM_ReplaceSelection(nullptr)` (a delete/cut) | **exactly one item**, regardless of how many characters it removes |
| `FORM_SelectAllText` | **none** — selecting is not an undoable edit |
| Focus change to a different field | **the stack is cleared** |

Evidence, test by test:

- `UndoRedo` (text, :2910-2938): type `ABCDE`, undo → `ABCD`, undo → `ABC`,
  redo → `ABCD`, redo → `ABCDE`. **One character per step.** After the last
  redo `CanUndo` is true (three more characters remain) and `CanRedo` is
  false.
- `UndoRedo` (combo, :2967-3000): click the non-editable combo (focused text
  `"Banana"`), then click the *editable* combo — `CanUndo` and `CanRedo` are
  both **false**. The stack is per-field and does not survive a focus change.
  Then type `ABC`, undo three times to `""`, at which point **`CanUndo` is
  false** — the stack bottom is observable.
- `ContinuouslyReplaceAndKeepSelection` (:3285-3318): a single
  `FORM_ReplaceAndKeepSelection(L"UVW")` into an empty field, then **one**
  undo returns it to `""` and `CanUndo` is false. Three characters, one item.
- `CutAllTextUndoRestoresAllCharacters` (:3793-3820): type `A`, `B`, `C`
  (three items), select all, `FORM_ReplaceSelection(nullptr)` (one item), then
  **one** undo restores the whole `"ABC"`. The cut is atomic even though the
  text it removed was built from three separate items.
- `ReplaceSelection` (:3320-3373) counts the whole stack: type `A`, type `B`,
  replace-`A`-with-`XYZ` — exactly **three** undo items, walked to the bottom
  and back up.

#### 1.5.1 The selection asymmetry — undo restores it, redo does not

`ReplaceAndKeepSelection` (:3239-3283) is the test that isolates it, and the
C++ carries an explicit comment naming the reason ("The selection is not an
undo item"):

1. Type `AB`; shift-Right once → selection `"A"`.
2. `FORM_ReplaceAndKeepSelection("XYZ")` → text `"XYZB"`, **selection
   `"XYZ"`** — the inserted text stays selected. This is the whole difference
   from `FORM_ReplaceSelection`, which leaves selection `""` (:3320, step 4).
3. **Undo** → text `"AB"`, **selection `"A"`** — the pre-edit selection is
   restored.
4. **Redo** → text `"XYZB"`, **selection `""`** — the kept selection is *not*
   restored.

`RedoCutSelection` (:3412-3451) confirms the undo half on a select-all: after
`FORM_SelectAllText` + `FORM_ReplaceSelection(L"")`, one undo restores both the
text `"AB"` **and** the selection `"AB"`.

So an undo item stores the selection that existed **before** the edit, applies
it on undo, and on redo restores text only, leaving the caret collapsed. A
Rust design that stores "selection before and after" and symmetrically
restores both is *wrong* against four tests. This is D5.

#### 1.5.2 A new edit truncates the redo branch

`ReplaceAndKeepSelection` step 9 (:3281-3282): after an undo has made redo
available, performing a fresh `FORM_ReplaceAndKeepSelection` leaves `CanUndo`
true and **`CanRedo` false**. Standard linear-undo semantics, stated because
it must be pinned by a property test (§4.4).

### 1.6 The `OnChar` / `OnKeyDown` split, and the platform modifier

The oracle moved every editing shortcut from `OnChar` to `OnKeyDown`, and the
tests assert the *negative* half of that move as hard as the positive half.

**The modifier is platform-dependent** — `fpdf_formfill_embeddertest.cpp:39-43`:

```cpp
constexpr int kModifier = BUILDFLAG(IS_APPLE) ? FWL_EVENTFLAG_MetaKey
                                              : FWL_EVENTFLAG_ControlKey;
```

The tests call this `kCorrectModifier` and define `kWrongModifier` as the other
one, then assert the wrong one is **rejected**. So the accelerator modifier is
a *configuration input*, not a constant — D6 makes it an explicit field on the
session rather than a `cfg!(target_os)`.

| Gesture | Entry point | Result | Test |
|---|---|---|---|
| `Ctrl/Cmd + A` | `OnKeyDown(FWL_VKEY_A, kCorrectModifier)` | **true**, selects all | `SelectAllWithOnKeyDown` :3480 |
| `Ctrl/Cmd + A` | `OnKeyDown(FWL_VKEY_A, kWrongModifier)` | **false**, no selection | :3499-3512 |
| `Ctrl/Cmd + Shift + A` | `OnKeyDown` | **false** — explicitly not select-all | :3510-3513 |
| `Ctrl/Cmd + A` as a **char** | `OnChar(ascii::kControlA, kCorrectModifier)` | **false**, nothing selected | `DoNotHandleSelectAllOnChar` :3453 |
| `Ctrl/Cmd + Z` | `OnKeyDown(FWL_VKEY_Z, kCorrectModifier)` | **true**, undo one step | `UndoWithOnKeyDown` :3516 |
| `Ctrl/Cmd + Shift + Z` | `OnKeyDown` | **true**, redo | :3536-3546 |
| `Ctrl/Cmd + Z` wrong modifier | `OnKeyDown` | **false**, unchanged | :3552-3555 |
| `Ctrl + Y` **non-Apple** | `OnKeyDown(FWL_VKEY_Y, kModifier)` | **true**, redo | `RedoWithCtrlYKeyboardShortcut` :3572-3580 |
| `Cmd + Y` **Apple** | same | **false**, unchanged | same |
| `Ctrl/Cmd + Shift + Y` | `OnKeyDown` | **false** on both platforms | :3567-3570 |
| `Ctrl/Cmd + C` / `+V` / `+X` | `OnKeyDown` | **false** — clipboard is the embedder's job | `DoNotHandleShortcutsOnKeyDown` :920-953 |
| `Left`, `Right`, `Up`, `Home`, `Ctrl+Home` | `OnKeyDown` | **true** — navigation is handled | :936-942 |

`Ctrl+Y` is redo on non-Apple and *not* redo on Apple, while `Ctrl/Cmd+Shift+Z`
is redo on both. That asymmetry is a real, tested difference and is
reproduced (D6).

### 1.7 Per-field-kind state machines

The six writable field kinds behave differently enough that they are six
separate transition tables rather than one parameterized one. Each is stated
here as `(current state, event) → (new state, effects)`.

`FieldKind` here is `pdfrum-doc`'s existing enum (`form/field.rs:34`), which
already matches the oracle's classification (`FieldKind::classify`, :57,
reading `/FT` plus `/Ff` bits 17 push-button, 16 radio, 18 combo).

#### 1.7.1 Text field (`FieldKind::Text`)

The full editing machine. State: `text: String`, `caret: usize` (a char
index), `selection: Option<(usize, usize)>`, `scroll: f32`, plus the undo
stack.

| Event | Effect |
|---|---|
| `LButtonDown(p)` | take focus if not focused; set caret to the character index nearest `p`; **clear** selection; start a drag anchor |
| `MouseMove(p)` while a drag anchor is live | extend selection from anchor to `p`'s index |
| `LButtonUp(p)` | end the drag; selection is `anchor..index`, normalized |
| `LButtonDoubleClick(p)` | **select the entire line**, not the word under the cursor — `DoubleClickInTextField` (:2765-2779) inserts `"Hello World"` and asserts the double-click selection is the whole `"Hello World"` |

  > **Correction (2026-09-01, M14 block 3).** "The entire line" is the
  > embeddertest's own comment, and the body it comments on is
  > `edit_impl_->SelectAll()` (`cpwl_edit.cpp:636-644`) — the whole **field**,
  > not the line under the pointer. The two coincide on that fixture because
  > its field is single-line, which is why the comment survived. They part on
  > a multiline field, where selecting the line takes one of several. The port
  > calls `select_all`, gated on the same `ClientHitTest` upstream gates it on;
  > `acceptance_click.rs`'s
  > `a_double_click_on_a_multiline_field_takes_every_line` asserts the two
  > readings actually differ before pinning the one the oracle takes. This row
  > is left as written so the record of what was believed stays readable.
| `Char(c)` with a selection | replace the selection with `c`, collapse the caret after it, push **one** undo item |
| `Char(c)` with no selection | insert `c` at the caret, advance, push **one** undo item; refuse if `/MaxLen` is reached |
| `KeyDown(Left/Right)` no shift | move the caret one character, clear selection |
| `KeyDown(Left/Right)` + Shift | extend the selection by one character |
| `KeyDown(Home/End)` | caret to line start/end |
| `KeyDown(Delete)` | delete the selection, or the character *after* the caret |
| `Char(kBackspace)` | delete the selection, or the character *before* the caret |
| focus lost | commit the value into the field; regenerate the appearance |

**The `/MaxLen` (comb / char-limit) rules are truncation, not rejection**, and
the truncation point is the *insertion*, not the field:

- `InsertTextInEmptyCharLimitTextFieldOverflow` (:2577-2602): a field with
  `/MaxLen 10` and `/V "Elephant"`, cleared, then
  `FORM_ReplaceSelection(L"Hippopotamus")` (12 chars) yields
  **`"Hippopotam"`** — truncated at the tail.
- `InsertTextInPopulatedCharLimitTextFieldLeft` (:2629-2643): caret at the
  start of `"Elephant"` (8 chars), insert `"Hippopotamus"` → **`"HiElephant"`**.
  Only the 2 characters that fit are inserted; the existing text is
  untouched. Middle (:2645) gives `"ElephHiant"`, right (:2667) gives
  `"ElephantHi"`.
- With a selection, the replaced characters free up room:
  `…CharLimitTextFieldWhole` (:2685) selects all of `"Elephant"` and inserts
  `"Hippopotamus"` → `"Hippopotam"`; `…Left` (:2705) selects `"Elep"` →
  `"Hippophant"`; `…Middle` (:2725) selects `"epha"` → `"ElHippopnt"`;
  `…Right` (:2745) selects `"hant"` → `"ElepHippop"`.

Note also `SelectTextWithKeyboard(12, Left, …)` on an 8-character field
selects all 8 — arrow movement **clamps** rather than erroring.

#### 1.7.2 Check box (`FieldKind::Check`) and radio (`FieldKind::Radio`)

No text, no caret, no undo. State is the `/AS` appearance state.

| Event | Effect |
|---|---|
| `LButtonDown`/`Up` inside the rect | toggle (checkbox) / select-this-one-and-clear-siblings (radio), then regenerate |
| `Char(kReturn)` or `Char(kSpace)` while focused | same as a click — **and returns `true` even when the field is read-only, without changing state** (`CheckReadOnlyInCheckbox` :1063, `CheckReadOnlyInRadiobutton` :1098) |
| `KeyDown(Tab)` | move focus |

Radio siblings are the widgets of the same field: `pdfrum-doc`'s
`Field { widgets: Vec<Widget> }` (`form/field.rs:174`) already holds exactly
that grouping, so "clear the siblings" is a walk of one `Field`.

`checkbox_radiobutton.evt` is the pixel-corpus fixture: check a box at
(145, 220), two `keycode,9` tabs, then `charcode,13` to select a radio.

#### 1.7.3 Combo box (`FieldKind::Combo`)

Two sub-machines chosen by `kChoiceEdit = 1 << 18`
(`constants/form_flags.h:37`; ISO 32000-1 calls it bit 19, 1-based): an
**editable** combo is a text field with a dropdown attached; a
**non-editable** combo is a chooser. `pdfrum-doc`'s `FieldFlags::is_combo`
(`form/field.rs:132`) reads the *combo* flag `1 << 17`; the *editable* flag
`1 << 18` is a second one this milestone must add to `FieldFlags` (§3).

Non-editable (`Combo1` in `combobox_form.pdf`, `/Ff 131072`):

| Event | Effect |
|---|---|
| focus | the current option's text becomes the focused text (`"Banana"`, the `/V` default) |
| `Char(kReturn)` | **true** — opens/closes the popup; does not insert |
| `Char(kSpace)` | **true** — consumed; does **not** insert a space |
| `KeyDown(Down)` | advance the selected index by one (`CheckIfEnterAndSpaceKeyAreHandled` :2502: index 1 → 2 → 3) |
| `Char(letter)` | **type-ahead**: `FocusChanges` (:2836) step 17 types `A` → `"Apple"`, then `A,B,C` → `"Cherry"`, then `A,B` → `"Banana"` — each `OnChar` jumps to the option starting with that letter, and consecutive chars are *independent* jumps, not an accumulating prefix |
| `Char(kTab)` / `Char(kTab, Shift)` | **true**, and the selection survives the round trip |
| `SetIndexSelected(i, false)` | **false** — a non-editable combo cannot be deselected (`SetSelectionProgrammaticallyNonEditableField` :1945) |
| `SetIndexSelected(±100, true)` | **false**, state unchanged |

Editable (`Combo_Editable`, `/Ff 393216` = 131072 | 262144):

| Event | Effect |
|---|---|
| `Char(kSpace)` | **inserts a literal space**, and **clears the index selection** — `CheckIfEnterAndSpaceKeyAreHandledOnEditableFormField` (:2545) reads focused text `" "` and index 0 no longer selected |
| typing after a programmatic selection | inserts at the caret: `SetIndexSelected(1, true)` gives `"Bar"`, then typing `ABCDE` with the caret at the field start gives **`"ABCDEBar"`** (:2021) |
| re-selecting the same option | **discards the in-place edit** — `FocusChanges` (:2836) step 21: option 0 → `"Foo"`, type `A` → `"AFoo"`, select option 0 again → `"Foo"` |
| `SetIndexSelected(i, false)` | **false**, same as non-editable |

The dropdown's geometry is a contract because every option-selection assertion
depends on it: `SelectOption` (:364-378) uses
`static constexpr double kChoiceHeight = 15` (:369) and clicks option *i* at
`(point.x - 20, point.y - 15 * (i + 1))`, the −20 being an explicit
"move left to avoid scrollbar" (:375).

#### 1.7.4 List box (`FieldKind::List`)

No text editing. State is a set of selected indices plus a scroll position,
and the *focused text* is a derived quantity with a surprising rule.

| Event | Effect |
|---|---|
| `LButtonDown` on a visible row | select that row, clearing others (single-select) |
| `SetIndexSelected(i, true/false)` | **true** for any in-range `i`, including a redundant set or a redundant clear; **false** for out of range (`±100`) |
| `SetIndexSelected(only_selected, false)` on a **single**-select list | **true**, and the list becomes fully empty with focused text `""` — unlike a combo, a single-select list box *can* be fully deselected (`SetSelectionProgrammaticallySingleSelectField` :3036, comment :3084) |

**The focused text of a multi-select list box is the text of the last index
*acted upon*, not a description of the selection.**
`SetSelectionProgrammaticallyMultiSelectField` (:3102-3173) is unambiguous:
after selecting 5, 6 and 20 the focused text is `"Ugli Fruit"` (index 20);
after deselecting 20 and 1 it is `"Banana"` (index 1); and after
`SetIndexSelected(3, false)` — which **changes nothing**, index 3 was already
unselected — it becomes `"Date"` (index 3). A no-op call moves the caret. That
is the behavior, and it is asserted.

Pre-selection is read from `/I` and `/V` with **`/V` winning on conflict**:
`CheckIfMultipleSelectedIndices` (:3175) reads `/I`,
`CheckIfMultipleSelectedValues` (:3185) reads `/V`, and
`CheckIfMultipleSelectedMismatch` (:3195) has them disagree and asserts the
`/V` answer.

**Scrolling has one asserted-correct rule and three asserted-wrong ones.**
`CheckForNoOverscroll` (:3220-3237) is the correct one and must be
reproduced: `Listbox_SingleSelectLastSelected` has 10 options with index 9
selected, so `/TI` names 9 as the top visible row — but the widget scrolls only
far enough to **fill the box**, which leaves index **8** in the first visible
row. Clicking that row selects index 8. Three sibling tests
(`CheckIfIndexSelectedMultiSelectField` :3016, `SetSelectionProgrammatically…`
:3102, `CheckIfVerticalScrollIsAtFirstSelected` :3205) carry
`TODO(bug_1377)` comments saying the asserted result is *wrong* and the list
should have been scrolled to the first selected item. **We reproduce the
asserted behavior, bug and all** — it is what the oracle does and what the
goldens contain — and record the three sites in §5.OQ3 so a future fix
upstream is recognizable rather than mysterious.

#### 1.7.5 Push button (`FieldKind::Button`) and signature (`FieldKind::Sig`)

A push button has no value and no editing state; it has a pressed/released
appearance and an action. `ButtonActionInvokeTest` (:3656-3668) asserts the
*current, broken* state: after focusing the button and sending
`FORM_OnChar(kReturn, 0)`, `DoURIAction` is called **zero** times and
`FORM_OnChar` returns **false**, both marked `TODO(crbug.com/1028991)` as
things that "should" be 1 and true. Reproduced as-asserted (§5.OQ3).

A signature widget is never given an appearance and never takes an edit
(SPEC §10 already records "a widget whose field type is not one of the six the
builder dispatches on gets no appearance at all").

### 1.8 Link and non-widget annotations in the event path

`FPDFFormFillActionUriTest` (:3615) makes link annotations focusable
(`FPDFAnnot_SetFocusableSubtypes(handle, {FPDF_ANNOT_WIDGET,
FPDF_ANNOT_LINK}, 2)`) and then tabs to them. So the tab ring is over
**focusable annotation subtypes**, a configurable set defaulting to widgets
alone, and `FORM_OnKeyDown(FWL_VKEY_Return, modifier)` on a focused link fires
its action with the modifier bits passed through:

- `LinkActionInvokeTest` (:3670): four Returns with modifiers `0`,
  `ControlKey`, `ShiftKey`, `ShiftKey|ControlKey` fire `DoURIAction` four
  times with the URI `"https://cs.chromium.org/"` in a non-XFA build.
- `…Version2` (:3757) instead expects `DoURIActionWithKeyboardModifier` with
  the modifier values `0`, `2`, `1`, **`3`** in order — the literal `3` pins
  `FWL_EVENTFLAG_ShiftKey | FWL_EVENTFLAG_ControlKey`.
- `InternalLinkActionInvokeTest` (:3705) tabs to annots 4, 5 and 6 and fires
  four Returns each: `DoGoToAction` exactly **12** times, with the zoom-mode
  argument pinned to **1**.
- On all three: `OnKeyDown` with a null handle or null page is **false**, and
  `FWL_VKEY_Shift`, `FWL_VKEY_Space` and `FWL_VKEY_Control` are **false**
  (a `TODO` says Space should eventually be true).

### 1.9 Appearance regeneration: when a widget draws from state vs. from `/AP`

This is the seam between M6's work and M14's, and `form_textfield_focused_ltr.evt`
states it in its own header comment (§1.2.7): **a focused field renders from
live editor state; an unfocused field falls back to a generated appearance
stream.** The full trigger list:

1. **On load, ungated.** `CPDFSDK_Widget::OnLoad` calls `ResetAppearance` on
   any widget whose appearance is not valid, and validity is
   `!!GetDictFor("AP")` and nothing deeper (`cpdfsdk_baannot.cpp:85-87`, as
   already recorded in SPEC §10). M6 implements this.
2. **On `/NeedAppearances`.** `CPDFSDK_PageView::NewAnnot` calls
   `ResetAppearance` whenever the flag is set, consulting no `/AP`
   (`cpdfsdk_pageview.cpp:108-113`). Already in SPEC §10, with the
   `GetCheckedAPState` / `/Opt` refinement.
3. **On value commit** — new in M14. When a field's value changes (focus
   loss, `FORM_ForceToKillFocus`, a checkbox toggle, an option selection),
   the widget's appearance is regenerated from the new value through the same
   `ap::field_body` path M6 built. `Bug1302455EditFirstForm` (:1447) is the
   test that isolates it: type `A`, `FORM_ForceToKillFocus`, render — the
   golden `bug_1302455_edit_first_form` is a *generated* appearance
   containing the typed character, and it survives `FPDF_SaveAsCopy` and a
   reload byte-for-byte.
4. **While focused** — also new. The field is drawn from the live edit state
   with caret and selection highlight, not from any `/AP`. The four
   `form_textfield_*` `.evt` goldens and `FormTextFieldBiDiLiveEdit`
   (:1708) / `FormComboBoxBiDiLiveEdit` (:1791) pixel goldens are this path.
5. **The field-highlight overlay** is independent of both and already
   implemented (`docs/status/pdfrum-render.md` §wave-9, `pdfium_test.cc`
   passes `FPDF_SetFormFieldHighlightColor(UNKNOWN, 0xFFE4DD)` and
   `…HighlightAlpha(100)`, `embedder_test.cpp:881-883`).
   `RemoveFormFieldHighlight` (:1533-1549) pins that removing and restoring
   the highlight is exactly reversible over three renders.

### 1.10 What fires without JS, and what is JS-only

The gate is `CPDFSDK_FormFillEnvironment::IsJSPlatformPresent()` plus the
`#ifdef PDF_ENABLE_V8` regions in `cpdfsdk_interactiveform.cpp`. The
milestone-defining measurement, from the test file itself:

- **11 of 139** tests are inside `#ifdef PDF_ENABLE_V8`
  (`fpdf_formfill_embeddertest.cpp:1132`–`:1361`), namely `DisableJavaScript`,
  `DocumentAActions`, `DocumentAActionsDisableJavaScript`, `Bug551248`,
  `Bug620428`, `Bug634394`, `Bug634716`, `Bug679649`, `Bug707673`,
  `Bug765384`, `Bug1477093`.
- **10 of 139** are inside `#ifdef PDF_ENABLE_XFA` (`:954`–`:1045`, five
  tests) or are XFA-fixture tests / XFA-gated expectations (`:1616`, `:2785`,
  `:3675`, plus `FPDFXFAFormBug1055869EmbedderTest.Paste` and
  `…1058653….Paste`).

**PLAN.md §M14 says "only 2 gated on V8". The measured number is 11.** This is
escalation §5.E2 — it does not change the milestone's shape (the V8-gated
eleven are all timer/alert/document-action tests, i.e. genuinely M15's, and
the *interaction* tests are all V8-free), but it changes the exit criterion's
arithmetic and PLAN.md must be corrected rather than quietly missed.

The cascade points, and their gating:

| Hook | Fires without JS? | Note |
|---|---|---|
| keystroke (`OnKeyStrokeCommit`) | **no** | the whole point of the hook is to run a script; with V8 off it is compiled out and the keystroke is accepted verbatim |
| validate (`OnValidate`) | **no** | same |
| calculate (`OnCalculate`, `/CO` order) | **no** | same |
| format (`OnFormat`) | **no** | the field displays its raw value; this is exactly the visible gap PLAN §M15 describes (`AFNumber_Format` showing `1234` where Acrobat shows `$1,234.00`) |
| `/MaxLen` truncation | **yes** | a data rule, not a script |
| checkbox/radio toggling and sibling clearing | **yes** | |
| combo/list selection, `/I` vs `/V` | **yes** | |
| caret, selection, undo/redo, scrolling | **yes** | all of `CPWL_*` is V8-free |
| appearance regeneration on commit | **yes** | |
| link/URI/GoTo actions on Return | **yes** | `FPDFFormFillActionUriTest` is not V8-gated |
| the `/Hide` document open action | **yes** | already implemented (SPEC §10) |

So with V8 off the cascade degenerates to: **accept the keystroke, apply the
data rules, commit, regenerate.** That is M14's whole job on the cascade axis,
and §3 names the seam where M15 inserts the four missing hooks.

### 1.11 The cascade, transcribed — and where the V8 gate actually is

§1.10's table said *what* runs without JS. This section says *why*, and the
answer corrects a natural assumption: **there is no `#ifdef PDF_ENABLE_V8`
anywhere in `fpdfsdk/formfiller/`.** Verified by grep over the whole
directory. The gating is three layers deep and only the innermost is
compile-time:

1. **Is there an action dictionary at all?** Each hook checks
   `pWidget->GetAAction(<type>).HasDict()` and returns the *permissive*
   default when there is not.
2. **`IsJSPlatformPresent()`** — `cpdfsdk_formfillenvironment.h:115`:
   ```cpp
   bool IsJSPlatformPresent() const { return info_ && info_->m_pJsPlatform; }
   ```
   Checked in `DoActionFieldJavaScript`
   (`cpdfsdk_formfillenvironment.cpp:931-932`) and, separately and earlier, at
   the very first line of `CPDFSDK_InteractiveForm::OnCalculate`
   (`cpdfsdk_interactiveform.cpp:255-257`) and `::OnFormat` (`:315-317`).
3. **The compile-time gate**, and it is in exactly one place —
   `fxjs/ijs_runtime.cpp:50-58`:
   ```cpp
   std::unique_ptr<IJS_Runtime> IJS_Runtime::Create(CPDFSDK_FormFillEnvironment* env) {
   #ifdef PDF_ENABLE_V8
     if (env->IsJSPlatformPresent()) { return std::make_unique<CJS_Runtime>(env); }
   #endif
     return std::make_unique<CJS_RuntimeStub>(env);
   }
   ```

**This is the whole V8 story, and its consequence is the design's foundation.**
With V8 out, `CJS_RuntimeStub` is substituted and every `RunScript` becomes a
no-op. The formfiller path is otherwise *byte-for-byte identical*: the same
functions in the same order, the same `CFFL_FieldAction` constructed and
populated, `OnAAction` still fired, the action tree still walked. Only the
script body never executes.

And `CFFL_FieldAction::bRC` **defaults to `true`** (`cffl_fieldaction.h:22`)
and is explicitly re-set to `true` by every caller before dispatch
(`cffl_interactiveformfiller.cpp:754`, `:790`, `:1021`). Since only JS can set
it false, **with V8 out every validate and every keystroke-commit returns
`true`: all commits succeed and no keystroke is ever rejected.**

So a V8-less build is not a *subset* of the code path — it is the same path
with exactly three value-mutation points inert: `fa.bRC` never goes false,
`fa.sChange` is never rewritten, and calculate/format never run. That is
precisely the seam M15 re-activates (§3.6).

#### 1.11.1 Cascade A — a character typed into a focused text field

| # | Step | `file:LINE` |
|---|---|---|
| 1 | host → `CFFL_InteractiveFormFiller::OnChar(widget, ch, flags)` | `cffl_interactiveformfiller.cpp:382` |
| 2 | **`if (nChar == ascii::kTab) return true;`** — Tab (0x09) swallowed at the dispatcher, never reaches a field | `:385-387` |
| 3 | `GetFormField(widget)` — **non-creating**; `false` if absent | `:389-390` |
| 4 | `CFFL_TextField::OnChar` — Return (0x0D) / Escape (0x1B) special cases | `cffl_textfield.cpp:113-150` |
| 5 | else → `CFFL_FormField::OnChar`: `IsValid()` gate, `GetPWLWindow(GetCurPageView())` | `cffl_formfield.cpp:180-189` |
| 6 | `pWnd->OnChar(nChar, nFlags)` into the edit control | `cffl_formfield.cpp:188` |
| 7 | the edit calls **back up** into `OnBeforeKeyStroke` (the `IPWL_FillerNotify` override) | `cffl_interactiveformfiller.cpp:979` |
| 8 | copy `pPageView` + `ObservedPtr<CPDFSDK_Widget>` **out of the per-window data first** — "the window owning it may not survive" | `:988-991` |
| 9 | gate: `if (notifying_ \|\| !GetAAction(kKeyStroke).HasDict()) return {bRC=true, bExit=false};` | `:1004-1006` |
| 10 | `AutoRestorer<bool>` sets `notifying_ = true` | `:1008-1009` |
| 11 | snapshot `nAge = GetAppearanceAge()`, `nValueAge = GetValueAge()` | `:1011-1012` |
| 12 | build `fa`: `sChange`, `sChangeEx`, `bKeyDown`, **`bWillCommit = false`**, `bRC = true`, `nSelStart`, `nSelEnd` | `:1014-1023` |
| 13 | `GetActionData(pPageView, kKeyStroke, fa)` | `:1024` → `cffl_textfield.cpp:188-197` |
| 14 | `SavePWLWindowState(pPageView)` | `:1025` → `cffl_textfield.cpp:228` |
| 15 | `pWidget->OnAAction(kKeyStroke, &fa, pPageView)` | `:1028` → `cpdfsdk_widget.cpp:1086` |
| 16 | → `DoActionField` → `ExecuteFieldAction` → `DoActionFieldJavaScript` — **the `IsJSPlatformPresent()` gate** | `cpdfsdk_formfillenvironment.cpp:931-936` |
| 17 | JS may set `fa.bRC` false and/or rewrite `fa.sChange` — **skipped without V8** | *(fxjs)* |
| 18 | `if (!pWidget \|\| !IsValidAnnot(...)) return {true, true};` | `:1030-1032` |
| 19 | if the appearance age changed: `ResetPWLWindowForValueAge`, **re-fetch the per-window data from the new window**, rebind widget/pageview, `bExit = true` | `:1038-1048` |
| 20 | `fa.bRC ? SetActionData(kKeyStroke, fa) : RecreatePWLWindowFromSavedState(pPageView)` | `:1049-1053` |
| 21 | `if (GetFocusAnnot() == pWidget) return {false, bExit};` — still focused, do **not** commit | `:1054-1056` |
| 22 | else `CommitData(pPageView, nFlag); return {false, true};` — JS moved focus, commit now | `:1058-1059` |

Without V8, steps 1–16 and 18–22 run unchanged, step 17 never happens, and
step 20 always takes the `SetActionData` branch — which re-applies
`fa.sChange` **unmodified**, i.e. exactly the character the user typed.

#### 1.11.2 Cascade B — a value commit

Reached from four places: `OnKillFocus` → `KillFocusForAnnot`
(`cffl_interactiveformfiller.cpp:455`); `CFFL_TextField::OnChar` on Return in a
single-line field (`cffl_textfield.cpp:134`); `OnBeforeKeyStroke` when JS moved
focus away (`:1058`); and checkbox/radio `OnChar`/`OnLButtonUp`
(`cffl_checkbox.cpp:71`, `:94`; `cffl_radiobutton.cpp:65`, `:87`).

**The canonical order** (`cffl_formfield.cpp:507-552`):

```
IsDataChanged  →  OnKeyStrokeCommit (bWillCommit=true)  →  OnValidate
               →  SaveData  →  OnCalculate  →  OnFormat
```

with an `ObservedPtr` null re-check between **every** pair (`:515`, `:521`,
`:526`, `:532`, `:537`, `:542`, `:547`) — the callbacks can destroy the widget.

`SaveData`'s write path differs per kind, and one difference is a real
divergence rather than an oversight:

| Kind | `SaveData` sequence | `file:LINE` |
|---|---|---|
| Text | `SetValue` → `ResetFieldAppearance` → `UpdateField` → `SetChangeMark` | `cffl_textfield.cpp:169,173,177,181` |
| Combo | `SetValue` **or** `SetOptionSelection` → `ResetFieldAppearance` → `UpdateField` → `SetChangeMark` | `cffl_combobox.cpp:105/108,113,117,121` |
| List | `ClearSelection` → `SetOptionSelection`×n → `SetTopVisibleIndex` → `ResetFieldAppearance` → `UpdateField` → `SetChangeMark` | `cffl_listbox.cpp:121,129/136,142,146,150,154` |
| Check / Radio | `SetCheck` → `UpdateField` → `SetChangeMark` — **no `ResetFieldAppearance`** | `cffl_checkbox.cpp:110,114,118`; `cffl_radiobutton.cpp:103,107,111` |

Without V8: `IsDataChanged`, `SaveData` and the tail run fully and
identically. `OnKeyStrokeCommit` and `OnValidate` still execute their whole
bodies — guards, `fa` construction, `GetActionData`, `SavePWLWindowState`,
`OnAAction`, the action-tree walk — and both return `true`, so **their reject
branches are unreachable**. `OnCalculate` and `OnFormat` return at their very
first line, so **no cross-field recalculation and no format-on-commit happens
at all**; a field's displayed text is its raw value.

#### 1.11.3 The load-bearing quirk: a failing validate does *not* keep focus

This is the single most counter-intuitive behavior in the subsystem, and it
must be reproduced deliberately or it will be "fixed" by accident.

On rejection, `CommitData` calls `ResetPWLWindow` — which for a text object
destroys and re-creates the window from the **stored field value**, discarding
the user's typing — and then **`return true;`**
(`cffl_formfield.cpp:519` and `:530`, both `true`). Because `CommitData`
returned true, `KillFocusForAnnot`'s early return at `:307` is *not* taken:
execution proceeds to `pWnd->KillFocus()` (`:311`) and `EscapeFiller` (`:325`),
`valid_` goes false, and `CPDFSDK_FormFillEnvironment::KillFocusAnnot` reports
success.

**A failing PDF `/AA /V` validate script cannot hold the caret in the field.**
The only `false` returns from `CommitData` are widget-destroyed-during-callback
— an error path, not a validation path. This differs from Acrobat, which many
PDFs assume, and it is a quirk to replicate, not repair (D9, §5.OQ2).

The asymmetry worth preserving: `OnBeforeKeyStroke` *does* honour rejection
meaningfully — `fa.bRC == false` → `RecreatePWLWindowFromSavedState`
(`:1052`) — because there the field is still focused and the keystroke is
simply undone.

### 1.12 The formfiller layer's own state, and its non-obvious rules

Recorded because each one changes observable behavior and none of it survives
into the Rust design as *shape* (§2), only as *behavior*.

- **Only 4 of ~19 dispatcher entries create a form field**: `OnMouseEnter`
  (`:132`), `OnMouseMove` (`:342`), and `OnSetFocus` twice (`:405`, `:436`).
  Every other entry uses the non-creating `GetFormField` and no-ops when the
  field was never instantiated. **Pure event *ordering* therefore changes
  observable behavior** — which is exactly why the tests' three-call click
  gesture (§1.3.2) begins with a mouse-move.
- `GetOrCreateFormField` returns **`nullptr` for `FormFieldType::kUnknown`**
  (`:565-567`), so signature widgets and unknown types never get an
  interaction state at all.
- **The reentrancy guard `notifying_`** is a plain bool with an
  `AutoRestorer`, checked at seven sites, with *inconsistent* return values
  when set: `OnButtonUp` returns **false** (`:260-262`),
  `OnKeyStrokeCommit`/`OnValidate` return **true** (`:735-737`, `:772-774`),
  `OnCalculate`/`OnFormat` return void early (`:806-808`, `:816-818`).
- **The value-age reset rule inverts intuition** (`cffl_formfield.cpp:611-617`):
  ```cpp
  return nValueAge == pWidget->GetValueAge() ? RestorePWLWindow(pPageView)
                                             : ResetPWLWindow(pPageView);
  ```
  Value age **unchanged** → *Restore* (preserve the in-progress edit); value
  age **changed** → *Reset* (discard it). Both wrap the return in an
  `ObservedPtr` because the `UpdateField()` inside can run JS that deletes the
  window (`cffl_textobject.cpp:23-37`).
- **Enter on a single-line text field toggles `valid_`**
  (`cffl_textfield.cpp:124`): valid→invalid commits and tears the window down;
  invalid→valid re-creates and focuses it. **Escape always discards**
  (`EscapeFiller(pPageView, true)`, `:144`). Multiline Enter falls through to
  the ordinary insert path.
- **Buttons destroy their window on blur; text/list/combo keep theirs cached**
  (`cffl_formfield.cpp:316-323`).
- **A checkbox's `OnChar` toggles (`SetCheck(!is_checked)`) while a radio's
  sets unconditionally (`SetCheck(true)`)** — `cffl_checkbox.cpp:68` vs
  `cffl_radiobutton.cpp:63`. A radio button cannot be un-selected by clicking
  it again.
- **Neither checkbox nor radio handles arrow keys.** `FWL_VKEY_Up/Down/Left/
  Right` fall through to the base and then to the window
  (`cffl_checkbox.cpp:34-43`). Radio-group arrow navigation is **not
  implemented** in this layer, contrary to what a viewer-behavior reading
  would predict.
- **`CFFL_Button::OnMouseMove` returns `true` unconditionally**
  (`cffl_button.cpp:57-61`), claiming the event while doing nothing.
- **`IsFillingAllowed` is `false` unconditionally for a push button**
  (`cffl_interactiveformfiller.cpp:522-524`); otherwise it is
  `HasPermissions(kFillForm | kModifyAnnotation | kModifyContent)` — bits
  `1<<8`, `1<<5`, `1<<3` (`constants/access_permissions.h:13-15`) — where
  **any one** bit suffices, evaluated with owner permissions granted.
- **`CFFL_ComboBox::SetIndexSelected` rejects deselection outright**
  (`if (!IsValid() || !selected) return false;`, `cffl_combobox.cpp:215-217`),
  while `CFFL_ListBox`'s supports it and sets the caret in **both** branches
  (`cffl_listbox.cpp:215-238`) — which is exactly the "a no-op deselect moves
  the caret" behavior §1.7.4 records from the tests.
- Three pieces of dead or accumulating code, called out so a reviewer does not
  read their absence as a porting error: `CFFL_ComboBox::SaveData:107` calls
  `GetSelectedIndex(0)` and discards it; `CFFL_TextField::SaveData:163`
  computes `sOldValue` and never uses it; and
  `CFFL_ListBox::SavePWLWindowState` (`:190-201`) **never clears `state_`
  before pushing**, so repeated saves accumulate indices.

### 1.13 Constants: the complete table

Everything numeric this milestone must carry, from both directories.

| Constant | Value | Source |
|---|---|---|
| `kDefaultFontSize` | `9.0f` | `pwl/cpwl_wnd.cpp:24`, applied `:40` |
| `kCaretFlashIntervalMs` | `500` ms | `pwl/cpwl_caret.cpp:89`, timer `:93` |
| scrollbar auto-repeat | `100` ms | `pwl/cpwl_scroll_bar.cpp:426`, `:441` |
| `kComboBoxDefaultFontSize` | `12.0f` | `pwl/cpwl_combo_box.cpp:21` |
| `kDefaultButtonWidth` (combo drop arrow) | `13` | `pwl/cpwl_combo_box.cpp:22` |
| `kButtonWidth` (scrollbar) | `9.0f` | `pwl/cpwl_scroll_bar.cpp:23` |
| `kPosButtonMinWidth` (scrollbar thumb) | `2.0f` | `pwl/cpwl_scroll_bar.cpp:24` |
| `kMaxListBoxHeight` (dropdown) | `140` | `formfiller/cffl_interactiveformfiller.cpp:706` |
| `kDefaultListBoxFontSize` | `12.0f` | `formfiller/cffl_listbox.cpp:36` |
| `kChoiceHeight` (test-side dropdown row) | `15` | `fpdf_formfill_embeddertest.cpp:369` |
| dash pattern for `BorderStyle::kDash` | `{3, 3, 0}` | `formfiller/cffl_formfield.cpp:368` |
| border-width multiplier, `kBeveled`/`kInset` | `×2` | `formfiller/cffl_formfield.cpp:372` |
| view-bbox inflation (focus ring margin) | `Inflate(1, 1)` | `cffl_formfield.cpp:49`, `cffl_interactiveformfiller.cpp:53` |
| default text colour | `MakeGray(0.0f)` — black | `cffl_formfield.cpp:355` |
| `CreateParams::dwBorderWidth` | `1` | `pwl/cpwl_wnd.h:105` |
| `CreateParams::nTransparency` | `255` | `pwl/cpwl_wnd.h:108` |
| `CreateParams::nBorderStyle` | `kSolid` | `pwl/cpwl_wnd.h:104` |
| per-window-data seed value age | `0` (hardcoded; upstream TODO) | `cffl_formfield.cpp:399-401` |

**There is no double-click time threshold anywhere in either directory.** The
platform delivers the double-click event; PDFium never measures an interval.
Likewise `CFFL_FormField::timer_` is declared and `reset()` twice but **never
constructed**, and `OnTimerFired` is an empty body no subclass overrides — all
timing lives in the window layer (caret blink 500 ms, scrollbar repeat 100 ms).

**ASCII constants** (`constants/ascii.h:13-26`), those the layer decides on:
`kTab = 0x09`, `kBackspace = 0x08`, `kNewline = 0x0A`, `kReturn = 0x0D`,
`kEscape = 0x1B`, `kSpace = 0x20`, `kDelete = 0x7F`, and the control-letter
codes `kControlA = 0x01`, `kControlC = 0x03`, `kControlV = 0x16`,
`kControlX = 0x18`, `kControlZ = 0x1A`.

**Field flags** (`constants/form_flags.h`), beyond the ones `pdfrum-doc`
already reads: `kReadOnly = 1<<0` (:13), `kTextMultiline = 1<<12` (:26),
`kTextPassword = 1<<13` (:27), **`kTextDoNotScroll = 1<<23`** (:30),
`kTextComb = 1<<24` (:31), `kTextRichText = 1<<25` (:32),
**`kChoiceEdit = 1<<18`** (:37), **`kChoiceMultiSelect = 1<<21`** (:39).
The three in bold are new to `FieldFlags` and are added in §3.

**The window style flags** (`pwl/cpwl_wnd.h:38-67`), which are how the
formfiller communicates a field's configuration to the editor. They are not
ported as flags (§2, D4) but the *mapping* is the contract:

```
kWindowBorder        0x40000000    kEditMultiline    0x0001
kWindowBackground    0x20000000    kEditPassword     0x0002
kWindowVScroll       0x08000000    kEditLeft         0x0004
kWindowVisible       0x04000000    kEditRight        0x0008
kWindowReadOnly      0x01000000    kEditMiddle       0x0010
kWindowAutoFontSize  0x00800000    kEditTop          0x0020
kWindowAutoTransparent 0x00400000  kEditCenter       0x0080
kWindowNoRefreshClip 0x00200000    kEditCharArray    0x0100
                                   kEditAutoScroll   0x0200
kListboxMultipleSel  0x0001        kEditAutoReturn   0x0400
kListboxHoverSel     0x0008        kEditUndo         0x0800
kComboboxAllowCustomText 0x0001    kEditRich         0x1000
                                   kEditTextOverflow 0x4000
```

And the mapping itself, from `CFFL_TextField::GetCreateParam`
(`cffl_textfield.cpp:43-89`):

| `/Ff` bit | Effect |
|---|---|
| `kTextPassword` (1<<13) | `kEditPassword` |
| `kTextMultiline` (1<<12) | `{kEditMultiline, kEditAutoReturn, kEditTop}`, **plus** `{kWindowVScroll, kEditAutoScroll}` unless `kTextDoNotScroll` |
| *(single line)* | `kEditCenter`, plus `kEditAutoScroll` unless `kTextDoNotScroll` |
| `kTextComb` (1<<24) | `kEditCharArray` |
| `kTextRichText` (1<<25) | `kEditRich` |
| — | **`kEditUndo` unconditionally** (`:73`) — undo is always available on a text field |
| `/Q` 0/1/2 | `kEditLeft` / `kEditMiddle` / `kEditRight` |
| `/MaxLen > 0` | `SetCharArray(n)` **if `kEditCharArray`** else `SetLimitChar(n)`; `<= 0` means unlimited |
| font size `<= 0` | `kWindowAutoFontSize` |

`vt::Config` (`crates/pdfrum-doc/src/vt/mod.rs:99-119`) already carries
`multi_line`, `auto_return`, `sub_word`, `limit_char` and `char_array` — the
five knobs this table sets. The mapping is therefore *already implemented* on
the layout side; M14 adds only `auto_scroll` (a scroll offset, §3.3) and the
read-only/undo switches, which are session state rather than layout config.

### 1.14 The rotation matrix, transcribed

`CFFL_FormField::GetCurMatrix` (`cffl_formfield.cpp:442-464`) is the only
place a widget's `/MK /R` rotation enters the event path, and it is
transcribed in full because §2's D3 folds it into a single transform:

```cpp
CFX_FloatRect rcDA = widget_->GetPDFAnnot()->GetRect();
switch (widget_->GetRotate()) {
  case 90:  mt = CFX_Matrix(0, 1, -1, 0, rcDA.right - rcDA.left, 0); break;
  case 180: mt = CFX_Matrix(-1, 0, 0, -1, rcDA.right - rcDA.left,
                                          rcDA.top - rcDA.bottom); break;
  case 270: mt = CFX_Matrix(0, -1, 1, 0, 0, rcDA.top - rcDA.bottom); break;
  case 0:
  default:  break;   // identity
}
mt.e += rcDA.left;
mt.f += rcDA.bottom;
```

With `W = right - left`, `H = top - bottom`, the four results are:

| Rotation | Matrix `(a, b, c, d, e, f)` |
|---|---|
| 0° | `(1, 0, 0, 1, left, bottom)` — pure translation |
| 90° | `(0, 1, -1, 0, W + left, bottom)` |
| 180° | `(-1, 0, 0, -1, W + left, H + bottom)` |
| 270° | `(0, -1, 1, 0, left, H + bottom)` |

And the window's own box, `GetPDFAnnotRect` (`:466-474`):

```cpp
float fWidth = rectAnnot.Width(), fHeight = rectAnnot.Height();
if ((widget_->GetRotate() / 90) & 0x01) { std::swap(fWidth, fHeight); }
return CFX_FloatRect(0, 0, fWidth, fHeight);
```

**`(rot / 90) & 1` swaps width and height for the odd quadrants.** Note the
inconsistency worth recording: `GetCurMatrix` uses `right - left` /
`top - bottom` (correct only for a *normalized* rect) while `GetPDFAnnotRect`
uses `Width()` / `Height()` (which normalize). pdfrum normalizes once, at the
plate-rect computation, which `ap::field_body::client_rect`
(`crates/pdfrum-doc/src/ap/field_body.rs:175`) already does and already
documents the inverted-rect rule for.

Page-space ↔ plate-space conversion is then `PWLtoFFL` = the forward matrix,
`FFLtoPWL` = its inverse (`cffl_formfield.cpp:491-505`), applied to every
mouse coordinate on entry (`:103`, `:116`, `:128`, `:140`, `:153`, `:160`,
`:167`).

### 1.15 Hit testing and routing — the two different lookups

`CPDFSDK_PageView` has **two** point-lookup helpers, and which one a handler
uses is observable.

**`GetFXAnnotAtPoint`** (`cpdfsdk_pageview.cpp:125-137`) — used by
`OnMouseMove` **and nothing else**. Iterates in draw order, **skips `POPUP`**
(`:129-131`), and returns the first annot whose **`GetViewBBox()` contains the
point** (`:132-134`). This is a *rect containment* test, so it matches **every
annot subtype**, not just widgets — which is exactly what makes the six
`annotation_highlight_*.evt` fixtures work: a bare `mousemove,128,713` over a
`/Highlight` annotation raises its popup.

**`GetFXWidgetAtPoint`** (`:139-152`) — used by every *other* mouse handler.
Calls the virtual `DoHitTest(point)`, which only widgets implement
(`CPDFSDK_BAAnnot::DoHitTest` is `return false;`,
`cpdfsdk_baannot.cpp:301-303`).

`CPDFSDK_Widget::DoHitTest` (`cpdfsdk_widget.cpp:731-748`) is the real gate,
in this order:

1. **false** if `IsSignatureWidget()` or `!IsVisible()` (`:732-734`);
   `IsVisible()` is "none of `kInvisible | kHidden | kNoView` set in `/F`"
   (`cpdfsdk_baannot.cpp:205-210`).
2. **false** if `GetFieldFlags() & kReadOnly` (`:736-738`). **A read-only
   widget is not hit-testable at all** — which is *not* the same as the
   read-only checkbox tests (§1.3.3), where the widget already had focus and
   received a *keyboard* event.
3. `do_hit_test = (GetFieldType() == kPushButton)`; for anything else it
   additionally requires user permissions to include `kFillForm` **or**
   `kModifyAnnotation` (`:740-746`).
4. Finally `do_hit_test && GetViewBBox().Contains(point)` (`:747`).

`GetViewBBox` is empty for a signature widget, else the **form filler's**
bbox — which is inflated by `(1, 1)` and unions in the focus box, so a focused
widget is one unit larger on every side than its `/Rect`
(`cffl_formfield.cpp:38-54`, `:49`).

**Note the asymmetry is deliberate.** Hover (`OnMouseMove`) uses rect
containment over all subtypes because popups need it; clicks use `DoHitTest`
over widgets only. Reproduce both.

#### 1.15.1 Draw / hit-test order — `CPDFSDK_AnnotIteration`

Not the tab order. `cpdfsdk_annotiteration.cpp:24-48`:

1. Copy the page's annot list in raw `/Annots` load order.
2. `std::stable_sort` by **`GetLayoutOrder()` ascending** (`:28-31`), with
   exactly three values: **1** for a `POPUP`
   (`cpdfsdk_baannot.cpp:266-272`), **2** for a widget
   (`cpdfsdk_widget.cpp:435-437`), **5** for everything else
   (`cpdfsdk_annot.cpp:123-125`). `stable_sort` preserves `/Annots` order
   within each band.
3. The **focused** annot is extracted and re-inserted — at the **front** for
   hit testing (`CPDFSDK_AnnotIteration(page_view)`, `:21-22`), so the focused
   widget wins overlap ties; at the **end** for drawing
   (`CreateForDrawing`, `:15-19`), so it paints on top
   (`cpdfsdk_pageview.cpp:91-94`).

#### 1.15.2 Tab order — `CPDFSDK_AnnotIterator`, a different class

Used **only** for focus traversal (`cpdfsdk_pageview.cpp:256-298`), never for
hit testing. Three orders, chosen from the page dict's **`/Tabs`** key
(`cpdfsdk_annotiterator.cpp:110-122`):

```cpp
enum class TabOrder : uint8_t { kStructure = 0, kRow, kColumn };
```

- `/Tabs "R"` → `kRow`; `/Tabs "C"` → `kColumn`; **anything else, including
  `"S"` and absent, → `kStructure`.** `"S"` is not special-cased.
- **`kStructure`** (`:126-128`) is plain `/Annots` document order, with **no
  sorting at all**.
- **`kRow`** (`:130-161`): sort by `left` ascending; then repeatedly pick the
  top-most remaining annot (scanning **downward** from the last index with a
  strict `>` against `fTop`, so ties resolve to the **lowest** index, i.e.
  leftmost), append it, then append every remaining annot whose vertical
  centre `(top + bottom) / 2` lies **strictly** between that annot's `bottom`
  and `top`, in ascending index (left-to-right) order.

  > **Correction (2026-09-01, M14 implementation).** The parenthetical above is
  > wrong about the tie-break, and the error was found by compiling the C++
  > loop and diffing it against the port. Scanning downward with a strict `>`
  > means a tie **never displaces** the running best, and the running best when
  > a tie is met was set by a *higher* index — so ties resolve to the
  > **highest** index, i.e. the **rightmost** after the left-ascending sort.
  > The authority is `docs/status/M14.md`'s seed rule 1 and the test named for
  > it, `row_order_seeds_each_band_with_the_rightmost_of_the_topmost`; this
  > paragraph is left as written so the record of what was believed stays
  > readable.
- **`kColumn`** (`:164-199`): the mirror, sorting by `top` descending and
  banding on horizontal centre `(left + right) / 2` strictly between `left`
  and `right`.
- The collection filter keeps only annots whose subtype is in
  `GetFocusableAnnotSubtypes()` — **default `{WIDGET}`**
  (`cpdfsdk_formfillenvironment.h:293-294`) — and **excludes signature
  widgets** (`:80-84`). `FPDFAnnot_SetFocusableSubtypes` is what
  `FPDFFormFillActionUriTest` uses to add `LINK` (§1.8).
- Rects come from `pAnnot->GetPDFAnnot()->GetRect()` — the **raw `/Rect`**,
  not `GetViewBBox` (`:23-25`).

**Two upstream bugs in this code, and pdfrum must not reproduce one of them.**
`if (nLeftTopIndex < 0) continue;` sits inside `while (!sa.empty())`
(`:145-147`, `:182-184`) — nothing is erased, so it is an **infinite loop**
whenever every remaining annot has `top <= 0.0f` in row order. A Rust port
cannot hang: D10 makes the row/column banding a terminating fold that appends
the remainder in index order when no candidate is found, and emits a
`Diagnostic`. (The column-order variant is unreachable in practice because its
seed branch sets `nLeftTopIndex = 0` rather than `i` — itself an asymmetry
worth recording, `:174-176`.)

#### 1.15.3 Routing per handler, transcribed

| Handler | On a hit | On a miss |
|---|---|---|
| `OnFocus` (`:347-357`) | `SetFocusAnnot(annot)`, **true** | `KillFocusAnnot(flags)`, **false** |
| `OnLButtonDown` (`:359-377`) | dispatch; **re-check the `ObservedPtr`** because JS may have destroyed the annot (`:371-373`); `SetFocusAnnot` — whose **return value is discarded**, so it returns `true` even if focus was refused | `KillFocusAnnot(flags)`, **false** |
| `OnLButtonDblClk` (`:392-410`) | byte-identical to `OnLButtonDown` | `KillFocusAnnot`, **false** |
| `OnLButtonUp` (`:379-390`) | **two-target dispatch**: if a *different* annot holds focus, the **focused** annot gets first refusal and, if it consumes, that is the answer; otherwise the annot under the point handles it | no kill focus |
| `OnRButtonDown`/`Up` (`:412-448`) | `if (ok) SetFocusAnnot(annot);` but **returns `true` unconditionally** | **no kill focus**, `false` |
| `OnMouseWheel` (`:507-516`) | dispatch, no focus change | `false` |
| `OnMouseMove` (`:450-479`) | enter/exit bookkeeping then dispatch; returns `true` whenever an annot was under the point, **discarding the handler's own result** | `ExitWidget(true)` if we were on one, `false` |
| `OnChar` (`:528-531`) | **no hit test** — operates on the focus annot alone | `false` |
| `OnKeyDown` (`:533-570`) | **no hit test**; Tab handling first (below), then the focus annot | `false` |

`OnLButtonUp`'s two-target dispatch is what makes a drag that releases outside
the field still land in the field, and it is why `SelectTextWithMouse`
(§1.3.2) works when the drag end is a different x.

`OnKeyDown`'s Tab branch, transcribed (`:533-570`):

```cpp
if (key_code == FWL_VKEY_Tab) {
  if (IsCTRLKeyDown(flags) || IsALTKeyDown(flags) || IsMETAKeyDown(flags))
    return false;                                  // a system shortcut, not ours
  if (!annot) {                                    // nothing focused yet
    ObservedPtr end(IsSHIFTKeyDown(flags) ? GetLastFocusableAnnot()
                                          : GetFirstFocusableAnnot());
    return end && form_fill_env_->SetFocusAnnot(end);
  }
  ObservedPtr next(IsSHIFTKeyDown(flags) ? GetPrevAnnot(focus_annot)
                                         : GetNextAnnot(focus_annot));
  if (!next) return false;                         // no wrap
  if (next.Get() != focus_annot) { SetFocusAnnot(next); return true; }
}
if (!annot) return false;   // "JS may have destroyed it in GetNextAnnot()"
return CPDFSDK_Annot::OnKeyDown(annot, key_code, flags);
```

This explains §1.4's rule 6 exactly: with nothing focused, a forward Tab takes
`GetFirstFocusableAnnot` and a Shift-Tab takes `GetLastFocusableAnnot` — which
in `annotiter.pdf` are annots **1** and **0** respectively. And there is no
wrap: `GetNextAnnot` returns `nullptr` at the end and `GetPrevAnnot` returns
`nullptr` at `begin()` (`cpdfsdk_annotiterator.cpp:48-74`), giving the
"fifth tab is false" result.

#### 1.15.4 Mouse enter / exit

State: `bool on_widget_` and `ObservedPtr<CPDFSDK_Annot> capture_widget_`
(`cpdfsdk_pageview.h:128-129`). `OnMouseMove` (`:450-479`):

```cpp
ObservedPtr<CPDFSDK_Annot> pFXAnnot(GetFXAnnotAtPoint(point));
ObservedPtr<CPDFSDK_PageView> pThis(this);          // JS can destroy the page view
if (pThis->on_widget_ && pThis->capture_widget_ != pFXAnnot) pThis->ExitWidget(true, nFlags);
if (!pThis || !pFXAnnot) return false;
if (!pThis->on_widget_) {
  pThis->EnterWidget(pFXAnnot, nFlags);
  if (!pThis) return false;
  if (!pFXAnnot) { pThis->ExitWidget(false, nFlags); return true; }
}
CPDFSDK_Annot::OnMouseMove(pFXAnnot, nFlags, point);
return true;
```

For a plain annotation, enter/exit are the popup toggles:
`SetOpenState(true)` / `SetOpenState(false)` plus `UpdateAnnotRects`
(`cpdfsdk_baannot.cpp:309-317`), which inflates each rect by `(1, 1)` before
invalidating (`:237-252`). That is the entire mechanism behind the six
`annotation_highlight_*` goldens.

### 1.16 Widget-level rules M14 inherits and must not re-derive

`CPDFSDK_Widget` carries several rules M6 already implements or that M14 now
needs. Recorded with citations so the implementation does not rediscover them.

- **`OnLoad`** (`cpdfsdk_widget.cpp:1104-1131`): skip signature widgets;
  `ResetAppearance` if `!IsAppearanceValid()`; then for **text fields and
  combo boxes** call `OnFormat()` — but **re-apply the formatted value only
  for the combo box** (`:1119-1121`). The text field's format result is
  computed and dropped. The whole body is `ObservedPtr`-guarded because format
  JS can destroy the widget.
- **`ResetAppearance(value, ValueChanged)`** (`:671-705`) does three things
  before dispatching to the six builders: `set_appearance_modified(true)`,
  `appearance_age_++`, and `value_age_++` **only when `kValueChanged`**. Those
  two `uint32_t` counters (`:196-197`) are the entire mechanism behind the
  value-age Restore/Reset rule of §1.12. `kSignature` and `kUnknown` fall to
  `default: break` — no appearance at all, which SPEC §10 already records.
- **`IsAppearanceValid()`** is `!!GetAnnotDict()->GetDictFor("AP")`
  (`cpdfsdk_baannot.cpp:85-87`) — presence alone. **`IsWidgetAppearanceValid
  (mode)`** (`cpdfsdk_widget.cpp:365-407`) is the *deeper*, separate test:
  the entry is `"N"`, or `"D"`/`"R"` for down/rollover **falling back to `"N"`
  if that key is absent** (`:380-382`); then text/combo/list/button/signature
  require `pSub->IsStream()`, while **checkbox and radio require a dictionary
  containing a stream keyed by `/AS`** — the rule SPEC §10 records as the
  `0xFFAAAAAA` hairline-outline trigger.
- **All four value setters use `NotificationOption::kDoNotNotify`**
  (`:617-652`): `SetCheck`, `SetValue`, `SetOptionSelection`,
  `ClearSelection`. They therefore do **not** fire
  `BeforeValueChange`/`AfterValueChange`, so no validate, no calculate, no
  format, and no appearance reset happens as a side effect — **the form-filler
  layer drives all of that explicitly**. A port that notifies here will
  spuriously double-fire the cascade. This is D11.
- **`SetTopVisibleIndex(int)` is an empty stub** (`:654`) even though
  `CFFL_ListBox::SaveData` calls it (`cffl_listbox.cpp:142`). A list box's
  scroll position is therefore **never persisted to the document**.
- `GetRotate()` uses a **signed `%`** and can return negative (`:458-461`),
  while `GetMatrix`/`GetRotatedRect` use `abs(rotation % 360)`
  (`:1022-1067`). pdfrum normalizes once, into `[0, 360)`, and records the
  divergence (D3).
- **`OnAAction`'s non-XFA path returns `false` unconditionally**
  (`:1086-1102`), even when the action ran; results come back only through the
  mutated `CFFL_FieldAction`.
- **`GetAAction` lookup policy** (`:1133-1162`): the *annotation's* `/AA` for
  the ten cursor/button/focus/page types (with a fallback to the annot's `/A`
  for `kButtonUp` and `kKeyStroke` only,
  `cpdfsdk_baannot.cpp:220-231`); the **field's** `/AA` preferred, annot's as
  fallback, for `kKeyStroke`, `kFormat`, `kValidate`, `kCalculate`. M15 needs
  this exactly.
- **Highlight defaults are "off"**: `RemoveAllHighLights` fills the colour
  array with `FXSYS_BGR(255,255,255)` and the needs-highlight array with
  `false` (`cpdfsdk_interactiveform.cpp:44`, `:648-651`), and the alpha
  defaults to **0**. So a bare embedder draws no highlight at all;
  `pdfium_test` opts in with `SetHighlightColor(UNKNOWN, 0xFFE4DD)` and
  `SetHighlightAlpha(100)` (`embedder_test.cpp:881-883`), and passing
  `FPDF_FORMFIELD_UNKNOWN` routes to `SetAllHighlightColors`, which sets
  **every** slot including index 0 (`:663-668`).
- **`DrawShadow`** (`:982-1006`) transforms only the two corner points of
  `GetRect()` through the page→device matrix, normalizes, and fills with
  `AlphaAndColorRefToArgb(alpha, colour)`. Correct for axis-aligned matrices,
  wrong for a rotating one — and it uses the raw `/Rect`, not `GetViewBBox`.
- **`CPDFSDK_InteractiveForm::OnCalculate` reads `/AcroForm /CO` and nothing
  else** (`cpdf_interactiveform.cpp:739-779`): no `/CO` array means no
  calculations at all, and entries that are not dictionaries or do not resolve
  to a field are silently skipped. It is guarded by a `busy_` bool with an
  `AutoRestorer` (`cpdfsdk_interactiveform.h:117`), which is what stops
  `SetValue(..., kNotify)` → `AfterValueChange` → `OnCalculate` from
  recursing. **`IsJSPlatformPresent()` is checked *before* `busy_`**
  (`:255-261`), so with V8 off the guard is never even armed. M15 needs both
  facts.

### 1.17 Focus ownership, transcribed

`CPDFSDK_FormFillEnvironment::focus_annot_` is an `ObservedPtr`
(`cpdfsdk_formfillenvironment.h:285`) — the environment observes, the page
view owns.

**`SetFocusAnnot`** (`:791-837`) is a gauntlet with **three** `focus_annot_`
re-checks bracketing every point where JS could run and steal focus:

```cpp
if (being_destroyed_) return false;
if (focus_annot_ == pAnnot) return true;               // already focused: success
if (focus_annot_ && !KillFocusAnnot({})) return false; // the old one must yield first
if (!pAnnot) return false;                             // may have died during the kill
if (!pAnnot->GetPageView()->IsValid()) return false;
if (focus_annot_) return false;                        // re-check 1
/* … XFA … */
if (!CPDFSDK_Annot::OnSetFocus(pAnnot, {})) return false;
if (focus_annot_) return false;                        // re-check 2
focus_annot_ = pAnnot;                                 // assigned only here
SendOnFocusChange(pAnnot);   // a failure here is NOT a failure of SetFocusAnnot
return true;
```

**`focus_annot_` is assigned only *after* the form filler's `OnSetFocus` has
fully returned.** That is precisely why `CFFL_InteractiveFormFiller::
OnLButtonUp` can meaningfully compare `GetFocusAnnot() != pWidget` afterwards
(`cffl_interactiveformfiller.cpp:242`).

**`KillFocusAnnot`** (`:839-866`):

```cpp
if (!focus_annot_) return false;
ObservedPtr<CPDFSDK_Annot> pFocusAnnot(focus_annot_.Get());
focus_annot_.Reset();                                  // cleared BEFORE the callback
if (!CPDFSDK_Annot::OnKillFocus(pFocusAnnot, nFlags)) {
  focus_annot_ = pFocusAnnot;                          // restored on refusal
  return false;
}
if (!pFocusAnnot) return false;                        // destroyed by OnKillFocus
/* text/combo only: tell the embedder to hide the IME */
return !focus_annot_;                                  // JS re-focused ⇒ reports FAILURE
```

Three orderings to reproduce: the field is cleared **before** the callback so
re-entrant script sees "no focus"; a refusing `OnKillFocus` **restores** it;
and the final `return !focus_annot_` reports **failure** when script
re-focused something during the kill, even though the kill happened.

### 1.18 `FPDF_FFLDraw` and the matrix

`FFLCommon` (`fpdf_formfill.cpp:210-287`) builds
`rect = FX_RECT(start_x, start_y, start_x + size_x, start_y + size_y)` and
`matrix = pPage->GetDisplayMatrixForRect(rect, rotate)`, sets the clip to
`rect`, maps `FPDF_LCD_TEXT` → ClearType, `FPDF_GRAYSCALE` → grey colour mode,
`FPDF_ANNOT` → `SetDrawAnnots`, installs a `kView` optional-content context,
and calls `PageView_OnDraw`.

**`CPDFSDK_PageView::matrix_` is written in exactly one place** —
`PageView_OnDraw` (`cpdfsdk_pageview.cpp:77`) — and read in exactly two:
`DrawShadow` (page→device for the highlight) and
`CPDFSDK_FormFillEnvironment::InvalidateRect`, which uses its **inverse** to
map a device rect back to page space (`:100-108`). So **the matrix is stale or
identity until the first draw**, and both readers are draw-order-dependent.
pdfrum has no invalidation channel (§2, D2) and computes the page→device
transform per render, so this ordering hazard simply does not exist — recorded
so its absence is understood as deliberate.

`FPDF_ANNOT` gates only popup painting (`cpdfsdk_baannot.cpp:282-285`);
**widgets draw unconditionally**.

### 1.19 One more hazard worth naming

`FormHandleToPageView` (`fpdf_formfill.cpp:191-201`) calls
`GetOrCreatePageView`, which runs `LoadFXAnnots`, which runs `OnLoad` on every
widget — which can generate appearance streams and run format JS. **So a
read-only-looking API like `FORM_CanUndo` can mutate the document on first
call.** pdfrum's `Form` session is constructed explicitly (§3.2) and page
loading is a separate, visible step, so this does not arise; recorded because
it explains why upstream's `FORM_OnAfterLoadPage` exists and why the
embeddertests call it through `LoadPage`.

Relatedly, `LoadFXAnnots` **disables the core's own AP construction while
building the annot list** (`cpdfsdk_pageview.cpp:594-600`:
`SetUpdateAP(false)` around the `CPDF_AnnotList` construction, restored
after), and then the SDK does it itself via `NewAnnot`'s `/NeedAppearances`
branch and `OnLoad`'s `!IsAppearanceValid()` branch. On a `/NeedAppearances`
page the ordering means `NewAnnot` resets the AP *before* the widget joins the
array and `OnLoad` runs *after* — at which point `IsAppearanceValid()` is now
true (because `GetAPDict()` **creates** `/AP`), so `OnLoad` does not reset a
second time.

### 1.20 The edit control: exactly what the editing half adds to `vt/`

`CPWL_EditImpl` owns a `std::unique_ptr<CPVT_VariableText> vt_`
(`cpwl_edit_impl.h:291`) and delegates **all structural mutation** to it —
`InsertWord`, `InsertSection`, `BackSpaceWord`, `DeleteWord`, `DeleteWords`,
`RearrangeAll`/`RearrangePart`, `GetPrev`/`NextWordPlace`, `GetUp`/
`GetDownWordPlace`, `GetLine`/`SectionBegin`/`EndPlace`, `SearchWordPlace`,
`WordIndexToWordPlace`, `WordPlaceToWordIndex`, `UpdateWordPlace`. Since
`pdfrum-doc/src/vt/` already *is* `CPVT_VariableText`, **the editing half adds
exactly six things**:

1. a caret word-place plus a previous-caret and a sticky desired-x column;
2. a two-place directional selection anchor;
3. an undo stack of *replay-by-re-execution* items;
4. a viewport transform (scroll offset + vertical-alignment padding);
5. a dirty-rect refresh accumulator;
6. the widget/event shell (`CPWL_Wnd`).

That list is this brief's entire new-code budget, and §3 is organized around it.

#### 1.20.1 The place model, and the caret invariant

`CPVT_WordPlace` (`core/fpdfdoc/cpvt_wordplace.h:12-71`) is
`(nSecIndex, nLineIndex, nWordIndex)`, all `int32_t`, all defaulting to `-1`.
Ordering is lexicographic (`:36-59`); `LineCmp` compares only the section and
line (`:61-66`). **pdfrum's `vt` already works in exactly these terms** —
`Layout { sections: Vec<Section> }`, `Section { words, lines }` — so a place is
a `(usize, usize, i32)` over the existing structures.

**The load-bearing invariant: the caret sits *after* word `nWordIndex`, and
`-1` means "before the first word of this line/section".** That is why
`SetText` inserts at `(0, 0, -1)` (`cpwl_edit_impl.cpp:945`), why `Backspace`
deletes the word *at* the caret, and why `Delete` deletes the word at
`GetNextWordPlace(caret)`.

`CPVT_WordRange`'s **two-argument constructor normalizes** (swaps if
`Begin > End`, `cpvt_wordrange.h:18-32`); the default constructor does not.
Normalization therefore happens at range *construction*, which is where the
selection's directionality is discarded — see 1.20.4.

`SetCaret` (`cpwl_edit_impl.cpp:1401-1404`) is **exactly two statements**:

```cpp
wp_old_caret_ = wp_caret_;
wp_caret_ = place;
```

No validation, no clamping, no notification. Everything downstream compares
`wp_caret_` against `wp_old_caret_` to detect a no-op — which is how "the VT
refused the insert because the limit was reached" is signalled.

`caret_point_` (`:300`) is the **sticky desired column** for up/down movement.
`SetCaretOrigin` (`:1949-1965`) re-seeds it from the caret, and it is called
after every *non-shift* movement and every mutation — but **deliberately not**
during shift-extended movement or up/down, so the column survives a run of
arrow presses.

#### 1.20.2 The mutation postlude — one rigid sequence, three deviations

Every mutation shares this shape, and it must be ported verbatim because each
step is observable:

```
UpdateWordPlace(caret)  →  SetCaret(vt.Op(...))  →  selection.set(caret, caret)
  →  [bail if caret unchanged]  →  [record one undo item]
  →  RearrangePart(range)  →  ScrollToCaret()  →  Refresh()
  →  SetCaretOrigin()  →  SetCaretInfo()
```

The three deviations, all real:

- **`Delete` has no unchanged-caret bail-out** (`:1770-1802`), because
  `DeleteWord` leaves the caret in place by design.
- **`Backspace` passes its range as `(new, old)`** rather than `(old, new)`
  (`:1763`) — harmless, since the range constructor normalizes, but recorded so
  a reviewer does not read the Rust as a transcription error.
- `InsertWord`/`InsertText` route through the `PaintInsertText` helper
  (`:1857-1866`) while `InsertReturn`/`Backspace`/`Delete`/`Clear` inline the
  identical five calls.

Per-operation specifics:

| Op | `file:LINE` | Notes |
|---|---|---|
| `SetText` | `:943-946` | private `Clear()` then `DoInsertText((0,0,-1), …)`. **Never adds an undo item, never repaints** — `CPWL_Edit::SetText` adds the `Paint()` |
| `DoInsertText` | `:2030-2059` | `\r` → section break, **and if the next char is `\n`, skip it**; lone `\n` or `\r` → one break each; `\t` → a literal space; else `InsertWord`. **pdfrum's `vt::split_sections` already implements this exact tokenizer** (`vt/mod.rs`, the four documented rules) |
| `InsertWord` | `:1695-1716` | bails on `IsTextOverflow()`; the undo item stores the **original** `charset` argument, not the resolved one |
| `InsertReturn` | `:1718-1739` | |
| `Backspace` | `:1741-1768` | **captures the word at the caret before deleting** (`:1746-1751`) so the undo item can restore it |
| `Delete` | `:1770-1802` | captures the **next** word; snapshots `bSecEnd = (caret == SectionEndPlace(caret))`. The `if/else` at `:1786-1794` is **dead code** — both branches construct the identical object |
| `ClearSelection` | `:1815-1834` | records `UndoClear(range, GetSelectedText())` **before** mutating |
| `InsertText` | `:1836-1855` | |

#### 1.20.3 The undo model — the definitive granularity answer

**Constants** (`cpwl_edit_impl.h:221-231`):

```cpp
static constexpr int kEditUndoMaxItems = 10000;
static constexpr int kMinEditUndoMaxItems = 4;
static_assert(kEditUndoMaxItems >= kMinEditUndoMaxItems,
              "CPWL_EditImpl::ReplaceSelection() inserts a group of several "
              "undo items, which must fit in the queue.");
```

The minimum of **4** is exactly the worst-case `ReplaceSelection` group:
sentinel + `UndoClear` + `UndoInsertText` + sentinel.

**Granularity: one item per character, never coalesced.** Every `InsertWord`
(a single `uint16_t`) makes one `UndoInsertWord`; every `Backspace` one
`UndoBackspace`; every `Delete` one `UndoDelete`. **There is no timer, no
run-merging and no typing-burst heuristic anywhere in the file** — which is
exactly what the `UndoRedo` embeddertest (§1.5) observes from the outside.

**There is no `BeginGroupUndo`/`EndGroupUndo` API. That idiom does not exist
here.** Grouping is a pair of **sentinel objects** (`UndoReplaceSelection`,
`:390-403`) pushed before and after a variable-length run, both of which are
no-ops (`Undo(){}`, `Redo(){}`) and answer `IsSentinel() → true` (`:399`). The
base `UndoItemIface::IsSentinel` returns false (`:189-191`), and its comment is
the spec: "the undo stack will continue performing undo/redo operations until
the next sentinel undo item."

`UndoStack::Undo` (`:201-223`), **decrement-then-act**:

```cpp
bool first_undo = true;
while (CanUndo()) {                       // cur_undo_pos_ > 0
  --cur_undo_pos_;
  item = undo_item_stack_[cur_undo_pos_];
  item->Undo();
  if (first_undo) { first_undo = false; if (!item->IsSentinel()) break; }
  else            { if (item->IsSentinel()) break; }
}
```

If the top item is *not* a sentinel, exactly one item is undone. If it *is*
(we are at the closing end of a group), the loop keeps going and stops **after**
consuming the opening sentinel. `Redo` (`:229-251`) is symmetric but
**acts-then-increments**. A `working_` flag `CHECK`s against reentrancy — which
is why every undo/redo body passes `bAddUndo = false`.

**Eviction is group-atomic.** `AddItem` (`:258-271`) drops the redo branch
first (`RemoveTails`, `:291-297`) and then, if at capacity, calls
`RemoveHeads` (`:273-289`): if the oldest item is a plain item, drop one; if
it is a **group's opening sentinel**, drop items until and including the next
sentinel. **A group is never left half-evicted.** The
`ReplaceSelectionUndoQueueLimit` test (`cpwl_edit_embeddertest.cpp:611-639`)
pins it with `SetMaxUndoItemsForTest(4)`.

The seven concrete items, and what each stores and replays:

| Item | `file:LINE` | Stores | `Redo()` | `Undo()` |
|---|---|---|---|---|
| `UndoInsertWord` | `:299-347` | old/new place, `word`, `charset` | `SetCaret(old); InsertWord(word, charset, false)` | `SetCaret(new); Backspace(false)` |
| `UndoInsertReturn` | `:349-388` | old/new place | `SetCaret(old); InsertReturn(false)` | `SetCaret(new); Backspace(false)` |
| `UndoBackspace` | `:405-456` | old/new, `word`, `charset` | `SetCaret(old); Backspace(false)` | `SetCaret(new)`, then **`new.nSecIndex != old.nSecIndex ? InsertReturn : InsertWord`** |
| `UndoDelete` | `:458-513` | old/new, `word`, `charset`, `sec_end_` | `SetCaret(old); Delete(false)` | `SetCaret(new)`, then `sec_end_ ? InsertReturn : InsertWord` |
| `UndoClear` | `:515-553` | range, the removed text | `SetSelection(range); Clear(false)` | `SetCaret(range.begin); InsertText(text); **SetSelection(range)**` |
| `UndoInsertText` | `:555-603` | old/new, text, charset | `SetCaret(old); InsertText(text, charset, false)` | `SetSelection(old, new); Clear(false)` |
| `UndoReplaceSelection` | `:390-403` | *(nothing)* | `{}` | `{}`; `IsSentinel() → true` |

**Undo is by replay of the inverse operation against the live layout, never by
snapshot restore.** `UndoInsertText::Undo` re-derives the region as
`SetSelection(old, new)` + `Clear` rather than deleting a recorded length. A
port that stores before/after strings will drift, because the places are
recomputed against a layout that intervening operations may have rearranged.
This is D5's other half.

Two asymmetries and one quirk, recorded rather than cloned:

- `UndoBackspace::Undo` **re-derives** "was this a paragraph merge?" from
  `new.nSecIndex != old.nSecIndex` at replay time, while `UndoDelete`
  **snapshots** it into `sec_end_` at record time. Same intent, two
  mechanisms; pdfrum uses the snapshot for both (D12).
- **`UndoClear::Undo` restores the selection** after reinserting — this is the
  mechanism behind §1.5.1's "undo restores the selection". Redo does not,
  because `UndoClear::Redo` ends at `Clear(false)`.
- `AddEditUndoItem` (`:2069-2072`) **does not check `enable_undo_`**; every
  call site checks it first *except* the sentinel pushes (`:1869`, `:1878`,
  `:1886`, `:1889`, `:1911`, `:1923`). So with undo disabled,
  `ReplaceSelection` still pushes two unreachable sentinels. pdfrum's
  `push_undo` checks the flag once, at the single entry point (D12).

#### 1.20.4 The two composite operations, and the typing entry point

`ReplaceSelection(text)` (`:1881-1890`) — four calls, and the comment at
`:1882-1885` explains the sentinels: the two middle calls each *optionally*
add an item, so the group length is variable and must be bracketed.

```cpp
AddEditUndoItem(UndoReplaceSelection());   // opening sentinel
ClearSelection();                          // 0 or 1 item
InsertText(text, FX_Charset::kDefault);    // 0 or 1 item
AddEditUndoItem(UndoReplaceSelection());   // closing sentinel
```

`ReplaceAndKeepSelection(text)` (`:1868-1879`) is the same bracket with the
selection re-established over the inserted text:

```cpp
AddEditUndoItem(UndoReplaceSelection());
ClearSelection();
CPVT_WordPlace caret_before_insert = wp_caret_;
InsertText(text, FX_Charset::kDefault);
CPVT_WordPlace caret_after_insert = wp_caret_;
sel_state_.Set(caret_before_insert, caret_after_insert);
AddEditUndoItem(UndoReplaceSelection());
```

`cpwl_edit_embeddertest.cpp:569-587` pins the observable result including the
**reversed-selection** case: `ABCDEFGHIJ`, `SetSelection(1,3)`,
`ReplaceAndKeepSelection("xyz")` → `AxyzDEFGHIJ` with selection `xyz` and
`GetSelection() == (1,4)`; then `SetSelection(4,1)` — *backwards* —
`ReplaceAndKeepSelection("12")` → `A12DEFGHIJ`, selection `12`,
`GetSelection() == (1,3)`.

`TypeChar` (`:1892-1925`) is what an `OnChar` actually reaches, and its
comments state the intent directly:

```cpp
bool was_selected = IsSelected();
if (word == ascii::kBackspace) {                  // 0x08
  if (was_selected) ClearSelection(); else Backspace();
  return;                                          // exactly ONE undo item
}
if (was_selected) { AddEditUndoItem(UndoReplaceSelection()); ClearSelection(); }
if (word == ascii::kReturn) InsertReturn(); else InsertWord(word, charset);
if (was_selected) AddEditUndoItem(UndoReplaceSelection());
```

**The sentinels are skipped when there is no selection precisely so that
ordinary single-character typing produces exactly one undo item.** That is the
mechanism behind the `UndoRedo` test's one-character-per-step result.

#### 1.20.5 Selection invariants

`SelectState` (`cpwl_edit_impl.h:172-186`, impl `:2099-2126`) is two bare
places:

- `BeginPos` — the **anchor** (fixed end).
- `EndPos` — the **active** end (moves with the caret).

The rules, each of which becomes a property test (§4.4):

1. **Stored directional, normalized only on read.** `Set(begin, end)` is a
   plain assignment with no ordering (`:2114-2118`); `BeginPos > EndPos` is a
   legal backwards selection. Normalization happens in `ConvertToWordRange()`
   (via the range constructor, `:2105-2107`) and in `GetSelection()`'s ordered
   pair (`:849-854`).
2. **Empty means `BeginPos == EndPos`, not "reset".** After every mutation and
   after a mouse-down the code does `sel_state_.Set(caret, caret)` — a
   *collapsed* selection at the caret, which `IsEmpty()` reports as empty but
   which is a **live anchor** for a following shift-move or drag. That is a
   different state from `Reset()` (both places `-1`), and the difference is
   observable.
3. **The shift-extend idiom is uniform** across Up/Down/Left/Right/Home/End:
   ```cpp
   if (sel_state_.IsEmpty()) sel_state_.Set(wp_old_caret_, wp_caret_);
   else                      sel_state_.SetEndPos(wp_caret_);
   ```
   Starting from empty anchors on `wp_old_caret_` — the position *before* this
   keystroke.
4. **Typing over a selection does not delete implicitly at the insert level.**
   `InsertWord`/`InsertReturn`/`InsertText` simply overwrite the selection with
   a collapsed one; it is `TypeChar` that calls `ClearSelection()` first.
5. **`Delete` with a selection is routed at the widget level**: `CPWL_Edit::
   OnKeyDownInternal` rewrites `FWL_VKEY_Delete` to `FWL_VKEY_Unknown` when a
   selection exists (`cpwl_edit.cpp:517-519`), and the `Unknown` case calls
   `ClearSelection()` (`:570-572`). `FWL_VKEY_Unknown == 0`.
6. **`SetSelection(0, -1)` is the "select all" magic pair**
   (`cpwl_edit_impl.cpp:808-809`); `nStartChar < 0` means select none
   (`:810-811`); otherwise the two indices are ordered before use
   (`:813-819`).
7. **Caret visibility is selection-dependent.** `SetCaretInfo` passes
   `sel_state_.IsEmpty()` as the visibility flag (`:1430`), and
   `CPWL_Edit::SetCaret` forces invisible when `!IsFocused() || IsSelected()`
   (`cpwl_edit.cpp:704-706`). **A field with a selection shows no caret.**
8. `SelectNone()` **early-outs on an already-empty selection** and does not
   refresh (`:1074-1081`).

`GetRangeText` (`:893-921`) and `GetText` (`:869-891`) emit **`"\r\n"`** at
every section change — note it is emitted *after* the word, on the transition
*into* the new section.

#### 1.20.6 Movement

There is **no page-up/page-down in the edit at all**; `FWL_VKEY_Prior`/`Next`
are not handled (`cpwl_edit.cpp:509-576`). The operations are:

| Op | `file:LINE` | Behavior |
|---|---|---|
| `OnVK_UP` / `OnVK_DOWN` | `:1470-1518` | `vt.GetUp/DownWordPlace(caret, caret_point_)` — the **sticky column**. `SetCaretOrigin()` is deliberately **not** called, in either branch |
| `OnVK_LEFT` shift | `:1525-1541` | first the **line-wrap skip** (if at a line begin that is not a section begin, step back once), *then* `GetPrevWordPlace` |
| `OnVK_RIGHT` shift | `:1571-1588` | **step first, then skip** — the inverse order |
| `OnVK_LEFT/RIGHT` no shift, selection live | `:1543-1552`, `:1590-1599` | **collapse to the left/right edge** of the selection; no `SetCaretOrigin()` |
| `OnVK_HOME/END` | `:1613-1693` | `bCtrl` selects document begin/end instead of line begin/end. Shift branch refreshes **unconditionally**, with no `caret != old_caret` guard, unlike the arrows |
| `OnMouseDown` | `:1436-1449` | `SelectNone()`, caret to `SearchWordPlace(EditToVT(point))`, **anchor dropped at the caret**. **`bShift` and `bCtrl` are accepted and completely ignored** |
| `OnMouseMove` | `:1451-1468` | caret to the hit place; **bails if unchanged**; `SetEndPos(caret)` — drag-extend. Flags also ignored. The only mover that calls `SetCaretOrigin()` while extending |

#### 1.20.7 Cut, copy and paste do not exist in this layer

**The clipboard is entirely the embedder's job.** There is no cut/copy/paste
inside `fpdfsdk/pwl/`. The paths are `FORM_GetSelectedText` → …
→ `edit_impl_->GetSelectedText()` for copy, and `FORM_ReplaceSelection` → …
→ `ReplaceSelection(text)` for paste; a cut is the embedder doing both.

`CPWL_Edit::OnCharInternal` explicitly **refuses** Ctrl/Cmd-modified
characters — `if (IsPlatformShortcutKey(nFlag) && !IsALTKeyDown(nFlag)) return
false;` (`cpwl_edit.cpp:595-597`) — so Ctrl+C/X/V fall through untouched. Only
Ctrl+A, Ctrl+Z, Ctrl+Shift+Z and (non-Apple) Ctrl+Y are handled internally,
and those are in `OnKeyDown`, exactly as §1.6 records from the tests.

**The complete `OnChar` rejection set** (`cpwl_edit.cpp:585-601`) is: **LF
(0x0A), ESC (0x1B), DEL (0x7F)**, plus any platform-shortcut-modified
character without Alt. CR (0x0D) and Backspace (0x08) **pass through** and are
dispatched by `TypeChar`. A read-only field **consumes** the character and
does nothing (returns `true`, not `false`). The DEL rejection is asserted:
`cpwl_edit_embeddertest.cpp:762-763` with the comment "Embedders may send an
extra OnChar event for delete key presses, which PDFium must ignore."

`CPWL_Edit::OnKeyDownInternal`'s dispatch table (`:509-576`):

```
Delete (0x2E) → Delete()                Up (0x26)   → OnVK_UP(shift)
Down (0x28)   → OnVK_DOWN(shift)        Left (0x25) → OnVK_LEFT(shift)
Right (0x27)  → OnVK_RIGHT(shift)       Home (0x24) → OnVK_HOME(shift, ctrl)
End (0x23)    → OnVK_END(shift, ctrl)
A (0x41) → shortcut && !shift && !alt ? SelectAllText(), true : false
Y (0x59) → [non-Apple only] shortcut && !shift && !alt ? Redo(), true : …
Z (0x5A) → shortcut && !alt ? (shift ? Redo() : Undo()), true : false
Unknown (0) → ClearSelection()
default → false
```

`IsPlatformShortcutKey` is `IS_APPLE ? IsMETAKeyDown : IsCTRLKeyDown`
(`cpwl_wnd.cpp:141-147`) — the mechanism behind §1.6's `kModifier`.

#### 1.20.8 Password, comb, limits and auto-size

- **The password substitution character is the ASCII asterisk `'*'` (0x2A)**,
  hardcoded at `cpwl_edit.cpp:125`. Substitution happens at **render time
  only** (`GetPDFWordString`, `cpwl_edit_impl.cpp:2084-2085`, bypassing the
  char-code lookup); the stored text is unaffected, so `GetText`,
  `GetSelectedText` and every undo item carry plaintext. pdfrum's
  `vt::Config::sub_word: Option<char>` already models exactly this
  ("A password field still records the real one; only its width and its
  output byte come from the substitute", `vt/mod.rs`).
- **Comb** (`kEditCharArray`) sets `vt_->SetCharArray(n)` and forces
  `SetTextOverflow(true)`. When auto-sizing, the size comes from
  `GetCharArrayAutoFontSize` (`cpwl_edit.cpp:263-277`):
  ```cpp
  if (!font || font->IsStandardFont()) return 0.0f;
  const FX_RECT& bbox = font->GetFontBBox();
  float xdiv = rcCell.Width() / nCharArray * 1000.0f / bbox.Width();
  float ydiv = -rcCell.Height() * 1000.0f / bbox.Height();
  return xdiv < ydiv ? xdiv : ydiv;
  ```
  Note the `1000.0f` glyph-space scale, the negated `ydiv`, and the
  **standard-font early return of 0**. Cell separators are drawn as
  `nCharArray - 1` vertical lines at `left + width * (i + 1)`, stroked with
  the border width and colour and `EvenOddOptions()`
  (`cpwl_edit.cpp:150-202`) — **`ap/field_body.rs` already generates these**.
- **`/MaxLen` is routed two ways** (`cffl_textfield.cpp:98-107`):
  `SetCharArray(n)` **if** the comb style is set, else `SetLimitChar(n)`; and
  `n <= 0` means unlimited. Enforcement is at VT level, *refusing* the insert
  (`cpvt_variabletext.cpp:226-232`, `:243-248`), which surfaces as
  "caret unchanged" and therefore **no undo item**.
- `IsTextFull()` (`:1975-1982`) is
  `IsTextOverflow() || (limit > 0 && total >= limit) || (array > 0 && total >= array)`.
  `IsTextOverflow()` (`:1984-2000`) is a **geometric** limit that applies only
  when neither scrolling nor overflow is enabled.
- Vertical alignment (`PEAV_TOP/CENTER/BOTTOM` = 0/1/2) is consumed **only**
  as a padding term in the viewport transform (`:1087-1129`) —
  `0`, `(plateH − contentH) × 0.5`, `plateH − contentH` — which is precisely
  the `offset` argument `ap::field_body` already passes to
  `vt::edit_ap::generate`, exactly as SPEC §10's E1 revision predicted.

#### 1.20.9 Scrolling, and what a `.evt` fixture can observe of it

The edit's own scroll state is one point, `scroll_pos_point_`, plus the
alignment padding, giving the `VTToEdit`/`EditToVT` pair (`:1087-1137`).

`SetScrollInfo` (`:1139-1160`) publishes:
`fPlateWidth = plate height` (the name is wrong upstream), `fContentMin/Max`
= content bottom/top, **`fSmallStep = plate height / 3`**, **`fBigStep = plate
height`**. `SetScrollLimit` (`:1207-1234`) clamps in both axes.
`ScrollToCaret` (`:1236-1286`) runs after essentially every mutation and
movement, with **doubled conditions** on the y-axis to prevent thrashing when
the caret is taller than the plate.

Scroll-bar geometry constants: `kWidth = 12.0f`, `kTransparency = 150`
(`cpwl_scroll_bar.h:80-81`), `kButtonWidth = 9.0f`,
`kPosButtonMinWidth = 2.0f` (`cpwl_scroll_bar.cpp:23-24`). The bar's internal
coordinate is `fContentMax - editY`, flipped on the way in
(`SetScrollPosition`, `:270-273`) and out (`NotifyScrollWindow`, `:490-498`).
Arrow-button auto-repeat is **100 ms**; the thumb has a **1-unit dead zone**
(`:463-465`).

**For the two `scrollable_widgets*.evt` fixtures the numbers that matter are
these**: `FORM_OnMouseWheel` on a multiline field moves by exactly **one
`GetFontSize()` per notch** (`cpwl_edit.cpp:407-413`); a click in the page
area moves by `fBigStep` = one plate height; an arrow-button click moves by
`fSmallStep` = a third of a plate height, repeating every 100 ms. And a scroll
is observable **only through rendering** — which words are drawn is
`GetVisibleWordRange()` (`:996-1013`) and where is `VTToEdit`. That is exactly
what a pixel golden compares, and it is why those two fixtures are the only
`.evt` scroll coverage.

**pdfrum has no invalidation channel** (D2), so the whole `RefreshState`
dirty-rect accumulator (`cpwl_edit_impl.h:145-170`) and the 1-unit inflation
in `CPWL_Wnd::InvalidateRect` (`cpwl_wnd.cpp:303`) are **not ported**. They
exist to tell an embedder which pixels changed; pdfrum's caller re-renders a
page and diffs whole pixmaps, which is what the conformance harness does
anyway. This is the single largest deletion in the milestone and §2 argues it.

#### 1.20.10 List box and combo box interaction

`CPWL_ListCtrl` is **a stack of one-line `CPWL_EditImpl`s** — each `Item` owns
a full edit with `SetAlignmentV(1)` (`cpwl_list_ctrl.cpp:23-26`). So the edit
engine is a prerequisite for the list box, not a sibling.

Selection is staged through a map before being committed:
`SelectState { std::map<int32_t, State> }` with
`State { DESELECTING = -1, NORMAL = 0, SELECTING = 1 }`
(`cpwl_list_ctrl.h:116-136`); `SelectItems()` (`:409-416`) commits the
non-`NORMAL` entries into each `Item::selected_` and then downgrades the map.
pdfrum uses a `BTreeSet<usize>` plus a staged delta (§3.4).

**Multi-select mouse rules** (`OnMouseDown`, `:172-210`):

- **Ctrl**: toggle the hit item; `ctrl_sel_` records the polarity (adding vs
  removing) for the drag that may follow; **`foot_index_` is re-anchored**.
- **Shift**: `DeselectAll(); Add(foot_index_, hit); SelectItems();` — and
  **`foot_index_` is *not* updated**, so successive shift-clicks all pivot on
  the same anchor.
- **No modifier**: deselect all, select the hit item, re-anchor.
- **Single-select ignores both modifiers entirely.**

`OnMouseMove` drag (`:212-240`): with Ctrl, continue the polarity from
mouse-down; otherwise range-select from the anchor — **`bShift` is irrelevant
during a drag**.

**Arrow keys** (`OnVK`, `:242-266`) — and the **empty `if (bCtrl) {}` block at
`:245-246` is deliberate**: Ctrl+arrow changes no selection but **does** still
move the caret, because `SetCaret(nItemIndex)` sits after the chain. Reproduce
it. `OnVK_LEFT` is an alias for Home and `OnVK_RIGHT` for End (`:276-282`) —
there is no horizontal notion in a list.

`OnChar` is **type-ahead** (`:292-301`) via `FindNext` (`:648-665`), a circular
scan from `index + 1` comparing `towupper(first_char)`. **It returns the last
probed index rather than `-1` when nothing matches**, which is why the combo
box's type-ahead in `FocusChanges` (§1.7.3) behaves as independent jumps.

`CPWL_ListBox::GetTopVisibleIndex` (`cpwl_list_box.cpp:335-338`) **has a side
effect** — it calls `ScrollToListItem(GetFirstSelected())` before reporting. A
pure-query port diverges; pdfrum keeps the side effect and names it
`top_visible_index_scrolling` (D13). And `CPDFSDK_Widget::SetTopVisibleIndex`
is an empty stub (§1.16), so the scroll position is **never persisted**.

The mouse wheel over a list box **changes the selection rather than scrolling
the view** (`cpwl_list_box.cpp:357-368`).

> **Added 2026-09-01 (M14 block 3).** Two things that paragraph leaves out,
> and the port needed both.
>
> `OnMouseWheel` passes `IsSHIFTKeyDown(nFlag)` and `IsCTRLKeyDown(nFlag)`
> into `OnVK_DOWN`/`OnVK_UP`, and those flags decide what a **multi-select**
> list does with the row it lands on. `CPWL_ListCtrl::OnVK`
> (`cpwl_list_ctrl.cpp:242-265`) branches three ways: **Ctrl's body is
> empty** — only the caret moves, which is how a caret is walked to a row
> before the row is toggled; Shift deselects all and adds the run from
> `foot_index_` to the new row; neither deselects all, selects the one row,
> and re-anchors. A single-select list ignores all three, because that whole
> structure sits inside `IsMultipleSel()`. The arrow keys take the same path,
> since `OnKeyDown` (`:103-119`) passes the same two flags to the same
> functions — so the wheel and the arrow keys are **one** operation upstream
> and must not diverge in a port.
>
> And a **combo box is not a list under the wheel**. It has no
> `OnMouseWheel` override at all, so it falls through to
> `CPWL_Wnd::OnMouseWheel` (`cpwl_wnd.cpp:412-429`), which returns `false`
> unless a child holds the keyboard capture — and a closed combo's list
> window is not shown. Sharing the list's arm with it lets a wheel notch
> silently change a committed value.

**The combo box dropdown state machine** — `SetPopup(bool)`
(`cpwl_combo_box.cpp:325-377`):

1. idempotent: same state, or no list → `true`;
2. **an empty list never opens** (`fListHeight > 0` required, `:333-336`);
3. closing: restore `old_window_rect_`;
4. opening fires `OnPopupPreOpen`, **which can veto** (`:341-343`);
5. `fPopupMin` = `count > 3 ? first_height * 3 + border*2 : 0` — **the minimum
   popup shows three rows, and only when there are more than three items**;
   `fPopupMax` = list height + border×2;
6. the **embedder decides** the direction and final size via
   `QueryWherePopup`, whose implementation clamps into
   `[fPopupMin, fPopupMax]` against `kMaxListBoxHeight = 140`
   (`cffl_interactiveformfiller.cpp:670-729`);
7. grow the window down or up, move, then `OnPopupPostOpen`.

Open/close triggers: the button toggles; a click-up on a list item does
`SetSelectText(); SelectAllText(); edit->SetFocus(); SetPopup(false);`;
**losing focus always closes** (`:52-58`); Enter toggles; **Space opens only
on a non-editable combo** (`:460-472`) — in an editable one it falls through
to the edit as a literal character, which is exactly what
`CheckIfEnterAndSpaceKeyAreHandledOnEditableFormField` (§1.7.3) asserts.

`SetSelectText()` (`:522-527`) is four statements:

```cpp
edit_->SelectAllText();
edit_->ReplaceSelection(list_->GetText());
edit_->SelectAllText();
select_item_ = list_->GetCurSel();
```

Because it routes through `ReplaceSelection`, **every combo-box selection
change pushes a four-item sentinel-bracketed undo group onto the edit's undo
stack.** That is a non-obvious consequence with a direct test consequence, and
it is why the combo `UndoRedo` test's stack behaves as it does.

Finally: **a non-editable combo box's edit child is a *read-only* `CPWL_Edit`**
(`:171-173`) — it still holds the text and supports selection and
`GetSelectedText`, but rejects typing at `OnCharInternal`. That is why
`GetSelectedTextEmptyAndBasicNormalComboBox` (§1.5 sources) can mouse-select a
substring of `"Banana"` in a field the user cannot type into.

Selection colours, shared by the edit and the list:
`crSelBK = ArgbEncode(255, 0, 51, 113)` and
`crWhite = ArgbEncode(255, 255, 255, 255)`
(`cpwl_edit_impl.cpp:620-621`, `cpwl_list_box.cpp:74-78`). These are what the
two `form_textfield_selected_*.evt` goldens contain.

### 1.21 Diagnostics mapping

Every recovery this crate performs emits a `Diagnostic` (STYLE §3). The
mapping, all of them new variants:

| Situation | Diagnostic |
|---|---|
| `.evt` line with an unrecognized verb | `EventVerbUnknown { line, verb }` |
| `.evt` line with the wrong field count | `EventArgsBad { line, verb }` |
| `.evt` mouse line too short (where upstream reads OOB) | `EventArgsBad`, event skipped (D8) |
| `.evt` `mousedown`/`mouseup` with a button other than `left`/`right`, or `mousedoubleclick` with `right` | `EventButtonBad { line, name }` |
| a non-numeric coordinate parsed as `0` by `atoi` | `EventNumberBad { line, field }` |
| an event delivered to a widget whose field type is `Unknown` or `Sig` | `EventNoTarget { page, index }` |
| a keystroke refused because `/MaxLen` was reached | `FieldLimitReached { field, max_len }` |
| a tab-order banding pass that found no candidate (upstream would hang) | `TabOrderDegenerate { page }` (D10) |
| an appearance regeneration that produced no stream for a widget that asked for one | reuses `pdfrum-doc`'s existing `AppearanceGenerated` family |

---

## 2. Divergences

Each entry names what the C++ does, what pdfrum does instead, and the reason.
STYLE §7's test applies throughout: what survives is **behavior**, never shape.

### D1 — No widget object hierarchy. A session record plus functions.

**C++:** four cooperating class families with per-widget heap objects, virtual
dispatch and back-pointers. `CFFL_InteractiveFormFiller` owns a
`std::map<CPDFSDK_Widget*, unique_ptr<CFFL_FormField>>`; each `CFFL_FormField`
owns a `std::map<const CPDFSDK_PageView*, unique_ptr<CPWL_Wnd>>`; each
`CPWL_Wnd` owns children, a `SharedCaptureFocusState` on the heap, an
`IPWL_FillerNotify` back-pointer, and an attached `CFFL_PerWindowData` holding
an `ObservedPtr` back to the widget. Object lifetime is so fragile that the
code re-checks an `ObservedPtr` after *every* callback — sixteen such checks in
`CommitData` and `OnBeforeKeyStroke` alone.

**pdfrum**, per PLAN.md §M14's binding constraint and STYLE §1: a **form
session record** and free functions.

```
FormSession  { focus: Option<FocusTarget>, fields: BTreeMap<FieldId, FieldState>,
               hover: Option<AnnotId>, drag: Option<DragAnchor>,
               dirty: BTreeSet<AnnotId>, config: SessionConfig }
FieldState   = Text(TextEdit) | Choice(ChoiceEdit) | Toggle(ToggleState) | Button(ButtonState)
apply(session, doc, event) -> (session', Vec<AppearanceUpdate>)
```

The `ObservedPtr` web disappears because nothing is a pointer: a focus target
is a `FieldId`, a dirty entry is an `AnnotId`, and a function that mutates the
session cannot invalidate a reference someone else holds. The sixteen
re-checks become **zero**, and they become *unnecessary* rather than
*forgotten* — the type system enforces what the C++ enforces by convention.

The per-page window map (`maps_`) disappears too: a `FieldState` is keyed by
field, not by (field, page-view) pair. The C++ needs the pair because the same
widget can be visible in several views at once, which is a *viewer* concern; a
library that hands back appearance updates has no views.

### D2 — No callback table, no invalidation channel. Updates are returned.

**C++:** `FPDF_FORMFILLINFO` is a 30-slot function-pointer table the embedder
fills in, and the engine pushes work out through it: `FFI_Invalidate` with a
dirty rect, `FFI_SetTimer`/`FFI_KillTimer`, `FFI_SetCursor`,
`FFI_OnFocusChange`, `FFI_DoURIAction`, and so on. Half the complexity of
`CPWL_Wnd` — the `RefreshState` accumulator, the per-line rect pushing, the
1-unit inflation, the `notify_ = nullptr` "Gone, dangling even" protocol
repeated four times — exists to feed `FFI_Invalidate`.

**pdfrum:** `apply` **returns** what changed.

```
AppearanceUpdate { annot: AnnotId, kind: UpdateKind }
UpdateKind = Regenerated(GeneratedAp) | LiveEdit(GeneratedAp) | Cleared | ActionRequested(Action)
```

No dirty rects: the caller re-renders whatever it wants, which is exactly what
the conformance harness does and what every `.evt` golden measures. This
deletes `RefreshState`, `RefreshPushLineRects`, `RefreshWordRange`,
`InvalidateRectMove` and the whole `notify_` lifetime protocol — several
hundred lines of C++ that exist only to describe pixels to someone else.

Timers go with it (M15 gets an explicit step function, D14). Actions
(`DoURIAction`, `DoGoToAction`, named actions) come back as
`UpdateKind::ActionRequested`, which is strictly more useful than a callback:
the caller can inspect, ignore, or execute. `LinkActionInvokeTest`'s
assertions (§1.8) become assertions about the returned vector, and the
modifier bits it pins (`0`, `2`, `1`, `3`) ride along in the `Action`.

**Focus change** likewise becomes an observable rather than a callback, which
resolves the version-1/version-2 split of §1.4 rule 7 by not having versions.

### D3 — One rotation transform, computed once, normalized.

**C++:** four coordinate spaces (§0.1) and two *different*, mutually
inconsistent rotation computations — `CFFL_FormField::GetCurMatrix`
(`cffl_formfield.cpp:442-464`) using `right - left` / `top - bottom`, and
`CPDFSDK_Widget::GetMatrix` (`cpdfsdk_widget.cpp:1044-1067`) using
`abs(rotation % 360)` against `Width()`/`Height()`. `GetRotate()` itself can
return a negative number (`:458-461`).

**pdfrum:** the rotation is normalized to `[0, 360)` once, at the point the
plate rect is computed, and page↔plate is a single `kurbo::Affine` on the
`FieldState`. `ap::field_body::client_rect`
(`crates/pdfrum-doc/src/ap/field_body.rs:175`) already normalizes the deflated
box and already documents the inverted-rect rule that makes
`bug_765384`'s one-by-one field draw; the event path reuses it rather than
introducing a second geometry.

The PWL/window space (§0.1) is erased entirely: there are two spaces, page and
plate, and one affine between them.

### D4 — Field configuration is read once into a record, not into style bits.

**C++:** the widget's `/Ff`, `/Q`, `/MaxLen` and `/DA` are translated into a
32-bit style mask (`cpwl_wnd.h:38-67`) whose low bits are **overloaded across
widget families** — `0x0001` means multiline for an edit, multi-select for a
list box, and allow-custom-text for a combo box — and made safe only by
masking sub-styles off when constructing children (`cpwl_wnd.cpp:170-181`).

**pdfrum:** a `FieldConfig` record per kind, built once when the field is
first touched, with named fields. The overloading cannot arise because the
kinds are separate types. `vt::Config` already carries five of the knobs
(§1.13); `FieldConfig` adds `read_only`, `undo_enabled`, `auto_scroll`,
`do_not_scroll`, `alignment_v` and `max_len: Option<NonZeroU32>` — the last
being how "`n <= 0` means unlimited" stops being a sentinel.

### D5 — The undo item stores the pre-edit selection and the inverse op.

Not really a divergence — it is a *decision to reproduce* something a
reasonable design would get wrong, and it is stated here because the wrong
version is the tempting one. See §1.5.1 and §1.20.3:

- an item records the selection **before** the edit, restores it on **undo**,
  and **does not** restore it on redo;
- undo is **replay of the inverse operation against the live layout**, never a
  snapshot restore, so places are recomputed rather than remembered;
- a paste, a cut and a delete-of-selection are each **exactly one** item
  regardless of length, while a typed character is one item per character;
- `SelectAllText` records nothing.

Four embeddertests fail against the symmetric design. pdfrum implements the
asymmetric one and pins it with a property test that asserts the asymmetry
(§4.4) so a future "cleanup" cannot silently restore symmetry.

### D6 — The accelerator modifier is configuration, not a compile-time platform test.

**C++:** `IsPlatformShortcutKey` is `BUILDFLAG(IS_APPLE) ? IsMETAKeyDown :
IsCTRLKeyDown` (`cpwl_wnd.cpp:141-147`), and Ctrl+Y is redo on non-Apple and
not redo on Apple (`cpwl_edit.cpp:550-559`).

**pdfrum:** `SessionConfig { accelerator: Modifier, redo_on_ctrl_y: bool }`,
defaulting to `Control` and `true` on non-Apple targets and `Meta`/`false` on
Apple — so the *default* matches the oracle on every platform while the tests
can drive both. Without this, half of `SelectAllWithOnKeyDown`,
`UndoWithOnKeyDown` and `RedoWithCtrlYKeyboardShortcut` are unrunnable on any
one machine, and STYLE §1's "no global state" forbids a `cfg!` read from
inside a function anyway.

### D7 — No `FORM_OnKeyUp`, no right-button handlers, no XFA.

`FORM_OnKeyUp` is documented as permanently unimplemented and always returns
false (§1.1); `FORM_OnRButtonDown`/`Up` do nothing outside XFA builds; XFA is
declined by PLAN.md. pdfrum's `Event` enum (§4.2) therefore has no `KeyUp`
variant, and `Event::MouseDown { button: Button::Right, .. }` is accepted by
the parser (the fixtures contain it) and returns `false` with no state change.

This is a *derivation*, not an omission: the `.evt` parser must accept
`mousedown,right,…` because `mouse_events.evt` and `bug_1069789.evt` contain
it, and the correct behavior for those lines is "consume nothing".

### D8 — The `.evt` parser is total; the dead arity guard is replaced by a diagnostic.

Upstream's mouse-verb arity guards are unsatisfiable (§1.2.4), so a short
mouse line reads out of bounds — undefined behavior. pdfrum cannot and must
not reproduce UB. A short mouse line is **skipped with an `EventArgsBad`
diagnostic**; extra trailing fields are ignored exactly as upstream ignores
them. No corpus fixture contains a short mouse line, so the choice is
unobservable against all 59, which is precisely why it may be made on safety
grounds.

Two smaller parser decisions, both recorded so they are not read as leniency
bugs: `'\r'` is **not** stripped (§1.2.2 — a lenient parser would diverge from
the oracle on a CRLF file), and the modifier field uses **substring search**
for `shift`/`control`/`alt` exactly as upstream does, not tokenization.

### D9 — A failing validate loses focus. Reproduced.

§1.11.3's quirk. pdfrum's `commit` returns
`CommitOutcome { committed: bool, reverted: bool }` and the focus-loss path
proceeds on `reverted` exactly as upstream proceeds on `CommitData`'s `true`.
Named in the code and documented, because the alternative reading ("a failed
validate should keep the caret") is what a reader expects and what Acrobat
does. The only thing pdfrum will *not* reproduce is the mechanism — there is
no `false`-means-widget-was-destroyed channel, because nothing can be
destroyed mid-call.

### D10 — Tab-order banding terminates.

Upstream's row-order loop can spin forever (§1.15.2). pdfrum's banding is a
fold that consumes its input: when no candidate is found, the remaining annots
are appended in index order and a `TabOrderDegenerate` diagnostic is emitted.
Every corpus file terminates identically; a hostile file terminates instead of
hanging. **A library that can hang on input is a bug regardless of what the
oracle does** — this is the one place in the brief where pdfrum's behavior is
deliberately *better* rather than identical, and it is the same reasoning
PLAN.md §M15 applies to boa's runtime limits.

**Correction 2026-09-02 (oracle-divergence audit, A64):** the paragraph above
is right about the code and wrong about the category. Under the oracle-bug
rule (PLAN.md §212–229, `81ba24d`) this is not a "divergence kept" at pdfrum's
discretion — it is an **oracle bug**, and the rule makes implementing the
correct behaviour mandatory rather than optional. Verified at the line for the
audit: `cpdfsdk_annotiterator.cpp:137` seeds `float fTop = 0.0f;`, `:140`
tests `rcAnnot.top > fTop`, and `:145-147` `continue`s inside
`while (!sa.empty())` (`:135`) without erasing, so with every remaining top
non-positive the loop state is bit-identical on re-entry and the pass never
returns. §12.5.5 puts `/Tabs R` order in the page's own coordinate space,
where a `/MediaBox` with a zero or negative origin makes a non-positive top
ordinary rather than hostile. pdf.js is silent on `/Tabs` — every widget gets
`tabIndex = 0` (`annotation_layer.js:412`) and the DOM orders them — so it
cannot hang here either. The site now carries `// [oracle-bug]` in `tab.rs`
with both citations. PLAN.md §226 names this ruling as the precedent the
oracle-bug rule generalises, which is why it is the first site to be labelled.

### D11 — The commit cascade is explicit, and notification is not a side channel.

Upstream's four value setters use `kDoNotNotify` and the form-filler drives
validate/calculate/format explicitly (§1.16), while
`CPDF_InteractiveForm`'s *own* setters use `kNotify` and re-enter the cascade
through the `NotifierIface`. Two paths, one of which exists only to be
suppressed.

pdfrum has one path: `commit(session, doc, field) -> (FieldEdit, Vec<AppearanceUpdate>)`
runs the cascade in order and returns. There is no notifier interface and no
suppression flag, so the double-fire the C++ guards against cannot occur, and
the `busy_` recursion guard (`cpdfsdk_interactiveform.h:117`) is replaced by
M15 passing a `depth` down its calculate walk (§3.6).

### D12 — Small cleanups where the C++ is redundant, each named.

Reproducing behavior does not mean reproducing dead code. These are the four
places pdfrum simplifies, all verified unobservable:

- `Delete`'s identical if/else undo construction (`:1786-1794`) collapses to
  one call.
- `UndoBackspace` uses the **snapshot** mechanism (`sec_end_`) that
  `UndoDelete` uses, rather than re-deriving from section indices at replay.
  Same result; one mechanism instead of two.
- `push_undo` checks the enable flag **once**, at the entry point, so a
  disabled stack receives no sentinels either.
- `CFFL_ComboBox::SaveData`'s discarded `GetSelectedIndex(0)` and
  `CFFL_TextField::SaveData`'s unused `sOldValue` are simply not written; and
  `CFFL_ListBox::SavePWLWindowState`'s missing `clear()` (which makes repeated
  saves accumulate indices, `cffl_listbox.cpp:190-201`) is fixed, since a
  saved state that grows is a leak with no observable consumer.

### D13 — Side effects that look like queries keep their side effects, and are named for it.

`GetTopVisibleIndex` scrolls before reporting (§1.20.10). pdfrum keeps the
behavior and names the function `top_visible_index_scrolling(&mut …)`, taking
`&mut` so the signature tells the truth. Two list-box assertions depend on it.

### D14 — Timers are M15's, and they are a step function.

`FFI_SetTimer`/`FFI_KillTimer` and the caret's 500 ms blink and the scroll
bar's 100 ms repeat all serve a live UI. A generated appearance has no blink
and a `.evt` replay has no wall clock — the oracle's own determinism recipe
freezes the clock at `--time=1399672130`. pdfrum exposes no timer in M14. M15,
which needs `app.setTimeInterval`, gets an explicit
`advance_time(&mut session, ms) -> Vec<AppearanceUpdate>` — the same shape as
the embeddertests' `EmbedderTestTimerHandlingDelegate::AdvanceTime`, which is
how all eleven V8-gated tests are written.

---

## 2b. The crate boundary — recommendation and argument

PLAN.md §M14 leaves this open: "Lives in `pdfrum-doc` (or a `pdfrum-form`
crate if the brief argues the boundary; `[spec]` either way)."

**Recommendation: a new crate, `pdfrum-form`, depending on `pdfrum-doc`.**

### The dependency argument

The decisive question PLAN.md names is *"it must not pull rendering into
doc"*. Check what the event path actually needs that the appearance path does
not:

| Need | Available in `pdfrum-doc` today? |
|---|---|
| field data model (`Form`, `Field`, `Widget`, `FieldKind`, `FieldFlags`) | **yes** — `form/field.rs` |
| variable-text layout (`vt::`) | **yes** |
| appearance generation (`ap::field_body`, `ap::widget`) | **yes** |
| font metrics for hit testing a character position | **yes**, through the same `TextFont` `ap::field_body` already resolves |
| the annotation list and `/Rect`s | **yes** — `annot/list.rs` |
| **page geometry to normalize page space** | `pdfrum-page` — already a dependency of `pdfrum-doc` |
| **rasterization** | **no, and never** |

So the event path needs **nothing** that `pdfrum-doc` lacks. Putting it in
`pdfrum-doc` would pull in no new dependency, and the "don't drag rendering
into doc" hazard does not materialize either way, because pdfrum's design
returns appearance *streams* rather than pixels (D2). **The dependency
argument alone does not decide it.** Saying so plainly matters, because the
easy version of this argument — "a new crate keeps rendering out of doc" — is
not true here and should not be offered.

### The argument that does decide it

Three reasons, in descending weight.

**1. Cohesion, measured against STYLE §4's own bar.** STYLE §4 requires that
"the public API of every crate fits in one `lib.rs` re-export block a reviewer
can read in one screen." `pdfrum-doc` is at 4093 lines of design brief, 48
source files and a `lib.rs` re-export block that already runs to 15 names
across bookmarks, name trees, destinations, actions, file specs, links, the
structure tree, page labels, viewer preferences, metadata, annotations,
appearance generation, the AcroForm data model and the variable-text engine.
It is the largest crate in the project after `pdfrum-page`, and it is the one
whose brief already needed three `[spec]` revisions to keep its scope legible.
Adding a caret, a selection model, an undo stack, an event enum, a hit-test
router, a tab-order iterator and a `.evt`-shaped event API puts a **second
complete subsystem** behind the same door. That is precisely the "context
object with 15 responsibilities" shape STYLE §1 names as the anti-pattern to
erase, applied at crate scale.

**2. The dependency runs one way, and only one way.** `pdfrum-form` needs
`pdfrum-doc`'s data model, layout engine and appearance generators.
`pdfrum-doc` needs **nothing** from `pdfrum-form` — no field of `Form`,
`Field`, `Widget`, `AnnotOverlay` or `GeneratedAp` would gain an event-related
member, and no function in `ap/` or `vt/` would gain an event-related
argument. A boundary with a genuinely acyclic, one-directional dependency and
no shared mutable state is a boundary that costs nothing to draw and buys
enforcement: the compiler makes "the appearance generator does not know about
carets" a fact rather than an intention.

**3. It keeps M14's scope reversal legible.** SPEC §10 currently says, in
force, that `fpdfsdk/pwl` is out of scope for `pdfrum-doc` — "the editing
widgets, the caret, the scroll bars, the focus machinery. None of it is
reachable from a generated appearance." That sentence is *correct about
`pdfrum-doc`* and stays correct if the editing machinery lands in a different
crate. Implementing it inside `pdfrum-doc` would require deleting a standing
ruling and rewriting SPEC §10's scope paragraph; implementing it in
`pdfrum-form` requires **adding** a section (SPEC §15) and leaving §10's
sentence true as written, with one clarifying clause. The smaller spec change
is the honest one, and it leaves the record readable: `pdfrum-doc` is what a
*document* is; `pdfrum-form` is what *interacting with* one is.

### The counter-argument, stated fairly

The strongest case for `pdfrum-doc` is that `FieldState` and `Field` are
intimate: committing an edit produces a `FieldEdit`, which is `pdfrum-doc`'s
type, and regenerating an appearance calls `ap::widget::generate_with_text`,
which is `pdfrum-doc`'s function. A separate crate means those are public API
rather than internal calls.

They already are. `form::apply`, `FieldEdit`, `GeneratedAp`,
`ap::widget::generate_with_text` and `vt::Config` are all `pub` today, because
the facade and `pdfrum-edit` already consume them (`crates/pdfrum/src/form.rs`
uses `form::Form::load`, `FieldValues`, `Field`). The boundary is already
drawn at exactly the right place; `pdfrum-form` becomes a second consumer of a
surface that already has one. No type moves, no signature changes.

The second counter-argument — "one more crate in the publish order" — is
real but small: `docs/status/M8.md` records the bottom-up family publish, and
`pdfrum-form` slots between `pdfrum-doc` and `pdfrum` with no back-edge.

### What lands where

| Crate | Gains |
|---|---|
| `pdfrum-doc` | **three additive changes only**: `FieldFlags` gains `is_editable_combo` (`kChoiceEdit = 1 << 18`), `is_multi_select` (`kChoiceMultiSelect = 1 << 21`) and `do_not_scroll` (`kTextDoNotScroll = 1 << 23`); `ap::field_body` gains a `caret_and_selection: Option<&Highlight>` parameter so a *focused* field can be drawn with its selection band; and `vt` gains three pure query functions the editor needs — `place_at_point`, `point_at_place`, `word_index_of_place`/`place_of_word_index`. All three are additive; nothing existing changes shape. |
| `pdfrum-form` *(new)* | everything else: the session record, the event enum, hit testing, tab order, focus, the four field state machines, the edit control (caret/selection/undo/scroll), the commit cascade, and the M15 seam. |
| `pdfrum` (facade) | `Form::on_mouse_down(...)` and siblings, over a `FormSession` the facade owns. |
| `pdfrum-tool` | `--send-events` and the `.evt` parser (the concurrent agent's work, §4.2). |

**This is a `[spec]` change either way** (PLAN.md says so explicitly), and it
is escalation §5.E3.

---

## 3. Module plan

### 3.1 Files

```
crates/pdfrum-form/
  Cargo.toml            deps: pdfrum-common, pdfrum-object, pdfrum-doc,
                              pdfrum-page, pdfrum-font, kurbo, thiserror
  src/
    lib.rs              the one-screen re-export block (STYLE §4)
    error.rs            Error enum (thiserror), Diagnostic variants (§1.21)
    event.rs            Event, Button, Modifiers, Key — the input vocabulary (§4.2)
    session.rs          FormSession, SessionConfig, FocusTarget, DragAnchor
    dispatch.rs         apply(session, doc, event) -> (session, Vec<AppearanceUpdate>)
    hit.rs              hit testing: annot_at_point, widget_at_point, layout order
    tab.rs              tab order: TabOrder, focus_ring, next/prev (D10)
    focus.rs            set_focus, kill_focus, force_kill_focus, the ordering rules
    geom.rs             page <-> plate affine, rotation normalization (D3)
    field/
      mod.rs            FieldState enum + FieldConfig, read_config()
      text.rs           the text-field machine
      choice.rs         combo + list: options, selection set, dropdown, type-ahead
      toggle.rs         checkbox + radio: /AS state, sibling clearing
      button.rs         push button: pressed state, action dispatch
    edit/
      mod.rs            TextEdit: the record and its invariants
      place.rs          Place, Range, caret arithmetic over vt::Layout
      caret.rs          caret movement: left/right/up/down/home/end, sticky column
      select.rs         the directional anchor, normalization on read
      undo.rs           UndoStack, UndoItem, the sentinel-group machinery
      scroll.rs         scroll offset, alignment padding, scroll_to_caret
      ops.rs            insert/backspace/delete/clear/replace + the shared postlude
    commit.rs           the cascade: is_changed -> keystroke -> validate ->
                        save -> calculate -> format, and the M15 seam
    update.rs           AppearanceUpdate, UpdateKind, regenerate()
```

Fourteen source files plus five in two submodules. Every module is named by
domain; none is `util`, `helpers` or `common` (STYLE §4).

### 3.2 Types beyond SPEC

The whole crate is new, so everything here is a proposed SPEC §15. The
essential shapes:

```rust
/// One interaction session over one document. A record of facts, not a manager.
pub struct FormSession {
    /// The field that owns keyboard input, if any. Document-wide, not per page.
    pub focus: Option<FocusTarget>,
    /// Per-field interaction state, created lazily on first touch.
    pub fields: BTreeMap<FieldId, FieldState>,
    /// The annotation the pointer is currently over (mouse enter/exit).
    pub hover: Option<AnnotId>,
    /// A live mouse drag, if one is in progress.
    pub drag: Option<DragAnchor>,
    /// Fields whose committed value differs from the document's.
    pub dirty: BTreeSet<FieldId>,
    /// Platform and policy switches.
    pub config: SessionConfig,
}

/// Where keyboard input goes. A widget, or a focusable non-widget annotation.
pub enum FocusTarget { Widget(FieldId, AnnotId), Annot(AnnotId) }

/// Newtype ids, never pointers (STYLE §2).
pub struct FieldId(u32);   // index into Form::fields
pub struct AnnotId { pub page: PageIndex, pub index: u32 }

pub struct SessionConfig {
    /// The accelerator modifier: Control everywhere but Apple (D6).
    pub accelerator: Modifier,
    /// Whether Ctrl+Y redoes. False on Apple (D6).
    pub redo_on_ctrl_y: bool,
    /// Which annotation subtypes join the tab ring. Default: [Widget].
    pub focusable: Vec<Subtype>,
    /// Depth and size caps, incl. max_undo_items (default 10_000).
    pub limits: Limits,
}

/// Per-field interaction state. One variant per behaviour family, not per
/// PDF field type: combo and list share a machine, check and radio share one.
pub enum FieldState {
    Text(TextEdit),
    Choice(ChoiceEdit),
    Toggle(ToggleState),
    Button(ButtonState),
}

/// The edit control. Six fields, and its invariant is one sentence:
/// `caret` and both ends of `selection` are valid places in `layout`.
pub struct TextEdit {
    pub text: String,
    pub layout: vt::Layout,
    pub config: TextConfig,        // vt::Config plus the editor's own switches
    pub caret: Place,
    pub selection: Selection,      // directional; empty iff begin == end
    pub sticky_x: f32,             // the desired column for up/down (§1.20.1)
    pub scroll: (f32, f32),
    pub undo: UndoStack,
}

/// A position in the layout: after word `word` of line `line` of section
/// `section`. `word == -1` means before the first word. (§1.20.1)
pub struct Place { pub section: u32, pub line: u32, pub word: i32 }

/// Stored directional: `begin` is the anchor, `end` the active end.
/// Normalized only on read, by `Selection::range()`. (§1.20.5)
pub struct Selection { pub begin: Place, pub end: Place }

pub struct UndoStack {
    items: VecDeque<UndoItem>,
    pos: usize,                    // items[..pos] are undoable
    max: usize,                    // default 10_000, minimum 4
    enabled: bool,
}

/// Each item replays the inverse operation against the live layout. (§1.20.3)
pub enum UndoItem {
    InsertWord  { old: Place, new: Place, ch: char, before: Selection },
    InsertReturn{ old: Place, new: Place, before: Selection },
    Backspace   { old: Place, new: Place, ch: char, section_break: bool, before: Selection },
    Delete      { old: Place, new: Place, ch: char, section_break: bool, before: Selection },
    Clear       { range: Range, text: String, before: Selection },
    InsertText  { old: Place, new: Place, text: String, before: Selection },
    /// A group boundary. Undo and redo continue past members until the
    /// matching sentinel. There is no Begin/End API. (§1.20.3)
    GroupBoundary,
}

/// What `apply` hands back. The whole output channel (D2).
pub struct AppearanceUpdate { pub annot: AnnotId, pub kind: UpdateKind }
pub enum UpdateKind {
    /// A committed value produced a new appearance stream.
    Regenerated(GeneratedAp),
    /// A focused field's live editor state, with caret and selection band.
    LiveEdit(GeneratedAp),
    /// The widget no longer draws anything generated.
    Cleared,
    /// A link, URI, GoTo or named action fired. The caller decides. (D2)
    ActionRequested { action: Action, modifiers: Modifiers },
    /// Focus moved. Replaces FFI_OnFocusChange. (§1.4 rule 7)
    FocusChanged { from: Option<AnnotId>, to: Option<AnnotId> },
}
```

`ChoiceEdit` carries `options: Vec<String>`, `selected: BTreeSet<usize>`,
`caret_index: Option<usize>` (the "last index acted upon" of §1.7.4),
`anchor: Option<usize>` (the shift pivot, §1.20.10), `top_visible: usize`,
`popup: Option<PopupState>`, and, for an editable combo, a `TextEdit`.

`ToggleState` is `{ state: Vec<u8>, /* the /AS name */ }` and `ButtonState` is
`{ pressed: bool }`. Both are two-line records with obvious invariants, which
is the bar STYLE §1 sets.

### 3.3 Data flow, end to end

The path from the facade's `Form::on_mouse_down` to a regenerated appearance
stream, with the responsible module at each hop:

```
pdfrum::Form::on_mouse_down(page_x, page_y, modifiers)      crates/pdfrum/src/form.rs
  └─ builds Event::MouseDown { button: Left, at, modifiers }        event.rs
  └─ dispatch::apply(&mut session, &doc, event)                  dispatch.rs
       ├─ 1. hit::widget_at_point(doc, page, at)                     hit.rs
       │      layout order (popup 1 / widget 2 / other 5), focused
       │      annot first; DoHitTest gate: signature? visible?
       │      read-only? push-button-or-permissions?         (§1.15)
       ├─ 2. focus::set(session, doc, target)                      focus.rs
       │      old focus commits first; a refused kill aborts;
       │      focus is assigned only after the field accepts (§1.17)
       │      └─ commit::run(session, doc, old_field)            commit.rs
       │           is_changed → keystroke → validate → save
       │           → calculate → format                        (§1.11.2)
       │           └─ M15 seam: Cascade (§3.6)
       ├─ 3. field state lookup or lazy creation                field/mod.rs
       │      FieldConfig::read(widget_dict, form, resolver)
       │      returns None for Unknown/Signature → no state    (§1.12)
       ├─ 4. geom::page_to_plate(field).transform(at)              geom.rs
       ├─ 5. the kind's machine                          field/{text,choice,…}.rs
       │      text: edit::caret::place_at_point → set caret,
       │            clear selection, drop the drag anchor      (§1.20.6)
       └─ 6. update::collect(session, doc)                       update.rs
              for each dirty or focused field:
                focused → ap::field_body with a Highlight  → LiveEdit
                otherwise → ap::widget::generate_with_text → Regenerated
```

Step 6 is where the `form_textfield_focused_ltr.evt` contract of §1.2.7 lives:
**a focused field takes the `LiveEdit` branch, an unfocused one the
`Regenerated` branch**, and the switch is one `if`.

`pdfrum-doc` is called at exactly three places — step 3 reads the field data
model, step 5's text machine calls `vt::` queries, step 6 calls
`ap::field_body`/`ap::widget`. Nothing calls back.

### 3.4 The edit control's internal flow

`edit::ops` implements the shared postlude of §1.20.2 **once**, as a function
the four mutations call:

```rust
/// Runs the common tail of every mutation. Returns whether anything moved.
fn settle(edit: &mut TextEdit, metrics: &Metrics<'_>, range: Range) -> bool
// = relayout(range) → scroll_to_caret() → sticky_x = caret_x() → true
```

so `insert_char`, `insert_return`, `backspace`, `delete` and `clear_selection`
each read as: guard, mutate the text, recompute the caret, collapse the
selection, bail if unmoved, push one undo item, `settle`. The three deviations
of §1.20.2 are three lines, each with a comment naming the behavior (not the
C++ file — STYLE §6).

Undo replay is the same functions with `push: false`:

```rust
fn undo(edit: &mut TextEdit, metrics: &Metrics<'_>) -> bool
// pops until a non-sentinel is consumed, or (starting on a sentinel)
// until the matching sentinel; each item calls the inverse op with push=false
// and then restores `item.before` selection  (§1.5.1, §1.20.3)
```

The "no reentrancy" `CHECK` the C++ needs becomes a `push: bool` parameter
threaded through — the compiler cannot express "not currently replaying", but
it can express "this call does not push", which is the property that matters.

`ChoiceEdit`'s selection staging (§1.20.10's three-state map) collapses to a
`BTreeSet<usize>` plus a `Vec<(usize, bool)>` delta applied in one pass — the
map exists upstream only because `SelectItems()` needs to know which entries
were touched, which a delta vector says directly.

### 3.5 The facade API

`crates/pdfrum/src/form.rs` gains an event surface over a session the caller
owns. The documented example PLAN.md's exit criterion asks for:

```rust
/// A live form-filling session over a document (ISO 32000-1 §12.7).
///
/// Events go in, appearance updates come out. Nothing is pushed to a
/// callback and nothing is drawn: the caller decides what to re-render.
///
/// ```
/// use pdfrum::{Document, FormSession, Modifiers};
///
/// let doc = Document::open("tests/fixtures/text_form.pdf")?;
/// let mut session = FormSession::new(&doc);
///
/// // Click into the text field, then type "ABC".
/// session.on_mouse_move(0, 120.0, 120.0, Modifiers::NONE);
/// session.on_mouse_down(0, 120.0, 120.0, Modifiers::NONE);
/// session.on_mouse_up(0, 120.0, 120.0, Modifiers::NONE);
/// for ch in "ABC".chars() {
///     session.on_char(ch, Modifiers::NONE);
/// }
/// assert_eq!(session.focused_text().as_deref(), Some("ABC"));
///
/// // Dropping focus commits the value and regenerates the appearance.
/// let updates = session.force_kill_focus();
/// assert_eq!(updates.len(), 1);
/// # Ok::<(), pdfrum::Error>(())
/// ```
pub struct FormSession<'a> { /* doc: &'a Document, inner: form::FormSession */ }
```

with `on_mouse_move`, `on_mouse_down`, `on_mouse_up`, `on_double_click`,
`on_mouse_wheel`, `on_focus_at`, `on_key_down`, `on_char`,
`force_kill_focus`, `focused_text`, `selected_text`, `select_all`,
`replace_selection`, `replace_and_keep_selection`, `can_undo`, `can_redo`,
`undo`, `redo`, `set_index_selected`, `is_index_selected`, `focused_annot`,
`set_focused_annot`, and `field_at_point` — one method per `FORM_*` entry that
is not a no-op, named to the Rust API guidelines (no `get_` prefixes, STYLE
§4). Each returns `Vec<AppearanceUpdate>` where the C++ returns `FPDF_BOOL`,
plus a `bool` "consumed" where the tests assert on it.

### 3.6 The M15 seam — named, and it is one type

M15 hangs the JavaScript cascade off M14 "without redesign". The seam is a
**single trait with one method**, and it is the *third* seam in the project
(STYLE §2b closes the seam list at `RenderDevice` and `Resolve`, so this is
itself a `[spec]` change — escalation §5.E4):

```rust
/// The four points where a field's `/AA` scripts can intervene.
///
/// M14 ships exactly one implementation, `NoScripts`, whose every method is
/// the permissive answer — which is what a V8-off PDFium build does
/// (§1.11): `bRC` defaults to true and is never set false, `sChange` is
/// never rewritten, and calculate and format return at their first line.
///
/// M15 adds `BoaScripts`. No other code changes.
pub trait Cascade {
    /// `/AA /K` with `bWillCommit = false`: may rewrite or reject a keystroke.
    fn keystroke(&mut self, _f: &FieldRef, change: Keystroke) -> KeystrokeOutcome {
        KeystrokeOutcome::Accept(change)
    }
    /// `/AA /K` with `bWillCommit = true`: may reject a commit.
    fn keystroke_commit(&mut self, _f: &FieldRef, _value: &str) -> bool { true }
    /// `/AA /V`: may reject a commit.
    fn validate(&mut self, _f: &FieldRef, _value: &str) -> bool { true }
    /// `/AA /C` over `/AcroForm /CO`: may rewrite other fields' values.
    fn calculate(&mut self, _doc: &mut FieldWrites, _trigger: &FieldRef) {}
    /// `/AA /F`: may return a display string that does not change the value.
    fn format(&mut self, _f: &FieldRef, _value: &str) -> Option<String> { None }
}

/// The V8-off behaviour, and M14's only implementation.
pub struct NoScripts;
impl Cascade for NoScripts {}
```

Four properties make this the right seam, each traceable to §1.11:

1. **The four methods are exactly the four gates.** Not five, not three — the
   C++ has precisely `OnBeforeKeyStroke`, `OnKeyStrokeCommit`, `OnValidate`,
   `OnCalculate` and `OnFormat`, and the first two are the same `/AA /K`
   action distinguished only by `bWillCommit` (§1.11.1 step 12 vs §1.11.2
   step 5b), which is why they are two methods over one action.
2. **The defaults *are* the V8-off behaviour**, not a stub of it. `Accept`,
   `true`, `true`, no writes, `None` — that is bit-for-bit what a
   `CJS_RuntimeStub` build does. So M14 is not "the real thing minus JS"; it
   is the real thing with the identity cascade, and M15 substitutes a
   different value for one parameter.
3. **`commit::run` takes `&mut dyn Cascade`** and calls the five methods in
   the order of §1.11.2 with the `ObservedPtr` re-checks replaced by nothing.
   M15 writes `BoaScripts` and changes no call site.
4. **The recursion guard belongs to the implementation, not the seam.**
   `FieldWrites` carries a `depth` that `calculate` may not exceed, replacing
   `busy_` (D11); `NoScripts` never increments it.

The one thing M15 must also add, and which M14 designs the room for: the
`/AcroForm /CO` calculation-order walk (§1.16) is a `Vec<FieldId>` computed
once from `/CO` by `pdfrum-doc` and handed to `calculate`. M14 computes it and
never uses it, so the list is already correct and tested when M15 arrives.

`Event` carries no `Idle` variant (§1.2.6), but `advance_time` (D14) is where
M15's timers land, and it takes the same `&mut dyn Cascade`.

---

## 4. Test plan

### 4.1 The embeddertest arithmetic, corrected

`fpdf_formfill_embeddertest.cpp` holds **139** `TEST_F`s. PLAN.md §M14 says
"only 2 gated on V8"; the measured split is:

| Group | Count | Disposition |
|---|---|---|
| `#ifdef PDF_ENABLE_V8` (`:1132`–`:1361`) | **11** | **M15's.** All are timer/alert/document-action tests |
| `#ifdef PDF_ENABLE_XFA` (`:954`–`:1045`) | 5 | **Out of scope** — XFA declined |
| XFA-fixture tests (`…Bug1055869…Paste`, `…Bug1058653…Paste`) | 2 | Out of scope |
| XFA-gated *expectations* inside otherwise-portable tests (`:1616`, `:2785`, `:3675`) | 3 | **Ported**, taking the non-XFA branch |
| Everything else | **121** | **M14's port target** |

So the honest exit criterion is **121 ported assertions**, plus the three
non-XFA branches, not 137. The eleven V8-gated names, for M15's brief:
`DisableJavaScript`, `DocumentAActions`, `DocumentAActionsDisableJavaScript`,
`Bug551248`, `Bug620428`, `Bug634394`, `Bug634716`, `Bug679649`, `Bug707673`,
`Bug765384`, `Bug1477093`. The five XFA-gated:
`DoNotHandleShortcutsOnKeyDownXFA`, `XFAFormFillFirstTab`,
`XFAFormFillFirstShiftTab`, `XFAFormFillContinuousTab`,
`XFAFormFillContinuousShiftTab`.

**And the file is not the whole corpus.** Four more embeddertest files carry
form-interaction assertions that PLAN.md's survey did not count:

| File | `TEST_F`s |
|---|---|
| `fpdfsdk/pwl/cpwl_edit_embeddertest.cpp` | **45** |
| `fpdfsdk/pwl/cpwl_combo_box_edit_embeddertest.cpp` | **19** |
| `fpdfsdk/pwl/cpwl_special_button_embeddertest.cpp` | **4** |
| `fpdfsdk/formfiller/cffl_combobox_embeddertest.cpp` | **2** |

**70 further tests**, all V8-free, and they are the *best* coverage of exactly
the machinery M14 adds — `cpwl_edit_embeddertest.cpp` is where the undo-queue
limit, the `ReplaceAndKeepSelection` reversed-selection case and the DEL-key
rejection are pinned. They are in scope and are §5.E2's second half.

**Total M14 port target: 121 + 70 = 191 assertions**, against a stated
criterion of 137. The number moved in both directions and the brief states it
rather than letting the milestone discover it.

### 4.2 The `Event` enum — the specification for the concurrent `pdfrum-tool` work

A concurrent agent is implementing the `.evt` parser and `--send-events` in
`pdfrum-tool`, plus the harness cluster. **This section is that agent's
contract.**

**Reconciliation with what has already landed** (`0eff591`, `07dcd46`,
`e4d5e82`). `crates/pdfrum-tool/src/events.rs` already defines a
`pdfrum_tool::events::Event` and a total `parse_evt`. That enum is a faithful
**grammar** type — one variant per verb, `i32` fields straight from `atoi`,
`KeyCode` deliberately representing the down/up *pair* as the grammar emits
it. It is correct as a parser output and this brief does not ask for it to
change shape.

What follows is the **semantic** enum `pdfrum-form` consumes. The two are
different layers and both are wanted:

| | `pdfrum_tool::events::Event` | `pdfrum_form::Event` |
|---|---|---|
| Role | what the `.evt` grammar *says* | what the form engine *does* |
| Fields | `i32` from `atoi`, `u32` modifier mask | `f32` page-space `Point`, typed `Key`/`Modifiers` |
| `keycode` | one `KeyCode` = the down/up pair | one `KeyDown` (the up is a no-op, note 1) |
| Lives in | `pdfrum-tool` | `pdfrum-form` |

The bridge is one function in `pdfrum-tool`, roughly
`fn to_form_event(e: &events::Event) -> Option<pdfrum_form::Event>`, which
widens the integers and drops the dead up-edge. Keeping them separate is what
lets `parse_evt` stay a pure, total description of the grammar while the
engine's input type is free to be the one the embeddertests need — and those
tests are not `.evt` files, so they need a type the grammar cannot express
(fractional coordinates, `FORM_OnChar` with a real modifier, note 3).

**One correction the landed code should take**, and it is the only one:
`events.rs`'s doc comments call the mouse coordinates "Device-pixel X/Y". They
are **page space** — PDF user space, y-up — on every mouse verb.
`public/fpdf_formfill.h` documents `page_x`/`page_y` as "in PDF user space"
for `OnMouseMove` (:1196-1197), `OnLButtonDown` (:1270-1271), `OnFocus`
(:1249-1250) and `DoubleClick` (:1327-1328), and the implementation passes
them through unconverted (`fpdf_formfill.cpp:444`, `:490`, and siblings) with
no transform. The sole exception in the header is `FORM_OnLButtonUp`, whose
comment says "in device" (:1298-1299) — and that is an **upstream doc bug**
(§1.1): its implementation is identical to `OnLButtonDown`'s. Reading the
labels as device-space would put every coordinate through a transform that
must not happen.

`pdfrum-form` defines these types; `pdfrum-tool` converts into them.

```rust
/// One input event, in page space. The complete set: there is no KeyUp
/// (upstream's is a documented permanent no-op returning false, §1.1) and
/// no Idle (a blank .evt line skips the idler, §1.2.6 — M15's concern).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `mousemove,<x>,<y>` — modifiers are hardcoded 0 by the .evt grammar.
    MouseMove { at: Point, modifiers: Modifiers },
    /// `mousedown,<left|right>,<x>,<y>[,<mods>]`
    MouseDown { button: Button, at: Point, modifiers: Modifiers },
    /// `mouseup,<left|right>,<x>,<y>[,<mods>]`
    MouseUp { button: Button, at: Point, modifiers: Modifiers },
    /// `mousedoubleclick,left,<x>,<y>[,<mods>]` — `right` is rejected by the
    /// grammar (§1.2.3), so this carries no button.
    DoubleClick { at: Point, modifiers: Modifiers },
    /// `mousewheel,<x>,<y>,<dx>,<dy>[,<mods>]` — deltas are platform-agnostic
    /// notches, negative dy meaning down.
    MouseWheel { at: Point, delta: (i32, i32), modifiers: Modifiers },
    /// `focus,<x>,<y>` — modifiers hardcoded 0.
    Focus { at: Point, modifiers: Modifiers },
    /// `keycode,<n>[,<mods>]` — note the .evt verb emits OnKeyDown *and*
    /// OnKeyUp; since OnKeyUp is a no-op, one KeyDown is the whole meaning.
    KeyDown { key: Key, modifiers: Modifiers },
    /// `charcode,<n>` — modifiers hardcoded 0 by the grammar. `ch` is a
    /// Unicode scalar; the grammar's integer is a code point (e.g. 1489 = Hebrew bet).
    Char { ch: char, modifiers: Modifiers },
}

/// Page space (PDF user space), y-up, from the .evt file's integers.
/// The grammar parses only integers (atoi), but the API takes f32 because
/// FORM_On* takes doubles and the embeddertests use fractional coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point { pub x: f32, pub y: f32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button { Left, Right }

/// A virtual key code, `public/fpdf_fwlevent.h`'s FWL_VKEY set.
/// A newtype over u16, not an enum: the grammar admits any integer and the
/// tests send codes we do not decide on (F1 = 0x70, digits, letters).
/// Named constants cover the ones the form layer decides on (§1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key(pub u16);

impl Key {
    pub const BACK: Key = Key(0x08);      pub const TAB: Key = Key(0x09);
    pub const NEWLINE: Key = Key(0x0A);   pub const CLEAR: Key = Key(0x0C);
    pub const RETURN: Key = Key(0x0D);    pub const ESCAPE: Key = Key(0x1B);
    pub const SPACE: Key = Key(0x20);     pub const PRIOR: Key = Key(0x21);
    pub const NEXT: Key = Key(0x22);      pub const END: Key = Key(0x23);
    pub const HOME: Key = Key(0x24);      pub const LEFT: Key = Key(0x25);
    pub const UP: Key = Key(0x26);        pub const RIGHT: Key = Key(0x27);
    pub const DOWN: Key = Key(0x28);      pub const INSERT: Key = Key(0x2D);
    pub const DELETE: Key = Key(0x2E);
    pub const A: Key = Key(0x41);         pub const Y: Key = Key(0x59);
    pub const Z: Key = Key(0x5A);
    pub const UNKNOWN: Key = Key(0x00);
}

/// The FWL_EVENTFLAG bits (§1.1). A bitflag newtype, not the `bitflags` crate
/// — DEPS.md is closed and this is nine constants (STYLE §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const NONE: Modifiers            = Modifiers(0);
    pub const SHIFT: Modifiers           = Modifiers(1 << 0);
    pub const CONTROL: Modifiers         = Modifiers(1 << 1);
    pub const ALT: Modifiers             = Modifiers(1 << 2);
    pub const META: Modifiers            = Modifiers(1 << 3);
    pub const KEYPAD: Modifiers          = Modifiers(1 << 4);
    pub const AUTO_REPEAT: Modifiers     = Modifiers(1 << 5);
    pub const LEFT_BUTTON: Modifiers     = Modifiers(1 << 6);
    pub const MIDDLE_BUTTON: Modifiers   = Modifiers(1 << 7);
    pub const RIGHT_BUTTON: Modifiers    = Modifiers(1 << 8);

    #[must_use] pub fn contains(self, other: Modifiers) -> bool;
    #[must_use] pub fn union(self, other: Modifiers) -> Modifiers;
}
```

**Notes the parser agent needs, each with its §1.2 citation:**

1. **`keycode` emits one `Event::KeyDown` only.** The grammar calls
   `FORM_OnKeyDown` then `FORM_OnKeyUp`, but `FORM_OnKeyUp` is a permanent
   no-op returning false (§1.1), so a second event would be dead weight. If
   the harness wants byte-exact call counting it may emit a `KeyDown` and
   ignore the up; the enum deliberately cannot express the up.
2. **Modifiers are matched by substring**, not equality:
   `shift`/`control`/`alt` anywhere in the field (§1.2.5). `shiftcontrol` and
   `xxaltxx` both work; `Shift` does not.
3. **`charcode` and `mousemove` and `focus` carry hardcoded modifier 0**
   (§1.2.3). The enum still has a `modifiers` field on all three because the
   embeddertests call `FORM_OnChar(ch, modifier)` with real modifiers
   (`DoNotHandleSelectAllOnChar`, §1.6) — the `.evt` grammar is a subset of
   the API, not the whole of it.
4. **`mousedoubleclick,right` is rejected** with a diagnostic; the verb
   carries no button (§1.2.3).
5. **`atoi` semantics**: leading whitespace and sign, stop at the first
   non-digit, `0` for unparseable, **leading zeros are decimal not octal**
   (`keycode,09` is 9, §1.2.3). Rust's `str::parse` is *not* a substitute —
   it rejects trailing garbage where `atoi` ignores it. Implement the
   `atoi` prefix scan.
6. **`'\r'` is not stripped** (§1.2.2). Split on `'\n'` alone.
7. **A blank or comment-only line is skipped entirely**; `#` starts a comment
   anywhere on a line, and **trailing whitespace before it survives**
   (§1.2.2).
8. **A short mouse line is skipped with a diagnostic**, not read out of
   bounds (D8).
9. **Do not repair unbalanced events.** A `mousedown` with no `mouseup` is
   intentional and `scrollable_widgets1.evt` depends on it (§1.2.7).
10. **Replay is per page.** `pdfium_test` sends the whole stream once for each
    page, before saving any image (§1.2.6). `--send-events` must do the same
    or the multi-page fixtures diverge.

The parser's own signature, so the two halves meet:

```rust
/// Parses one `.evt` file. Total: never fails, never panics, records every
/// malformed line in `diags`. (§1.2, D8)
pub fn parse_events(text: &str, diags: &mut Diagnostics) -> Vec<Event>;
```

### 4.3 Ported assertions — the nextest inventory

The 191 assertions become `#[test]` functions in `crates/pdfrum-form/tests/`,
grouped by the behaviour they pin rather than by the C++ file they came from.
Every one is a hand-written test (STYLE §2b bans macro-generated per-file
tests). The table below is the port target: **C++ test name → the fixture,
the event sequence, and the assertion restated over pdfrum's types.**

#### 4.3.1 Fixture constants (needed by every row)

The four interactive fixtures' coordinates, transcribed from
`fpdf_formfill_embeddertest.cpp:204-601`, because the assertions are
meaningless without them:

| Fixture | PDF | Constants |
|---|---|---|
| `…TextForm…` (`:204-272`) | `text_form_multiple.pdf` | `kFormBeginX = 102.0` (:268), `kFormEndX = 195.0` (:269), `kCharLimitFormY = 60.0` (:270), `kRegularFormY = 115.0` (:271). Fields: "Text Box"; "ReadOnly" `/Ff 1`; "CharLimit" `/MaxLen 10 /V Elephant` |
| `…ComboBoxForm…` (`:274-385`) | `combobox_form.pdf` | `kFormBeginX = 102.0` (:380), `kFormEndX = 183.0` (:381), `kFormDropDownX = 192.0` (:382), `kEditableFormY = 360.0` (:383), `kNonEditableFormY = 410.0` (:384), **`kChoiceHeight = 15`** (:369), option click at `(x − 20, y − 15·(i+1))` (:375). Fields: "Combo_Editable" `/Ff 393216` (Foo/Bar/Qux); "Combo1" `/Ff 131072` (26 fruit); "Combo_ReadOnly" `/Ff 131073` |
| `…ListBoxForm…` (`:387-601`) | `listbox_form.pdf` | all at `x = 102.0`; y-pairs (first/second visible row): single 371/358, multi 423/408, multi-indices 273/258, multi-values 223/208, multi-mismatch 173/158, last-selected 123/108 (:588-600). **Only the first two rows of each list are clickable** |
| `…Version2` (`:603-609`) | as text form | `SetFormFillInfoVersion(2)` — in pdfrum, focus change is always observable (D2), so the v1/v2 pair collapses to one test |

`kModifier` (`:39-43`) is `Meta` on Apple, `Control` otherwise — pdfrum's
`SessionConfig::accelerator` (D6), and every shortcut row is run **twice**,
once per configuration.

#### 4.3.2 Cluster A — event model, focus, tab order (24 tests)

| C++ test | Assertion, restated |
|---|---|
| `FirstTest` | opening `hello_world.pdf` and loading page 0 produces **no** `AppearanceUpdate` |
| `Bug487928`, `Bug507316`, `Bug900552`, `Bug901654Case1/2`, `Bug1477093` | *(V8-gated or timer-driven — M15)* except the no-crash shape, which becomes a fuzz seed (§4.5) |
| `Bug514690` | a mouse-move with no form present is `false`, no panic |
| `GetFocusedAnnotation` | initially `focused_annot() == None`; after clicking (410, 210) on each of pages 0/1/2, `focused_annot() == Some(AnnotId { page: i, index: 3 })` |
| `SetFocusedAnnotation` | `set_focused_annot` on annot 2 of each page yields that annot; a nonexistent annot returns `false` |
| `FormFillFirstTab` | `Tab` with nothing focused → annot **1** |
| `FormFillFirstShiftTab` | `Shift+Tab` with nothing focused → annot **0** |
| `FormFillContinuousTab` | four tabs → 1, 2, 3, 0, each `true`; **the fifth is `false`** |
| `FormFillContinuousShiftTab` | four shift-tabs → 0, 3, 2, 1; the fifth `false` |
| `TabWithModifiers` | Tab with Control / Alt / Meta / Control+Shift / Alt+Shift / Meta+Shift — **all six `false`** |
| `KeyPressWithNoFocusedAnnot` | each of `{NewLine, Return, Space, Delete, '0', '9', 'A', 'Z', F1}` is `false` and creates no focus |
| `DoNotHandleShortcutsOnKeyDown` | Left / Ctrl+Home / Home / Up / Right are `true`; Ctrl+C, Ctrl+V, Ctrl+X are **`false`** |
| `Bug851821` | a document open action does **not** produce `ActionRequested` for a URI |
| `CheckReadOnlyInCheckbox` | a read-only checkbox: `Char(Return)` and `Char(Space)` both return **`true`** while the checked state is **unchanged** |
| `CheckReadOnlyInRadiobutton` | same for a radio button, starting unchecked |
| `FocusChanges` (text, `:2803`) | the full 15-step walk of §1.4, asserting `focused_text()` after each |
| `FocusChanges` (combo, `:2836`) | the full 21-step walk of §1.7.3, including type-ahead and the option-reset rule |
| `FocusAnnotationUpdateToEmbedder` (v1 + v2) | one `UpdateKind::FocusChanged` is returned on the click (D2 collapses the version pair) |
| `HasFormInfoNone/AcroForm/XFAFull/XFAForeground` | `Document::form_type()` reports None / AcroForm / XfaFull / XfaForeground |
| `HasFormFieldAtPointForXFADoc` | `field_at_point` off-page is `None` (non-XFA branch of `:1616`) |
| `ButtonActionInvokeTest` | focusing the button and sending `Char(Return)` produces **no** `ActionRequested` and returns **`false`** — the asserted-broken behaviour of §1.7.5 |
| `LinkActionInvokeTest` (v1 + v2) | four `KeyDown(Return, m)` with `m ∈ {NONE, CONTROL, SHIFT, SHIFT\|CONTROL}` produce four `ActionRequested { action: Uri("https://cs.chromium.org/"), modifiers: m }`, **in order**, with the raw bit values 0, 2, 1, **3** |
| `InternalLinkActionInvokeTest` | tabbing to annots 4, 5, 6 and firing four Returns each yields **12** `ActionRequested { action: GoTo { zoom_mode: 1, .. } }`; and `Shift`, `Space`, `Control` keydowns are each `false` |

#### 4.3.3 Cluster B — text editing, selection, char limits (46 tests)

`GetSelectedTextEmptyAndBasicKeyboard/Mouse`,
`GetSelectedTextFragmentsKeyBoard/Mouse`, the five `DeleteTextField…`, the
four `InsertTextIn…TextField…`, the four
`InsertTextAndReplaceSelectionInPopulatedTextField…`, the eight
`…CharLimitTextField…`, `DoubleClickInTextField`, `SelectAllText`,
`FormTextFieldBiDiLiveEdit`, `SetTextDirectionUpdates…`, plus the 45 from
`cpwl_edit_embeddertest.cpp`.

The load-bearing rows:

- **`DoubleClickInTextField`**: a double-click selects **the whole line**
  (`"Hello World"`), not the word under the cursor.

  > **Corrected 2026-09-01 (M14 block 3):** the whole **field**. See the
  > correction under §1's mouse table — on this single-line fixture the two
  > readings give the same answer, and the upstream body is `SelectAll()`.
- **The eight char-limit rows** pin §1.7.1's truncation table exactly:
  `"HiElephant"`, `"ElephHiant"`, `"ElephantHi"`, `"Hippopotam"`,
  `"Hippophant"`, `"ElHippopnt"`, `"ElepHippop"`.
- **`SelectAllText`** pins the UTF-16 byte lengths: `"Hello"` is **12**
  (5 × 2 + terminator), empty is **2**. pdfrum returns `Option<String>`, so
  the ported assertion is `Some("Hello")` / `Some("")` — with a *separate*
  test asserting that a `None` (no focus) is distinguishable from an empty
  string, which the C++ API cannot express.
- **`SetTextDirectionUpdates…`** is hash-relational rather than golden:
  auto ≠ rtl, rtl ≠ ltr, **auto == ltr** for a first-strong-LTR input.
  Ported as three `GeneratedAp` byte comparisons.
- From `cpwl_edit_embeddertest.cpp`: `ReplaceAndKeepSelection`'s
  **reversed-selection** case (§1.20.4), the **DEL-key rejection** (§1.20.7),
  and `ReplaceSelectionUndoQueueLimit` / `ReplaceSelectionRedoQueueLimit`
  with `max_undo_items = 4`.

#### 4.3.4 Cluster C — undo and redo (11 tests)

`UndoRedo` ×2, `RedoCutSelection`, `CutAllTextUndoRestoresAllCharacters`,
`ReplaceAndKeepSelection`, `ContinuouslyReplaceAndKeepSelection`,
`ReplaceSelection`, `ContinuouslyReplaceSelection`,
`DoNotHandleSelectAllOnChar`, `SelectAllWithOnKeyDown`, `UndoWithOnKeyDown`,
`RedoWithCtrlYKeyboardShortcut`. Each is transcribed step-by-step from §1.5
and §1.6 and asserts `focused_text()`, `selected_text()`, `can_undo()` and
`can_redo()` after **every** step, which is what the C++ does.

`RedoWithCtrlYKeyboardShortcut` runs twice: with `redo_on_ctrl_y = true`
(Ctrl+Y redoes) and `false` (Cmd+Y does nothing), covering both platform
branches on one machine — which the C++ cannot do.

#### 4.3.5 Cluster D — choice fields (48 tests)

The 22 `FPDFFormFillComboBoxFormEmbedderTest` rows, the 10
`FPDFFormFillListBoxFormEmbedderTest` rows, the 19 from
`cpwl_combo_box_edit_embeddertest.cpp`, and `cffl_combobox_embeddertest.cpp`'s
2. The rows that pin something no other test does:

- **`SetSelectionProgrammaticallyMultiSelectField`**: the focused text of a
  multi-select list is the **last index acted upon**, including after a
  **no-op deselect** — `set_index_selected(3, false)` on an already-deselected
  index 3 returns `true` and moves the focused text to `"Date"` (§1.7.4).
- **`CheckForNoOverscroll`**: `/TI` names index 9 but the list scrolls only far
  enough to fill the box, leaving index **8** in the first visible row.
  **Asserted as correct.**
- The three `TODO(bug_1377)` rows
  (`CheckIfIndexSelectedMultiSelectField`,
  `SetSelectionProgrammaticallyMultiSelectField`'s final click,
  `CheckIfVerticalScrollIsAtFirstSelected`) — **ported as-asserted**, each
  with a doc comment naming the upstream bug so the expectation is
  recognizable as deliberate (§5.OQ3).
- **`CheckIfMultipleSelectedMismatch`**: `/I` and `/V` disagree; **`/V` wins**.
- **`CheckIfEnterAndSpaceKeyAreHandled…`** ×2: Space is consumed by a
  non-editable combo and **inserts a literal space** in an editable one, where
  it also **clears the index selection**.
- **`SetSelectionProgrammaticallyNonEditableField`**: `set_index_selected(i,
  false)` on a combo is **`false`**; on a single-select list box it is
  **`true`** and can empty the list.
- **`BadApiInputs{Text,ComboBox,ListBox}`**: index `−1` and `100` are
  `false`; a text field never accepts index selection. pdfrum's signature
  takes `usize`, so the `−1` rows become "no such method call is
  representable" and the `100` rows stay — recorded as a *reduction* in
  reachable API misuse, not a dropped assertion.

#### 4.3.6 Cluster E — appearance and save round-trip (12 tests)

`FormText`, `Bug1281`, the four `Bug1302455…`, `RemoveFormFieldHighlight`,
`FormComboBoxBiDiLiveEdit`, and the four `cpwl_special_button_embeddertest`
rows. These are the pixel-golden tests, and in pdfrum they split:

- The **appearance-stream bytes** are asserted in `pdfrum-form`'s unit tests
  (`GeneratedAp` is a value; `insta` snapshots it — STYLE §6).
- The **pixels** go to the conformance harness (§4.6), because comparing them
  needs the oracle.
- `FormText`'s save round-trip becomes a `pdfrum-edit` test: typed text
  survives `save()` and a reload byte-for-byte, and the right-click-keeps-focus
  / left-click-drops-focus pair is asserted on the *updates*, not the pixels.

### 4.4 Property tests for the edit control

`proptest` is not in DEPS.md and this brief does not propose adding it
(STYLE §5: "when tempted to pull a helper crate for 30 lines of code, write
the 30 lines"). These are **hand-written generative tests** over a small
deterministic operation-sequence generator seeded by a counter — the same
pattern `pdfrum-parser`'s recovery tests already use.

**P1 — undo/redo round-trip.** For every sequence of ≤ 40 operations drawn
from `{insert_char, backspace, delete, move, select, replace, replace_keep,
select_all}`: undoing to the bottom of the stack yields the initial text, and
redoing to the top yields the final text. `can_undo()` is false exactly at the
bottom and `can_redo()` false exactly at the top.

**P2 — the selection asymmetry (D5).** For every sequence, at every point
where an undo is possible: `undo()` restores the selection that was live
before that edit, and the immediately following `redo()` leaves the selection
**empty**. This is the property four embeddertests observe, stated once.

**P3 — undo granularity.** Typing *n* characters pushes exactly *n* items; a
`replace_selection` of any length pushes exactly one; `select_all` pushes
zero; a focus change to another field empties the stack.

**P4 — group atomicity under eviction.** With `max_undo_items ∈ {4, 5, 7}`,
after any sequence the stack never contains an unmatched `GroupBoundary`, and
`undo()` never leaves the text in a state that no prefix of the operation
sequence produced.

**P5 — selection invariants.** At all times: both ends of the selection are
valid places in the layout; `is_empty()` ⟺ `begin == end`; `range()` is
normalized; and a collapsed-at-caret selection is distinguishable from a
reset one (§1.20.5 rule 2).

**P6 — caret validity.** After every operation the caret is a valid place, and
`place_of_word_index(word_index_of_place(p)) == p` for every reachable `p`.

**P7 — the text is the layout.** `text` and `layout` never disagree:
re-laying-out `text` from scratch with the same `Config` and metrics produces
a `Layout` equal to the incremental one. This is the property that makes the
incremental `RearrangePart` safe to have at all.

**P8 — no panic, ever.** Every operation on every state, including an empty
field, a field at `/MaxLen`, a zero-width plate (`bug_765384`'s one-by-one
box) and an inverted `/Rect` (`bug_889099`'s `[100 100 200 -130]`), returns
without panicking. `clippy::indexing_slicing` is denied in this crate.

### 4.5 Fuzz targets

Two, per STYLE §3's "fuzz target for every byte-consuming entry point":

- `fuzz_evt_parse` — arbitrary bytes into `parse_events`. Must never panic and
  must always return (D8 makes this reachable; upstream's version reads out of
  bounds).
- `fuzz_form_session` — a corpus PDF plus an arbitrary byte string decoded as
  an event sequence, driven against a `FormSession`. Must never panic, never
  hang (D10 makes the tab-order banding terminating), and never exceed the
  undo cap.

The seeds are the 27 in-scope `.evt` files plus the no-crash regression PDFs
the V8-gated tests use (`bug_487928`, `bug_507316`, `bug_900552`,
`bug_901654`, `bug_901654_2`, `bug_1477093`) — those tests' *only* assertion
is "does not crash", which is a fuzz seed rather than a nextest.

### 4.6 The `form-events` conformance cluster

The concurrent agent implements the harness half; this section specifies what
it must produce.

**Not a seventh `Pass`.** The obvious design — a `Pass::FormEvents` beside the
six in `conformance/src/oracle.rs:63-77`, carrying
`&["--send-events", "--png", "--md5"]` — is *not* what shipped, and the
shipped choice is better. `Pass::ALL` is `[Pass; 6]` and a seventh entry would
run for every corpus file, including the ~99% with no `.evt`, doubling the
render pass to produce byte-identical output. Instead the event-driven run is
an **extra invocation inside the Render generator**, taken only when a sibling
`.evt` exists (`conformance/src/generate.rs:265`, `:307-343`), and scored as
its own scoreboard row rather than its own pass.

The naming problem that design still has to solve: an event-driven PNG has the same page name as
the plain render of the same page, so `Pass::owns_artifact` — which takes only
a name and whose doc comment promises it is a pure function of one — cannot
tell them apart. The requirement is that the disambiguator live **in the
name**, so no existing artifact is renamed and the standing "scoreboard must
not move except upward" rule (PLAN.md §Phase 3) is mechanically satisfied.

**This has already landed and the brief adopts it rather than proposing an
alternative.** `conformance/src/generate.rs:36-38` defines
`EVENTS_PNG_SUFFIX = ".events.png"`, so `input.pdf.0.png` becomes
`input.pdf.0.events.png`, with `is_events_png` (`:234-239`) and
`events_png_name` (`:241-250`, idempotent so a second rename cannot collide)
as the pure name predicates. `scoreboard.rs:64-67` gives the failure its own
row tag, `{path}#form-events`, and `main.rs:480` scores that row only when a
sibling `.evt` exists. A suffix rather than the directory prefix this brief
first drafted — same property, already shipped, and one fewer path to
special-case in the golden store's layout.

**Corpus selection.** The cluster covers the **27 non-XFA `.evt` files**
(§1.2.7). The 32 XFA ones are excluded by the same suppression mechanism that
already excludes `testing/corpus/xfa_specific` (`conformance/src/corpus.rs`
walks and the suppressor filters). The 4 JavaScript ones are collected but
**suppressed until M15**, with the suppression naming M15 so it is retired
rather than forgotten.

**Threshold.** The standard `ssim = 0.99` global floor
(`conformance/thresholds.toml`), with no per-file loosening — the ratchet is
one-directional and this cluster starts empty, so every file that passes
earns its entry.

**Invocation.** The oracle must be run exactly as `test_runner.py` does
(§1.2.6): `--send-events` **unconditionally**, plus the existing determinism
recipe (`--time=1399672130`, `--croscore-font-names`, `--font-dir=…`), and the
`.evt` copied next to the PDF first for `.in`-derived fixtures. `use_ahem` in
a fixture's path selects the Ahem font directory — only
`xfa_specific/use_ahem/` uses it, so no in-scope fixture needs it, but the
generator should carry the rule so the XFA files can be added later without a
second pass over this code.

**The four `_expected.txt`-less JavaScript fixtures** assert *empty output*
(`_VerifyEmptyText`, §1.2.6). When M15 enables them, that is the assertion —
recorded here because it is not obvious and would otherwise read as missing
coverage.

### 4.7 Snapshot tests

`insta` snapshots (STYLE §6, "for dump-shaped outputs") for:

- the `GeneratedAp` content stream of a **focused** field with a caret and a
  selection band, for each of: single-line LTR, single-line RTL, multiline,
  comb, password, combo (editable and not), and list box with one and with
  several rows selected. Eight snapshots that make the `LiveEdit` branch
  reviewable as text rather than only as pixels.
- the parsed `Vec<Event>` of each of the 27 in-scope `.evt` files — 27 small
  snapshots that pin the grammar against the corpus and would catch an
  `atoi`-vs-`parse` regression immediately.

---

## 5. Open questions and escalations

### Escalations (SPEC §0 — these change SPEC.md, PLAN.md or DEPS.md)

#### E1 — SPEC §10's standing decline of `fpdfsdk/pwl` must be amended, not deleted

SPEC §10 says, in force since 2026-08-29:

> What stays declined, unchanged: porting `CPWL_EditImpl` itself, and
> everything else in `fpdfsdk/pwl` — the editing widgets, the caret, the
> scroll bars, the focus machinery. **None of it is reachable from a generated
> appearance.**

That sentence is **true and remains true**. Its own justification scopes it to
what a generated appearance can reach, and M14 is authorized by PLAN.md
§Phase 3 to build something that reaches further. The requested amendment is
therefore a **clarifying clause, not a reversal**:

> …*None of it is reachable from a generated appearance, and it stays out of
> `pdfrum-doc` on that ground. PLAN.md §M14 puts the interaction half in a
> separate crate, `pdfrum-form` (SPEC §15), which consumes this crate's `vt`
> and `ap` modules; the decline above continues to bind `pdfrum-doc` itself.*

**Why this matters and is not bookkeeping:** the sentence was written to stop
a second variable-text engine being built. That risk is still real — the
tempting way to implement a caret is to re-lay-out text inside the editor —
and the clause preserves the guard while naming the one authorized consumer.
E1 also confirms the ruling's technical premise held: §1.20 measures the
editing half at **six additions over `vt`**, and none of them is layout.

**Needs the user's ruling.**

#### E2 — PLAN.md §M14's test arithmetic is wrong in both directions

PLAN.md says "**137 of 139** tests run with V8 off" and sets the exit criterion
at "137/137 non-V8 embeddertest assertions pass". Measured (§4.1):

- **11**, not 2, are inside `#ifdef PDF_ENABLE_V8` (`:1132`–`:1361`).
- **7** more are XFA (5 gated + 2 XFA-fixture), which PLAN.md declines.
- So the portable count from that file is **121**, not 137.
- And **70 further V8-free form-interaction tests** exist in four files the
  survey did not count: `cpwl_edit_embeddertest.cpp` (45),
  `cpwl_combo_box_edit_embeddertest.cpp` (19),
  `cpwl_special_button_embeddertest.cpp` (4),
  `cffl_combobox_embeddertest.cpp` (2). These are the *best* coverage of
  exactly what M14 adds.

**Proposed exit criterion:** "**191/191** assertions ported and passing — 121
from `fpdf_formfill_embeddertest.cpp` (139 less 11 V8-gated and 7 XFA) plus 70
from the four `pwl/` and `formfiller/` embeddertests; the 11 V8-gated named
and deferred to M15."

Similarly, PLAN.md's `.evt` count of 59 is correct but undifferentiated:
**32 are XFA and 4 are JavaScript**, leaving **27** in scope for M14 (§1.2.7).
Proposed: "the **27** non-XFA `.evt` corpus pixel goldens pass at the standard
threshold; the 4 JavaScript ones are suppressed naming M15; the 32 XFA ones
are excluded by the standing XFA decline."

**Needs the user's ruling** — it changes a milestone's exit criterion.

#### E3 — A new crate, `pdfrum-form`, and a new SPEC §15

PLAN.md §M14 says "`[spec]` either way". §2b recommends the new crate and
gives the argument, whose decisive points are cohesion against STYLE §4's
one-screen bar, a strictly one-directional dependency with no shared mutable
state, and keeping E1's amendment small. The dependency argument that would be
easiest to make — "it keeps rendering out of `pdfrum-doc`" — **is not true
here** and is not offered.

Requires: a SPEC §15 with the type contracts of §3.2; a `Cargo.toml` with
**no new external dependency** (`pdfrum-common`, `pdfrum-object`,
`pdfrum-doc`, `pdfrum-page`, `pdfrum-font`, `kurbo`, `thiserror` — all
already in DEPS.md); and a slot in the publish order between `pdfrum-doc` and
`pdfrum`, with no back-edge. **DEPS.md needs no change**, and that should be
verified mechanically at the milestone's close the way M12d verified it.

Plus **three additive changes to `pdfrum-doc`** (§2b): `FieldFlags` gains
`is_editable_combo`, `is_multi_select` and `do_not_scroll`;
`ap::field_body` gains an optional caret/selection `Highlight` parameter;
`vt` gains `place_at_point`, `point_at_place` and the word-index/place pair.
None changes an existing signature's meaning.

**Needs the user's ruling.**

#### E4 — `Cascade` is a third trait seam, and STYLE §2b closes the list at two

STYLE §2b: "the seam list is closed — `RenderDevice`/`RasterBackend` and
`Resolve`. **Adding a third seam is a `[spec]` change.**" §3.6's `Cascade` is
a third.

The case for it is the case STYLE §2b itself makes for a seam — *polymorphism
on purpose, with a real second implementation*. `Cascade` has exactly two:
`NoScripts` (M14) and `BoaScripts` (M15), and the second is already planned in
PLAN.md. It is not a one-implementation trait, which is the smell STYLE §2b
names.

Two alternatives were considered and are worse:

- **An enum `Cascade { None, Boa(BoaEngine) }`.** This puts `boa` — a
  cargo-feature-gated dependency — into `pdfrum-form`'s type definitions, so
  `pdfrum-form` must depend on `boa` or carry a `#[cfg]` in a public enum.
  That inverts the dependency M15 wants and makes the JS engine visible from a
  crate that has no business knowing it exists.
- **Function pointers or closures on `SessionConfig`.** Five closures with
  five different signatures, no shared state between them, and `calculate`
  needs `&mut` access to a field-write sink — this is a trait written badly.

The trait is `&mut dyn Cascade` at exactly one call site (`commit::run`), so
it adds one `dyn` beyond `RenderDevice`'s. If the user prefers to hold the
line at two seams, the fallback is a **generic parameter** `C: Cascade`
threaded through `commit::run` and `apply` — which STYLE §2b permits as
"plumbing" but which puts a type parameter on the crate's main entry point and
therefore on the facade, contradicting "the facade uses concrete types only".
The brief recommends the trait and records the fallback.

**Needs the user's ruling.**

#### E5 — `Limits` gains `max_undo_items`

Additive, in the same shape as the existing `max_name_tree_depth` (SPEC §10).
Default **10 000**, the oracle's `kEditUndoMaxItems`
(`cpwl_edit_impl.h:221`); minimum enforced at **4**, the oracle's
`kMinEditUndoMaxItems`, because a `ReplaceSelection` group is four items and
the C++ carries a `static_assert` saying so. Needed by the two
`ReplaceSelection…QueueLimit` tests, which set it to 4.

**Additive; recorded rather than escalated**, unless the user wants every
`Limits` field named in SPEC.

### Open questions resolved in this brief (recorded, not escalated)

**OQ1 — Should the `.evt` parser be lenient about `'\r'`?** *No.* §1.2.2:
upstream does not strip it, and a CRLF file therefore fails to match a
verb-only line. Leniency here is divergence. Recorded because a reviewer's
instinct is the opposite.

**OQ2 — Should a failing validate keep focus?** *No*, however wrong that
looks. §1.11.3 traces it: `CommitData` returns **`true`** on rejection, so
kill-focus proceeds and the edit is silently reverted. This differs from
Acrobat, which many PDFs assume. Reproduced (D9), named in the code, and
**flagged for M15** — when JS can actually set `bRC` false, this becomes
user-visible for the first time, and M15's brief should re-examine whether the
oracle's behavior is what we want to ship or a bug we reproduce with a
documented waiver. **M14 has no way to observe it** (no JS ⇒ `bRC` is always
true), so it costs nothing now and could cost something later.

**OQ3 — What to do about the four upstream bugs the tests assert as correct?**
*Reproduce, and name each.* Three `TODO(bug_1377)` list-box scroll/selection
sites (`:3025`, `:3166`, `:3210`) and two `TODO(crbug.com/1028991)`
button-action sites (`:3658`, `:3665`). Each ported test carries a doc comment
naming the upstream bug, so the expectation reads as deliberate and a future
upstream fix is a recognizable diff rather than a mystery. **The one exception
is the tab-order infinite loop (D10)**, which is not asserted by any test, is
not observable on any corpus file, and is a hang — a library that can hang on
input is a bug regardless of the oracle, and pdfrum terminates.

**OQ4 — Per-page or per-document session?** *Per document.* The C++ owns focus
on the environment (document-wide, `cpdfsdk_formfillenvironment.h:285`) and
window state per (field, page-view) pair. The pair exists because one widget
can be visible in several views at once, which is a viewer concern with no
analogue here (D1). `FormSession` is per document, keyed by `FieldId`.

**OQ5 — Does the live-edit render need a `RenderDevice`?** *No*, and this is
what keeps §2b's dependency question from mattering. A focused field's
appearance is still a **content stream** — the caret is a filled rectangle
0.4 units wide (`cpwl_caret.h:41`) and the selection band is a filled
rectangle in `ArgbEncode(255, 0, 51, 113)` behind white text
(`cpwl_edit_impl.cpp:620-621`). Both are ordinary `re`/`f` operators that
`ap/emit.rs` already writes. `pdfrum-form` produces `GeneratedAp` values and
touches no rasterizer.

**OQ6 — What is the caret's blink state in a generated appearance?** *Always
drawn.* The 500 ms blink (`cpwl_caret.cpp:89`) is a wall-clock effect, and the
oracle's own determinism recipe freezes the clock. `pdfium_test`'s `.evt`
replay renders after the stream completes; whether the caret happens to be lit
at that moment is a function of how many `idler()` calls elapsed, which is not
reproducible across machines. **This is the one place where the `.evt` pixel
goldens might diverge for a reason we do not control**, and it is worth
checking early: if `form_textfield_focused_ltr_expected.pdf.0.png` shows a
caret, we draw one; if it does not, we draw none. **Verify at golden
generation, before writing the caret code**, and record the answer in the
status doc. Listed as an open question rather than an escalation because it is
answerable by looking at one PNG.

**OQ7 — How does the harness reconcile event-driven and plain renders of the
same file?** By writing event-driven artifacts under an `events/` prefix
(§4.6), so `Pass::owns_artifact` stays a pure function of the artifact name
and no existing golden's name changes. The alternative — teaching
`owns_artifact` about corpus entries — would change a signature whose doc
comment promises otherwise.

**OQ8 — Should `FORM_OnKeyUp` appear in the API for symmetry?** *No.* It is a
documented permanent no-op returning false (`fpdf_formfill.h:1368-1370`,
implementation `fpdf_formfill.cpp:569-574` is literally `return false;`). An
API that models it invites callers to send it, and the `.evt` grammar's
`keycode` verb already emits it uselessly. The `Event` enum cannot express it
(§4.2 note 1).

**OQ9 — What about `FPDF_FFLDrawSkia`?** Out of scope: it is the Skia-backend
variant of `FPDF_FFLDraw` and pdfrum's backends are selected through
`RenderDevice` already.

### The things this brief could not settle

**U1 — Whether the focused-field pixel goldens are reproducible at all.**
OQ6's caret question is the visible edge of a broader one: `.evt` pixel
goldens capture a *live UI state*, and the oracle's own `_expected_mac` /
`_expected_win` / `_expected_skia` variants for 14 of the 17 in-scope pixel
fixtures say upstream already finds them platform-sensitive. The conformance
harness compares against the Linux/AGG golden, which is the right one — but
if a fixture's goldens differ across platforms for reasons *inside* the
widget (font fallback, caret phase) rather than in the rasterizer, the SSIM
floor may not be reachable. **This is measurable before any code is written**:
generate the 27 goldens, render each with the *existing* engine (which draws
the unfocused appearance), and read the distribution. A fixture that is
already at 0.99 unfocused tells us the interaction contributes little; one at
0.6 tells us where the work is. Recommended as the milestone's **first**
task, before the crate exists — the same "measure before you build" discipline
M12d's D1 established.

**U2 — Whether `place_at_point` can be exact without a second layout pass.**
Hit-testing a character position requires mapping a page-space point to a
`Place`, which needs the same per-character x positions `vt::Layout` already
computes — so in principle it is a search over `Layout::words()`. What is not
settled is the **tie-breaking** at a character boundary: the oracle's
`CPVT_VariableText::SearchWordPlace` has its own rounding, and a click exactly
on a glyph boundary must land on the same side as upstream or the
`InsertTextInPopulatedTextFieldMiddle` family (which clicks at x = 134 and
expects the caret after exactly 4 characters) will be off by one. The C++ was
not read closely enough on this point to state the rule, and it is the one
place in the brief where the inventory is admittedly incomplete.
**Recommended: read `cpvt_variabletext.cpp`'s `SearchWordPlace` /
`SearchLineWord` before implementing `hit.rs`, and record the tie-break in the
status doc.** The clicked-coordinate tests (§4.3.1's fixture constants) will
catch an error immediately, so the risk is contained — but it is a known gap,
not an assumption.

**U3 — The interaction between `/NeedAppearances` and a live edit.** §1.9's
trigger 2 regenerates every widget's appearance when the flag is set, and
§1.19 records the load-order dance that makes it a no-op in practice. What
happens when a `/NeedAppearances` document's field is *focused* and then
loses focus — does the commit-time regeneration differ from the load-time
one? No test covers it and no corpus fixture combines the two. Left open;
the implementation should assert the two paths produce the same stream and
report if they do not.

---

## Appendix — C++ file inventory and disposition

`fpdfsdk/formfiller/` — 26 files, 3974 lines.

| File | Disposition |
|---|---|
| `cffl_interactiveformfiller.{h,cpp}` (223/1088) | **behavior ported** into `dispatch.rs` + `focus.rs`; the `map_` and `notifying_` shapes erased (D1) |
| `cffl_formfield.{h,cpp}` (178/641) | **ported**: the commit cascade → `commit.rs`, the rotation matrix → `geom.rs` (D3); `maps_`, `valid_`, the age counters and `timer_` erased |
| `cffl_textobject.{h,cpp}` (35/46) | font-map ownership; **absorbed** — `ap::field_body` already resolves the face |
| `cffl_textfield.{h,cpp}` (61/269) | **ported** into `field/text.rs`; the Enter-toggles-`valid_` and Escape-discards rules kept (§1.12) |
| `cffl_combobox.{h,cpp}` (66/277) | **ported** into `field/choice.rs` |
| `cffl_listbox.{h,cpp}` (51/261) | **ported** into `field/choice.rs`; the missing `clear()` fixed (D12) |
| `cffl_button.{h,cpp}` (51/104) | **ported** into `field/{toggle,button}.rs`; the three-state appearance selection kept |
| `cffl_checkbox.{h,cpp}` (42/129) | **ported** into `field/toggle.rs` |
| `cffl_radiobutton.{h,cpp}` (43/122) | **ported** into `field/toggle.rs`; `SetCheck(true)`, never a toggle |
| `cffl_pushbutton.{h,cpp}` (26/26) | **ported** into `field/button.rs` — it has no data to commit |
| `cffl_fieldaction.{h,cpp}` (30/11) | **becomes** `Cascade`'s `Keystroke`/`KeystrokeOutcome` (§3.6); `bRC = true` default preserved |
| `cffl_perwindowdata.{h,cpp}` (52/30) | **erased** (D1) — nothing needs a back-pointer to a widget |
| `cffl_combobox_embeddertest.cpp` (54) | **2 assertions ported** (§4.1) |

`fpdfsdk/pwl/` — 35 files.

| File | Disposition |
|---|---|
| `cpwl_edit_impl.{h,cpp}` (2126 lines) | **the core port**, into `edit/`. Layout delegated to the existing `vt` (§1.20) |
| `cpwl_edit.{h,cpp}` | **ported**: style mapping → `FieldConfig` (D4), key dispatch → `field/text.rs`, comb separators already in `ap::field_body` |
| `cpwl_wnd.{h,cpp}` | **mostly erased** (D1, D2): the style bits become a record, `SharedCaptureFocusState` becomes `FormSession::focus`, `InvalidateRect` and the whole refresh protocol go |
| `cpwl_caret.{h,cpp}` | **partially ported**: the 0.4-unit width and the geometry, into the `LiveEdit` appearance. The 500 ms blink timer is erased (D14, OQ6) |
| `cpwl_list_ctrl.{h,cpp}` | **ported** into `field/choice.rs`; the three-state staging map becomes a delta vector (§3.4) |
| `cpwl_list_box.{h,cpp}` | **ported**; `GetTopVisibleIndex`'s side effect kept and named (D13) |
| `cpwl_combo_box.{h,cpp}` | **ported**: the `SetPopup` state machine and `SetSelectText`'s undo consequence (§1.20.10) |
| `cpwl_cblistbox.{h,cpp}`, `cpwl_cbbutton.{h,cpp}` | **absorbed** into `field/choice.rs`; the dropdown triangle's `kComboBoxTriangleLength = 6.0` geometry into the appearance |
| `cpwl_scroll_bar.{h,cpp}`, `cpwl_sbbutton.{h,cpp}` | **behavior ported, widget erased**: the `kWidth = 12.0` reservation and the step sizes matter to layout and to the two `scrollable_widgets` fixtures; the thumb-drag machinery, the 100 ms repeat and the gradient rendering do not (no live UI) |
| `cpwl_button.{h,cpp}`, `cpwl_special_button.{h,cpp}` | **ported** into `field/{toggle,button}.rs` |
| `ipwl_fillernotify.h` | **erased** (D2) — replaced by the returned `Vec<AppearanceUpdate>` |
| `cpwl_edit_embeddertest.cpp` (876) | **45 assertions ported** (§4.1, §4.3.3) |
| `cpwl_combo_box_edit_embeddertest.cpp` (364) | **19 assertions ported** |
| `cpwl_special_button_embeddertest.cpp` (154) | **4 assertions ported** |
| `cpwl_combo_box_embeddertest.{h,cpp}` (82) | a fixture header, no tests — **its fixture shape informs §4.3.1** |

`fpdfsdk/` top level, the routing layer.

| File | Disposition |
|---|---|
| `cpdfsdk_pageview.{h,cpp}` | **behavior ported**: hit testing → `hit.rs`, routing → `dispatch.rs`, enter/exit → `FormSession::hover`. `matrix_`'s draw-order hazard does not arise (§1.18) |
| `cpdfsdk_annotiteration.{h,cpp}` | **ported** into `hit.rs` — the layout-order bands 1/2/5 and the focused-first / focused-last split |
| `cpdfsdk_annotiterator.{h,cpp}` | **ported** into `tab.rs`, made terminating (D10) |
| `cpdfsdk_widget.{h,cpp}` | **split**: the appearance half is already `pdfrum-doc`'s (`ap::widget`); the interaction half's rules (§1.16) are consumed by `field/mod.rs` and `hit.rs` |
| `cpdfsdk_baannot.{h,cpp}` | popup open/close on hover → `field/mod.rs`'s non-widget path (the six `annotation_highlight_*` fixtures) |
| `cpdfsdk_interactiveform.{h,cpp}` | the cascade → `commit.rs` + `Cascade` (§3.6); the highlight map → the facade's render options, already implemented |
| `cpdfsdk_formfillenvironment.{h,cpp}` | focus ownership → `focus.rs`; the 30-slot callback table **erased** (D2) |
| `fpdf_formfill.cpp` + `public/fpdf_formfill.h` | the **API shape** the facade mirrors (§3.5); the handle/page-view plumbing erased |
| `testing/pdfium_test/event.{cc,h}` | the **grammar** (§1.2); reimplemented in `pdfrum-tool` by the concurrent agent to the §4.2 contract |

Not ported, with the reason: everything under `#ifdef PDF_ENABLE_XFA` (XFA
declined, PLAN.md); `FPDF_FFLDrawSkia` (backend selection is
`RenderDevice`'s job, OQ9); `FORM_OnKeyUp` (permanent no-op, OQ8);
`FORM_OnRButtonDown`/`Up` (XFA-only effect, D7); the timer callbacks (D14);
`FFI_SetCursor`, `FFI_OutputSelectedRect`, `FFI_GetLocalTime`,
`FFI_ExecuteNamedAction`, `FFI_SetTextFieldFocus` and the rest of the
version-1 table (D2).
