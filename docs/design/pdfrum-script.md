# Design brief — JavaScript (M15)

**Status:** brief only. Nothing here is implemented. Escalations are in §7 and
are *raised*, not decided.

**Sources read:** PLAN.md §M15 (settled decisions, four-step order), PLAN.md
§M14 + docs/status/M14.md (what the event model became, and what the `Cascade`
seam *shipped* as, which is not what §M14 planned), docs/design/pdfrum-form.md
(the structural model for this document), STYLE.md (binding; §2b's closed seam
list and its 2026-09-01 invert-versus-expose clause), SPEC.md §10 and §15,
DEPS.md, docs/status/M14-gaps.md, and the C++ oracle at
`/mnt/data2/pdfium/pdfium-c++` — `fxjs/` entire, `fpdfsdk/cpdfsdk_*`,
`testing/resources/javascript/`, `testing/tools/`.

**Three C++ file names PLAN.md and the task brief both use do not exist in this
oracle revision.** They were merged upstream, and the merge changes the design,
so it is recorded here before anything else rather than silently worked around:

| Named in the plan | Reality at this revision |
|---|---|
| `fxjs/cjs_eventrecorder.{h,cpp}` | **Gone.** `CJS_EventRecorder` was folded into `CJS_EventContext`. Zero occurrences of "EventRecorder" in the tree. The recorder *is* the context — one class both records the event fields and runs the script, which removes the bind/unbind step §3 was expected to describe. |
| `fpdfsdk/cpdfsdk_actionhandler.cpp` | **Gone.** `CPDFSDK_ActionHandler` was folded into `CPDFSDK_FormFillEnvironment`. The `DoAction_Field`-style names are now `DoActionField` etc., members of `fpdfsdk/cpdfsdk_formfillenvironment.cpp`. |
| `fxjs/cjs_global.cpp` (spelled `cjs_globaldata.cpp` in the task) | Both exist, but the persistent store is `fxjs/cfx_globaldata.cpp`, not `cjs_globaldata.cpp`. |

Section map:

- §1 — **the `AF*` interface contract**, written first and self-contained for the concurrent agent
- §2 — the `Cascade` implementation: what shipped, what is missing, and the additive change
- §3 — the object-model inventory and its three tiers
- §4 — the `event` object's lifecycle
- §5 — the sandbox as a testable property
- §6 — the test plan
- §7 — escalations and open questions

---

## 1. The `AF*` interface contract — self-contained

**This section is the whole specification a concurrent agent needs to write the
`AF*` library. It depends on nothing else in this document, and nothing else in
this document may change it without a `[spec]` note here.** Read §1 and stop.

> **`[spec]` 2026-09-02 — the library shipped as a crate, not a module, and its
> vocabulary is named accordingly.** §1.2 called for
> `crates/pdfrum-form/src/af/`; what landed is **`crates/pdfrum-script`**, a
> workspace member depending on `thiserror` and nothing else. The full
> reasoning is in `docs/reviews/claude-review-af-library.md`; in short, §1.2's
> requirement is a *list of things the library must not reach*, and a separate
> crate turns that list into a compile error rather than something a reviewer
> has to check by reading `use` lines. The crate also carries `util.printf`,
> `util.printd`, `util.printx` and `util.scand` — §1.5.3 had already observed
> that `StringPrintx` is shared between `util.printx` and `AFSpecial_Format`,
> and splitting one mask engine across two homes to honour a boundary drawn
> around the `AF*` names alone would cost more than it bought. The crate is
> named `-script` rather than `-af` for that wider scope.
>
> **The three data types of §1.3 landed with their shapes intact and their
> names changed**, plus two additions:
>
> | §1.3 | as shipped | note |
> |---|---|---|
> | `AfEvent` | `Keystroke` | The same fields, less `rc` — which the outcome reports rather than the event carrying it mutated in place. Named for what it models rather than for the engine object it is a slice of. |
> | `AfEffects` | `AfEffects` | Unchanged, and **adopted exactly as §1.3 specified**: alerts and text colour come back as data. Worth saying plainly, because the first implementation dropped the colour half entirely and documented the loss; §1.3 chose this shape precisely so it would not be lost, and the review restored it. |
> | `AfError` | `Error` | §1.6's message table, verbatim, with `ParamCount`, `BadObject`, `NoEventHandler` and `DateKeystrokeArity` present. |
> | — | `Thrown` | An `Error` together with the `AfEffects` a failing call still asked for. `AFNumber_Keystroke` on a bad commit both notifies **and** throws, and the transcript records both lines; an `Err` carrying only the message would lose half the answer. |
> | — | `AfFormat`, `KeystrokeResult` | The outcome and its effects as one record, keeping §1.4's uniform return shape without a bare tuple. |
>
> **§1.4's signature rule is relaxed where it bought nothing.** `&mut AfEvent`
> first was chosen so one function could write several event fields; in
> practice each writes at most one, so the shipped signatures take
> `&Keystroke` and return what changed. A caller applying that to a live event
> does the same copy-back §1.4's consequence (1) describes, from a value rather
> than through a reference — and gains that a function cannot corrupt an event
> it then fails on. Argument structs were **not** adopted: with `curr_style`
> correctly absent no function exceeds six arguments, and the positional form
> is the one Adobe's own reference documents, which is what a reader checking
> this code against the specification has in front of them.
>
> **§1.5's `SimpleOp` landed with its case rule inverted.** §1.5 asked for a
> `matched_exact_case: bool` reproducing the oracle's asymmetry between a
> case-insensitive match and a case-sensitive divide. The user ruled that
> asymmetry a defect rather than behaviour, so `SimpleOp::parse` is
> case-insensitive throughout. Five further oracle defects were corrected the
> same way; `docs/status/M15.md` § "`AF*`: where we diverge from the oracle on
> purpose" lists all six with citations, and the two golden assertions the
> corrections cost.
>
> **§1.7 stands**: the crate takes no `Diagnostics`.

### 1.1 What the library is, in one sentence

`AF*` is the Acrobat form-field format/keystroke/validate/calculate helper
library that PDF files call from their `/AA` scripts; in the oracle it is 22
bare global functions registered by `fxjs/cjs_publicmethods.cpp:48-71`, and here
it is a module of **pure functions over plain data** with no engine, no `boa`,
no session and no document.

### 1.2 Where it lives, and what it must not depend on

**Module: `crates/pdfrum-form/src/af/`.** Not a crate — a module, because it is
a few thousand lines of arithmetic and string formatting with one dependency
(`pdfrum-common`'s `Diagnostics`, and not even that if §1.7's design is taken).

It **must not** name, import, or transitively reach:

- `boa_engine` or any `boa_*` crate, or any type from one;
- the M15 engine module (§2) — no `Session`, no `Realm`, no `Context`;
- `pdfrum_doc::form` — no `Field`, no `Form`, no `FieldValues`;
- `pdfrum_form::session`, `::route`, `::edit` — no `FormSession`, no `TextEdit`;
- the clock, the filesystem, the locale, or any `static`.

`crates/pdfrum-form/src/af/mod.rs` declares `#![forbid(unsafe_code)]` (inherited)
and the module doc states the above as a rule, so a reviewer can check it by
reading the `use` lines.

The mechanical check: `af/` compiles with only
`use crate::af::…;` and `use core::…/std::…` imports. If a `use super::` or a
`use pdfrum_doc::` appears in `af/`, the contract is broken.

### 1.3 The three data types — the whole vocabulary

```rust
/// The mutable slice of `event` an `AF*` function may read and write.
///
/// This is a *value*, passed in and handed back. It is deliberately not a
/// reference into a session: an `AF*` function cannot reach a field, a
/// document, or the engine, and this type is why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AfEvent {
    /// `event.value` — the field's text.
    pub value: String,
    /// `event.change` — the text being inserted.
    pub change: String,
    /// `event.changeEx` — a choice field's export value. Read-only upstream.
    pub change_ex: String,
    /// `event.selStart`, as a character index. `-1` means "no selection".
    pub sel_start: i32,
    /// `event.selEnd`, as a character index.
    pub sel_end: i32,
    /// `event.willCommit`.
    pub will_commit: bool,
    /// `event.fieldFull`.
    pub field_full: bool,
    /// `event.rc` — the accept flag. Starts `true` on the paths that read it.
    pub rc: bool,
}

/// What an `AF*` function did, besides mutating the event.
///
/// Two things the oracle does through the host that this library cannot:
/// raise an alert, and recolour the field's text. Both come back as data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AfEffects {
    /// Alerts to raise, in order. `AlertIfPossible` is a no-op without a
    /// form-fill environment upstream, so an empty vector is a valid answer.
    pub alerts: Vec<AfAlert>,
    /// A text colour the function asked the *target field* to take.
    /// `AFNumber_Format` negStyle 1 and 3 are the only producers.
    pub text_color: Option<AfColor>,
}

/// Why an `AF*` call could not proceed. Maps to the oracle's thrown exception.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AfError { /* variants in §1.6 */ }
```

`AfAlert` is `{ caller: String, message: String, icon: u8, button: u8 }`.
Every alert an `AF*` function raises uses `icon = 3`
(`JSPLATFORM_ALERT_ICON_STATUS`) and `button = 0`
(`JSPLATFORM_ALERT_BUTTON_OK`) — `AlertIfPossible`,
`fxjs/cjs_publicmethods.cpp:94-102`. `caller` is the function's own name.

`AfColor` is `{ space: AfColorSpace, components: [f32; 4] }` with
`AfColorSpace` one of `Transparent | Gray | Rgb | Cmyk`, matching the
`["T"] / ["G",g] / ["RGB",r,g,b] / ["CMYK",c,m,y,k]` array encoding of
`fxjs/cjs_color.cpp:54-141`.

### 1.4 The signature shape — one rule, applied 22 times

**Every `AF*` function has this shape and no other:**

```rust
pub fn af_number_format(ev: &mut AfEvent, args: &NumberFormatArgs)
    -> Result<AfEffects, AfError>;
```

That is: **`&mut AfEvent` first, a per-function argument struct second, and
`Result<AfEffects, AfError>` out.** Three consequences the concurrent agent
should hold onto:

1. **Mutation is on the event, not the return.** `AFNumber_Format` writes
   `ev.value`; `AFNumber_Keystroke` writes `ev.value` and `ev.rc`;
   `AFSpecial_KeystrokeEx` writes `ev.change` and `ev.rc`. The caller in §2
   copies back only the fields §4.4's table says are live for that event kind,
   and discards the rest — so writing a field that is dead for the event is
   correct and harmless, exactly as upstream's dummy-fallback does.
2. **`AfEffects` is never `()`, even when empty.** Uniform return type keeps
   the dispatch table in §2 a single match arm shape.
3. **Functions that *return a JS value* return it as a third thing.** Four do:
   `AFSimple`, `AFMakeNumber`, `AFParseDateEx` return a number, and
   `AFMergeChange` and `AFExtractNums` return a string and a list. Those take
   the same `&mut AfEvent` (some read it) and return
   `Result<(T, AfEffects), AfError>` with `T` the value.

**Argument structs, not positional parameters.** `AFNumber_Format` takes six
arguments of four different meanings; a six-tuple would be unreadable and
STYLE §4's "plain config structs with `Default`" is the house rule.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SepStyle { CommaDot, NoneDot, DotComma, NoneComma, ApostropheDot }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegStyle { MinusBlack, Red, ParensBlack, ParensRed }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberFormatArgs {
    pub decimals: u32,
    pub sep_style: SepStyle,
    pub neg_style: NegStyle,
    pub currency: String,
    pub currency_prepend: bool,
}
```

**Note there is no `curr_style` field, deliberately.** The oracle takes six
arguments and its own comment at `fxjs/cjs_publicmethods.cpp:637` says
`// params[3] is iCurrStyle, it's not used.` The *arity* is six and the
argument-count check must still be six (§1.6), but the value is discarded, so
the struct does not carry a field nothing reads. The arity check lives in the
engine binding (§2), not here.

