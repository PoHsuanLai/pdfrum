# `.evt` harness review — M14 slice `0eff591`..`6f33c82`

**Reviewer:** Claude (Anthropic), cross-vendor review, 2026-09-01.
**Under review:** `0eff591`, `07dcd46`, `e4d5e82`, `6f33c82` (Grok).
**Oracle:** `/mnt/data2/pdfium/pdfium-c++` at `out/Release/pdfium_test`, read-only.

**Summary.** The parser is faithful — I checked it against `event.cc` line by
line and found no behavioural divergence, including the quirks that are easy to
get wrong (the dead arity guard, `atoi`'s decimal leading zeros, case-sensitive
substring modifiers, and the first-`.pdf` sibling rule). The harness design is
sound and the brief was right to adopt it. Two real defects sat underneath it,
both in the golden store rather than the parser, and both fixed here: event
goldens were keyed by the PDF alone, so nine fixtures that expand to identical
PDFs shared one image; and the four JavaScript fixtures were scored as M14's
work with no milestone naming M15.

**Definitive answer to the ruling's question: 30 rows, 12 pass, 18 fail.**

**Fixed:** findings 1, 2, 3, 6, 7. **Recorded, not fixed:** findings 4, 5.
**No issue:** findings 8–14.

---

## The 38-vs-27 reconciliation (the ruling's explicit requirement)

Both numbers were correct about different things, which is why they did not
meet: **27 counts `.evt` files, 38 counted scoreboard rows**, and the harness
has scored `.in` templates and their checked-in `.pdf` siblings as separate
rows corpus-wide since M0.

```
  59   .evt files in the checkout          (find testing -name '*.evt')
 -32   under an xfa path                   (4 corpus/xfa_specific, 22
                                            resources/javascript/xfa_specific,
                                            6 resources/pixel/xfa_specific)
 = 27  the brief's count (§1.2.7)          6 annots + 4 javascript + 17 pixel
  -2   bug_40643646.in, reset_button.in    suppressed `* * * *` in the oracle's
                                            own testing/SUPPRESSIONS:667,:740
  -4   the JavaScript fixtures             deferred to M15 (finding 2)
 = 21  distinct fixtures M14 owes          6 annots + 15 pixel
  +9   checked-in .pdf siblings            an .in already counted also ships as
                                            a .pdf; both get a row
 = 30  M14-target rows
```

And from the other direction, the 38 the harness recorded:

| Group | Rows | Disposition |
|---|---:|---|
| `corpus/pdfium/annots/*.pdf` | 6 | M14 |
| `resources/pixel/*.in` | 15 | M14 |
| `resources/pixel/*.pdf` | 9 | M14 (siblings of 9 of the 15) |
| `resources/javascript/{*.in,*.pdf}` | 8 | **deferred to M15** |
| **Total** | **38** | → **30** M14 rows |

**No XFA row ever slipped in** — `corpus.rs::is_xfa` excludes any path with an
`xfa`/`xfa_specific` component, and I verified all 32 are absent from the board.
**No `.evt` lacks a PDF.** **The `.in`/`.pdf` pairs are not a bug**: among the
1675 pre-existing rows there are 441 `.in` and 1234 `.pdf`, so scoring both is
the established convention and changing it would move the existing board, which
the monotone rule forbids.

### The 30 rows, as re-recorded

12 pass, 18 fail. `*` marks a row whose pass is real but not earned (finding 4).

