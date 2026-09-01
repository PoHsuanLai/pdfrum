# Review — the `AF*` library (M15, first slice), `crates/pdfrum-script`

Reviewer: Claude, reviewing Grok Build's implementation — the project rule is
that each vendor reviews the other's slice. Scope: the uncommitted worktree at
`.grok-build/worktrees/grok-mtiy69s4-8yl04w` (`Cargo.toml`, `Cargo.lock`, the
new crate `crates/pdfrum-script/`), read against the design brief
`docs/design/pdfrum-script.md` §1, `STYLE.md`, `DEPS.md`, and the C++ oracle at
`/mnt/data2/pdfium/pdfium-c++` — `fxjs/cjs_publicmethods.{h,cpp}`,
`fxjs/cjs_publicmethods_unittest.cpp`, `fxjs/cjs_util.cpp`,
`fxjs/js_resources.cpp`, `fxjs/fx_date_helpers.cpp` and its unit test, and the
fixtures `testing/resources/javascript/{public_methods,util_printx,util_printd,
util_scand}.{in,_expected.txt}`. The oracle was read, never built or edited.

**Verdict: landable, after the fixes recorded below — all applied in the
worktree.** The transcription quality is high: `StringPrintx`, the
picture-driven date parser and `PrintDateUsingFormat` are faithful to the line,
and Grok found and reproduced genuinely obscure behaviour (the `X`/`A`/`9`
source-advances-but-format-does-not asymmetry, the `iStop = 1` for negative
percents, `First(SIZE_MAX)` keeping the whole string). What needed work was
not the arithmetic but the **contract**: the error strings, the alert shape,
and the colour half of the two red negative styles, all of which the brief §1
had specified and the implementation had dropped or paraphrased.

Test tally after the review, by one `cargo nextest run -p pdfrum-script`:
**104 tests run, 104 passed, 0 skipped** — 89 unit plus 15 integration in the
new `tests/public_methods.rs`. Grok's starting point was 58 unit tests.

---

## 0. The reconciliation: crate or module?

The brief §1.2 specifies **`crates/pdfrum-form/src/af/`, a module**, with types
`AfEvent` / `AfEffects` / `AfError` and a uniform
`fn af_x(ev: &mut AfEvent, args: &XArgs) -> Result<AfEffects, AfError>`. Grok
built **`crates/pdfrum-script`, a crate**, with `AfValue` / `AfOutcome` /
`Keystroke` / `KeystrokeOutcome` / `Error` and positional parameters. The two
were written concurrently and disagree; the disagreement has to be settled
rather than left standing.

**Ruling: keep the crate. Adopt the brief's data shapes inside it.** Two
separable questions were conflated, and they have different answers.

**Where it lives — the crate wins, on the brief's own argument.** §1.2's
requirement is not "be a module"; it is a list of eleven things the library
must not name, import, or transitively reach — `boa_engine`, the M15 engine
module, `pdfrum_doc::form`, `pdfrum_form::{session,route,edit}`, the clock, the
filesystem, the locale, any `static`. §1.2 then offers the enforcement
mechanism: *"a reviewer can check it by reading the `use` lines"*. That is a
manual check that decays. A separate crate whose only dependency is
`thiserror` makes every item on that list a **compile error** — `cargo tree -p
pdfrum-script` is the check, it runs in CI, and it cannot be forgotten. The
brief chose a module to avoid a crate for "a few thousand lines of arithmetic";
the crate is 5 000 lines including tests and its manifest is nine lines, so the
cost the brief was avoiding did not materialise.

The crate also has a scope argument the brief half-anticipated. §1.5.3 observes
that `StringPrintx` is shared by `util.printx` and `AFSpecial_Format` and says
"put it in `af/mask.rs` and let §3's `util` binding call it". That works inside
`pdfrum-form` only if `pdfrum-form` is willing to export a mask engine to the
engine crate. `util.printf`, `util.printd` and `util.scand` have exactly the
same shape — pure functions over text and an explicit clock — and no better
home. Grok put all four in the crate, which is why it is named `-script` and
not `-af`. That is the right call and it is why the boundary the brief drew
around the `AF*` names alone would have been awkward.

