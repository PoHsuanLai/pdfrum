<!-- Durable copy of the M14 review record. Source: session
     ede35de1-5746-41fa-ab05-706565ff2804 scratchpad/grok-review-form-block4.md, 2026-09-01. Verbatim. -->

# Grok cross-vendor review: M14 Block 4 (form)

Reviewer: Grok (non-Claude). REPORT-ONLY. Scope: `4a25a37`, `8e45897`, `e451d7f`.
Read: `docs/status/M14.md` Block 4, `M14-doc.md` §8–§9, `SPEC.md` §15.8, `STYLE.md`, C++ oracle under `/mnt/data2/pdfium/pdfium-c++`.

---

## Findings

1. **should-fix** — `crates/pdfrum-form/src/edit/ops.rs:430` (`insert_char` / `replace_range`) and `ops.rs:596` (`scroll_to_caret`).
   **C++:** `CPWL_EditImpl::IsTextOverflow` (`fpdfsdk/pwl/cpwl_edit_impl.cpp:1984-1996`) is true when `!enable_scroll_ && !enable_overflow_` and content is bigger than the plate; `InsertWord` (`:1698`), `InsertReturn` (`:1719`) and `InsertText` (`:1839`) then return without mutating. `enable_overflow_` stays false for a normal text field (`CFFL_TextField::GetCreateParam` never sets `kEditTextOverflow`; only comb sets it, `cpwl_edit.cpp:285`). PDF 32000-1 Table 228: DoNotScroll means once the field is full, **no further text is accepted**.
   **Wrong:** the port maps `enable_scroll_` to `TextEdit::auto_scroll` and uses it only to skip `scroll_to_caret`. A `DoNotScroll` field still accepts characters past the plate; the caret walks off and the extra text sits in the value. That is fewer gates than C++.
   **Fix:** port `IsTextOverflow` (current content vs plate, `FXSYS_IsFloatBigger`) and consult it at the start of `insert_char` / `replace_selection` / newline, matching C++ (check *before* the new character; the overflowing character itself is accepted, the next is not). Keep `scroll_to_caret` gated as now. Add a test that 11 chars into a 10-wide `auto_scroll: false` plate leave the 11th out.

2. **should-fix** — `crates/pdfrum-form/src/route.rs:965-991` (`scroll_text`).
   **C++:** `SetScrollPosY` (`cpwl_edit_impl.cpp:1175-1178`) returns on `!enable_scroll_`. `GetCreateParam` (`cffl_textfield.cpp:54-57`) also withholds `kWindowVScroll` on multiline DoNotScroll.
   **Wrong:** the wheel handler writes `edit.scroll.1` with no `auto_scroll` check. A DoNotScroll multiline field still pans.
   **Fix:** `if !edit.auto_scroll { return; }` at the top of `scroll_text`, same as `scroll_to_caret`.

3. **should-fix** — `crates/pdfrum/tests/form_substitution.rs:184-221` (`a_hebrew_live_edit_sets_its_text_in_the_second_face`); commit `8e45897` added no tests of its own.
   **C++:** `CPWL_EditImpl::Provider::GetCharWidth` (`cpwl_edit_impl.cpp:124-137`) measures the face `GetWordFontIndex` selected (`cpdf_bafontmap.cpp:116-151`). Layout and encoding share that index.
   **Wrong:** the Hebrew test only asserts `/_B1` in the stream and `"_B1"` in `Debug` of resources. A regression that puts `substitute` on `LiveInput` but leaves the width closure on the `/DA` font still passes. That is exactly the F2 residue (`form_textfield_selected_rtl` band ending at col 101, glyphs to 111) this commit claims to close.
   **Fix:** after typing `בחר`, assert a layout number that is the substitute's, not Arial's — e.g. caret/band width, or that the last `Td`/`Tm` advance is `substitute_width` not `TextFont::char_width` for U+05D1. Drive it through `FormSession` so `with_font` is the subject.

4. **nit** — `crates/pdfrum-form/src/edit/ops.rs:615-621` (`SetScrollLimit` port).
   **C++:** `SetScrollLimit` (`:1215-1220`) clamps with `FXSYS_IsFloatSmaller` / `IsFloatBigger` (`core/fxcrt/fx_system.h:36-41`), so a value within 0.0001 of a bound is left alone. The caret tests later in the same function correctly use the 0.0001 helpers.
   **Wrong:** `f32::clamp` uses raw `<=` / `>=`. Harmless for the integer-advance tests; a rounding-edge caret could be pulled a hair further than upstream.
   **Fix:** clamp with `is_float_smaller` / `is_float_equal`, same helpers already in the file.

5. **nit** — `crates/pdfrum/tests/form_substitution.rs:122-136`.
   **C++ / fixture:** `13.392` is Arimo `(905+211)*12/1000`, matching M14-doc §8.5's measured oracle caret; `11.244` is base-14 Helvetica. The fixture is `form_textfield_focused_ltr.in` expanded (provenance is right).
   **Wrong:** if `pdfium-c++/third_party/test_fonts/test_fonts` is absent the test `return`s and CI stays green. The path itself is correct (that directory exists in this checkout).
   **Fix:** `#[ignore]` with a reason, or fail when an env flag says the oracle tree is required — do not silently skip the only assertion that the hermetic face is 13.392.

6. **nit** — `crates/pdfrum-form/src/edit/ops.rs:551-580` (and the `e451d7f` commit message).
   **C++:** `password` is `/MaxLen 5`, value `"tiger"`; M14.md Block 4 later says neither side scrolls and the 189 vs 199 caret is `'*'` 311/1000 (base-14) vs 389/1000 (Arimo).
   **Wrong:** the comment still says password types nine chars into a five-wide box and that the one-frame simplification *is* the 189/199 bug. The two-frame arithmetic is still load-bearing; the password story is the earlier, retracted diagnosis.
   **Fix:** drop the password anecdote; keep the VT vs edit-space table.