**Enum conversion is where the clamping lives.** `ValidStyleOrZero`
(`fxjs/cjs_publicmethods.cpp:174-176`, over `WithinBoundsOrZero` at `:170-172`)
turns any out-of-range style into `0`, so:

```rust
impl SepStyle {
    /// `AFNumber_Format`'s clamp: 0..=3, anything else becomes 0.
    pub fn from_number_format(n: i32) -> SepStyle { /* 0..4 window */ }
    /// `AFPercent_Format`'s clamp: 0..=4 — style 4 (apostrophe) is reachable
    /// here and not from `AFNumber_Format`, which is why these are two
    /// constructors rather than one `TryFrom`.
    pub fn from_percent_format(n: i32) -> SepStyle { /* 0..5 window */ }
}
```

The asymmetry is real and load-bearing: `public_methods_expected.txt` line 406
records `AFPercent_Format(0, 4)` producing `98'765'432'100%`, the apostrophe
separator, which `AFNumber_Format` can never reach.

### 1.5 The functions, grouped by what they need

**Group A — genuinely pure, no event, no host (5).** Write these first; they
are unit-testable against the transcript with nothing else in place.

| Rust | oracle | signature |
|---|---|---|
| `af_simple` | `:1321-1345` | `fn(op: SimpleOp, a: f64, b: f64) -> Result<f64, AfError>` |
| `af_make_number` | `:1347-1364` | `fn(s: &str) -> f64` — infallible past arity; unparseable is `0.0` |
| `af_extract_nums` | `:1523-1557` | `fn(s: &str) -> Option<Vec<String>>` |
| `af_merge_change` | `:1278-1298` + `CalcMergedString` `:127-137` | `fn(ev: &AfEvent) -> String` |
| `print_date_using_format` | `:477-611` | `fn(epoch_ms: f64, picture: &str) -> String` |

`SimpleOp` is `Avg | Sum | Prd | Min | Max`. **Reproduce the case asymmetry:**
`ApplyNamedOperation` (`:215-232`) matches case-**in**sensitively, but the AVG
divisor test (`:1341`, `:1456`) and the PRD seed (`:1390`) use case-**sensitive**
`EqualsASCII("AVG")` / `("PRD")`. So `"avg"` sums and never divides. Model this
by parsing the name into `SimpleOp` *plus* a `matched_exact_case: bool`, and
gate the divisor and the seed on the flag. It is ugly; it is the behaviour, and
STYLE §7 asks for behaviour.

**Group B — pure over `AfEvent`, host reachable only through `AfEffects` (13).**

`af_number_format`, `af_number_keystroke`, `af_percent_format`,
`af_percent_keystroke`, `af_special_format`, `af_special_keystroke`,
`af_special_keystroke_ex`, `af_range_validate`, `af_date_format_ex`,
`af_date_keystroke_ex`, `af_time_format_ex`, `af_time_keystroke_ex`, and the
four preset wrappers `af_date_format`, `af_date_keystroke`, `af_time_format`,
`af_time_keystroke` (which are index lookups over §1.5.2's tables plus a call
into the `_Ex` form).

**Group C — cannot be pure, and is therefore NOT in this module (2).**

- `AFSimple_Calculate` needs the whole document: `CountFields(name)`,
  `GetField`, `GetFieldType`, `GetValue`, `CountControls`, `IsChecked`,
  `GetExportValue`, `CountSelectedItems` — `fxjs/cjs_publicmethods.cpp:1382-1451`.
  **Split it.** The `af/` module exposes the pure half:
  ```rust
  /// One field's contribution to a calculation, already resolved by the caller.
  pub struct AfFieldValue { pub numeric: f64, pub counts: bool }

  pub fn af_simple_calculate_over(op: SimpleOp, exact_case: bool,
                                  values: &[AfFieldValue]) -> f64;
  ```
  and the *name resolution* — walking `/CO`, matching a name to fields,
  extracting a number per field kind — lives in the engine binding (§2), which
  has the document. `counts` is what carries the `nFieldsCount` rule: a field
  that exists but is a push button still increments the AVG divisor with a
  `0.0`, while a name that matches **no** field contributes nothing and does not
  increment. That distinction is why `AFSimple_Calculate('AVG', [1,'nonesuch',
  {'crud':32}])` is `0` in the transcript rather than `NaN`.
- `AFParseDateEx` and every date *parse* seed unspecified fields from the wall
  clock (`FX_ParseDateUsingFormat`, `fxjs/fx_date_helpers.cpp:318, 324-329`) and
  fall back to V8's `Date.parse`. **The `af/` module takes the clock as a
  parameter, never reads it:**
  ```rust
  pub fn parse_date_using_format(value: &str, picture: &str, now_ms: f64)
      -> Result<f64, DateParseFailure>;
  ```
  The `Date.parse` fallback (`ParseDateUsingFormat` stage 2,
  `fxjs/cjs_publicmethods.cpp:451-475`) is **not** this module's: it is the
  engine's, because it is literally the JS engine's date parser. `af/` returns
  `DateParseFailure::BadDate` and the caller decides whether to try
  `boa`'s `Date.parse`. This keeps `af/` free of `boa` — which is the whole
  point of the split — at the cost of one enum variant crossing the boundary.

### 1.5.1 `AFNumber_Format` — the algorithm, since it is the one that shows

Order, from `fxjs/cjs_publicmethods.cpp:618-728`:

1. Arity must be exactly 6 → else `AfError::ParamCount`. Checked by the caller.
2. No event value → the literal error string `"No event handler"` (`:625`) —
   note this is **not** one of the `JSMessage` constants and must be reproduced
   as a literal.
3. Trim ASCII spaces from `ev.value`; if empty, **return success and leave the
   value untouched** (`:630-632`).
4. `decimals = abs(nDec)`; `sep_style`/`neg_style` clamped per §1.4.
5. Replace `,` with `.` (`NormalizeDecimalMark`, `:206-208`), then `atof` — which
   stops at the first invalid character and yields `0.0` for total junk. So
   `"1,234.5"` becomes `"1.234.5"` and parses as `1.234`.
6. **If `decimals > 0`, add `1e-15`** (`kDoubleCorrect`, `:76`) — before the
   absolute value is taken, so a negative number is nudged toward zero.
7. `CalculateString` (`:105-124`): record the sign, take the magnitude, cap
   precision at `f64::DIGITS` (15), and format with fixed-point rounding at
   `min(decimals, 15)` places. `iDec2` is the index of the `.` or, if there is
   none, the whole length.
8. Separators (`:663-678`): if a decimal point exists, swap `.`→`,` for
   styles 2 and 3, and insert a leading `0` when `iDec2 == 0` (i.e. `.5`→`0.5`)
   — **without updating `iDec2`**, which the next step then uses stale. Then,
   for the separator styles, insert the thousands character at `iDec2-3`,
   `iDec2-6`, … while the index stays `> 0`.
9. Currency is attached **before** the sign (`:680-686`), so a parenthesised
   negative reads `($1,234.50)`, not `$(1,234.50)`.
10. Sign, per `neg_style` (`:688-727`):
    - `MinusBlack` — prepend `-`.
    - `Red` — **no sign at all**; the magnitude alone, with the sign carried by
      `AfEffects::text_color = Some(rgb(1,0,0))`.
    - `ParensBlack` — wrap in `( )`.
    - `ParensRed` — wrap in `( )` **and** set the red colour.
    For `Red` and `ParensRed` on a *non-negative* value, the oracle sets black
    — and only when it differs from the field's current colour, which requires
    reading it back. Return `Some(rgb(0,0,0))` unconditionally and let §2 do the
    compare; the compare needs the field and this module cannot see one.

| `sep_style` | thousands | decimal | `1234.5`, 1 dp |
|---|---|---|---|
| `CommaDot` (0) | `,` | `.` | `1,234.5` |
| `NoneDot` (1) | — | `.` | `1234.5` |
| `DotComma` (2) | `.` | `,` | `1.234,5` |
| `NoneComma` (3) | — | `,` | `1234,5` |
| `ApostropheDot` (4) | `'` | `.` | `1'234.5` — **percent only** |

**`AFNumber_Format` and `AFPercent_Format` are compiled out entirely on
Android** (`#if !BUILDFLAG(IS_ANDROID)`, `:618`/`:828`). We do not reproduce a
platform gate; the functions exist on every target. Recorded as a deliberate
divergence, not an oversight.

### 1.5.2 The preset tables — transcribed, because PLAN's count is wrong

`kDateFormats`, `fxjs/cjs_publicmethods.cpp:79-82` — **14 entries, not three.**

`"m/d"`, `"m/d/yy"`, `"mm/dd/yy"`, `"mm/yy"`, `"d-mmm"`, `"d-mmm-yy"`,
`"dd-mmm-yy"`, `"yy-mm-dd"`, `"mmm-yy"`, `"mmmm-yy"`, `"mmm d, yyyy"`,
`"mmmm d, yyyy"`, `"m/d/yy h:MM tt"`, `"m/d/yy HH:MM"`.

`kTimeFormats`, `:84-85` — 4 entries: `"HH:MM"`, `"h:MM tt"`, `"HH:MM:ss"`,
`"h:MM:ss tt"`.

Index selection is `WithinBoundsOrZero`, so **any out-of-range or non-numeric
index silently becomes 0** — no error.

Picture tokens, `PrintDateUsingFormat` (`:477-611`). Runs of the same letter are
counted; anything else is emitted literally.

| token | output | note |
|---|---|---|
| `y` | the literal `y` | a quirk, `:505-507` |
| `yy` | 2-digit year | |
| `yyy` | the literal `yyy` | falls through the 3-run default |
| `yyyy` | 4-digit year | |
| `m` / `mm` | month, unpadded / 2-digit | 1-based |
| `mmm` / `mmmm` | `Jan`… / `January`… | **hard-coded English**, `fx_date_helpers.cpp:185-191` |
| `d` / `dd` | day, unpadded / 2-digit | |
| `ddd` / `dddd` | the literals `ddd` / `dddd` | **not weekday names** |
| `H` / `HH` | hour 0-23, unpadded / 2-digit | |
| `h` / `hh` | `hour > 12 ? hour-12 : hour` | so 0 stays 0 and 12 stays 12 |
| `M` / `MM` | minute | `MM` is minutes, never month |
| `s` / `ss` | second | |
| `t` / `tt` | `a`/`p`, `am`/`pm` | lowercase; `hour > 12` |

The two-digit-year window on *parse* is `0..=99 → +2000` (`fx_date_helpers.cpp:
556`), so `85` is 2085. Validators are loose: day `1..=31` regardless of month,
hour `0..=24`, minute and second `0..=60` (`:232-252`).

### 1.5.3 The mask engine — one function, three callers

`StringPrintx` (`fxjs/cjs_util.cpp:294-381`) is shared by `util.printx` and by
`AFSpecial_Format`. Put it in `af/mask.rs` and let §3's `util` binding call it.