**What it looks like — the brief wins, on every point that matters for M15.**
The brief's vocabulary was not arbitrary; it was derived from the transcript,
and Grok's substitutions lose information the transcript pins:

- **`AfEffects` is the whole point of §1.3, and it was dropped.** The brief
  says, twice, that the two things the oracle does through the host — raise an
  alert, recolour the field — come back as *data* "at the cost of one enum
  variant crossing the boundary". Grok's crate documented the opposite: its
  `README.md` and module doc both said "Color side-effects that PDFium applies
  for some negative-number styles are omitted". For `negStyle == 1` that is not
  a side effect. That style **prints no minus sign at all**
  (`cjs_publicmethods.cpp:688-727`, and pdf.js `aform.js:125-151` agrees, so it
  is Adobe's specification and not a defect) — the colour *is* the sign, and a
  host given only the string cannot tell `-12.50` from `12.50`. Restored: see
  finding 3.
- **The alert needs a caller, an icon and a button**, because the transcript
  compares the whole rendered line. `public_methods_expected.txt` carries
  sixteen of them, in the form
  `AFRange_Validate[icon=3,type=0]: The input value must be …`. Grok's
  `AfAlert` was a bare message enum. Restored as the brief's
  `{ caller, message, icon, button }`: see finding 2.
- **The error strings are API, and they were paraphrased.** Finding 1, the
  most serious of the review.

**What the brief specified and I did *not* adopt**, with reasons, so the
divergence is deliberate rather than drift:

- **`&mut AfEvent` first.** The brief's consequence (1) argues for it so a
  function can write several event fields and the caller copies back only the
  live ones. In practice no function writes more than one: `AFNumber_Format`
  writes `value`, `AFNumber_Keystroke` writes `value` or `rc`,
  `AFSpecial_KeystrokeEx` writes `change` or `rc`. Returning what changed is
  the same information, and it has a property the `&mut` form does not: a
  function that fails cannot leave a half-written event behind. The caller does
  the same copy-back, from a value.
- **Argument structs.** The brief cites STYLE §4's "plain config structs with
  `Default`". That rule is aimed at options nobody can remember the order of.
  With `curr_style` correctly absent — the brief is right that the oracle reads
  and discards it, `cjs_publicmethods.cpp:637` — no function exceeds six
  arguments, and the positional order is the one Adobe's own reference
  documents. A reader checking this code against the specification has that
  order in front of them; a struct would make them translate. Kept positional.

The brief's §1 now carries a `[spec]` note recording all of this, per its own
rule that "nothing else in this document may change it without a `[spec]` note
here".

---

## 1. Findings

Severity: **blocker** — wrong observable behaviour or a broken contract;
**should-fix** — correct but loses information a caller needs; **nit**.

### Finding 1 — blocker — the error messages were paraphrases, and they are API

`src/error.rs`, every variant. Grok's `#[error(...)]` strings were lowercased
rewordings: `"incorrect parameter value"`, `"the input value is invalid"`,
`"the input value can't be parsed as a valid date/time ({format})"` — no
capital, no trailing period.

The oracle's `JSGetStringFromID` (`fxjs/js_resources.cpp:9-95`) is a hard-coded
switch with no localisation layer, and the goldens compare its output character
for character. `public_methods_expected.txt` carries 67 lines of the form
`Alert: PASS: AFSimple('nonesuch', 2, 3) threw AFSimple: Incorrect parameter
value.` Every one of them would fail against a paraphrase.

The variant set was also short: `ParamCount`, `BadObject`, `NoEventHandler` and
`DateKeystrokeArity` were all absent, and the last two are the ones the brief
§1.6 specifically warns "a tidy port would normalise and must not" — they are
bare literals with **no trailing period**
(`cjs_publicmethods.cpp:625` and `:1011-1012`).

**Fixed.** `Error` now carries the brief's full table verbatim, and
`src/error.rs` opens with a module doc saying why the strings must not be
tidied. Two tests pin them: `error_display_is_the_oracle_message_table` and
`the_two_bare_literals_have_no_trailing_period`.

One quirk worth recording, since it looks like a typo and is not:
`AFTime_KeystrokeEx()` throws **`AFDate_KeystrokeEx's parameter size not
correct`** — naming the callee. `public_methods_expected.txt:369-370` records
it. `af_time_keystroke_ex`'s doc says so.

### Finding 2 — blocker — the alert lost its caller, icon and button

`src/af/mod.rs`, `src/error.rs`. Grok's `AfAlert` was a message-only enum, and
the outcomes carried `alert: Option<AfAlert>` inline.

The transcript's sixteen notification lines are rendered as
`{caller}[icon={icon},type={button}]: {message}`, so all four fields are
observable. The brief §1.3 specifies exactly that shape and even pins the
constants (`icon = 3`, `button = 0`, `AlertIfPossible`,
`cjs_publicmethods.cpp:94-102`) — confirmed at that line.

The attribution is not mechanical, either. `AFPercent_Keystroke` is
`AFNumber_Keystroke` registered twice (`:921-926`), and its alert names
`AFNumber_Keystroke` while the *thrown* text names `AFPercent_Keystroke` —
`public_methods_expected.txt:280-281` shows both on adjacent lines.

**Fixed.** `AfAlert { caller, message, icon, button }` with a `Display` that
renders the transcript line, `AlertMessage` carrying the text alone, and
`AfEffects { alerts, text_color }` as the brief specified.
`alert_display_is_the_transcript_notification_line` pins six of them verbatim,
and `tests/public_methods.rs` drives all sixteen.

### Finding 3 — blocker — `negStyle` 1 and 3 lost their colour entirely

`src/af/number.rs:116-124` as written:

```rust
        // i_neg == 1: red, no minus — colour omitted.
```

With the colour omitted, `AFNumber_Format(2, 0, 1, 0, '', false)` answers
`"12.50"` for both `12.5` and `-12.5`. The sign is unrecoverable. This is the
single case the brief's `AfEffects` was designed for.

**Fixed.** `af_number_format` returns `AfFormat`, whose `effects.text_color` is
`Some(AfColor::RED)` for a negative under style 1 or 3 and `Some(AfColor::BLACK)`
for a non-negative under those styles — the oracle writes black back to undo an
earlier red (`:707-724`), comparing against the field's current colour first;
the comparison needs the field, so black is reported unconditionally and the
host compares, exactly as the brief prescribes. `AfColor` / `AfColorSpace`
match the `["T"] / ["G",g] / ["RGB",r,g,b] / ["CMYK",…]` encoding.
`negative_styles_split_the_sign_between_text_and_colour` and
`red_styles_ask_for_black_when_the_value_is_not_negative` pin it.

### Finding 4 — should-fix — a throw that also notifies dropped the notification

`src/af/number.rs`, `src/af/date.rs`. `AFNumber_Keystroke` on a non-numeric
commit does three things (`cjs_publicmethods.cpp:757-763`): sets `Rc = false`,
calls `AlertIfPossible`, **and** returns `Failure`. The transcript records two
lines for that one call:

```
AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.
Alert: PASS: AFNumber_Keystroke(1, 2) threw AFNumber_Keystroke: The input value is invalid.
```

Grok returned a bare `Err(Error::InvalidInput)`; the notification was gone.
`AFDate_FormatEx` and `AFParseDateEx` have the same shape (`:955-960` and `:1310-1316`).

**Fixed.** `Thrown { error, effects }` pairs the exception with whatever the
same call still asked of the host. `commit_of_a_non_number_notifies_and_throws`
pins both halves.

### Finding 5 — should-fix — `AF_MakeArrayFromList` dropped a trailing empty name; the port did not

`src/af/calc.rs`, `af_split_field_list`. Grok used `s.split(',')`, so `"a,"`
yields `["a", ""]`.

`AF_MakeArrayFromList` (`cjs_publicmethods.cpp:330-373`) walks with
`strchr(p, ',')` inside `while (*p)`, so after consuming `"a"` the pointer is at
the terminator and the loop ends — one name, not two. I confirmed by compiling
the C++ loop standalone: `""` → 0 names, `"a,"` → 1, `"a,b,"` → 2, `",a"` → 2
(a *leading* empty name is kept), `"a,,b"` → 3.

This matters: a trailing comma in a `/CO` name list would add a phantom field
that resolves to nothing — harmless for `SUM`, but it changes an `AVG` divisor
if the caller counts what it is handed.

**Fixed**, with the leading and interior empty names preserved.
`splitting_a_name_list` covers all six cases.

### Finding 6 — should-fix — `AFExtractNums` planted its zero in the wrong place

`src/af/merge.rs`. Grok inserted the `'0'` into the working string and then ran
the digit-run split over it, which is what the C++ does
(`:1530-1532`, `InsertAtFront` then the loop) — so the *answer* was right. But
the code read as though `.5` would come back as one number, and Grok's own test
comment had to explain that it comes back as two. The nicety in the oracle does
not achieve what it looks like it achieves: the mark that prompted the zero
immediately ends the run the zero started.

**Fixed** — the zero is now pushed as its own run, which is what actually
happens, and the doc says so. Behaviour unchanged. One test expectation of mine
was itself wrong here and the code corrected me: `af_extract_nums("...")` is
`Some(["0"])`, not `None`, because the leading mark plants a zero even when no
digit follows.

### Finding 7 — should-fix — the crate-level `allow`s hid twenty-nine fixable lints

`src/lib.rs` carried nine crate-level `allow`s, with the justification "Date/
number transcription matches C++ `static_cast` and long format parsers". I
removed all nine and counted what fired: **85 warnings**, of which 29 were
real and fixable in the library. The `cast_*` family was hiding, among other
things, `i32 as usize` on `sel_start` — which is *load-bearing* (a negative
start must saturate, and the code relied on it silently) — and eight
unchecked `f64 as i32` narrowings in `parse_date_as_gmt` and `trunc_i32` where
a malformed date can supply a value outside `i32`.

**Fixed.** The library now has **no `cast_*` allows at all**. Every narrowing
is either `usize::try_from(...)` with the saturating case written out and
commented, or goes through a documented `trunc_i32` / `narrow_to_i32` that
clamps before truncating and carries a targeted `#[expect(…, reason = "…")]`
proving the clamp makes the cast safe. Three allows survive, each earned:
`fn_params_excessive_bools` and `too_many_arguments` (reproducing fixed
external signatures) and `too_many_lines` (the picture-driven parsers are one
table-shaped match each). `cargo clippy -p pdfrum-script --all-targets --
-D warnings` is clean.

### Finding 8 — nit — `iDec2` was advanced where the oracle leaves it stale

`src/af/number.rs`. The brief §1.5.1 step 8 flags that the `.5` → `0.5`
insertion does **not** update `iDec2`, which the separator pass then uses stale.
Grok wrote `i_dec2 = 1;` after the insert. I confirmed by compiling
`CalculateString` plus the separator loop: the branch is unreachable in
practice, because `std::fixed` always emits the leading zero itself, so
`iDec2` is never `0` when the value has a fractional part. Harmless either way,
but the divergence was silent.

**Fixed** — the update is removed and the reason the branch is dead is written
down, so a future reader does not "fix" it back.

### Finding 9 — nit — `combined_len`'s unsigned wraparound was modelled in `i64`

`src/af/special.rs`. `CJS_PublicMethods::AFSpecial_KeystrokeEx` computes
`valEvent.GetLength() + wChange.GetLength() + SelStart() - SelEnd()` in
`size_t` (`:1200-1202`). With `SelStart() == -1` and `SelEnd() >= 0` the
subtraction wraps to an enormous number and the keystroke is refused as too
long. Grok's `i64` arithmetic gives a small number instead and accepts it.

Unreachable in practice — a selection end without a selection start is not a
state the event model produces — but the two disagree.

**Fixed** by taking the branch the oracle's arithmetic lands on directly: a
`sel_start` that will not convert to `usize` refuses with "too long", which is
both what the wraparound produces and what makes sense (there is nowhere for
the keystroke to go). Written as a `let ... else` so the reasoning is visible.

---

## 2. The "surprising behaviours reproduced on purpose", re-verified

Grok listed twelve. I checked each at its cited line rather than taking the
list on trust. **All twelve are true of the C++.** Six of them are also
**defects rather than specification**, and the user ruled that we implement the
correct behaviour and bucket the goldens that pin the defect. The full
divergence record — six entries with both citations, and the two golden
assertions it costs — is in `docs/status/M15.md` § "`AF*`: where we diverge
from the oracle on purpose". Summarised here with the verdict on each:

| Grok's claim | True of the C++? | Verdict |
|---|---|---|
| Inverted leap-year rule | Yes, `fx_date_helpers.cpp:71` | **Bug — corrected.** The clause reads `year % 400 != 0`; it should be `== 0`. It also contradicts `DayFromYear` two functions above. |
| Two-digit years → 2000–2099 | Yes, `:330` + `:556` | **Bug — corrected** to pivot at fifty. `nYearSub = 99;  // nYear - 2000;` has the intended formula in a comment. |
| `IsNumber` quirks | Yes, `:272-308` — one mark of either spelling, sign only at index 0, exponent needs an explicit sign | **Specification, except the empty string.** Verified against `cjs_publicmethods_unittest.cpp:14-45`, all twenty rows. The empty-and-whitespace case is flagged `TODO(weili)` in the oracle's own test; corrected. Unobservable through `AF*` — `af_number_keystroke` accepts an empty commit *before* consulting the grammar. |
| neg style 1 emits no minus | Yes, `:688-726` | **Specification** — pdf.js `aform.js:125-151` agrees. Kept; the colour is now returned so the sign is not lost. |
| `AVG` case-sensitivity | Yes — `:215-232` matches `NoCase`, `:1341`/`:1442`/`:1456` re-test with `EqualsASCII` | **Bug — corrected.** pdf.js matches all five consistently. |
| 12-hour clock, hour 12 is AM | Yes, `:517-519` and `:526-528` with `:535` | **Bug — corrected.** Noon is pm and midnight prints `12`. |
| `printd` weekday always Sunday | Yes, `cjs_util.cpp:267` — `struct tm time = {}` never sets `tm_wday` | **Bug — corrected.** This is the only one that costs a golden: two assertions. |
| `nDec > 512` → `"%"` | Yes, `:851-858` | **Specification** (Acrobat's buffer). Kept. |
| `sepStyle` clamps | Yes — `ValidStyleOrZero` at `:174-176` for `AFNumber_Format`, and the `0..=49` **validate-and-throw** for `AFPercent_Format` at `:846-848` (`kMaxSepStyle` at `:840`) | **Specification.** The asymmetry is real and load-bearing: style 4's apostrophe separator is reachable only through the percent entry point, which `public_methods_expected.txt:406` pins. Kept. |
| `AFExtractNums(".5")` | Yes, `:1523-1557` | **Specification.** Kept — see finding 6. |
| Day 1..31 ignoring month | Yes, `fx_date_helpers.cpp:236-239`, carrying its own `TODO(thestig)` | **Specification** — the loose validator is what makes the heuristic date parse work at all. Kept. |
| `ParseDateAsGMT` token count | Yes, `:963-981` — exactly 8 tokens or return 0 | **Specification.** Kept. Worth noting: the oracle's own example comment, `"Tue Aug 11 14:24:16 GMT+08002009"`, splits into **seven** tokens and so returns 0. Pinned. |
| `"12302015"` + `mm/dd/yyyy` | Yes — the picture's separators consume value characters | **Specification** in the sense that the fixture's own comment calls it wrong (`TODO(crbug.com/572901)`) but Acrobat's behaviour is unknown, so there is nothing better to implement. Kept and pinned. |

Two notes on the list itself. First, one entry was mis-attributed: the "12-hour
clock" quirk is described in the brief's §1.5.2 table as belonging to
`PrintDateUsingFormat`, which is right, but Grok's `printd.rs` correctly used a
*different* rule there (`strftime`'s `%I`, which is already correct); only
`print_date_using_format` had the bug. Both now use the same corrected helper.
Second, the brief's §1.5.3 mask table lists `X` as "copy if ASCII alphanumeric"
— that is `O`. `MaskSatisfied` at `:311-323` gives `X` an unconditional
`return true`, and `StringPrintx` at `cjs_util.cpp:333-343` gives its own `X`
the alphanumeric test. Two different `X`s in two different mask languages.
Grok got both right; the brief's table is what is wrong, and the review notes it
here rather than editing §1.5.3, since the code is the authority now.

---

## 3. The partials Grok recorded

Each was checked for whether the limitation is recorded *in the code*, not only
in a hand-off note.

| Partial | Recorded? | Action |
|---|---|---|
| `AFSimple_Calculate` split into arithmetic + pre-resolved values | Yes, and matching brief §1.5 Group C | **Kept**, with the rule the brief flagged now written into the doc: a field that exists but yields nothing contributes a zero *and still divides the average*, while a name matching no field contributes nothing. A caller that collapses those two gets the wrong mean. |
| neg styles 1/3 without text colour | Documented as permanent | **Reversed** — finding 3. The brief chose `AfEffects` precisely so this would not be lost. |
| `printd` without `FX_LocalTime` | Yes, in the module doc | **Kept.** `GetLocalTZA` and `GetDaylightSavingTA` both return 0 unless `FPDF_POLICY_MACHINETIME_ACCESS` is enabled (`fx_date_helpers.cpp:38-68`), so the oracle's default *is* our behaviour. Doc now says the caller adds its own offset. |
| Hand-written `printf` `%e`/`%g` | Yes | **Kept.** No fixture exercises them; recorded as the one place the crate reimplements a C library routine by hand. |
| No V8 `Date.parse` fallback | Yes, on `parse_date_with_fallback` | **Kept**, and correct per brief §1.5: the fallback is "literally the JS engine's date parser" and belongs to the engine binding. The crate falls through `kBadDate` to the heuristic, which is what the `AF*` paths need. |

---

## 4. `public_methods.in` driven directly: **291 of 356 assertions**

The brief §1.8 calls this fixture "the entire `AF*` conformance suite in one
fixture" and asks how much is reachable without an engine. Counted, then
driven, in `crates/pdfrum-script/tests/public_methods.rs` (15 tests):

| | count | where it goes |
|---|---|---|
| `expectEventValue` | 276 | 261 driven; 15 need a form to resolve field names |
| `expect` (return value) | 13 | all 13 driven |
| `expectError` | 67 | 10 driven (value errors); 57 are argument-count checks |
| **total** | **356** | **291 driven** |

The 57 argument-count assertions are not a gap — arity is checked by the caller
per brief §1.4, and every one of them expects the same
`Incorrect number of parameters passed to function.` that `Error::ParamCount`
now carries; the string is pinned here and the assertions belong to the engine
binding's tests. The 15 `AFSimple_Calculate` cases are driven through
`af_simple_calculate` with the fixture's own field values (`Text2` = 123,
`Text3` = 456, `Text4` = 407.96) substituted for the names.

The largest single block is `AFPercent_Format`: **197 unique rows** sweeping
five separator styles across seven decimal counts and both sign placements,
transcribed from the fixture with its expected strings, including the
308-decimal-place row. All 197 pass.

Across all four `AF*` fixtures: **387 assertions reachable without an engine,
385 byte-exact, 2 in the oracle-bug bucket** (`docs/status/M15.md` has the
table).

---

## 5. Gates

From the worktree, after the fixes:

| gate | result |
|---|---|
| `cargo build -p pdfrum-script` | clean |
| `cargo nextest run -p pdfrum-script` | **104 passed, 0 failed, 0 skipped** |
| `cargo test --doc -p pdfrum-script` | 2 passed |
| `cargo clippy -p pdfrum-script --all-targets -- -D warnings` | clean, with no `cast_*` allows |
| `cargo fmt -p pdfrum-script --check` | clean |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok |
| `cargo tree -p pdfrum-script` | `thiserror` and its proc-macro chain, nothing else |
| `cargo build --workspace` | clean |
| `cargo nextest run --workspace` | 3745/3747 — see below |

**DEPS.md is honoured.** The crate adds no external dependency: `thiserror` is
already in the closed manifest and is the only edge in the tree.

**STYLE.md.** `#![forbid(unsafe_code)]` present. No `unwrap`, `expect`,
`panic!` or unchecked indexing on any input-reachable path in the library —
`clippy::indexing_slicing` is warned crate-wide and fires nowhere; every slice
goes through `get()`; every narrowing is checked or documented-and-clamped.
`unwrap`/`expect` appear only in tests, under a `cfg_attr(test)` allow.

**The two workspace failures are environmental, not regressions.**
`pdfrum-font::corpus`'s `decoding_a_corpus_page_never_panics_and_widths_are_finite`
and `every_extracted_character_is_reachable_through_some_page_font` both fail
with `no golden resolved to a corpus file`: `conformance/goldens/` is untracked
and so absent from the worktree (1 357 entries on main, 0 here). Both pass on
main, and I re-ran the workspace there to confirm after landing.

---

## 6. For filing upstream (crbug.com/pdfium)

Two of the six are one-line repros and worth reporting. Paragraphs written to
be pasted as-is.

**`IsLeapYear` has an inverted century-exception clause.**
`fxjs/fx_date_helpers.cpp:71` reads
`return (year % 4 == 0) && ((year % 100 != 0) || (year % 400 != 0));`. The
second clause of the disjunction should be `year % 400 == 0`; as written it is
true for every year not divisible by 400, so the `% 100` exception can never
fire and the `% 400` exception fires backwards. The result is that 2000 is
reported as a common year and 1900 as a leap year — `IsLeapYear(2000)` is
`false` and `IsLeapYear(1900)` is `true`, both inverted. This also puts the
function at odds with `DayFromYear` three functions above it, which counts leap
days with the correct `/4 − /100 + /400` cadence, so for a century year the two
disagree about how many days the year has; `TimeFromYearMonth` and
`MonthFromTime` consume both. The bug survives because the only leap year
`fx_date_helpers_unittest.cpp` exercises is 1972, on which both spellings
agree. Repro: `EXPECT_TRUE(IsLeapYear(2000))` fails; equivalently,
`FX_ParseDateUsingFormat(L"29/02/2000", L"dd/mm/yyyy", &out)` returns
`kBadDate` for a date that exists. Mozilla's pdf.js, implementing the same
Adobe-specified library, delegates to the JavaScript `Date` object and is
unaffected.

**Two-digit years are hardcoded to 2000–2099 with the intended formula left in
a comment.** `fxjs/fx_date_helpers.cpp:330` declares
`int nYearSub = 99;  // nYear - 2000;` and `:556` then applies
`if (nYear >= 0 && nYear <= nYearSub) { nYear += 2000; }`, mapping every
two-digit year unconditionally into 2000–2099. The commented-out expression
suggests a sliding window relative to the current year was intended and never
written. Acrobat's documented behaviour, and Mozilla's pdf.js implementation of
the same library (`src/scripting_api/util.js`, `_scand`), pivot at fifty:
`n < 50` adds 2000, otherwise `n < 100` adds 1900. The practical effect is that
a date typed into an `AFDate_Keystroke` field as `31/12/85` — a birth date, a
document date, an expiry — is parsed as 31 December **2085**, and
`AFDate_Format` then prints it back as such. Repro:
`FX_ParseDateUsingFormat(L"31/12/85", L"dd/mm/yy", &out)` yields a time value
whose `FX_GetYearFromTime` is 2085, where Acrobat gives 1985.

---

## 7. Credit

The transcription is Grok Build's, and it is good work: the mask engine, the
picture parser and `PrintDateUsingFormat` are faithful to the line, the C++
unit-test tables were ported rather than guessed at, and the twelve surprising
behaviours it listed were all genuinely true of the oracle — a list assembled by
reading the code rather than by repeating folklore, which is exactly what the
review asked for and is rarer than it should be. The gaps were all on the
contract side, where the brief had already done the design work and it had not
been picked up.
