<!-- Durable copy of the M14 review record. Source: session
     ede35de1-5746-41fa-ab05-706565ff2804 scratchpad/claude-review-form.md, 2026-09-01. Verbatim. -->

# Review — `pdfrum-form` (M14), commits `62f0f58..ca6d930`

Reviewer: independent Claude/Opus. Scope: the 33 committed commits in the
range, path-scoped to `crates/pdfrum-form/**`, `crates/pdfrum/src/form_session.rs`,
`crates/pdfrum/src/lib.rs`, `SPEC.md` §15, `docs/status/M14.md`. Reviewed the
committed range, not the working tree. Oracle read at `/mnt/data2/pdfium/pdfium-c++`,
never built or edited.

Test tally confirmed by one `cargo nextest run -p pdfrum-form`:
**198 tests run, 198 passed, 0 skipped** — 151 unit + 47 integration
(`tab_order` 7, `keyboard` 13, `undo_shape` 9, `choice_fields` 9,
`never_panics` 9).

Verdict summary: **1 blocker, 9 should-fix, 6 nits.** The three places the
implementer asked for scrutiny (tab-order seeds, undo pair-eviction, the caret
OQ6 ruling) are all **correct** — verified against the C++ by hand simulation,
detailed under "Verified correct". The blocker is a contract-document
divergence, not a behavioural bug.

**State on main.** Main moved during this review, from `ca6d930` to `a43928c`
(twelve commits). The verdicts above are on the range, as instructed, but I
re-checked each finding against `HEAD` so the list is actionable:

- **Finding 3 (`AnnotId::index`) is already fixed on main** by `061e0c4` (code
  doc, plus three tests pinning the raw index through the band sort and the
  focus ring) and `62c8c75` (`[spec]`, making it normative in §15.2). Both fixes
  are correct and are exactly what the finding asks for; it is recorded below
  as a range finding with no action left.
- **Findings 2, 4, 6, 7 still stand verbatim at `HEAD`**: `tab.rs`'s
  `seed_index` doc still says "the lowest index … the leftmost" (`HEAD` line
  293); `hit::contains` still does not normalize (`HEAD:hit.rs:150-152`);
  Escape is still filtered to `Ignore` (`HEAD:field/text.rs:197`); `Home`/`End`
  still gate on the session accelerator (`HEAD:field/text.rs:142`).
- **Finding 1 is partly addressed and mostly stands.** `62c8c75` fixed the
  `AnnotId` comment and renamed `Cleared` → `RevertedToFileAppearance` (with
  `d406e99` doing the same in `update.rs`, so those two now agree). The other
  six rows of the table are unchanged at `HEAD`: `apply`/`FormContext`
  (§15.1, `HEAD:SPEC.md:1373-1384`), `TextEdit`/`ChoiceEdit`/`ToggleState`
  (§15.3, `:1430-1464`), and the eight absent facade methods (§15.8,
  `:1610-1611`).
- **The stale test count still stands**: `HEAD:docs/status/M14.md:131` still
  reads "38 ported assertion tests passing".
- Also landing after the range and relevant to the blocked work:
  `30d03c0 doc(vt): map points to caret places and back` and
  `68bfd46 doc(annot): let a caller lay its own appearances over the annotation
  pass` — the two `pdfrum-doc` pieces `docs/status/M14.md` named as the
  blockers. I did not review them; see "Could not verify".

---

## Findings

### 1. BLOCKER — SPEC §15 does not describe the code it is the shape contract for

`SPEC.md` §15 (written at `62f0f58`) declares an API surface that the range's
own code never lands, and the mismatch is not "not yet implemented" — several
declared types have *different fields and different names* from what shipped.
SPEC §0's rule is that the section is the shape contract; a reviewer, a bridge
author, or M15 reading §15 today gets the wrong answer on eight separate points.

Concretely, `SPEC.md` §15 vs `ca6d930`:

| SPEC §15 declares | Code at `ca6d930` |
|---|---|
| §15.1 `pub fn apply(session, ctx: &FormContext, event, cascade, diags) -> Response` | **No `apply`, no `FormContext`, no `Diagnostics` anywhere in the crate.** The only dispatch is `pdfrum::FormSession::dispatch`, which is `Response::ignored()`. |
| §15.2 `SessionConfig { accelerator, redo_on_ctrl_y, focusable, max_undo_items }` | `session.rs:88-108` also has `max_calculate_depth: u32` (undeclared) |
| §15.3 `FieldState::{Text(TextEdit), Choice(ChoiceEdit), Toggle, Button}` | `field/mod.rs:118-127` — `Text(TextState)`, `Choice(ChoiceState)`. Neither `TextEdit` nor `ChoiceEdit` exists. |
| §15.3 `TextEdit { text, layout: vt::Layout, config, caret: Place, selection, sticky_x, scroll, undo }` | `field/mod.rs:135-143` `TextState { text, config, undo }` — five of eight fields absent |
| §15.3 `ChoiceEdit { options, selected, caret_index, anchor, top_visible, popup: Option<PopupState>, edit: Option<TextEdit> }` | `field/mod.rs:158-176` `ChoiceState { options, selected, caret_index, anchor, top_visible, config }` — **no `popup`, no `edit`**, and a `config` field §15 does not mention |
| §15.3 `ToggleState { pub state: Vec<u8> }` (the `/AS` name) | `field/toggle.rs:29-37` `ToggleState { state: String, on_state: String }` |
| §15.6 `UpdateKind::Regenerated(GeneratedAp)` / `LiveEdit(GeneratedAp)` / `ActionRequested { action: Action, … }` | `update.rs:56,64,72` — all three are `Box<…>` |
| §15.6 `UpdateKind::Cleared` | `update.rs:67` `Cleared` *(agrees in the range; both sides renamed to `RevertedToFileAppearance` after it, by `d406e99` + `62c8c75` — a good fix, and the model for the rest)* |
| §15.8 facade lists 23 methods including `selected_text`, `select_all`, `replace_selection`, `replace_and_keep_selection`, `undo`, `redo`, `set_focused_annot`, `field_at_point` | `form_session.rs` has **none of those eight**; it has an `on_button` §15.8 does not list |