| format char | effect |
|---|---|
| `\` | next format char is literal |
| `<` / `>` / `=` | lowercase / uppercase / preserve case, for what follows |
| `?` | copy one source char unconditionally |
| `X` | copy if ASCII alphanumeric; **advances source either way**, advances format only on a match |
| `A` | as `X`, ASCII alphabetic |
| `9` | as `X`, decimal digit, and **no case translation** |
| `*` | copy the rest of the source; format does not advance until source is exhausted |
| other | emitted literally |

The `X`/`A`/`9` asymmetry — source advances, format does not — is what makes a
mask "wait" for a matching character and skip junk. Reproduce it.

`AFSpecial_Format` masks (`:1113-1148`): `0` → `99999`; `1` → `99999-9999`;
`2` → `(999) 999-9999` if the source yields **≥ 10** digits through the probe
mask `9999999999`, else `999-9999`; `3` → `999-99-9999`. An out-of-range
selector leaves the mask empty and the value becomes `""`.

`AFSpecial_Keystroke` masks are **different** — digits only, no punctuation
(`:1241-1276`): `0` → `99999`; `1` → `999999999`; `2` → `9999999999` if
`value.len + change.len > 7` else `9999999`; `3` → `999999999`. The punctuation
is added later by `_Format`, and the two must not be conflated.

### 1.6 `AfError` — the variants are the oracle's message table

`JSGetStringFromID` (`fxjs/js_resources.cpp:9-95`) is a hard-coded switch with
no localisation layer, and **16 of the 46 golden transcripts pin these strings
verbatim**, so they are API, not diagnostics. The enum carries them as
`#[error("…")]`:

| variant | message |
|---|---|
| `ParamCount` | `Incorrect number of parameters passed to function.` |
| `InvalidInput` | `The input value is invalid.` |
| `ParamTooLong` | `The input value is too long.` |
| `ParseDate(String)` | `The input value can't be parsed as a valid date/time ({0}).` |
| `RangeBetween(String, String)` | `The input value must be greater than or equal to {0} and less than or equal to {1}.` |
| `RangeGreater(String)` | `The input value must be greater than or equal to {0}.` |
| `RangeLess(String)` | `The input value must be less than or equal to {0}.` |
| `NotSupported` | `Operation not supported.` |
| `WrongType` | `Incorrect parameter type.` |
| `WrongValue` | `Incorrect parameter value.` |
| `BadObject` | `Object no longer exists.` |
| `NoEventHandler` | `No event handler` — **a bare literal, `:625`, not in the table, and with no trailing period** |
| `DateKeystrokeArity` | `AFDate_KeystrokeEx's parameter size not correct` — **also a bare literal, `:1011-1012`, also no period** |

The two literals are the ones a tidy port would normalise and must not. They
appear in `public_methods_expected.txt` exactly as written.

**Range comparisons are inclusive-pass**: only `<` and `>` trigger, so a value
equal to a bound passes. The `%ls` placeholders are filled with the *raw string
form of the argument*, not the parsed double — hence `2` and `4`, not `2.0`.

### 1.7 Diagnostics: none

`af/` does **not** take a `&mut Diagnostics`. Every recoverable oddity in this
library is already a return value: a bad style clamps, a bad index clamps, an
unparseable number is `0.0`, an out-of-range date is an `AfError`. There is no
"proceeded past damage" to record that the caller cannot see in the result.
This is the one place the project's damage-tolerance channel is deliberately
absent, and the reason is stated so a reviewer does not add it back.

### 1.8 Test surface for the concurrent agent

`public_methods.in` (650 lines) and `public_methods_expected.txt` (400 lines)
are the entire `AF*` conformance suite in one fixture. The agent can work
against it directly without an engine by writing a small table-driven test that
sets `AfEvent::value`, calls one function, and compares `value`, `rc` and the
alert text — that is exactly what the fixture's own
`expectEventValue(initial, expression, expected)` helper does. Everything in
Group A and Group B is reachable that way. The three `cjs_publicmethods_unittest.cpp`
`IsNumber` cases (`:14-45`) port directly as a 20-row table.

**The `IsNumber` grammar** (`:272-309`) is worth stating because it is odd:
`.` and `,` are the *same* token and **at most one may appear anywhere** (so
`"1,000,000"` is false but `"560,024"` is true); a sign is legal only at index
0; `e`/`E` must be followed immediately by an explicit `+` or `-` (so `"-1e5"`
is false and `"e-5"` is true); and **the empty string and an all-spaces string
are both `true`**.

---

## 2. The `Cascade` implementation

### 2.1 What actually shipped — read this before the design

`crates/pdfrum-form/src/cascade.rs` ships five methods and one implementation:

```rust
pub trait Cascade {
    fn keystroke(&mut self, _f: &FieldRef, change: Keystroke) -> KeystrokeOutcome;
    fn keystroke_commit(&mut self, _f: &FieldRef, _value: &str) -> bool;
    fn validate(&mut self, _f: &FieldRef, _value: &str) -> bool;
    fn calculate(&mut self, _writes: &mut FieldWrites, _trigger: &FieldRef);
    fn format(&mut self, _f: &FieldRef, _value: &str) -> Option<String>;
}
pub struct NoScripts;
impl Cascade for NoScripts {}
```

`NoScripts` does nothing at all: every method takes its default, and the
defaults are the identity cascade — accept the keystroke unchanged, accept the
commit, accept the validation, write no other field, format nothing. The M14
brief's §3.6 and SPEC §15.7 both argue at length that these are the V8-off
behaviour rather than stubs, and the argument is correct: `CJS_RuntimeStub`
(`fxjs/cjs_runtimestub.cpp:33-36`) returns `std::nullopt` from `ExecuteScript`
and registers no objects, so a V8-off build runs the same path with the same
three mutation points inert.

`FieldWrites` carries the recursion budget (`with_max_depth`, `enter`, `leave`,
`can_recurse`), which is the right place for it — the seam does not know about
recursion and `NoScripts` never spends any budget.

The single `&mut dyn Cascade` is in `commit::run`
(`crates/pdfrum-form/src/commit.rs:73`), which runs the normative order
`is_changed → keystroke_commit → validate → save → calculate → format` and
reproduces the load-bearing quirk that a rejected commit still reports success
so focus proceeds (`commit.rs:82-89`, D9).

### 2.2 **The seam as shipped cannot carry the milestone. Three things are missing.**

This is the part the task brief asked to be checked rather than assumed, and the
check fails three times.

**(a) `commit::run` is never called.** `route::apply` — the single event entry
point, and the only thing a `FormSession` user reaches — does not call it.
Grepping the workspace for `commit::run` finds the definition, its own unit
tests, and nothing else; `crates/pdfrum-form/src/lib.rs:45` re-exports only
`CommitOutcome`. What routing actually does on blur is `route::take_focus`
(`route.rs:1607-1637`) and `route::kill_focus` (`:1648-1682`), both of which
call `redraw(session, ctx, field, annot)` and push a `FocusChanged` update.
Neither consults a `Cascade`; neither has one to consult, because
`route::Context` (`route.rs:56-67`) has five fields — `page`, `catalog`,
`resolve`, `fonts`, `permissions` — and none is a cascade.

So the seam is real, correct and *disconnected*. M14's exit criteria did not
notice because `NoScripts` is the identity: a cascade that changes nothing is
indistinguishable from a cascade that is never run. **The moment a non-identity
implementation exists, the difference is every script in the milestone.**

**(b) `Cascade::keystroke` — the per-character hook — has no call site at all,
not even in `commit::run`.** `commit::run` calls `keystroke_commit`, `validate`,
`calculate` and `format`; `keystroke` is called only from
`cascade.rs`'s own tests and `tests/field_actions.rs:141`. That is correct as
far as it goes — the per-character hook belongs on the typing path, not the
commit path — but the typing path is `route::char_typed`, which does not have it
either. Step 3 of PLAN §M15's order is "the full field-event cascade
(keystroke → validate → calculate → format)", and its first arrow currently has
no wire.

**(c) The seam cannot express what a keystroke script actually returns.**
`KeystrokeOutcome::Accept(Keystroke)` hands back the whole payload, and
`Keystroke::applied()` (`cascade.rs:190-215`) splices `change` into `value` at
`[selection_start, selection_end)` — which is exactly `CalcMergedString`
(`fxjs/cjs_publicmethods.cpp:127-137`) and is right. But the oracle's keystroke
handler can also move the caret by writing `event.selStart`/`selEnd`, and
`SetActionData` (`fpdfsdk/formfiller/cffl_textfield.cpp:216-222`) applies
`SetSelection(fa.nSelStart, fa.nSelEnd)` **before** `ReplaceSelection(fa.sChange)`.
`Keystroke` carries `selection_start`/`selection_end` as `u32`, so a script
writing `event.selStart = -1` — which the oracle stores as an `int` and
`CalcMergedString` then treats as an out-of-range `size_t`, yielding an *empty*
prefix rather than a clamp — is not representable. The type is `u32`; upstream's
is `int`; `-1` is a real value there (`CFFL_FieldAction` does not default it to
`-1`, but `event.selStart` is settable to any `i32` via `ToInt32Reentrant`,
`fxjs/cjs_event.cpp:206`).

**(d) The `/CO` calculation order the M14 brief promised does not exist.**
Brief §3.6's closing paragraph says "the `/AcroForm /CO` calculation-order walk
(§1.16) is a `Vec<FieldId>` computed once from `/CO` by `pdfrum-doc` and handed
to `calculate`. M14 computes it and never uses it, so the list is already
correct and tested when M15 arrives." It does not: grepping `crates/` for `CO`,
`calculation_order` and `calc_order` finds nothing in `pdfrum-doc` or
`pdfrum-form`. The promise was made and not kept, and M15 inherits the work.
`CountFieldsInCalculationOrder` / `GetFieldInCalculationOrder`
(`core/fpdfdoc/cpdf_interactiveform.cpp:739-761`) are the oracle, and the rule
they encode matters: **a document with no `/CO` array runs no calculation at
all**, however many fields carry `/AA /C`.

### 2.3 The additive change — smallest thing that closes (a) through (d)

Four changes, all additive, none breaking `NoScripts` or any existing caller.

**A1 — `route::Context` gains a cascade.** One field:

```rust
pub struct Context<'a, R: Resolve> {
    pub page: &'a PageForm,
    pub catalog: &'a Dict,
    pub resolve: &'a R,
    pub fonts: &'a ap::FormFonts,
    pub permissions: Permissions,
    /// The script hooks. `&mut NoScripts` for a build without them.
    pub cascade: &'a mut dyn Cascade,          // NEW
}
```