| Status | ssim | Fixture |
|---|---|---|
| fail | 0.976851 | `corpus/pdfium/annots/annotation_highlight_author_content.pdf` |
| pass | 0.999999 | `corpus/pdfium/annots/annotation_highlight_empty_content.pdf` |
| fail | 0.924741 | `corpus/pdfium/annots/annotation_highlight_long_content.pdf` |
| fail | 0.978698 | `corpus/pdfium/annots/annotation_highlight_no_author.pdf` |
| pass | 0.999999 | `corpus/pdfium/annots/annotation_highlight_no_author_no_content.pdf` |
| pass | 0.999999 | `corpus/pdfium/annots/annotation_highlight_no_content.pdf` |
| pass\* | 0.991774 | `resources/pixel/bug_113910.in` |
| pass\* | 0.991774 | `resources/pixel/bug_113910.pdf` |
| fail | 0.918875 | `resources/pixel/bug_1372651.in` |
| fail | 0.918875 | `resources/pixel/bug_1372651.pdf` |
| fail | 0.975664 | `resources/pixel/bug_477200528.in` |
| fail | 0.975664 | `resources/pixel/bug_477200528.pdf` |
| fail | 0.978497 | `resources/pixel/bug_736695_2.in` — *was passing on the wrong golden* |
| pass | 0.997003 | `resources/pixel/bug_736695_3.in` |
| pass | 0.999998 | `resources/pixel/bug_736695_4.in` |
| pass | 0.999998 | `resources/pixel/bug_736695_4.pdf` |
| pass\* | 0.990167 | `resources/pixel/checkbox_radiobutton.in` |
| pass\* | 0.990167 | `resources/pixel/checkbox_radiobutton.pdf` |
| pass\* | 0.994617 | `resources/pixel/combobox_form.in` |
| pass\* | 0.994617 | `resources/pixel/combobox_form.pdf` |
| fail | 0.926985 | `resources/pixel/form_textfield_focused_ltr.in` |
| fail | 0.930355 | `resources/pixel/form_textfield_focused_rtl.in` |
| fail | 0.930355 | `resources/pixel/form_textfield_focused_rtl.pdf` |
| fail | 0.909175 | `resources/pixel/form_textfield_selected_ltr.in` |
| fail | 0.908047 | `resources/pixel/form_textfield_selected_rtl.in` |
| fail | 0.953963 | `resources/pixel/password.in` |
| fail | 0.953963 | `resources/pixel/password.pdf` |
| fail | 0.986707 | `resources/pixel/scrollable_widgets1.in` |
| fail | 0.986707 | `resources/pixel/scrollable_widgets1.pdf` |
| fail | 0.979377 | `resources/pixel/scrollable_widgets2.in` |

### The existing board did not move

`1675 → 1675 rows, 0 changed`, diffed field-by-field against the board as of
`6f33c82^` — i.e. against the state before *any* of this slice landed, not
merely before my edits. `run --check-regressions` reports exactly one
previously-passing row now failing, `resources/pixel/bug_736695_2.in#form-events`,
which is finding 1's intended effect: it was passing against another fixture's
image.

---

## Findings

### 1. **fixed** — Event goldens were keyed by the PDF alone, so nine fixtures shared one image

`goldens::key_for` is `sha256(pdf_bytes)[..16]`, and the event PNG was stored as
`input.pdf.N.events.png` with nothing in the name identifying the script. Nine
in-scope fixtures expand to **byte-identical PDFs** and differ *only* in their
`.evt`:

| Golden key | Fixtures sharing it |
|---|---|
| `54a8f12a521c255c` | `bug_736695_2`, `bug_736695_3`, `bug_736695_4` |
| `7c9ffc0dafcfa42f` | `form_textfield_focused_ltr`/`_rtl`, `form_textfield_selected_ltr`/`_rtl` |
| `1b74251ab464ae5e` | `scrollable_widgets1`, `scrollable_widgets2` |

`ensure_event_goldens` short-circuits as soon as *any* events artifact exists in
the manifest, so whichever fixture generated first defined the golden for its
whole group and the rest were scored against a render driven by someone else's
script. The manifests said so plainly — `54a8f12a521c255c` listed five `sources`
and one `input.pdf.0.events.png`.

The consequence was a false pass. `bug_736695_4`'s script opens a dropdown,
hovers, then clicks far off at (6,6) and dismisses without selecting, so its
event render equals its plain render. That is the image the group stored. Run by
hand:

```
$ pdfium_test --png --md5 input.pdf                 # bug_736695_2
MD5:input.pdf.0.png:...                             plain
$ pdfium_test --png --md5 --send-events input.pdf
MD5:input.pdf.0.png:...                             differs
```