Fix: bring §15 into agreement with the code in one `[spec]` commit — either by
amending the declarations to what shipped (preferred for §15.2/§15.3/§15.6, where
the code's shape is defensibly better: `Box` on a fat `GeneratedAp`, an `on_state`
the `/AS` name alone cannot carry) or by marking the not-yet-landed halves
(`apply`, `FormContext`, `vt::Layout`, `PopupState`, the eight facade methods)
explicitly as **planned, not present**, the way `docs/status/M14.md` already does
for the blocked clusters. Silently shipping a shape contract that eight facts
contradict is the thing SPEC §0 exists to prevent, and the status doc's own
honesty about the *test* count makes the §15 silence look like an oversight
rather than a decision.

---

### 2. SHOULD-FIX — the `seed_index` doc comment states the opposite of what the code does, and matches the brief's own error

`crates/pdfrum-form/src/tab.rs:274-277`:

```rust
/// The scan runs **downward** with a strict comparison, so a tie resolves to
/// the lowest index — which after the primary sort means the leftmost of the
/// tied annotations for a row pass.
```

The code is right and this comment is wrong. Scanning `.rev()` with strict `>`
means a tie **never displaces** the running best, and the running best at the time
a tie is met was set by a *higher* index — so the winner is the **highest**
index among the ties, i.e. the **rightmost** after the left-ascending sort. That
is exactly what `tab.rs:435` (`row_order_seeds_each_band_with_the_rightmost_of_the_topmost`,
expecting `3, 1, 0, 2`) asserts and what `docs/status/M14.md`'s seed rule 1 says.
I re-derived both by hand against `cpdfsdk_annotiterator.cpp:135-147` and against
`annotiter.pdf`'s own rects; the code and the tests are correct.

The reason this matters beyond a typo: `docs/design/pdfrum-form.md` §1.15.2 makes
**the same mistake** — "scanning downward from the last index with a strict `>`
against `fTop`, so ties resolve to the **lowest** index, i.e. leftmost" — and the
brief is the behaviour contract. Right now the contract, the doc comment, and the
code disagree, with only the status doc and the test carrying the right answer.
The next person to "fix the code to match the brief" breaks four ported assertions.

Fix: correct `tab.rs:274-277` to say *highest index / rightmost*, and correct
brief §1.15.2's parenthetical the same way (a `[spec]`-adjacent doc edit; the
brief is a design doc, not SPEC, so a plain commit suffices) with a one-line note
that the status doc's rule 1 is the authority.

---

### 3. SHOULD-FIX — `AnnotId::index` is still not pinned to an index space *(fixed on main by `061e0c4` + `62c8c75`; no action left)*

`crates/pdfrum-form/src/session.rs:31-32`:

```rust
    /// Which annotation of it, in load order.
    pub index: u32,
```