This is the change that makes the seam load-bearing, and it is the one that
costs something: `Context` becomes `&mut` at every call site that today takes
`&Context`, because a cascade must be able to mutate (a script has state). That
is a mechanical edit across `route.rs`'s ~40 `ctx: &Context<'_, R>` parameters
and the facade's `FormSession`. **This is the single largest piece of
non-`boa` work in M15, and it must be budgeted as such rather than discovered.**

An alternative that avoids the `&mut` churn — pass the cascade as a *second*
parameter to `apply`, `choose`, `kill_focus` and the three other public
entry points, leaving `Context` shared — is genuinely lighter and is offered as
the fallback in escalation E1. It costs a parameter on six public functions
instead of a mutability change on forty internal ones. **Recommendation: the
second parameter.** `Context`'s doc comment says it "owns nothing" and is
"assembled at the call site from things the caller already has", and a `&mut dyn`
in it contradicts that sentence; a cascade is not a borrowed view of the
document, it is a live thing with state, and the type should say so.

**A2 — `commit::run` gets called.** `take_focus` and `kill_focus` call it on the
outgoing field, between `focus::set`/`focus::kill` and `redraw`, and thread its
`CommitOutcome` into the redraw: `stored` becomes the field's new value,
`display` — when `Some` — is what the appearance draws instead (§4.5), and
`writes` are applied to the other fields, each of which then needs its own
redraw. `reverted` restores the stored value and still proceeds, which is D9
and is already the shape `CommitOutcome` was built for.

**A3 — `Cascade::keystroke` gets called from `route::char_typed`**, before the
edit is applied, with `Keystroke::of(edit, ch)`. `KeystrokeOutcome::Reject`
returns `Response::consumed()` with no update — the character is dropped and the
field is unchanged, which is `RecreatePWLWindowFromSavedState`
(`fpdfsdk/formfiller/cffl_interactiveformfiller.cpp:1052`) restated as "do
nothing". `Accept(k)` applies `k.applied()` rather than the raw character.

**A4 — `Keystroke`'s selection indices become `i32`.** Two fields, `u32 → i32`,
and `applied()` gains the out-of-range rule: an index that is negative or past
the end yields an **empty** slice rather than a clamp, per
`StringViewTemplate::Substr` (`core/fxcrt/string_view_template.h:226-245`). The
current `.min(len)` clamp is the reasonable behaviour and the wrong one.

**A5 — the `/CO` order, in `pdfrum-doc`.** `form::calculation_order(catalog,
r) -> Vec<FieldId>`, reading `/AcroForm /CO` as an array of field-dictionary
references and mapping each to a field, dropping entries that resolve to
nothing. Empty when `/CO` is absent — and the emptiness must be *the* answer,
not a fallback to "all fields", because that difference is visible on every file
that has `/AA /C` without `/CO`.

### 2.4 Where the `boa` implementation lives, and the argument

**Recommendation: a new module `crates/pdfrum-form/src/script/`, behind a
default-off cargo feature `script`, in the `pdfrum-form` crate — not a new
crate.**

The M14 brief argued its boundary in three descending reasons and stated the
counter-argument fairly; the same discipline applies here, and it lands the
other way.

**The dependency argument, checked first because it is the one that usually
decides.** What does the engine need?

| need | where it is |
|---|---|
| the `Cascade` trait to implement | `pdfrum-form::cascade` |
| `FieldRef`, `Keystroke`, `KeystrokeOutcome`, `FieldWrites` | `pdfrum-form::cascade` |
| field values, kinds, options, flags | `pdfrum-doc::form` — already a `pdfrum-form` dependency |
| `/AA` triggers and `/JS` payloads | `pdfrum-doc::nav::{AActionType, Action, additional_action}` — **already exist and are `pub`** |
| the `/CO` order | `pdfrum-doc`, per A5 |
| the `AF*` library | `pdfrum-form::af`, per §1 |
| a JS engine | `boa_engine` — new |

Everything except `boa_engine` is already in `pdfrum-form`'s tree. Unlike M14 —
where the dependency argument was honestly neutral and cohesion decided it —
here the dependency argument points one way and is not neutral: **a separate
`pdfrum-script` crate would have to depend on `pdfrum-form` to implement
`Cascade`, and `pdfrum-form` would have to depend on `pdfrum-script` to
construct one, or the facade would have to wire them and every consumer would
have to know both names.** That is a back-edge or a burden, and neither is worth
paying.

**The cohesion argument, which cut the other way in M14, cuts this way here.**
M14's case for a new crate was that `pdfrum-doc` was already 48 files and 15
re-exports and a second complete subsystem would not fit behind one door. The
engine is not a second subsystem behind `pdfrum-form`'s door; it is *the second
implementation of a trait that crate already owns*, which is the textbook reason
to keep it next to the first. `pdfrum-form`'s `lib.rs` gains **one** name,
`ScriptCascade`, and only under the feature.

**The feature flag is what keeps the cost off everyone else.** With `script`
off, `boa_engine` and its 132 transitive crates are not in the tree at all, and
`cargo add pdfrum` puts no JS engine in anyone's dependency graph — the same
property `scripts/check-no-wgpu.nu` enforces for the GPU backend, and it should
be enforced the same way, mechanically (§6.5).

**The counter-argument, stated fairly.** A `pdfrum-script` crate would make the
`boa` dependency impossible to reach accidentally, where a feature flag can be
enabled by any crate in a workspace via feature unification. That is a real
weakness of features and it is why the check in §6.5 is not optional. But the
back-edge is worse: a trait's second implementation belongs with its first, and
a crate that exists only to break a cycle it created is a worse artefact than a
feature flag with a CI assertion.

**Public surface — three names, and the third is the interesting one:**

```rust
/// A `boa`-backed `Cascade`: the scripts a document's `/AA` entries carry.
pub struct ScriptCascade { /* private */ }

/// What a scripting session is allowed to do and how far it may go.
#[derive(Debug, Clone)]
pub struct ScriptConfig {
    pub limits: pdfrum_common::Limits,
    /// The clock scripts see, in milliseconds since the epoch. `None` reads
    /// the host clock; `Some` freezes it, which is what a golden run needs.
    pub clock_ms: Option<i64>,
    /// The local timezone offset in seconds. Defaults to 0 (UTC).
    pub timezone_offset_secs: i32,
}