and measured directly, `bug_736695_2` changes 3798 px (1.87% of the page,
maxdiff 250) and `bug_736695_3` changes 2250 px (1.11%, maxdiff 241) — yet both
scored `ssim 0.999998, max_channel_diff 19`, which are `bug_736695_4`'s numbers,
not their own.

**Fix** (`d72e8ac`): the artifact name now carries the first 8 hex digits of the
script's SHA-256 — `input.pdf.N.events-<digest>.png`. Same store layout, same
key, plain-render artifacts untouched; the event walk selects only its own
script's PNG via `is_events_png_for`, while `is_events_png` stays the broad
predicate the plain walk needs to exclude *every* event artifact in a shared
directory. `PngSet::select` became a closure to carry the digest. After
regeneration the three groups hold 3, 4 and 2 distinct goldens (24 → 30 total,
+6 previously lost), all with different md5s.

### 2. **fixed** — The four JavaScript fixtures were scored as M14's work

The ruling says the 4 JS `.evt` files are "suppressed naming M15" and brief §4.6
repeats it. Nothing in the harness did so: `bug_1445426`, `bug_1447268`,
`mouse_events` and `public_methods` contributed 8 rows (`.in` + `.pdf` each) to
the 38. Their events only do anything through a field script, so no JS-free form
layer can ever reproduce the oracle's render — they would have sat as permanent
failures reading as M14's debt.

**Fix** (folded into `d72e8ac`, see finding 7): `DEFERRED_FORM_EVENTS` in
`run.rs` is a four-entry table of `(corpus stem, milestone)`, consulted by
`form_events_deferred_to` and checked in `score_form_events` before any work.
The milestone is in the data, so the rows are retired rather than forgotten. A
unit test pins all four names, both extensions, and that M14's own fixtures are
*not* deferred.

### 3. **fixed** — Lost update on the shared manifest under `--jobs > 1`

Surfaced by finding 1's fix. `ensure_event_goldens` reads the manifest, runs the
oracle, then writes the modified copy back. Because several fixtures share one
key, a sibling can finish during that oracle run; the stale copy then overwrote
its entry, leaving the PNG on disk but absent from the manifest. `bug_736695_2`
hit this on the first regeneration and scored
`no --send-events golden for script fe1feead` while
`input.pdf.0.events-fe1feead.png` sat in the directory.

**Fix** (`abfadfd`): re-read the manifest immediately before merging, and
deduplicate md5 lines by path. The artifact write has already happened by then,
so only the list needs union-ing. Verified: all 30 event goldens on disk are
listed in their manifests, 0 mismatches across the whole store.

### 4. **recorded, not fixed** — Three rows pass on a difference SSIM 0.99 cannot see

The ruling's U1 says 15 of 38 "already pass with the unfocused appearance". I
checked whether each pass is legitimate by running the oracle by hand with and
without `--send-events` and comparing, which is stronger than comparing md5s
alone because it measures *how much* changed:

| Fixture | Oracle changed px | % of page | maxdiff | Verdict |
|---|---:|---:|---:|---|
| `bug_736695_4` | 0 | 0.000% | 0 | legitimate |
| `mouse_events` | 0 | 0.000% | 0 | legitimate (now M15's anyway) |
| `annotation_highlight_empty_content` | 0 | 0.000% | 0 | legitimate |
| `annotation_highlight_no_content` | 0 | 0.000% | 0 | legitimate |
| `annotation_highlight_no_author_no_content` | 0 | 0.000% | 0 | legitimate |
| `bug_113910` | 177 | 0.295% | 153 | **not earned** |
| `checkbox_radiobutton` | 820 | 0.911% | 170 | **not earned** |
| `combobox_form` | 3380 | 0.697% | 255 | **not earned** |

The goldens are **correct** — I confirmed the generator passes `--send-events`
unconditionally (`generate.rs::run_send_events`) and copies the sibling script to
`input.evt` before the run (`copy_sibling_evt`, called at both `:146` and `:161`),
and the oracle printed `Using event file` / `Sending events from:` on every hand
run. So this is not the failure mode the task asked me to rule out. It is the
coarser one: SSIM is a global average, and typing four digits or ticking a
checkbox changes under 1% of the page, which clears a 0.99 floor even at
channel-delta 255.

Not fixed, deliberately. `thresholds.toml`'s ratchet only tightens, and a
per-file floor is a claim about what the renderer *can* achieve; setting one now,
while dispatch is stubbed, would pick a number off a stub. The edit-control slice
should tighten these three once it can draw them — that is the moment the number
means something. Flagged here so nobody reads "12 pass" as 12 fixtures working.

### 5. **recorded, not fixed** — `atoi` overflow saturates where glibc wraps

`events.rs::atoi` clamps to `i32::MIN`/`MAX`. Real `atoi` is UB on overflow and
glibc actually returns `-2147483648` for `"2147483648"` and `2147483647` for
`"-2147483649"` — i.e. it wraps, and the doc comment's claim that saturation "is
what glibc `strtol` does for `atoi`" is wrong in the first direction:

```
[2147483648]  -> -2147483648      (glibc)     vs  2147483647  (ours)
[-2147483649] ->  2147483647      (glibc)     vs -2147483648  (ours)
```

Unreachable on this corpus — no `.evt` file holds a value outside ±5 digits, and
I checked all 59. Saturating is also the better behaviour for a parser of
untrusted input (STYLE §3). Recorded so nobody later "verifies" the claim and is
misled; if the grammar ever meets a hostile file the divergence is a diagnostic
away, not a bug.

### 6. **fixed** — The coordinate-space doc comments said "device pixels"

The module doc and eleven field docs called the mouse coordinates device pixels.
They are **page space** — PDF user space, y-up. `public/fpdf_formfill.h`
documents `page_x`/`page_y` that way for `OnMouseMove`, `OnLButtonDown`,
`OnFocus` and `DoubleClick`, and `fpdf_formfill.cpp` passes them through with no
transform. The header's "in device" on `FORM_OnLButtonUp` is an upstream doc bug
— that function's body is identical to `OnLButtonDown`'s. Brief §4.2 calls this
out as "the one correction the landed code should take", and it matters: the
`pdfrum-form` implementer reading these labels literally would insert a
device→page transform that must not happen. Fixed in `1d7328d`.

### 7. **fixed, with a process note** — the corpus test could not fail

`every_corpus_evt_parses_without_error` asserted only that `parse_evt` returned
`Ok` for all 59 files — and `parse_evt` is infallible by construction, so that
assertion could never fail. It did confirm the checkout has 59 files, and I
confirmed it is **not** skipping on this machine (it runs and passes; the
`files.len() == 59` assert would fire if the root were missing, and I ran it
explicitly with `--no-capture`). But the parse itself was unchecked.

It now requires each file to yield at least one event, and the event count to
equal the file's non-comment non-blank line count — which is the real property,
since every corpus line is a verb the grammar knows. A regression that silently
dropped a verb passed before and fails now. Fixed in `1d7328d`; verified the
strengthened assertion holds across all 59.

**Process note, disclosed rather than hidden:** finding 2's deferral table and
finding 1's fix both live in `conformance/src/run.rs` and landed in the single
commit `d72e8ac`, whose message describes only finding 1. A path-scoped commit
cannot split one file, and rewriting history is forbidden here, so the deferral
is documented in this review and in `1fa7527`'s message instead. Reviewers
looking for finding 2's code should look in `d72e8ac`, not in a commit named for
it.

### 8. **no issue** — Parser fidelity against `event.cc`, line by line

Checked every rule; all match.

- **Tokenization.** `string_split` keeps empty pieces and always returns at
  least one element, matching `fx_string_testhelpers.cpp:27-41`. Rust's
  `str::split` has exactly this shape.
- **Comments.** `StringSplit(line, '#')` then `command[0].empty()` → skip
  (`:169-172`). Reproduced exactly, including that `#` cuts anywhere on the line
  and that trailing whitespace before it survives into the token.
- **`'\r'` is not stripped** — the script is split on `'\n'` alone. Correct.
- **`atoi`.** Leading `isspace`, optional sign, digits, stop at first non-digit,
  `0` when no digits. I verified against a compiled C program: `"  97"`→97,
  `"+13"`→13, `"3x"`→3, `"abc"`→0, `""`→0, `"0x1A"`→0, and critically
  **`"09"`→9, decimal not octal** — which `xfa_specific/bug_1060549.evt`
  actually exercises with `keycode,09`. `str::parse` would reject `"3x"`; the
  hand-written prefix scan is required and is correct.
- **Modifiers.** Substring, not equality, and case-sensitive: `shiftcontrolalt`
  sets all three, `Shift` sets none. Matches `GetModifiers` (`:19-32`) using
  `std::string::find`.
- **The dead arity check.** `event.cc:63,84,104,135` write `size < N && size > N+1`,
  which is never true, so extra tokens on the four mouse verbs are ignored rather
  than rejected. The Rust reproduces this rather than "fixing" it, with the
  reasoning in a comment. Correct call — the `||` the C++ author meant would
  change behaviour.
- **Bounds.** Where C++ would index `tokens[2]`/`tokens[3]` past the end, the
  Rust uses `?` and skips the line. This is D8's sanctioned divergence (the C++
  would crash; a total parser may not), and it is the only place the two differ.