7. **nit** — tests that pin the port, not the oracle:
   - `form_substitution.rs::a_session_built_on_a_default_context_agrees_with_the_default_constructor` — two default `BuildContext`s agree. Plumbing.
   - `scroll_to_caret.rs::the_scroll_is_a_distance_and_not_an_absolute_position` — C++ stores `scroll_pos_point_` as an absolute seeded at `rcPlate.left` (`:1105-1106`); this asserts *our* distance representation. Necessary, not an embeddertest number.
   - `scroll_to_caret.rs` `5.0` / `10.0` come from the C++ formula on a 1-unit synthetic face (`SetScrollPosX(ptHead.x - rcPlate.Width())` → `15-10=5`), not from `fpdf_formfill_embeddertest.cpp`. Legitimate formula tests; they would not catch a `caret_x` vs `CPVT_Word::CaretX()` mismatch.

---

## Verified correct

1. **`4a25a37` shared `BuildContext`.** `FormSession::with_context` / `with_config_in` pass the caller's ctx into `FormFonts::load` (`form_session.rs:184-185`). `SubstitutionOptions` is re-exported (`lib.rs:226`). `pdfrum-tool` builds one `BuildContext::with_substitution(substitution_options(options))` per file (`run.rs:136`) and hands it to both `FormSession::with_context` and `walk_pages`. No remaining `BuildContext::new(` in `pdfrum-form`. Remaining `BuildContext::new(` in `pdfrum` is `FormSession::new` / `with_config` (intentional default, documented in SPEC §15.8 and the rustdoc) plus `Page::render` / `text` defaults — the same pair the facade already used. The facade default **does** still resolve `/Arial` through default substitution (no dirs, no Croscore) and therefore still uses built-in base-14 Helvetica 718/−219 when the caller did not thread a context; that is no longer silent, and `pdfrum-tool` is no longer such a caller.

2. **`8e45897` LiveInput / WIDTH closure.** `route.rs::with_font` (`:1371-1386`) is the same construction as `generate_appearances_with_text` (`pdfrum-doc` `ap/mod.rs:687-701`): `da_charset`, first `SUBSTITUTABLE_CHARSETS` entry ≠ DA (Hebrew), `da_font_writes` gate, `substitute_width` in the closure, `TextFont::metrics_of(font, &width)`. `generate` calls `generate_with_live_faces` with `LiveInput { caret_and_selection, live, substitute }` (`route.rs:1562-1572`). `field_body::face_for` uses the same `da_font_writes` for encoding. Callers that bind `_substitute` (`build_edit`, `highlight_of`, `row_height`) still measure through `font.metrics`, so the substitute widths reach layout, caret, bands, and the wheel's content height. Matches `CPWL_EditImpl::Provider::GetCharWidth` using the font `GetWordFontIndex` picked, for characters the `/DA` charset cannot write. (Sticky-font-index behaviour after a Hebrew run — C++ `GetWordFontIndex` with `nFontIndex > 0` — is a pre-existing `font_map` limit, shared with the stored path, not introduced here.)

3. **`e451d7f` horizontal `ScrollToCaret`.** Distance vs absolute table matches `VTToEdit` (`:1105-1106`) and the two `SetScrollPosX` assignments (`:1266-1270`). Left edge: smaller **or equal** (caret on the left counts as out); right edge: `IsFloatBigger` only (caret on the right counts as in). Empty-plate guard is `left == right` with 0.0001. `SetScrollLimit` first: plate wider than content → `scroll.0 = 0` (= `SetScrollPosX(rcPlate.left)`). `auto_scroll` is `!DoNotScroll` (`field/mod.rs:81`, `cffl_textfield.cpp:54-63` for **both** single-line and multiline), applied in `build_edit` (`route.rs:1300`); combo hard-codes `true` (`choice.rs:299`) matching `cpwl_combo_box.cpp:164`. Called from `settle` (every mutation), `move_caret`, `click_at` / `drag_to` / `select_line_at` — the C++ call sites that matter. Vertical half omitted as documented (`:1274-1285` doubled conditions). Gating **more** than C++ on `SetScrollLimit` when `!auto_scroll` is observationally equal, because C++ `SetScrollPosX` no-ops anyway.

5. **STYLE.** No `unwrap` / `expect` / `panic!` in library code these commits touched. `unwrap_or` / `unwrap_or_else` / `unwrap_or_default` only. `form_session.rs:662` `panic!` is `#[cfg(test)]`. `thiserror` unused here (no new error type). `TextEdit::auto_scroll` is one named bool, consistent with `TextConfig`.

---

## Could not verify

- Did not run `cargo nextest`, clippy, or conformance (user: no benchmarks; machine under unrelated load). Counts in M14.md (566 tests, 1651/54 board) are implementer claims.
- Did not re-dump goldens or re-read Arimo `hhea` 905/−211 from `Arimo-Regular.ttf`; 13.392 is trusted from M14-doc §8.5 plus arithmetic.
- Did not walk the whole pixel corpus for a multiline `/Tx` that types past the plate; vertical `ScrollToCaret` being unreachable is plausible (`password`, focused/selected LTR/RTL are single-line) but not proven here.
- Did not execute C++ embeddertests; no Block 4 test number is copied from `fpdf_formfill_embeddertest.cpp`.
- `with_config_in` has no caller and no test; only read the signature.

---

Counts: **0 blockers, 3 should-fix, 4 nits.**