impl ScriptCascade {
    /// Builds a session over one document's form.
    pub fn new(form: &Form, catalog: &Dict, r: &impl Resolve,
               config: ScriptConfig, diags: &mut Diagnostics) -> ScriptCascade;
    /// The document-open action: `/OpenAction` and the `/Names /JavaScript`
    /// tree, in that order. Step 2 of PLAN §M15's order.
    pub fn run_document_open(&mut self, diags: &mut Diagnostics);
    /// Everything `app.alert` and friends emitted, in order — the transcript
    /// the 46 goldens are scored against.
    pub fn transcript(&self) -> &[TranscriptLine];
}
impl Cascade for ScriptCascade { /* the five methods */ }
```

**`transcript()` is a value getter, not a callback, and that is STYLE §2b's
2026-09-01 clause applied.** The clause says: invert — take a trait — only when
the library must ask a question it cannot answer and cannot proceed until it
hears back; do *not* invert to let a host draw or display something the library
merely knows about. An alert is exactly the second case. PDFium's
`IPDF_JSPLATFORM::app_alert` is a host callback because C++ had no better
option; here the alerts are a list the session accumulates and the host reads on
its own schedule, the same way `PopupView`/`ScrollView` replaced the
`FormChrome` trait M14 declined. **A fourth trait seam is not needed for M15.**
`Cascade` remains the third and last.

`TranscriptLine` is an enum, because the transcript is not only alerts — §6.1's
taxonomy has ten shapes and every one is `pub` data:

```rust
pub enum TranscriptLine {
    Alert { title: String, message: String, icon: u8, button: u8 },
    Beep(i32),
    Response { question: String, /* … */ },
    MailMsg { /* … */ },
    Print { /* … */ },
    SubmitForm { url: String, data: Vec<u8> },
    GotoPage(i32),
    NamedAction(String),
}
```

### 2.5 How each method gets its context

`ScriptCascade` holds the document context once, at construction, so the five
trait methods — whose signatures cannot change — need nothing extra:

| method | what it needs | how it gets it |
|---|---|---|
| `keystroke` | the field's `/AA /K` script, `willCommit = false` | `self.actions[field.index]`, built at `new` |
| `keystroke_commit` | the same script, `willCommit = true` | same, different event setup |
| `validate` | the field's `/AA /V` | same |
| `calculate` | **the `/CO` order and every field's value** | `self.order: Vec<FieldId>` and `self.values`, both snapshotted at `new` and updated as writes land |
| `format` | the field's `/AA /F` | same |

`FieldRef` carries `{ name, index }` and nothing else — deliberately, per its
own doc comment: "nothing here can be used to reach back into the session". The
index is the key into `ScriptCascade`'s own tables, which is exactly enough.

**`calculate` is the one that needs care**, because `FieldWrites` is the only
channel out and the oracle's `OnCalculate` sweeps the whole `/CO` list writing
each field in turn (`fpdfsdk/cpdfsdk_interactiveform.cpp:270-310`). The mapping:
one call to `Cascade::calculate` runs the entire sweep, pushing one
`writes.set(field_index, value)` per field whose script changed its value.
The `busy_` re-entrancy guard (`:259-264`) — which makes the outer sweep
authoritative and turns every nested call into a no-op — maps onto
`FieldWrites::enter`/`leave`, and the budget it enforces should be **1**, not a
depth: upstream permits no nesting at all. `SPEC §10`'s `Limits` gains
`max_calculate_depth` (§7, E4) so the value is configurable rather than a
constant, but the default is the oracle's, which is one level.

The three-way gate at `:307` is normative and must be reproduced: a calculated
value is written **only if** the script did not throw, **and** `event.rc` is
still truthy, **and** the string actually changed.

---

## 3. The object-model inventory

### 3.1 What the goldens actually reach — measured, not assumed

`testing/resources/javascript/` holds **47 `.in` fixtures and 46
`_expected.txt` transcripts**. (PLAN §M15's "46 goldens" is right about goldens;
the 47th fixture, `bug_1445426.in`, has no `_expected.txt` and its assertion is
that stdout is **byte-empty** — `_VerifyEmptyText`, `testing/tools/test_runner.py`.
That is a real assertion, not a gap, and it is scored.)

Coverage by object, counted across the 47:

| object / function | fixtures | notes |
|---|---|---|
| `app.alert` | **42** | the transcript itself; nothing else matters as much |
| `app.*` (any) | 44 | |
| `this.*` (Doc) | 22 | `Document` is the V8 **global**, so `this.getField` and bare `getField` are the same call |
| `Field` (any) | 19 | |
| `this.getField` | 12 | |
| `.value` | 7 | |
| `event.*` | 2 | `event_properties`, `public_methods` |
| `color.*` | 2 | |
| `util.byteToChar` | 2 | |
| `util.printf` / `printd` / `printx` / `scand` | 1 each | one fixture apiece, each exhaustive |
| `global.*` | 1 | |
| `console.*` | 1 | |
| any `AF*` | 3 | but `public_methods.in` alone is 650 lines and covers all 22 |
| `app.beep` / `response` / `launchURL` / `setTimeOut` | 1 each | |
| `app.setInterval` | **0** | **no fixture reaches it** |

Twenty of the 47 need no document at all; 19 need a real AcroForm, four of which
share one `field.fragment` skeleton, so one good form fixture unlocks a cluster.

### 3.2 The tiers

**Tier 1 — implement in M15 (the transcript depends on it).**

| object | oracle | why |
|---|---|---|
| `app.alert` | `fxjs/cjs_app.cpp:224-279` | 42 of 47 fixtures. See §3.3 for the argument form and the conditional decoration. |
| `app` properties | `:38-52` | `viewerType` `"pdfium"`, `viewerVariation` `"Full"`, `platform` `"WIN"`, `language` `"ENU"`, `viewerVersion` 8, `formsVersion` 7, `calculate` r/w, `runtimeHighlight` r/w; the rest **error with `Operation not supported.`**, and the errors are asserted |
| `Document` (as the global) | `:39-117` | 22 fixtures; `getField`, `getNthFieldName`, `numFields`, `numPages`, `pageNum`, `info`, the metadata properties, `calculateNow`, `resetForm`, `getAnnot(s)`, `gotoNamedDest` |
| `Field` | `cjs_field.cpp:548-637` | 19 fixtures; `value`/`valueAsString` are the load-bearing pair, plus the ~50 property getters `field_properties.in` walks |
| `event` | `cjs_event.cpp:14-34` | §4 |
| `util` | `cjs_util.cpp:77-82` | five methods, four exhaustive fixtures |
| `color` | `cjs_color.cpp:20-35` | 12 constants + `convert` + `equal` |
| `global` | `cjs_global.cpp` | one fixture, but it needs the interceptor semantics (§3.4) |
| `console` | `cjs_console.cpp:14-17` | four methods, **all four are no-ops upstream**, including `println` which discards its output — so this is four empty functions and one fixture passes |
| the 9 constant namespaces | `border`, `display`, `font`, `highlight`, `position`, `scaleHow`, `scaleWhen`, `style`, `zoomtype` | `consts.in`; pure tables, ~60 constants total, an afternoon |
| the 22 `AF*` globals | §1 | |
| `IDS_*` string consts + `RE_*` regex arrays | `cjs_globalconsts.cpp:19-48`, `cjs_globalarrays.cpp:42-90` | 23 bare globals; tables |
| `Annot` | `cjs_annot.cpp:15-18` | `annot_properties.in`, `bug_421304870.in`; three properties |
| `Icon` | `cjs_icon.cpp:9-10` | one read-only `name`; `icons.in` |

**Tier 2 — stub with a diagnostic (present, inert, and *saying* so).**

The oracle itself stubs most of these, and reproducing "returns success and does
nothing" is *correct behaviour*, not a shortcut — a script that calls
`app.browseForDoc` must not throw. The divergence is that we record a
`Diagnostic` where the oracle records nothing, so a caller can find out.

| name | oracle behaviour | our behaviour |
|---|---|---|
| `app.browseForDoc`, `execDialog`, `findComponent`, `goBack`, `goForward`, `launchURL`, `newFDF`, `openFDF` | **no-op, returns success** (`cjs_app.cpp:532, 608, 296, 443, 449, 502, 207, 219`) | same + `Diagnostic` |
| `Doc.addAnnot`, `addField`, `addLink`, `closeDoc`, `createDataObject`, `deletePages`, `exportAs*`, `extractPages`, `getLinks`, `getOCGs`, `getPageBox`, `getURL`, `importAn*`, `importTextData`, `insertPages`, `removeIcon`, `replacePages`, `saveAs`, `syncAnnotScan` | no-op success | same + `Diagnostic` |
| `Doc.removeField` | **does not remove the field** — inflates each widget rect by 1pt and sets the change mark (`cjs_document.cpp:503-551`) | reproduce, because two fixtures depend on the field *surviving* |
| `Doc.filesize` | **hardcoded 0** (`:932`) | same |
| `Field.defaultIsChecked` | returns `IsCheckBoxOrRadioButton`, **ignoring the default state** (`:2562-2585`) | reproduce; `isDefaultChecked` is the correct one and is separate |
| `Field.buttonGetIcon` | returns a **blank** `Icon` with no name (`:2463-2496`) | reproduce |
| ~30 `Field` setters | validate, then **do nothing** (`alignment`, `charLimit`, `comb`, `multiline`, `fillColor`, `textColor`, `textFont`, `textSize`, …) | reproduce — the validation errors *are* asserted by `field_properties.in` |
| `event.richChange`, `richChangeEx`, `richValue` | read `undefined`, writes ignored (`cjs_event.cpp:145-170`) | same |
| the timer objects | `app.setTimeOut`/`setInterval`/`clearTimeOut`/`clearInterval` + `TimerObj` | §3.5 |

**Tier 3 — decline, with the reason.**

| name | oracle | why declined |
|---|---|---|
| `app.execMenuItem`, `newDoc`, `openDoc`, `popUpMenu`, `popUpMenuEx` | **already error** with `Operation not supported.` | the oracle declines them; we return the same error, which is not a stub but the specified behaviour |
| `Doc.print`, `Doc.mailDoc`, `Doc.mailForm`, `Doc.submitForm` | **implemented** upstream, reaching `IPDF_JSPLATFORM::app_*` | §5.3 rules on each; they become `TranscriptLine` entries, not host actions |
| `Doc.getPrintParams`, `getPageNthWordQuads` | error `Operation not supported.` | same as row 1 |
| `Field.signature*` (6 methods), `setLock`, `getLock` | error `Operation not supported.` | same |
| `ADBC`, `Directory`, `Net`, `dbg`, `security` | **do not exist** | `unsupported.in` asserts they do not; declining is the assertion |
| `Doc.getAnnot3D`, `getAnnots3D`, `Collab`, `layout`, `media`, `mouseX`, `mouseY`, `zoom`, `zoomType`, `pageWindowRect`, `bookmarkRoot` | stubs returning `undefined` | Tier 2 in effect; listed here because `document_properties.in` asserts the `undefined` |
| XFA | `fxjs/xfa/` | PLAN's standing decline; three fixtures (`bug_679642`, `bug_735912`, and the 53 in `xfa_specific/`) are suppressed for it |
| `global` **persistence** | `cfx_globaldata.cpp` | **the oracle never persists.** `CJS_Global`'s constructor passes a hard-coded `nullptr` delegate (`cjs_global.cpp:178-182`), and both `LoadGlobalPersistentVariables` and `SaveGlobalPersisitentVariables` return `false` immediately on a null delegate. `CommitGlobalPersisitentVariables` is annotated dead code. So `setPersistent` sets a flag nothing reads, and reproducing the flag without the file is exact. **Do not implement the RC4-encrypted 4088-byte store**; it is unreachable, and it would be the only file-writing code in the library. |

### 3.3 `app.alert` — the one function the milestone is scored on

Argument handling is `ExpandKeywordParams(params, 4, "cMsg", "nIcon", "nType",
"cTitle")` (`fxjs/js_define.cpp:65-98`): positional, **or** — if and only if
there is exactly one argument, it is an object, and it is not an array — the
four named properties are read off it. A property that is `undefined` stays
"unknown" and takes its default.

- missing `cMsg` → `Incorrect number of parameters passed to function.`
- **an array `cMsg` becomes `"[" + join(", ", elements) + "]"`** (`:239-250`)
- `nIcon` defaults 0 (Error), `nType` defaults 0 (OK), `cTitle` defaults the
  literal `"Alert"`
- returns 1/2/3/4 for OK/Cancel/No/Yes; **with no form-fill environment it
  returns 0 without erroring** (`:233-236`)
- it calls `KillFocusAnnot` before alerting (`:274`) — a side effect, and one
  that matters because it can commit a field

**The transcript decoration is conditional and must be reproduced exactly.** A
default alert prints `Alert: <msg>`. A non-default title prints
`<title>: <msg>`. A non-default icon or type prints
`<title>[icon=N,type=N]: <msg>`. The `AF*` library's own alerts always take the
third form with `icon=3,type=0`, which is why `public_methods_expected.txt`
carries lines like
`AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.` **unprefixed by
`Alert:`** and interleaved with the `Alert:` lines. Getting the interleaving
right is a scoring requirement, not a nicety.

### 3.4 `global` — an interceptor, not a property bag

`CJS_Global` installs five V8 named-property interceptors (`cjs_global.cpp:158-163`)
over a `map<ByteString, JSGlobalData>`. Three rules that a naive port gets wrong:

- **Deletion is a tombstone, not an erase** (`:199`): `bDeleted = true`. A
  tombstoned key reads `undefined`, is skipped by enumeration, and makes
  `setPersistent` fail with `Global value not found.`
- **Assigning `undefined` deletes** (`:231-263`).
- Strings are stored as `ByteString` — **UTF-8-lossy** — and objects are held
  live but never persisted.

### 3.5 Timers — Tier 2, with the shape named

`app.setTimeOut(script, ms)` and `setInterval(script, ms)` return a `TimerObj`,
an opaque handle carrying only an integer id (`cjs_timerobj.cpp:20-23`: no
properties, no methods). The machinery is a process-wide
`map<int32_t, GlobalTimer*>` (`fxjs/global_timer.cpp:18-19`) — **a global
registry, which STYLE §1 forbids outright** ("No global state. None.").

Three facts make the decline easy:

1. **No fixture calls `app.setInterval`**, and the one that touches
   `setTimeOut` (`constructor.in`) only asks whether the constructor is
   callable.
2. `RunJsScript` bails entirely if the runtime is blocking
   (`cjs_app.cpp:433-441`), so a timer inside an alert never fires.
3. A one-shot with `ms == 0` **never runs its script**, because `TimerProc`
   (`:418-423`) gates on `!IsOneShot() || GetTimeOut() > 0`.

**Ruling: `setTimeOut`/`setInterval` return a real `TimerObj` and record the
script and interval; nothing fires it in M15.** M14's D14 already reserved
`advance_time` as the step function where timers land, and the M14 brief says it
"takes the same `&mut dyn Cascade`". That is where a future milestone fires
them. The registry is per-`ScriptCascade`, never global.

---

## 4. The `event` object's lifecycle

### 4.1 The shape, corrected

There is no `CJS_EventRecorder`. `CJS_EventContext`
(`fxjs/cjs_event_context.{h,cpp}`) *is* both the recorder and the runner: one
class that records the fields, runs the script, and is destroyed. The lifecycle
is a stack on the runtime — `NewEventContext` pushes
(`cjs_runtime.cpp:131-134`), `ReleaseEventContext` pops with a
`DCHECK_EQ` that it is the same one (`:136-139`), and every `event.*` getter
calls `GetCurrentEventContext()` to reach the top. `ScopedEventContext`
(`ijs_runtime.h:34-47`) is the RAII wrapper. Nesting is legal and stacks.

### 4.2 The dummy-fallback pattern — the design's load-bearing trick

Four fields are **pointers aliased into the caller's stack**: `change_`,
`sel_start_`, `sel_end_`, `pb_rc_`. When a setup method does not install a
pointer, the accessor silently redirects to a per-context throwaway
(`change_du_`, `sel_start_du_`, `sel_end_du_`, `rc_du_`). So a script writing
`event.change` during a Format event **succeeds, is readable back inside the
script, and is discarded on destruction**.

`value_` is the exception: **no dummy**. `HasValue()` is `!!value_`, and both
the getter and the setter hard-fail with `Object no longer exists.` when it is
null (`cjs_event.cpp:271-273, 284-286`).

This maps cleanly onto Rust and should be kept rather than tidied: `AfEvent`
(§1.3) is the recorded state, and **which fields are copied back after the
script is a property of the event kind**, per §4.4's table. Writes to a field
that is dead for the kind land in the struct and are dropped — the same
observable behaviour, without any pointer aliasing.

### 4.3 Every property, and its gate

Twenty properties (`cjs_event.cpp:14-34`), all with a getter and a setter; a
read-only property is one whose setter returns `Operation not supported.`

| property | R | W | gate / meaning |
|---|---|---|---|
| `change` | ✔ | ✔ | the setter is guarded by `IsString()` (`:64`) — a non-string assignment is **silently dropped without throwing** |
| `changeEx` | ✔ | ✘ | a choice field's export value; cleared to `""` when the field is full |
| `commitKey` | ✔ | ✘ | **`0` for Keystroke and Format, `-1` for everything else.** No code path ever assigns a real key code — Acrobat's 0/1/2/3 semantics are simply not implemented |
| `fieldFull` | ✔ | ✘ | **throws `unrecognized event`** — a bare literal, `:94-96` — unless `Name() == "Keystroke"` |
| `keyDown`, `modifier`, `shift` | ✔ | ✘ | |
| `name` | ✔ | ✘ | `"Keystroke"`, `"Validate"`, `"Calculate"`, `"Format"`, `"Mouse Down"` (with a space), … (`:331-378`) |
| `rc` | ✔ | ✔ | **`ToBooleanReentrant` — JS truthiness, not a type check.** `event.rc = 'boo'` is `true` |
| `richChange`, `richChangeEx`, `richValue` | ✔ | ✔ | complete no-op stubs; read `undefined`, writes ignored |
| `selStart`, `selEnd` | ✔ | ✔ | gated on `Name() == "Keystroke"`; **outside it the getter returns `undefined` and the setter silently does nothing — neither throws**, unlike `fieldFull` |
| `source`, `target` | ✔ | ✘ | fresh `Field` objects; **`source_name_` is set only by Calculate**, so everywhere else `event.source` is a field attached to the empty name |
| `targetName` | ✔ | ✘ | the **fully-qualified** name; for a document-open event it is the JS name-tree key instead |
| `type` | ✔ | ✘ | `"Field"`, `"Doc"`, `"Page"`, `"External"` |
| `value` | ✔ | ✔ | requires `Type() == "Field"` **and** a bound value; **numbers are accepted and stringified, booleans/null/undefined throw `Set not possible, invalid or unknown.`** — note there is no `IsString()` guard here, unlike `change` |
| `willCommit` | ✔ | ✘ | §4.4 |

The three gates behave differently on failure — `fieldFull` throws, `selStart`
returns `undefined`, `value` throws a *different* message — and
`event_properties_expected.txt` pins all three.

### 4.4 Which fields are live, per event kind

Every setup method calls `Initialize(kind)` first, which resets everything
(`cjs_event_context.cpp:289-310`) — note `commit_key_` resets to **`-1`**, not 0.

**P** = pointer aliased to the caller (writes propagate); **V** = set by value
(read-only in effect); blank = the `Initialize` default.

| kind | targetName | source | value | change | commitKey | sel | willCommit | fieldFull | rc |
|---|---|---|---|---|---|---|---|---|---|
| Doc\* / Page\* / External | — | — | — | — | −1 | — | — | — | — |
| MouseEnter/Exit/Down/Up | V | — | — | — | −1 | — | — | — | — |
| Focus / Blur | V | — | **P** | — | −1 | — | — | — | — |
| **Keystroke** | V | — | **P** | **P** | **0** | **P,P** | V | V | **P** |
| **Validate** | V | — | **P** | **P** | −1 | — | — | — | **P** |
| **Calculate** | V | V | **P** | — | −1 | — | — | — | **P** |
| **Format** | V | — | **P** | — | **0** | — | **true** | — | — |

Read off it:

- **Format hard-codes `willCommit = true`** (`:282`) — which is why
  `event_properties.in`, a Format handler, reads `true`.
- **Format leaves `rc` unbound**, so a Format script's `event.rc` writes go to
  the dummy and nothing reads them. `OnFormat` has no `bRc` at all — only "did
  the script throw" (`cpdfsdk_interactiveform.cpp:338-341`).
- **Only Keystroke sets `fieldFull` and the selection.**
- **`event.source` is meaningful only in Calculate.**
- Keystroke's `willCommit` is the caller's: `true` on the commit path
  (`cffl_interactiveformfiller.cpp:752`) and `false` per character (`:1020`).
  **The two `OnKeyStrokeCommit` implementations disagree**: the `CFFL_` one sets
  `true`, the `CPDFSDK_InteractiveForm` one leaves the `CFFL_FieldAction`
  default, which is `false`. Reproduce the asymmetry or record a divergence;
  do not average them.

### 4.5 **The crux — how a script mutating `event.value` reaches the field**

Three different answers, and the difference is the design.

**Format — the write reaches the *appearance only*, never `/V`.**
`OnFormat` (`fpdfsdk/cpdfsdk_interactiveform.cpp:313-346`) binds `value_` to a
**local** `WideString sValue`, runs the script, and at `:340` returns
`sValue` — the mutated string — as a `std::optional<WideString>`. That optional
is threaded into `ResetFieldAppearance(pField, formatted)` (`:588`), then
`CPDFSDK_Widget::ResetAppearance`, and lands at
`fpdfsdk/cpdfsdk_appstream.cpp:1752`:

```cpp
pEdit->SetText(sValue.value_or(pField->GetValue()));
```

**That one line is the whole mechanism.** The formatted value substitutes for
`/V` when synthesising the appearance stream and is *never persisted*.
Re-running with `std::nullopt` reverts the appearance to the raw value. Two
strings, one field.

This is exactly what `Cascade::format`'s `Option<String>` already models, and
what `CommitOutcome::display` already carries — "a display string a formatting
hook produced, which does not change the stored value". **The M14 seam got this
one right and needs no change.** What is missing is only that nothing calls it
(§2.2 a) and nothing draws it: `display` must reach the appearance generator as
the text to lay out instead of the stored value, which is a `LiveInput`-adjacent
change in `pdfrum-doc::ap::widget` that M15 must budget.

**Calculate — the write reaches `/V`, behind a three-way gate.**
`OnCalculate` (`:300-310`) binds `value_` to a local seeded from
`pField->GetValue()`, runs the script, and writes back only if
`!err.has_value() && bRC && sValue != sOldValue` — no exception, `rc` still
truthy, and the value actually changed — with `NotificationOption::kNotify`,
which re-enters the notifier chain and is what the `busy_` guard exists to stop.

**Keystroke and Validate — the write is DISCARDED.** `value_` aliases
`CFFL_FieldAction::sValue`, and **no code anywhere reads it back**. The only
post-script reads are `fa.bRC` (six sites) and, on the non-commit keystroke path
only, `fa.nSelStart`, `fa.nSelEnd` and `fa.sChange` via `SetActionData`. So a
Keystroke script influences the field through **`rc`** (accept/reject) and
**`change` + the selection** (rewrite the pending insertion), and assigning
`event.value` compiles, does not throw, mutates the recorded string, and is
thrown away.

`Cascade::keystroke` returning `KeystrokeOutcome::Accept(Keystroke)` — with
`change` and the two indices, and `applied()` splicing them — is precisely this
contract, modulo the `i32` fix (§2.3 A4). `keystroke_commit` and `validate`
returning `bool` is precisely `bRC`. **The seam's shape is right; its wiring is
absent.**

### 4.6 The commit pipeline, and the guards

`CFFL_FormField::CommitData` (`fpdfsdk/formfiller/cffl_formfield.cpp:507-552`)
is the canonical order, with an `ObservedPtr` liveness re-check after **every**
step because a script can delete the widget:

```
IsDataChanged → OnKeyStrokeCommit → OnValidate → SaveData(/V) → OnCalculate → OnFormat
```

A refusal at either of the first two calls `ResetPWLWindow` and **returns
`true`** — the commit "succeeded", the edit is silently reverted, and focus
proceeds. That is D9, already reproduced in `commit.rs`.

`commit::run`'s order is
`is_changed → keystroke_commit → validate → save → calculate → format`, which is
the same sequence. **It matches. No change needed.**

Four independent recursion guards exist upstream and each needs a home:

| guard | oracle | ours |
|---|---|---|
| `CPDFSDK_InteractiveForm::busy_` — the whole calculate sweep is non-reentrant | `cpdfsdk_interactiveform.cpp:259-264` | `FieldWrites`' depth budget, default 1 |
| `CJS_EventContext::busy_` — per-context re-entry → `System is busy.` | `cjs_event_context.cpp:32-38` | a flag on `ScriptCascade` |
| `CJS_Runtime::field_event_set_` — a `(targetName, kind)` set; a duplicate → `Duplicate formfield event found.` | `:41-45` | a `HashSet<(u32, EventKind)>` on `ScriptCascade` |
| the action-tree `visited` set — a set of already-executed action **dictionaries**, no depth limit; a cycle terminates the walk and aborts every ancestor | `cpdfsdk_formfillenvironment.cpp:994-998` | `nav::Action::chain` already has a `seen` set and a depth cap |
| `CFFL_InteractiveFormFiller::notifying_` — stops JS-triggered UI changes recursively firing more field actions | `cffl_interactiveformfiller.h:220` | a flag on `ScriptCascade` |

**All five are needed.** Missing any one of them turns a hostile file into a
hang, and §5 makes non-hanging a tested property.

---

## 5. The sandbox, as a testable property

PLAN §M15 says the sandbox is "a *stronger* property than the C++ has; say so in
the brief." It is, and here is the measurement — but the plan's framing needs
one correction, and it is the most important finding in this section.

### 5.1 What `boa` gives, and what it does not

`boa_engine 0.22.0`'s `RuntimeLimits` (`src/vm/runtime_limits.rs`) has exactly
four knobs:

| boa field | default | our `Limits` field |
|---|---|---|
| `loop_iteration` | `u64::MAX` (unlimited) | `max_script_loop_iterations` |
| `recursion` | 512 | `max_script_recursion` |
| `stack_size` | 10 240 | `max_script_stack` |
| `backtrace_limit` | 50 | *(not exposed; a diagnostic detail)* |

**Measured on this machine, `boa_engine 0.22.0`, release build:**

| script | result |
|---|---|
| `while(true){}` | **`RuntimeLimitError: reached the maximum number of iteration loops` in 9.5 ms** |
| `function f(){return f();} f();` | **`RuntimeLimitError: reached the maximum number of recursive calls` in 0.31 ms** |
| the same with `stack_size = 2048` | same error, 0.27 ms |
| `var a=[]; for(i<100000)a.push(i)` | completes, 73 ms |
| **`var s='x'; for(var i=0;i<30;i++) s=s+s;`** | **completes — 1 GiB string, 1.1 s, 30 loop iterations** |
| **`/(a+)+$/.test('a'.repeat(28)+'b')`** | **completes — 14.7 s, and ~4x per added character** |

The first three are the property PLAN promises and it holds: a script that
exhausts a limit produces a catchable error in milliseconds, never a hang and
never a panic.

**The last two are holes, and PLAN §M15's sandbox paragraph does not cover
them.** No `RuntimeLimits` field bounds *memory* or *regex backtracking*.
`s = s + s` reaches a gigabyte in thirty iterations, so a loop limit of a
million never fires; and a catastrophically-backtracking regex is pure
computation inside one builtin, so no loop or recursion counter sees it. Both
are trivially reachable from a `/AA` script in an untrusted file, and both are
exactly the shape M14's tab-order finding took — "a library that can hang on
input is a bug regardless of what the oracle does" (SPEC §15.9). **This is
escalation E2.**

### 5.2 The `Limits` fields, and what exhausting one produces

SPEC §10's `Limits` gains four fields (E4), all additive per §10's own note that
`Limits` is deliberately not `#[non_exhaustive]`:

| field | default | rationale |
|---|---|---|
| `max_script_loop_iterations: u64` | 10_000_000 | ~10 s of the tightest loop; no real form script iterates a thousand times |
| `max_script_recursion: usize` | 512 | boa's own default; upstream has no equivalent |
| `max_script_stack: usize` | 10_240 | boa's own default |
| `max_calculate_depth: u32` | 1 | upstream's `busy_` permits **no** nesting; the field makes it configurable, not looser |

Exhausting any of them is a **`Diagnostic`**, never an `Err` and never a panic:
the script stops, the cascade method returns its *refusing* answer — `Reject`
for `keystroke`, `false` for `keystroke_commit` and `validate`, no writes for
`calculate`, `None` for `format` — and the diagnostic records which limit and
which field. The refusing answer rather than the permissive one is deliberate: a
script that ran out of budget did not say "accept", and inventing an acceptance
on its behalf is the failure mode that lets a hostile file bypass a validator.

### 5.3 What the DOM must not expose — and the ruling on each

The three the task named, plus the four the inventory found:

| oracle entry point | what it does upstream | ruling |
|---|---|---|
| **`app.launchURL`** | `cjs_app.cpp:502-506` — the body is literally a comment and `return Success()`. **It does not even parse its arguments.** | **Nothing to decline.** Reproduce the no-op; it already is one. Record a `Diagnostic`. |
| **`Doc.mailDoc` / `Doc.mailForm` / `app.mailMsg`** | genuinely implemented; reach `IPDF_JSPLATFORM::JS_docmailForm` (`cjs_document.cpp:334-438`, `cjs_app.cpp:455-500`). `mailForm` additionally exports FDF. | **Emit a `TranscriptLine::MailMsg`; send nothing.** 14 golden lines depend on the transcript text, so this is not a decline — it is the scored behaviour. No network, no MAPI, no process spawn. |
| **`Doc.print`** | implemented, requires `IsUserGesture()`, reaches `JS_docprint` (`cjs_document.cpp:440-498`) | **Emit `TranscriptLine::Print`; print nothing.** 4 golden lines. Reproduce the user-gesture check, because the error is asserted. |
| `Doc.submitForm` | implemented; **posts form data to a URL** (`:617-693`) | **Emit `TranscriptLine::SubmitForm { url, data }`; make no request.** 2 golden lines carry the URL and a hex dump. This is the single most dangerous entry point in the object model and the one where "return the data, do not act on it" matters most. |
| `Doc.saveAs`, `exportAsFDF`, `getURL`, `importAnFDF` | already no-ops, each with an "Unsafe, not supported" comment | reproduce |
| `app.response` | prompts the user (`cjs_app.cpp:558-598`) | **Return the empty string**, emit `TranscriptLine::Response`. 2 golden lines. |
| `global` persistence | never reached (§3.2 Tier 3) | **no file is written, ever.** This is the only filesystem path in `fxjs/` and it is dead upstream. |
| `Date`, `Math.random` | boa provides both | **`Date` stays, `Math.random` stays.** `util.printd`/`scand` need `Date`, and the frozen-clock recipe (§5.4) makes it deterministic. `Math.random` is not a capability — it reaches nothing. |
| `eval`, `Function` | boa provides both | **stay.** `expect.js` — used by 7 fixtures — is built on `eval`, and `constructor.in` tests `Function`. Removing them fails goldens for no security gain: they compile source that is already in the file. |
| `gc`, `WebAssembly` | V8-only | **verified absent**: `typeof gc` and `typeof WebAssembly` are both `"undefined"` in boa, which is what `v8_features_expected.txt` asserts. Free. |
| any filesystem, network, socket, process, or environment access | none exists in `fxjs/` | **the module has no such capability to remove.** `af/` forbids it by §1.2, and `script/` reaches the host only through `pdfrum-doc`'s already-loaded object store. Nothing in the design opens a file. |