- **`mousedoubleclick` is left-only** (`:112-115`) — reproduced, and the verb
  carries no button. Note the check order differs from the C++, which reads
  `x`/`y` *before* rejecting a non-`left` button; since both paths produce no
  event, this is unobservable.
- **Exact arities.** `charcode` requires exactly 2 tokens, `mousemove` and
  `focus` exactly 3, `keycode` 2 or 3. All match, including that `focus,x,y,0`
  is rejected.
- **Unknown verbs and blank lines are skipped**, not fatal (`:190-192`).
- **Sibling path** (`pdfium_test.cc:2152-2156`): `sibling_evt_path` finds the
  **first** `".pdf"` substring and replaces those four bytes — matching
  `std::string::find`, not an extension strip. The test pins
  `/tmp/a.pdf/b.pdf` → `/tmp/a.evt/b.pdf`, which is the case that distinguishes
  the two readings. Correct.

### 9. **no issue** — `Event` shape against brief §4.2

The brief explicitly adopts what shipped and asks for no shape change; I agree
with that judgement. `pdfrum_tool::events::Event` is a faithful *grammar* type —
one variant per verb, `i32` straight from `atoi`, and `KeyCode` deliberately
representing the down/up pair as the grammar emits it. The semantic
`pdfrum_form::Event` is a different layer. Keeping them apart is what lets
`parse_evt` stay a pure total description of the grammar while the engine's input
type is free to be the one the embeddertests need. See "For the `pdfrum-form`
implementer" below for what crossing the bridge requires.

### 10. **no issue** — Goldens were generated with `--send-events`

The specific failure mode the task asked me to rule out did not occur.
`run_send_events` passes `--send-events` unconditionally alongside the
determinism recipe (`--time=1399672130`, `--croscore-font-names`,
`--font-dir=…`), `copy_sibling_evt` places the script at `input.evt` before the
run, and the oracle confirmed on every hand run that it found and used it
(`Using event file input.evt.` / `Sending events from: input.evt`). No golden was
produced by an oracle that silently rendered without events.

### 11. **no issue** — The cluster design

An extra invocation inside the Render generator, taken only when a sibling `.evt`
exists, rather than a seventh `Pass` that would run for every corpus file. The
`{path}#form-events` row tag keeps the cluster separate, `is_events_png` keeps
the plain Tier-B walk from ever comparing an event artifact, and `triage` lists
`form-events` as its own cluster. This is the right shape and the brief is
correct to adopt it.

### 12. **no issue** — STYLE compliance