"Load order" is ambiguous, and `docs/status/M14.md` ("What the facade still lacks
for the renderer", item 2) identifies the exact hazard: `pdfrum_doc`'s
`AnnotOverlay` is keyed by the **raw `/Annots` array index** (`overlay` looks up
`list.source_indices[slot]`, not `slot`), while `AnnotList::load` filters pop-ups
out — so on any page with a pop-up the two spaces differ and an update lands on
the wrong annotation. The status doc asks for "one sentence on `AnnotId::index`"
and the sentence was never written.

This also silently interacts with `hit.rs`: `LayoutBand::Popup` exists as a
candidate band, so the hit-test list is expected to *contain* pop-ups — which is
only consistent with the raw `/Annots` space. Meanwhile `tab.rs::FocusRing::build`
documents its input as "already filtered to the focusable subtypes", which is the
filtered space. Two modules in the crate index differently and neither says so.

Fix: state on `AnnotId::index` that it is the **raw `/Annots` array index** of the
page (matching `AnnotOverlay`'s key), and add one sentence to `FocusRing::build`
saying that filtering the input list does *not* renumber — the surviving ids keep
their raw indices. Cheap now; a silent wrong-annotation bug the moment routing
lands.

**Already done on main**, after the reviewed range and independently of this
review: `061e0c4` rewrites the `AnnotId` doc to say raw `/Annots`, pop-ups
counted, adds the `FocusRing` sentence about gaps not being closed up, and pins
it with three tests (a widget at raw index 1 behind a pop-up still reports 1; the
band sort reorders without renumbering; the focus ring keeps the gaps
unfocusable annotations leave). `62c8c75` makes it normative in SPEC §15.2. Both
are exactly the fix. Recorded here because it is a finding against the range, but
there is nothing left to do.

---

### 4. SHOULD-FIX — `hit::contains` does not normalize, and the C++ `Contains` does

`crates/pdfrum-form/src/hit.rs:133-136`:

```rust
pub fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.left && x <= rect.right && y >= rect.bottom && y <= rect.top
}
```

`CFX_FloatRect::Contains(const CFX_PointF&)` (`core/fxcrt/fx_coordinates.cpp:229-234`)
copies the rect, calls `Normalize()`, **then** compares. So a widget whose `/Rect`
is written inside out is still hit-testable upstream; here it is unreachable —
every point fails `x >= left && x <= right` when `left > right`.

`Candidate.rect`'s doc says "Its rectangle, normalized", pushing the duty onto the
caller — but no caller exists yet, and `tests/never_panics.rs:56-58` deliberately
feeds `Rect::new(100.0, 100.0, 200.0, -130.0)` ("Written inside out, as
bug_889099's field is") straight into `widget_at_point`. That test only asserts
"never invents an annotation", so the silent miss passes. `geom::normalize`
(`geom.rs:163-171`) is right there and is exactly what `Plate::new` already
applies.

Fix: normalize inside `contains` (one `normalize(rect)` call), matching the
oracle, and drop the "normalized" precondition from `Candidate.rect`'s doc — a
precondition no caller can be trusted to honour on untrusted geometry is the wrong
place for it. Then extend the `never_panics` hit-test property to assert that an
inside-out rect containing the point *is* hit, which is what would have caught this.

---

### 5. SHOULD-FIX — the combo box's four-item undo group per selection change is not implemented, and is not recorded as missing

The task's acceptance list and brief §1.20.10 are explicit: `SetSelectText()`
routes through `ReplaceSelection`, so **every combo-box selection change pushes a
four-item sentinel-bracketed undo group onto the edit's undo stack**, and that is
what makes the combo `UndoRedo` assertion behave as it does.

Nothing in the crate does this. `field/choice.rs::set_index_selected` and
`select_only` mutate `state.selected` and `state.caret_index` and touch no undo
stack; `ChoiceState` has no `edit: Option<TextEdit>` to hold one (finding 1), so
there is no stack for a combo to push onto. `git grep GroupBoundary` over
`crates/pdfrum-form/src` outside `edit/undo.rs` and its tests returns nothing.

This is defensible as blocked-on-`vt` work, but unlike the text-editing clusters
it is **not listed** in `docs/status/M14.md`'s "How the remaining 153 divide" — the
two named blockers there are the vt place queries and the caret rectangle, and a
combo box's undo group needs neither: it needs a `TextEdit` on `ChoiceState` and
four `push` calls.

Fix: either implement it (`ChoiceState` gains the editable combo's `TextState`,
and `set_index_selected`/`select_only` on a combo push
`[GroupBoundary, Clear, InsertText, GroupBoundary]`), or add it to the status doc's
blocked list with the reason, so the exit criterion's arithmetic stays honest.

---

### 6. SHOULD-FIX — Escape is filtered to `Ignore`, but upstream consumes it and escapes the filler

`crates/pdfrum-form/src/field/text.rs:188-190`:

```rust
    if matches!(ch, LINE_FEED | ESCAPE | DELETE) {
        return Disposition::Ignore;
    }
```

That reproduces `CPWL_Edit::OnCharInternal`'s FILTER switch
(`cpwl_edit.cpp:583-590`), which does return **false** for those three. But
Escape never reaches `OnCharInternal`: `CFFL_TextField::OnChar`
(`cffl_textfield.cpp:141-146`) intercepts it one layer up —

```cpp
    case pdfium::ascii::kEscape: {
      CPDFSDK_PageView* pPageView = GetCurPageView();
      DCHECK(pPageView);
      EscapeFiller(pPageView, true);
      return true;
    }
```

— discarding the in-progress edit and returning **true** (consumed). Brief
§1.11.1 step 4 names this ("`CFFL_TextField::OnChar` — Return (0x0D) / Escape
(0x1B) special cases"), and the crate even has the action for it:
`TextAction::Escape` (`text.rs:56`) is declared, documented as "Discard the edit
and give up focus", and **never constructed anywhere** — `git grep 'TextAction::Escape'`
over the whole range hits only its declaration.

The Return half of the same C++ switch *is* modelled (`text.rs:213-216`,
`Commit` on a single-line field), so the omission reads as an oversight rather
than a decision.

Fix: return `Disposition::Do(TextAction::Escape)` for `'\u{1B}'` before the
filter, and pin it with a test asserting the event is consumed and the action is
`Escape`. Leave line feed and forward-delete filtered — those two really are
`OnCharInternal`'s and really do return false.

---

### 7. SHOULD-FIX — `Home`/`End` bind document-wise motion to the platform accelerator, where the oracle hard-codes Control

`crates/pdfrum-form/src/field/text.rs:134-152` selects `Motion::DocStart` /
`Motion::DocEnd` when `shortcut` is true, i.e. when the modifiers equal the
session's `accelerator`. Upstream is not accelerator-gated here:

```cpp
    case FWL_VKEY_Home:
      edit_impl_->OnVK_HOME(IsSHIFTKeyDown(nFlag), IsCTRLKeyDown(nFlag));
      return true;
```
(`cpwl_edit.cpp:538-543` — `IsCTRLKeyDown`, not `IsPlatformShortcutKey`, and the
same for `End`.)

So on an Apple configuration the oracle still takes **Ctrl**+Home to the start of
the document while this crate takes **Meta**+Home there and treats Ctrl+Home as a
plain line-start. `fpdf_formfill_embeddertest.cpp:938-940` asserts exactly the
Ctrl form and carries `// TODO(448699368): This should work with the meta key on
macos` — upstream knows, has not fixed it, and the port has silently fixed it.

The consumed/not-consumed answer is unaffected (Home always returns `Do(Move…)`),
so no ported assertion catches it; it is a *which motion* divergence that a
caret-position assertion will catch the moment the vt queries land.

Fix: gate `Home`/`End`'s document-wise motion on `Modifiers::CONTROL` explicitly,
independent of `config.accelerator`, with a comment naming `crbug 448699368` so
the deviation is visible when upstream fixes it. If the "better" behaviour is
wanted instead, it needs a numbered divergence in brief §2 the way D6 and D10 have
— right now it is neither reproduced nor recorded.

---

### 8. SHOULD-FIX — two different accelerator predicates, and both differ from the oracle's subset test

Three ways of asking the same question live in the crate:

- `session.rs:190` — `modifiers.without(Modifiers::SHIFT) == self.config.accelerator`
  (strips **shift only**, exact equality)
- `text.rs:110` — `modifiers.without(Modifiers::SHIFT | Modifiers::ALT) == accelerator`
  (strips **shift and alt**, exact equality)
- `text.rs:196` — `modifiers.contains(accelerator) && !modifiers.contains(Modifiers::ALT)`
  (subset test)

`CPWL_Wnd::IsPlatformShortcutKey` (`cpwl_wnd.cpp:141-146`) is a plain **subset**
test — `nFlag & FWL_EVENTFLAG_ControlKey` (or `MetaKey`) — with `!IsSHIFTKeyDown`
and `!IsALTKeyDown` added at each call site. So the oracle's answer for
`Ctrl|Meta + A` on a non-Apple build is *shortcut*; `text.rs:110` says *not a
shortcut*, and `session.rs:190` says *not a shortcut* even for `Ctrl|Alt`. No
ported assertion distinguishes these, because the embeddertests only ever pass one
modifier at a time (`kModifier` / `kWrongModifier` are single bits,
`fpdf_formfill_embeddertest.cpp:39-43`).

`FormSession::is_accelerator` is public API and is used by nothing in the range —
so the divergence is currently only latent, but the two predicates will diverge
under any caller that routes through the session rather than `route_key`.

Fix: one predicate, matching the oracle: `modifiers.contains(accelerator) &&
!modifiers.contains(Modifiers::ALT)`, with shift handled per-call-site the way the
C++ does (`Key::A` and `Key::Y` add `!shift`, `Key::Z` reads it as meaning). Then
`session::is_accelerator` should either be that function or be deleted.

---

### 9. SHOULD-FIX — the undo tests named after upstream tests do not assert the granularity they claim

`crates/pdfrum-form/tests/undo_shape.rs` is nine tests named after `UndoRedo`,
`CutAllTextUndoRestoresAllCharacters`, `ReplaceSelection` and friends, but every
one of them drives `UndoStack` with **hand-constructed items**:

```rust
    for ch in "ABCDE".chars() {
        stack.push(typed(ch, Selection::EMPTY));
    }
    assert_eq!(stack.len(), 5, "one item per character, never coalesced");
```
(`undo_shape.rs:65-68`)

The assertion "one item per character, never coalesced" is guaranteed by the loop
that pushed five items, not observed. Same for `a_three_character_replace_is_one_undo_step`
(the `replace_group` helper builds the bracketed group by hand),
`a_cut_is_one_step_however_many_characters_it_removed`, and
`selecting_all_is_not_an_undoable_edit` (which asserts nothing about select-all —
it never calls one).

This is precisely the failure mode the implementer caught and fixed in
`2527890`'s own commit message ("A stack cannot pair what the caller never paired,
so that was testing the generator"); it survives here. What the file *does* pin
correctly — the walk shape, the group-atomic eviction, `before` on undo, the redo
truncation — is real and valuable. The granularity half is not tested by anything
in the range, because the operation layer that would decide it does not exist yet.

Fix: rename the file's module doc and the individual tests to say what they
assert (stack shape) rather than the upstream test names, or add a note per test
naming the *unasserted* half. When the operation layer lands, the granularity
assertions become real and the names become correct. Leaving upstream test names
on assertions that cannot fail against a wrong implementation inflates the ported
count.

---

### 10. SHOULD-FIX — the worst-case four-item group at the capacity floor is never tested

`UndoStack::MIN_MAX = 4` exists for exactly one shape: `[GroupBoundary, Clear,
InsertText, GroupBoundary]`. The test that claims to cover it,
`undo_shape.rs:220-243` (`a_replace_group_fits_at_the_smallest_capacity`), calls
`replace_group("", &format!("v{round}"), …)` — an **empty** `removed`, so
`replace_group` (`undo_shape.rs:29-47`) emits only three items. The four-item case
is never constructed at `max = 4`.

I simulated the four-item case by hand against both implementations and they
agree, including on the upstream `ReplaceSelectionUndoQueueLimit` scenario
(`cpwl_edit_embeddertest.cpp:610-638`: 4-item group at max 4, then two typed
chars, then `CanUndo` false after two undos) — so this is a test gap, not a bug.
But the floor's whole justification is untested.

Fix: add a round with a non-empty `removed` — `replace_group("A", "XYZ", …)` at
`max = 4` — and assert the group survives intact before the next push evicts it
whole.

---

### 11. NIT — `UndoStack::undo`'s doc says "oldest first"; it returns newest first

`edit/undo.rs:258-259`: "The items one `undo` would replay, **oldest first**".
The walk decrements `pos` and pushes, so a group comes back newest-first — which
is what `edit/undo.rs:420` asserts (`vec!['Z', 'Y', 'X']`) and is the right order
for replaying inverses. `redo` genuinely is oldest-first. Fix: say "newest first"
on `undo`, and note the asymmetry is deliberate.

### 12. NIT — catch-all `_ =>` arms on the crate's own enums, against STYLE §1

STYLE §1: "Avoid `_ =>` arms on our own enums — when a variant is added, every
match site must fail to compile." Eight sites do it on crate-owned enums:
`update.rs:93` and `update.rs:167` (`UpdateKind`), `edit/undo.rs:362`
(`UndoItem`, in a test helper), `focus.rs:80` (`Button`), and
`form_session.rs:262,271,284,295,309` (`FieldState`). `geom.rs:57`,
`tab.rs:114` and `text.rs:170,221` are fine — those match on integers and `char`,
not on our enums.

`form_session.rs`'s five are the ones that bite: adding a `FieldState` variant
should force every facade query to be revisited, and today it silently answers
`false`/`None`. Fix: exhaustive arms with an explicit grouped no-op arm
(`FieldState::Toggle(_) | FieldState::Button(_) => false`).

### 13. NIT — the facade's "defaults for this platform" are always the non-Apple ones

`form_session.rs:74-80`: `FormSession::new` doc says "with the defaults **for this
platform**" but calls `Inner::new()`, which is `SessionConfig::default()` —
Control + `redo_on_ctrl_y: true`, unconditionally. `SessionConfig::apple()`
exists and is never reachable from the facade except by the caller constructing it
themselves. D6 makes the accelerator configuration deliberately, which is right;
the doc comment promising a platform default is what is wrong. Fix: either say
"with the non-Apple defaults; pass `SessionConfig::apple()` for Apple keyboards",
or make `new` pick with `cfg!(target_os = "macos")` (which D6 argues against).

### 14. NIT — facade tests pass vacuously when the fixture is missing

`form_session.rs:340-343`: `fn document() -> Option<Document> {
Document::open("tests/fixtures/text_form.pdf").ok() }`, and every test opens with
`let Some(doc) = document() else { return; };`. The fixture does exist
(`crates/pdfrum/tests/fixtures/text_form.pdf`), so these run today — but a rename
or a parse regression turns four tests green-and-empty instead of red. Fix: `let
doc = document().expect(...)` is barred by the workspace lint, so
`Document::open(...).unwrap_or_else(|e| panic!(...))` is equally barred — use
`assert!(document().is_some(), "fixture missing")` first, or make the helper
return `Document` via a `#[should_panic]`-free `match … { Err(e) => panic }` in a
`#[cfg(test)]` block, which the lint permits in tests.

### 15. NIT — the `/V`-beats-`/I` test uses invented options, contradicting the file's own stated method

`tests/choice_fields.rs:4-8` says the option lists are "transcribed from
`listbox_form.in` … rather than invented, because … a made-up list would turn a
behavioural assertion into a tautology". The FRUIT and PROVINCES arrays honour
that exactly (I diffed both against `listbox_form.in` — 26 fruit with index 20 =
"Ugli Fruit" and index 24 = "Yangmei", 10 provinces with `/TI 9`; both correct).
But `the_value_entry_wins_over_the_index_entry` (`choice_fields.rs:152-183`) uses
the **Values** fixture's Greek names with fabricated indices `[0, 1]` for the
mismatch case, where upstream's `CheckIfMultipleSelectedMismatch` fixture is
`Listbox_MultiSelectMultipleMismatch` — `/Opt [(Alligator) (Bear) (Cougar) (Deer)
(Echidna)]`, `/V [(Alligator) (Cougar)]`, `/I [1 3 4]`, expecting `{0, 2}`
(`listbox_form.in:118-124`, `fpdf_formfill_embeddertest.cpp:3195-3203`). Same
property, different fixture. Fix: transcribe the Mismatch row's own five animals
and its `/V`+`/I`, which is a five-line change.

### 16. NIT — `top_visible_for`'s `visible_rows` is hand-supplied, so the fixture geometry is not what drives the answer

`tests/choice_fields.rs:141-143`: `let top = top_visible_for(PROVINCES.len(), 2, 9);
assert_eq!(top, 8, ...)` with the comment "The widget shows two rows". Nothing
derives `2` — the fixture's `/Rect [100 100 200 130]` and `/F1 12 Tf`
(`listbox_form.in:132-133`) are what make it two, and the test hard-codes the
answer. Picking `3` instead would give `7` and the test would be rewritten to
match. Fix: derive `visible_rows` from the rect height and font size in the test
(`(130.0 - 100.0) / 12.0` floored), so the "no overscroll" property is what is
being asserted rather than the arithmetic of `min`.

---

## Verified correct

Everything below I checked against the C++ or by hand simulation and found right.

**The three places the implementer flagged.**

1. **Tab-order banding seeds.** Both quirks are faithfully reproduced.
   - *Row seed, rightmost of the topmost.* `tab.rs:279-288` scans `.rev()` with
     strict `>` against a running `top` initialised to `0.0`, exactly
     `cpdfsdk_annotiterator.cpp:135-143`. I hand-simulated the unit test's
     four-annotation geometry (`tab.rs:436-441`): after the left-ascending sort
     the order is `[1, 2, 0, 3]`, the seed scan keeps index 3 (`#3`, top 550)
     because `#1`'s equal 550 is not *strictly* greater, band picks up `#1`
     (centre 525 ∈ (500, 550)), then `#0` seeds and `#2` joins → **`3, 1, 0, 2`**,
     which is what the test asserts. The status doc's rule 1 is right; the doc
     comment (finding 2) is the only thing wrong.
   - *Column seed, `left < 0` seeding index zero.* `tab.rs:289-315` reproduces
     `cpdfsdk_annotiterator.cpp:170-186` exactly, including that the guard
     **re-fires** on later iterations when a page's `left` is itself negative, and
     that it assigns `best = Some(0)` rather than `Some(i)`. The comment there is
     accurate and is the right place for it.
   - *Termination (D10).* `band` is a fold that consumes `remaining`; the
     `else` on `seed_index` appends the remainder in index order and sets
     `degenerate`. Where the C++ `if (nLeftTopIndex < 0) continue;`
     (`:145-147`, `:182-184`) sits inside a `while (!sa.empty())` that erases
     nothing — a genuine hang on any page whose remaining annots all have
     `top <= 0.0f` — this returns. Pinned three ways:
     `banding_terminates_where_the_oracle_would_hang` (`tab.rs:466-477`),
     `a_zero_top_is_on_the_hanging_side_of_the_comparison` (the strict-comparison
     boundary, `:483-489`), and a 200×3 generative permutation check
     (`:493-518`). The banding arithmetic is bit-identical: sums halved rather
     than `f32::midpoint`, comparisons strict at both ends, with
     `#![allow(clippy::manual_midpoint, clippy::float_cmp)]` and a comment saying
     why both lints would move annotations between bands. That is the right call.

2. **The undo pair-eviction invariant.** All of brief §1.5 / D5 that is
   *expressible* without the operation layer is right.
   - `evict_to_fit` (`edit/undo.rs:238-256`) matches `RemoveHeads`
     (`cpwl_edit_impl.cpp:273-289`): a plain head is one `pop_front`; a head that
     is an opening boundary drops items until and including the next boundary.
     The C++ calls `RemoveHeads` once per `AddItem` (an `if`) where this is a
     `while`; I checked the steady state and they are equivalent, since one
     eviction always brings the length below the cap.
   - I hand-simulated the upstream `ReplaceSelectionUndoQueueLimit`
     (`cpwl_edit_embeddertest.cpp:610-638`) against both: `[B, Clear, InsertText, B]`
     at `max = 4`, then two typed characters. Both evict the whole group on the
     first push, both leave `[A, B]`, and both report `CanUndo` false after two
     undos. **They agree.**
   - `push` truncating the redo branch (`items.truncate(self.pos)`, `:229`)
     matches `RemoveTails` (`:291-297`), including that it covers sentinels
     automatically.
   - The `undo`/`redo` walks (`:266-311`) reproduce the decrement-then-act /
     act-then-increment asymmetry and the `first_undo` flag from
     `cpwl_edit_impl.cpp:201-251` faithfully — one item when the top is an
     ordinary edit, the whole group when it is a closing sentinel.
   - `MIN_MAX = 4` with `max.max(MIN_MAX)` clamping **up** (`:158`) is right, and
     is a genuine improvement on the C++'s `CHECK_GE` (`:253-254`), which
     aborts rather than clamping.
   - `UndoItem::before` on every non-boundary variant, `undo` handing it back and
     `redo` not, is D5's asymmetry correctly modelled at the data level; the
     seven variants match brief §1.20.3's table including the D12 decision to
     snapshot `section_break` on **both** `Backspace` and `Delete` where the C++
     re-derives it on one and snapshots it on the other.
   - `set_enabled` gating at the single `push` entry point (`:227`) is D12's
     stated cleanup and is correct: the C++'s `AddEditUndoItem` does *not* check
     `enable_undo_`, so a disabled stack there still accumulates unreachable
     sentinels.

3. **The caret-always-drawn OQ6 ruling.** The reasoning in `docs/status/M14.md`
   is sound and I verified its two load-bearing claims against the source.
   `CPWL_Caret::DrawThisAppearance` does return early on `!flash_`; `flash_` is
   set true when the caret becomes visible and is cleared only by `OnTimerFired`
   on a 500 ms `CFX_Timer`, which `pdfium_test` never pumps in a V8-off build. The
   selection gate — a field with a live selection shows **no** caret — is
   correctly identified as the other half of the switch, and matches the two
   `form_textfield_selected_*` goldens. The ruling ("draw unconditionally when
   focused with an empty selection") is the right one and is recorded where the
   PLAN ruling asked. No caret code exists yet to check against it, which is
   consistent with the LiveEdit branch being blocked.

**The rest of the acceptance list.**

- **`OnChar` / `OnKeyDown` split (§1.6).** `route_key`/`route_char`
  (`text.rs:104-224`) reproduce `OnKeyDownInternal`/`OnCharInternal`
  (`cpwl_edit.cpp:509-601`) row for row: select-all needs the accelerator with
  neither shift nor alt; `Ctrl/Cmd+Shift+A` is explicitly *not* select-all; `Z`
  is the only shortcut that reads shift as a meaning rather than a disqualifier;
  the clipboard trio C/V/X falls through to `Ignore` so an embedder sees it;
  `Key::UNKNOWN` clears the selection; and forward-delete-with-a-selection is
  rewritten to `Key::Unknown`'s branch before the table is consulted, exactly as
  `cpwl_edit.cpp:517-519` does. `Ctrl+A` arriving as a **character** is `Ignore`
  (`text.rs:194-198`), matching `DoNotHandleSelectAllOnChar`. The `\u{7F}`
  refusal is correct and its reason (an embedder may send both a delete char and
  a delete key) is the right one.
- **D6, the accelerator as configuration.** `SessionConfig::apple()` vs
  `default()` differ in exactly the two switches (`session.rs:110-133`), and
  `tests/keyboard.rs` runs every shortcut row both ways on one machine — which is
  a real improvement on upstream, where `#if !BUILDFLAG(IS_APPLE)`
  (`cpwl_edit.cpp:549-559`) makes half of each test unreachable per build. The
  asymmetry is right: `Y` redoes off Apple only, `Shift+Z` redoes on both.
- **The two hit tests (§1.15).** `annot_at_point` is rect containment over every
  band but `Popup`, matching `GetFXAnnotAtPoint`
  (`cpdfsdk_pageview.cpp:125-137`) including the `POPUP` skip;
  `widget_at_point` gates on `WidgetHit::accepts_click`, which reproduces
  `CPDFSDK_Widget::DoHitTest`'s four gates in the oracle's own order — signature,
  visibility, **read-only**, then push-button-or-permissions with `kFillForm`
  **or** `kModifyAnnotation` sufficing. The read-only rule is correctly separated
  from the read-only-consumes-a-keystroke rule, with the distinction stated in
  the module doc and pinned by `a_read_only_widget_is_not_clickable`.
- **Layout bands and the two orderings.** `LayoutBand::{Popup = 1, Widget = 2,
  Other = 5}` are the oracle's own values with its own gaps; `band_sorted` uses
  `sort_by_key` (stable), matching `std::stable_sort` at
  `cpdfsdk_annotiteration.cpp:28-31`; focused-to-front for hit testing and
  focused-to-end for drawing match `:21-22` and `:15-19`. `FOCUS_INFLATION = 1.0`
  matches `cffl_formfield.cpp:49`, and is applied only to the focused widget.
- **Focus / kill-focus ordering (§1.4, §1.17) and D9.** `focus::set` commits the
  outgoing field *before* the incoming takes over; re-focusing the holder is a
  success that neither commits nor clears; the undo stack survives a move between
  two widgets of the **same** field and is cleared by a move to a different one
  (`focus.rs:74-79`) — which is §1.4 rule 4 and the combo `UndoRedo` assertion.
  `miss_drops_focus` reproduces the left/right asymmetry that makes a right-click
  outside a focused field leave it focused (§1.4 rule 1). D9 is implemented and
  is well guarded: `commit::run` returns `committed: true` on a refusal with
  `reverted: true` alongside, `CommitOutcome`'s field docs say why, and
  `a_refused_commit_still_reports_success` (`commit.rs:161-177`) is a test that
  would fail if someone "fixed" it. The two-field shape is better than the
  oracle's single bool and I'd keep it.
- **The cascade order and the V8-off defaults (§1.11).** `commit::run`'s order is
  `is_changed → keystroke_commit → validate → save → calculate → format`, matching
  `cffl_formfield.cpp:507-552`, and is asserted rather than assumed
  (`the_hooks_run_in_the_specified_order`). Every `Cascade` default is the
  permissive answer — `bRC` defaults true (`cffl_fieldaction.h:22`) and is re-set
  true by every caller, so with V8 out every validate and keystroke-commit
  returns true — and `cascade.rs`'s module doc states this correctly and at
  length. `format` returning `None` meaning "display the raw value" is exactly
  the V8-off behaviour. The refusal short-circuit (`a_refusal_stops_the_cascade`)
  is right.
- **The `Cascade` seam.** STYLE §2b's amended clause is honoured to the letter:
  one trait, one `&mut dyn Cascade`, at exactly one call site (`commit::run`'s
  `cascade: &mut dyn Cascade` parameter, `commit.rs:71`). `git grep 'dyn '` over
  the crate returns only that parameter and its test uses. Object safety is
  pinned by a test. `NoScripts` is the single shipped impl and `impl Cascade for
  NoScripts {}` is empty, which is the point.
- **Choice state machines (§1.7).** The three counter-intuitive rules are all
  there and each has a test that would fail if tidied: a combo box refuses every
  deselection while a single-select list box can be genuinely emptied
  (`set_index_selected`, `choice.rs:70-93`); a redundant clear on a list box
  reports success and **still moves `caret_index`**; and `focused_text` follows
  the row last acted upon rather than describing the selection
  (`field/mod.rs:194-204`). `find_next` reproduces `CPWL_ListCtrl::FindNext`
  (`cpwl_list_ctrl.cpp:648-665`) exactly, **including the quirk that it returns
  the last probed index rather than failing** when nothing matches — which is what
  makes type-ahead read as independent jumps, and the ported
  `combo_type_ahead_treats_each_character_as_its_own_jump` trace (A→Apple,
  A,B,C→Cherry, A,B→Banana) is right. Shift-click ranges without re-anchoring and
  accelerator-click toggles **with** re-anchoring, matching
  `CPWL_ListCtrl::OnMouseDown` (`:172-210`); single-select ignores both modifiers.
  `top_visible_for` reproduces the asserted-wrong no-overscroll behaviour rather
  than the `bug_1377` behaviour upstream says it should have, which is the right
  choice and is labelled as such.
- **Fixture geometry against the C++ constants.** I diffed every transcribed
  fixture against its source. `annotiter.pdf`'s four widgets —
  `Sub_LeftBottom [200 200 220 220]`, `Sub_RightTop [401 401 421 421]`,
  `Sub_LeftTop [201 400 221 420]`, `Sub_RightBottom [400 201 420 221]` in that
  `/Annots` order — match `testing/resources/annotiter.in:88-121` exactly, and I
  re-derived the row order `1, 2, 3, 0` and its reverse `0, 3, 2, 1` from those
  coordinates through the C++ algorithm, matching `FormFillContinuousTab` /
  `FormFillContinuousShiftTab` and the two differing "first tab" answers (1
  forward, 0 backward). The 26 fruit and the 10 provinces match
  `listbox_form.in` verbatim, including index 24 = "Yangmei" (the error the
  commit message says transcription caught) and `/TI 9`.
- **`Modifiers` bit values.** All nine constants (`event.rs:120-138`) match
  `FWL_EVENTFLAG` (`public/fpdf_fwlevent.h:19-27`) bit for bit, and
  `actions_come_back_in_order_with_their_modifiers` asserts the raw values as
  part of the contract. `Key`'s constants match the `FWL_VKEY` codes it branches
  on. The decision to make `Key` a newtype rather than an enum is right and the
  reason given (the wire format admits any integer, and the tests deliberately
  send codes the layer does not decide on) is the correct one.
- **No `unsafe`, no reachable panics.** `#![forbid(unsafe_code)]` at
  `lib.rs:23` plus `#![warn(clippy::indexing_slicing)]`. `git grep` over
  `crates/pdfrum-form/src` and `form_session.rs` finds zero `unwrap()`,
  zero `expect(`, zero `panic!`/`unreachable!` outside `#[cfg(test)]`
  (`text.rs:371` is inside a test's `let…else`), and no bare slice indexing —
  every lookup is `get()`, `checked_sub`, `saturating_sub` or a `let…else`.
  `tab.rs:248` (`remaining.remove(seed)`) is the only index-taking call and is
  guarded by a `get(seed)` on the line above. `FieldWrites::leave` uses
  `saturating_sub`. `Rotation::from_degrees` uses `rem_euclid`, so
  `i32::MIN` degrees is fine.
- **`never_panics.rs` (2527890).** The generative suite is real: awkward rects
  including inside-out and 1e6, rotations from −450 to 1 000 000, choice fields
  named by rows they do not have, and — the two that matter — termination
  assertions on the focus ring and the undo walk, which are the two paths the
  oracle can spin on. The commit message's self-correction (the first version
  pushed random boundaries and asserted their count stays even, which tested the
  generator) is exactly the right diagnosis and the fix — assert that *eviction*
  never splits a well-formed pair — is the right invariant. Only finding 4's
  inside-out-rect hit test is weaker than it looks.
- **Rotation and the plate transform (D3).** `Rotation::from_degrees` normalizes
  into `[0, 360)` once with `rem_euclid` then integer-divides, so a negative or
  out-of-range angle folds correctly and 450° behaves as 90°. `Plate::new`
  normalizes the rect once, `to_plate`/`to_page` are inverses, and `width`/
  `height` swap on the odd quadrants. The round-trip is property-tested.
  Collapsing the C++'s four spaces to two and its two disagreeing rotation
  computations to one is D3 done correctly.
- **STYLE compliance, broadly.** No `Rc<RefCell<…>>`, no global state, no
  builders, no proc-macros, no OOP emulation: every module is a small record plus
  free functions, and the "returns what changed" design in `update.rs` is a
  genuine improvement over the oracle's thirty-slot callback table rather than a
  transliteration of it. One `Error` enum with `thiserror` and `#[non_exhaustive]`.
  The transliteration test (STYLE §7) passes — `band`, `route_key`, `commit::run`
  and `evict_to_fit` all read like Rust someone wrote, not like C++ with `&mut`
  out-params, while preserving the behaviour the briefs pin.
- **`docs/status/M14.md`'s honesty.** The "not portable" reduction (the index −1
  cases against an unsigned index) is recorded rather than quietly dropped; the
  three `*`-marked sub-1%-difference conformance passes are marked rather than
  claimed; the renderer blocker is written as three numbered obstacles with the
  precise `pdfrum-doc` signature change needed, rather than worked around; and the
  `debug_assert!` guarding the inert-dispatch assumption is described as "the
  alarm that fires on the first run where this section stops being true", which is
  the right instinct. The scoreboard genuinely is unmoved (26 lines changed in
  `conformance/scoreboard.json` in the range, all from the earlier `1fa7527`
  baseline re-record, not from this slice).

---

## Could not verify

- **The three status-doc test counts are stale.** `docs/status/M14.md` says "**38**
  ported assertion tests passing … Plus 147 unit tests"; the actual tally at
  `ca6d930` is **47** integration and **151** unit (198 total), and the doc's own
  per-file table sums to 47 (7 + 13 + 9 + 9 + 9). The `never_panics.rs` row was
  added to the table at `2527890` without the headline number being updated. Not a
  finding against the code — the task brief's "47/191" is the right figure — but
  the doc should be corrected, since 38 vs 47 vs 191 is what the exit criterion is
  scored on.
- **The Return-key `valid_` toggle.** `CFFL_TextField::OnChar`'s non-multiline
  Return branch (`cffl_textfield.cpp:117-136`) flips `valid_`, and on the flip to
  *true* recreates the window, sets focus, and `break`s — falling through to
  `CFFL_TextObject::OnChar`, which types the return character. Only the second
  Return commits. `route_char` returns `Commit` on the first. I could not
  determine from reading alone whether the first-Return path is reachable in a
  V8-off `pdfium_test` run (it depends on `valid_`'s state after
  `CreateOrUpdatePWLWindow`), and no ported assertion exercises it. Worth a
  measurement before the operation layer commits to `Commit`-on-first-Return.
- **`pdfrum-doc` interface fit for the blocked work (brief §3.2).** Within the
  range this could not be checked meaningfully — the awaiting code does not
  exist in a form that could mismatch. `TextState` (`field/mod.rs:135-143`)
  carries only `text`, `config` and `undo`, no `layout`/`caret`/`selection`/
  `scroll`, so there is no `vt::Layout` consumer to compare against.

  I did check the one thing that *was* checkable, and then re-checked it against
  `HEAD` once `30d03c0` landed the queries. **The duplicate-`Place` mismatch I
  expected to find has already been resolved, correctly.** At `ca6d930`,
  `pdfrum-form::edit::Place` was a standalone `{section: u32, line: u32, word:
  i32}` with no conversion to anything in `pdfrum-doc`; `30d03c0` then landed a
  structurally identical `pdfrum_doc::vt::hit::Place` — two same-shaped types
  across the seam, needing a conversion at every one of `place_at_point`,
  `point_at_place`, `caret_rect`, `word_index_of_place` and
  `place_of_word_index`. `a43928c` fixes it the right way round: `edit/place.rs`
  now does `pub use pdfrum_doc::vt::hit::Place;` and keeps only the editor's own
  vocabulary — the ordered `Range` and a `PlaceExt` extension trait — with the
  reasoning ("the operations belong to the engine that owns the layout, so a
  second structurally identical type here would buy nothing and cost a
  conversion at every one of those calls") stated in the module doc. The layout
  engine owns the type and the editor extends it. No finding.

  The semantics also agree where they must: both spell `word == -1` as the line
  header, matching `CPVT_WordPlace`'s convention, and `vt/hit.rs:160-165`
  documents the strict-midpoint rule as **strictly** past, which is U2's answer
  as `docs/status/M14.md` recorded it. The remaining fit question — whether
  `TextState` grows the `layout`/`caret`/`selection`/`scroll` fields SPEC §15.3
  declares, and whether `caret_rect`'s `(layout, plate, config, metrics, offset,
  place, width)` arity is convenient from the editor — is still open, because
  none of that code exists yet.
- **`.evt` corpus goldens.** Out of scope for the range (the dispatch is inert by
  design) and the status doc's own reconciliation of the 30-row baseline is
  recorded. I did not run the conformance harness.
- **Later commits on main.** Main advanced from `ca6d930` to `a43928c` during the
  review — twelve commits: `e251303`, `39336fa`, `1ab8029`, `e172d63`, `5b4af15`,
  `061e0c4`, `d406e99`, `62c8c75`, `cc09988`, `30d03c0`, `68bfd46`, `a43928c`.
  Four of them touch files in this review's scope (`061e0c4`, `d406e99`,
  `62c8c75`, `cc09988`) and I checked each finding against `HEAD` — see "State on
  main" at the top. **I did not review those twelve commits**; the verdicts stand
  on the range, and the two `pdfrum-doc` pieces that unblock the text-editing
  clusters (`30d03c0`, the `vt` point→place queries, 925 lines; and `68bfd46`,
  the caller-supplied annotation overlay that `docs/status/M14.md` asks for by
  name) landed too late for me to assess the interface fit that the task asked
  about. That check is now possible and is the thing to do next: whether
  `edit::Place`'s `{section: u32, line: u32, word: i32}` is what
  `pdfrum_doc::vt::hit` hands back, or whether a conversion is needed at every
  boundary.