### 5.4 The clock — a determinism requirement, not only a sandbox one

`testing/tools/test_runner.py` runs `pdfium_test --send-events --time=1399672130`
with `TZ=America/Los_Angeles`, and **the frozen clock leaks into the expected
bytes**: `public_methods_expected.txt` pins `AFParseDateEx(1, 2) = 1399672130000`,
and every `util.printd` line is shifted to the Los Angeles offset.

`boa 0.22`'s `HostHooks::utc_now()` is **deprecated and dead** — nothing calls
it. The live seam is `Context::clock()` and the `Clock` trait
(`src/context/time.rs:147`), with `FixedClock` (`:212`) provided. Verified:

```rust
let clock = Rc::new(FixedClock::from_millis(1_399_672_130_000));
let mut ctx = Context::builder()
    .host_hooks(Rc::new(Tz(-7 * 3600)))   // local_timezone_offset_seconds
    .clock(clock)
    .build()?;
```

produces `Date.now() == 1399672130000` and
`new Date().toString() == "Fri May 09 2014 14:48:50 GMT-0700"` — PDFium's seed
time and offset, exactly. **`ScriptConfig::clock_ms` and
`timezone_offset_secs` (§2.4) are those two knobs**, and the conformance runner
sets them from the same constants the oracle does.

### 5.5 How a test proves termination

A `#[test]` that would hang is a bad test, so termination is proved with a
budget rather than a wall clock:

```rust
#[test]
fn a_runaway_loop_terminates_with_a_diagnostic() {
    let mut diags = Diagnostics::default();
    let mut c = cascade_with(Limits { max_script_loop_iterations: 100_000, ..d() });
    let out = c.validate(&field(), "x");        // the field's /AA /V is `while(true){}`
    assert!(!out, "an exhausted script refuses rather than accepts");
    assert!(diags.iter().any(|d| matches!(d, Diagnostic::ScriptLimit { .. })));
}
```

The assertion is on the *diagnostic and the refusal*, not on elapsed time — a
timing assertion would be flaky on a loaded machine, and this project has
measured its machine at load 42. The same shape covers recursion and stack. For
the two limits `boa` does not provide (§5.1), the equivalent test cannot be
written until E2 is decided, and **that is the argument for E2 rather than a
detail of it**: the milestone's own exit criterion — "a script with `while(true)`
terminates via the limit" — is satisfiable today, but the neighbouring hostile
scripts are not, and shipping a sandbox that stops one and not the others is
worse than saying so.

---

## 6. The test plan

### 6.1 The 46 transcripts — the comparison contract, transcribed

`testing/tools/run_javascript_tests.py` is a 17-line shim; the contract is in
`testing/tools/test_runner.py`:

1. **Generate**: `python3 testing/tools/fixup_pdf_template.py --output-dir=<wd>
   <test>.in` → `<wd>/<test>.pdf`. A sibling `<test>.evt` is **copied next to
   the generated PDF first**; `pdfium_test` discovers it by path, not by flag.
2. **Run**: `pdfium_test --send-events --time=1399672130 <wd>/<test>.pdf` with
   `TZ=America/Los_Angeles`, capturing **stdout only** (stderr is not compared).
   `--send-events` is passed **unconditionally** for this suite.
3. **Normalise**: **there is none.** No sed, no regex, no sorting, no path
   rewriting. The raw stdout bytes are the artefact; determinism comes entirely
   from the frozen clock and the fixed timezone.
4. **Compare**: `testing/tools/text_diff.py` — `difflib.unified_diff` over
   `readlines()`. **Line-wise, order-sensitive, whitespace-sensitive exact
   equality.** Files open in text mode, so CRLF/LF is normalised and nothing
   else is. `expect.js` emits deliberate trailing spaces and `difflib` will not
   forgive them.
5. **Three branches**: no `_expected.txt` → stdout must be **byte-empty**;
   `--disable-javascript` and not suppressed → byte-empty; otherwise diff.

`fixup_pdf_template.py` is a two-pass byte-level expander with nine tokens —
`{{include}}`, `{{header}}`, `{{xref}}`, `{{trailer}}`, `{{trailersize}}`,
`{{startxref}}`, `{{startxrefobj x y}}`, `{{object x y}}`, `{{streamlen}}` —
with recursive includes (cycle-detected), CRLF→LF rewriting for `.js`/`.fragment`
/`.in`/`.xml`, 20-byte xref rows, and `{{streamlen}}` counting the bytes between
`stream` and `endstream` **minus one**. Eleven fixtures use includes, pulling in
`expect.js` (7), `field.fragment` (4), `constructor.js` and
`property_test_helpers.js`.

`testing/SUPPRESSIONS:643-651` suppresses only **three** javascript fixtures,
each under a narrow config: `bug_679642.in`, `bug_735912.in` (`noxfa`) and
`named_action.in` (`nov8`, because its line comes from a non-JS callback).

### 6.2 Where the cluster goes in our harness

The M14 brief's §4.6 established the pattern and this follows it: **not a
seventh `Pass`.** `Pass::ALL` is `[Pass; 6]` and a seventh would run for every
corpus file. The javascript suite is a different shape again — it is not a
corpus file with goldens beside it, it is a **template that must be expanded
first** — so it wants a **new tier**, not a new pass.

**Proposal: `conformance/src/script.rs`, a `js-transcript` tier**, scored as its
own scoreboard row family `{fixture}#js-transcript`, sitting beside the existing
`{path}#form-events` row tag (`scoreboard.rs:64-67`).

Three pieces it needs:

1. **A template expander.** `fixup_pdf_template.py` reimplemented in Rust, in
   the harness (not a library crate) — nine tokens, recursive include, the
   `{{streamlen}}` off-by-one, the 20-byte xref rows. ~250 lines. It is the only
   way to score the fixtures, and it is testable on its own against the 44
   checked-in `.pdf` outputs, which are the expander's own goldens.
2. **A transcript sink.** `ScriptCascade::transcript()` (§2.4) rendered to the
   ten line shapes of §6.3, joined with `\n`.
3. **A line-wise differ**, matching `text_diff.py`: compare `lines()` pairwise,
   report the first divergence. Not SSIM, not a threshold — **byte-exact, and
   the tier has no threshold knob at all**, which is right for a text tier and
   is what makes it Tier-A.

The four `.evt` files in the javascript directory (`bug_1445426.evt`,
`bug_1447268.evt`, `mouse_events.evt`, `public_methods.evt`) are replayed
through the M14 event path before the transcript is read — which is the point at
which M15's two halves meet, and is why `public_methods.in` is `.evt`-driven at
all.

**Retire M14's suppression.** M14's §4.6 collected the four JavaScript `.evt`
corpus files and suppressed them "until M15, with the suppression naming M15 so
it is retired rather than forgotten". M15 retires it.

### 6.3 The transcript line taxonomy — ten shapes, all scored

Counted across all 2004 expected lines:

| shape | count | producer |
|---|---|---|
| `Alert: <msg>` | 1922 | `app.alert`, default title/icon/type |
| `<title>: <msg>` | 3 | `app.alert` with `cTitle` |
| `<title>[icon=N,type=N]: <msg>` | 8 | `app.alert` with non-default icon/type |
| `<FnName>[icon=3,type=0]: <msg>` | ~14 | the `AF*` internal alerts |
| `BEEP!!! <n>` | 1 | `app.beep` |
| `PDF: <q>, defaultValue=, label=, isPassword=0, length=2048` | 2 | `app.response` |
| `Mail Msg: <bUI>, to=, cc=, bcc=, subject=, body=` | 14 | `mailMsg` / `mailForm` |
| `Doc Print: <8 fields>` | 4 | `Doc.print` |
| `Doc Submit Form: url=<url> + <N> data bytes:` + hex | 2 | `Doc.submitForm` |
| `Goto Page: <n>` | 9 | `gotoNamedDest` / page nav |
| `Execute named action: Print` | 1 | `named_action.in` |

**16 of the 46 (35%) hard-code exception message text**, which is why §1.6's
table is API. Three of those sixteen — `apply`, `array_buffer`,
`immutable_proto` — pin **V8's own error strings**, and boa's will differ. That
is the one category reimplementation cannot satisfy, and §7's E3 is where it is
escalated rather than quietly written off.

### 6.4 The 11 V8-gated embeddertests

M14 recorded them, outside its denominator, at docs/status/M14.md §"Bucket 2"
and brief §4.1 — exactly the contents of the single `#ifdef PDF_ENABLE_V8` block
at `fpdf_formfill_embeddertest.cpp:1132-1361`:

`DisableJavaScript`, `DocumentAActions`, `DocumentAActionsDisableJavaScript`,
`Bug551248`, `Bug620428`, `Bug634394`, `Bug634716`, `Bug679649`, `Bug707673`,
`Bug765384`, `Bug1477093`.

**PLAN §M15's exit criterion says "2/2 V8-gated formfill tests pass". The
measured number is 11, and M14 already corrected it.** Restated: **11/11**, or
11 accounted for with a reason each in the M14 style — ported, or not portable
by construction and said why. `DisableJavaScript` and
`DocumentAActionsDisableJavaScript` are the interesting pair: they assert that
with scripting *off* nothing runs, which is `NoScripts` and is already true, so
they are the two that pass on day one and the two that prove the feature flag
works.

### 6.5 Unit tests, snapshots, fuzz, and the dependency gate

**Unit tests for `AF*`** — the concurrent agent's, per §1.8. Table-driven over
`public_methods.in`'s own assertions, plus the 20-row `IsNumber` table from
`cjs_publicmethods_unittest.cpp:14-45` and the two-digit-year cases from
`cjs_publicmethods_embeddertest.cpp:67-72`. These need no engine, which is the
whole point of the split.

**Snapshots** (`insta`, STYLE §6): the `event` field table of §4.4, one snapshot
per event kind — 7 small snapshots that pin "which fields are live" as text and
would catch a regression in the copy-back rule immediately.

**Fuzz targets** — three, all in the existing `fuzz/` workspace:

- `fuzz_script_source` — arbitrary bytes as a script body, run under the
  smallest `Limits`. The property is **termination and no panic**, not a result.
- `fuzz_af_number_format` — arbitrary `AfEvent::value` × the style matrix. Pure,
  fast, and the highest-density arithmetic in the milestone.
- `fuzz_af_date_parse` — arbitrary value × arbitrary picture string.