No `unwrap`/`expect`/`panic` outside `#[cfg(test)]` in either file;
`thiserror` on `EvtError` and `SuppressionError`; `anyhow` confined to
`pdfrum-tool` and the harness as STYLE §3 permits. `Event` and `MouseButton` are
plain data enums with public fields and no behaviour attached — the data/logic
split STYLE §1 asks for. The verb dispatch is one `match` over `&str` with one
small function per verb, none of which is a transliteration of its C++
counterpart: the C++'s eight `void Send*Event(form, page, tokens)` procedures
became eight `fn(&[String]) -> Option<Event>` pure functions, which is the
STYLE §7 test passed rather than dodged. `EvtError` is currently unconstructible
in practice, which STYLE would normally flag; the doc comment says so explicitly
and justifies it as the public failure channel, and I agree with keeping it.

### 13. **no issue** — Suppression handling

`bug_40643646.in` and `reset_button.in` are absent from the board because the
oracle's own `testing/SUPPRESSIONS:667,:740` lists them `* * * *`, and
`corpus.rs::skip_reason` applies it. Neither has a `.pdf` sibling, so nothing is
half-excluded. This is the oracle declining them, not the harness, and it is
correct: `reset_button` needs the reset *action*, which is not a rendering
question.

### 14. **no issue** — Gates

`cargo fmt -p pdfrum-tool -p conformance -- --check` clean.
`cargo clippy -p pdfrum-tool -p conformance --all-targets -- -D warnings` clean
(one `needless_pass_by_value` I introduced by making `PngSet` generic, fixed by
taking it by reference). `cargo nextest run -p pdfrum-tool -p conformance`:
**296 passed, 0 skipped**. `cargo test --doc` reports no library targets — both
are binary crates, so there are no doctests to run. Benchmarks not run, per the
load constraint.

---

## For the `pdfrum-form` implementer

1. **The coordinates are page space, y-up.** They were mislabelled "device
   pixels" until `1d7328d`. Do not insert a transform. The one header comment
   that says "in device" (`FORM_OnLButtonUp`) is an upstream doc bug.
2. **`Event::KeyCode` is the down/up *pair*, not one edge.** The grammar's
   `keycode` verb fires `FORM_OnKeyDown` then `FORM_OnKeyUp`. Since `OnKeyUp` is
   a permanent no-op returning false, the bridge should emit one
   `pdfrum_form::Event::KeyDown` and drop the up.
3. **`charcode`, `mousemove` and `focus` carry hardcoded modifier 0** — the
   `.evt` grammar has no field for them. Keep `modifiers` on the semantic enum
   anyway: the embeddertests call `FORM_OnChar(ch, modifier)` with real
   modifiers, so the grammar is a subset of the API, not the whole of it.
4. **`Event::CharCode.code` is an `i32` code point, not a `char`.** The bridge
   must do the fallible narrowing (`u32::try_from` then `char::from_u32`).
   `form_textfield_focused_rtl.evt` sends 1488–1514, Hebrew — this path is
   exercised by the corpus, not hypothetical.
5. **Do not repair unbalanced mouse events.** A `mousedown` with no `mouseup` is
   intentional; `scrollable_widgets1.evt` depends on it.
6. **Replay is per page.** `pdfium_test` sends the whole stream once for each
   page before saving any image. `--send-events` dispatch must do the same or the
   multi-page fixtures diverge.
7. **The row count you are burning down is 30, not 38 or 27**, and 3 of the
   12 currently "passing" rows are finding 4's not-earned passes. The honest
   count of fixtures that genuinely need no work is **9**, all of which produce
   zero oracle pixel change.
8. **Your goldens are now script-addressed.** If you add an `.evt` fixture whose
   PDF matches an existing one, it gets its own golden automatically. Regenerate
   with `--jobs 1` when several fixtures share a key, or rely on finding 3's
   re-read; verify with the disk-vs-manifest check rather than assuming.

## Commits

| Commit | What |
|---|---|
| `d72e8ac` | Finding 1 (script-keyed event goldens) + finding 2 (M15 deferral table) |
| `1d7328d` | Finding 6 (page-space docs) + finding 7 (corpus test strengthened) |
| `abfadfd` | Finding 3 (manifest lost-update race) |
| `1fa7527` | Re-recorded baseline: 30 rows, 12 pass, 18 fail; 1675 existing rows unchanged |