**The dependency gate is a test, not a claim.** `scripts/ci.nu` gains an
assertion in the shape of `check-no-wgpu.nu`: with default features, **no
workspace crate reaches `boa_engine`, `boa_ast`, `boa_parser`, `boa_gc`,
`boa_interner`, `boa_string` or `boa_macros`** — plus a fourth assertion that
`pdfrum-form --features script` *does*, so the first three cannot pass
vacuously. That negative control is what caught a leaked edge in M12c and it is
worth repeating.

### 6.6 The DEPS.md audit `boa` needs — run, not promised

DEPS.md admits nothing on a claim. Measured on this machine, 2026-09-02:

| question | answer |
|---|---|
| crate and version | **`boa_engine = "=0.22.0"`**, pinned exactly, per DEPS.md's policy |
| features | **`default-features = false`**, nothing enabled. The defaults are `float16`, `xsum`, `temporal` — `temporal` alone drags `icu_calendar`, `temporal_rs` and `timezone_provider`, none of which any golden needs |
| tree size | **132 crates**, normal dependencies, `x86_64-unknown-linux-gnu` |
| `-sys` crates | **none** |
| `cc` / `cmake` / `bindgen` / `pkg-config` | **none** |
| build scripts compiling C | **none** |
| `cargo deny check licenses bans` against this repo's `deny.toml` | **`bans ok, licenses ok`** |
| licence spread | 62 `MIT OR Apache-2.0`, 18 `Apache-2.0 OR MIT`, 18 `Unicode-3.0`, 16 `MIT`, 10 `Unlicense OR MIT`, and singletons; **every `OR` expression resolves to an allowlisted branch**, so no `deny.toml` edit is required |
| duplicate versions | three warnings (`hashbrown`, `syn`, `synstructure`), non-fatal under `multiple-versions = "warn"` |
| MSRV | **1.91.0**, declared by every `boa_*` crate |
| builds clean | yes, 32 s cold |

**The MSRV is the one number that touches another milestone.** M13's exit
criterion is "MSRV declared and CI-checked"; `boa 0.22` sets a floor of **1.91.0**
for any build with `--features script`. Since the feature is default-off, the
floor applies to the feature rather than to the workspace — but it must be
*written down* in M13's declaration as a per-feature MSRV, not discovered by a
consumer. Recorded here so M13 inherits a fact rather than a surprise.

The DEPS.md row this earns:

> | `boa_engine` **lib, feature-gated** | JavaScript engine (`pdfrum-form --features script`) — M15 | Pure Rust, 132 crates, **zero `-sys`, zero `cc`**, `cargo-deny` clean against the existing allowlist. 95.5% of test262; register VM; `RuntimeLimits` for loop/recursion/stack, which is a sandbox the C++ has no equivalent of. Pinned `=0.22.0`, `default-features = false` (the `temporal` default drags ICU for nothing we use). MSRV 1.91.0 applies to this feature only. Alternatives `rquickjs` and `deno_core` bind C and V8 and fail the purity rule outright. **Reachable from no crate's default features**, asserted mechanically by `scripts/ci.nu`. |

---

## 7. Escalations and open questions

### Escalations (SPEC §0 — each changes SPEC.md, PLAN.md, DEPS.md or STYLE.md)

#### E1 — **The `Cascade` seam as shipped is not wired to anything, and PLAN §M15's central premise depends on it being wired**

PLAN §M15 opens: *"It landed as E4 ruled — one `&mut dyn Cascade` at one call
site … so item (3) below attaches a `boa` implementation to an existing trait
rather than threading a new parameter through routing."*

**The second half is false.** The one call site is `commit::run`, and
`commit::run` is called by nothing but its own tests. `route::apply` — the only
event entry point — has no cascade, and `route::Context` has no field for one.
`Cascade::keystroke` has no call site anywhere in the crate. Threading a new
parameter through routing is **exactly** what M15 must do, and it is the largest
non-`boa` item in the milestone.

This is not a criticism of M14, whose exit criteria could not have caught it: a
cascade that changes nothing is indistinguishable from one that never runs.

**Raised for decision, with the recommendation in §2.3:** thread the cascade as
a **second parameter** on the entry points that can commit a field — `apply`,
`choose` and `kill_focus` of the eight `route` re-exports at
`crates/pdfrum-form/src/lib.rs:55-57` — rather than as a `&mut dyn` field on
`Context`. (`close_popup`, `focus_of`, `popup_view` and `scroll_view` commit
nothing and keep their signatures.) The parameter costs three signatures; the
field costs making `Context` `&mut` at ~40 internal sites and contradicts its
own doc comment — "a borrowed view rather than an owned context object … it
owns nothing" (`route.rs:52-55`). Either way,
**PLAN §M15's "changes no call site" sentence must be withdrawn**, because it is
the sentence that would otherwise let the work be under-budgeted.

#### E2 — **`boa`'s runtime limits do not bound memory or regex backtracking, and PLAN §M15's sandbox claim is written as though they do**

Measured, §5.1: `var s='x'; for(var i=0;i<30;i++) s=s+s;` allocates **1 GiB in
1.1 seconds using thirty loop iterations**, and `/(a+)+$/.test('a'.repeat(28)+'b')`
runs **14.7 seconds and quadruples per added character**. Neither is bounded by
any `RuntimeLimits` field, and both are one line in a `/AA` script.

PLAN §M15 says "a script that exhausts a limit is a `Diagnostic`, never a hang
or a panic" and calls it "a *stronger* property than the C++ has". For loops,
recursion and stack that is exactly right and is measured. For memory and regex
it is not true, and the same reasoning M14 applied to tab-order banding — "a
library that can hang on input is a bug regardless of what the oracle does",
SPEC §15.9 — applies with more force here, because the input is a script rather
than a geometry.

**Raised for decision. Three options, none free:**

1. **Accept and document.** Ship M15 with the three limits `boa` provides, and
   state in SPEC §10 that memory and regex are unbounded. Cheapest; leaves a
   known hang in a library whose whole value is surviving hostile files.
2. **Bound them outside the engine.** Run the script on a thread with a wall
   clock and a memory watermark, and abandon the thread on breach. Costs a
   thread per script and makes `ScriptCascade` non-`Send`-friendly in a way
   STYLE §4's "all public types are `Send + Sync`" has to be checked against.
3. **Upstream it.** `RuntimeLimits` is a small, additive struct; a
   `set_allocation_limit` and a regex step budget are plausible upstream
   contributions. Right long-term, useless for this milestone's schedule.

**Recommendation: (1) for M15, with the measurement recorded in the status doc
and a named reopening condition**, on the same pattern DEPS.md uses for declined
perf dependencies — the number is written down, and the condition that would
change the answer is written down with it. Option (2) is the fallback if the
user judges a hang unacceptable to ship at all, which is a defensible reading of
this project's stated values and is why this is escalated rather than decided.

#### E3 — **Three goldens pin V8's own error strings and cannot be met by any reimplementation**

`apply_expected.txt`, `array_buffer_expected.txt` and
`immutable_proto_expected.txt` assert exception text produced by **V8**, not by
`fxjs/`. Boa's messages differ, and matching them would mean hard-coding another
engine's diagnostics.

PLAN §M15's exit criterion is **"46/46 `_expected.txt` byte-exact"**.

**Raised for decision.** The honest restatement, in M14's own idiom, is
**"46/46 accounted for: 43 byte-exact, 3 not achievable by construction with a
reason each"** — the reason being that the assertion is about V8's error
messages rather than about PDF. M14 set the precedent by carrying 10 rows as
"not portable by construction" and counting them explicitly rather than
quietly. The alternative — a message-translation table from boa's wording to
V8's — would be asserting something about a language, not about PDF, which is
the exact phrase M14 used to reject that move.

Two further counts want correcting in the same edit: the directory holds **47
`.in` fixtures**, not 46, and the 47th (`bug_1445426.in`) asserts **byte-empty
output**, which is a real scored assertion; and three fixtures are suppressed
upstream (`bug_679642`, `bug_735912` under `noxfa`; `named_action` under `nov8`).

#### E4 — **`Limits` gains four fields; SPEC §10 and STYLE §2b need one line each**

Additive, per SPEC §10's own note that `Limits` is deliberately not
`#[non_exhaustive]`:

- `max_script_loop_iterations: u64 = 10_000_000`
- `max_script_recursion: usize = 512`
- `max_script_stack: usize = 10_240`
- `max_calculate_depth: u32 = 1` — upstream's `busy_` permits no nesting at all

And a **STYLE §2b clarification, not an amendment**: the seam list stays closed
at three. `ScriptCascade` is `Cascade`'s second implementation, which is what
E4 (M14) anticipated. **No fourth seam is proposed** — the alert transcript is a
value getter, not a `ScriptHost` trait, under §2b's own 2026-09-01 clause about
inverting only when the library must ask a question it cannot answer.

#### E5 — **PLAN §M15's "2/2 V8-gated formfill tests" is 11, and M14 already measured it**

A one-line PLAN edit, listed separately from E3 because it is arithmetic rather
than a judgement, and because M14's ruling on the same file (E2 there) already
established the number. Restate as **11/11 accounted for**, names in §6.4.

#### E6 — **The M14 brief promised the `/CO` calculation order and it was not built**

Brief §3.6: *"the `/AcroForm /CO` calculation-order walk is a `Vec<FieldId>`
computed once from `/CO` by `pdfrum-doc` and handed to `calculate`. M14 computes
it and never uses it, so the list is already correct and tested when M15
arrives."* Grepping `crates/` for `CO`, `calculation_order` and `calc_order`
finds nothing in either crate.

Not an escalation about a decision — the work is unambiguously M15's now — but
recorded because the brief's promise is load-bearing on M15's estimate, and
because the rule it encodes is easy to get wrong: **a document with no `/CO`
array runs no calculation at all**, however many fields carry `/AA /C`
(`core/fpdfdoc/cpdf_interactiveform.cpp:739-745`).

### Open questions resolved in this brief (recorded, not escalated)

- **Which crate?** `pdfrum-form`, as a feature-gated module, argued in §2.4
  against the M14 precedent rather than by appeal to it.
- **A fourth trait seam for the host?** No. §2.4, under STYLE §2b's own clause.
- **Timers?** Recorded, never fired; §3.5. No global registry.
- **`global` persistence?** Never implemented; the oracle's is dead code and the
  file is never written. §3.2 Tier 3.
- **The frozen clock?** `FixedClock` + a timezone `HostHooks` override,
  verified end-to-end against PDFium's own seed constants. §5.4.
- **`eval` and `Function`?** Kept. Removing them fails seven goldens for no gain.
- **A separate `AF*` crate?** No — a module, §1.2, with a dependency rule a
  reviewer can check by reading the `use` lines.

### The things this brief could not settle

- **Whether `boa`'s `Date.parse` matches V8's** on the legacy formats
  `ParseDateUsingFormat`'s stage 2 falls through to
  (`fxjs/cjs_publicmethods.cpp:451-475`). Two goldens depend on it. Settle by
  running the fixture, not by reading the spec.
- **What `Field.value`'s number coercion does to a boa string.**
  `MaybeCoerceToNumber` (`fxjs/cjs_runtime.cpp:218-244`) returns the original
  value when the number is NaN *unless* the source string was literally `"NaN"`
  — a text field holding `"42"` comes back as the **number** 42. Whether boa's
  `ToNumber` agrees on every fixture input is measurable and unmeasured.
- **Whether the appearance generator can draw a formatted value.**
  `CommitOutcome::display` exists and nothing consumes it (§4.5). The change in
  `pdfrum-doc::ap::widget` is small in principle and adjacent to `LiveInput`,
  which docs/status/M14-gaps.md's closing note warns is delicate — read that
  note before touching it.
