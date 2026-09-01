# Oracle divergence audit — Phases 1–3 re-sorted under the oracle-bug rule

**Date:** 2026-09-02. **Oracle pin:** `pdfium-c++` at `6f2272e`.
**Tiebreaker:** Mozilla pdf.js at `645324bb` (2026-09-01), cloned fresh for this
audit. **Scoreboard:** `conformance/scoreboard.json`, 1705 files, 1652 pass /
53 fail.

## The rule this applies

PLAN.md §212–229 (`81ba24d`), user-ruled 2026-09-02:

> The oracle is the arbiter of *what PDF means*, not of *what is correct*.
> When the C++ is shown to be wrong against the specification or against an
> independent implementation — verified at the cited line, not asserted —
> pdfrum implements the **correct** behaviour, marks the site `[oracle-bug]`
> with both citations, and moves the golden assertions that pin the wrong
> answer into the **not-achievable-by-construction** bucket. […] Where the
> oracle is merely *surprising* but matches Adobe, it is reproduced, cited,
> and not called a bug.

First applied to M15's `AF*` library. This document applies the same sorting
retroactively to everything Phases 1–3 reproduced.

**The four M15 findings were re-verified at the line for this audit**, since
they are the rule's only precedent and no status doc records them:
`fxjs/fx_date_helpers.cpp:75-77` — `IsLeapYear` returns
`(y%4==0) && ((y%100!=0) || (y%400!=0))`, so **2000 is not a leap year**
(the second disjunct must be `y%400==0`); `:330` — `int nYearSub = 99;  //
nYear - 2000;`, the intended formula in a comment, with `:556-557` mapping
every two-digit year into 2000–2099; `fxjs/cjs_publicmethods.cpp:519, 528,
548, 557` — all four use `nHour > 12`, so **noon formats as `12 am`**; and
`:218` selects the averaging strategy with `EqualsASCIINoCase("AVG")` while
`:1456` divides only under `EqualsASCII("AVG")`, so lowercase `avg` returns
the **sum**.

## Method, and what "verified" means here

Every row was settled by **opening the PDFium file and reading the operative
expression**. That discipline paid for itself: **five rows in our own briefs
and status docs describe the oracle wrongly**, and are corrected in §6. The
spec column cites ISO 32000-1:2008. The pdf.js column cites `src/…`
file:line; where pdf.js does not implement the feature that is stated rather
than inferred as agreement (§4).

**Verdicts** (closed list): **SPEC** — PDFium matches the spec, or the spec is
silent and the choice is conventional; keep reproducing, not a bug.
**BUG** — contradicts the spec and/or pdf.js implements the specified
behaviour. **ROBUSTNESS** — recovery for malformed input where the spec is
silent; keep ours, matching the oracle is the tie-break. **ACROBAT** — matches
Acrobat where the spec is ambiguous. **UNKNOWN** — not settled; the evidence
that would settle it is named.

**Blast radius** counts scoreboard *rows* (`.in` and `.pdf` of one fixture are
two rows). Where a feature is visible only in an uncompressed stream the count
is a lower bound, marked "≥".

---

## 1. The table

| id | area | behaviour | PDFium | spec | pdf.js | verdict | rows | action |
|---|---|---|---|---|---|---|---|---|
| **A1** | cmap | `usecmap` in an **embedded** CMap is an explicit no-op; the `/UseCMap` **dictionary key is never read either** | `cpdf_cmapparser.cpp:61` — `} else if (word == "usecmap") {` with an empty body; no `"UseCMap"` lookup anywhere in `core/fpdfapi/` | §9.7.5.3 defines both channels and the inherit-then-override rule | **implements both**: `cmap.js:608-613`, `:639-648`, `extendCMap` `:650-669` incl. codespace inheritance and `if (!cMap.contains(key))` child-wins | **BUG** | 0 | implement; `[oracle-bug]`; keep the diagnostic |
| **A2** | page/function | negative encoded input lands on the **top** cell | `cpdf_sampledfunc.cpp:128` — `std::clamp(static_cast<uint32_t>(encoded_input[i]), 0U, sizes-1)` | §7.10.2: `e'ᵢ = min(max(eᵢ,0), Sizeᵢ−1)` | `function.js:231` `MathClamp(e, 0, size_i-1)` → cell **0** | **BUG** | ≥29 | §3 (A2) |
| **A3** | page/function | interpolation is a **sum of per-axis gradients** against one base sample; exact only for one input | `cpdf_sampledfunc.cpp:163` `float encoded = sample;` + `:184` `encoded += (encoded_input[j]-index[j])*(sample2-sample)` — reads `m+1` samples, never the `2^m` corners | §7.10.2 specifies multilinear interpolation | `function.js:205-260` — true multilinear over `1<<inputSize` hypercube vertices, weights as a **product** in `cubeN` | **BUG** | ≥29 | §3 (A3) |
| **A4** | page/function | a degenerate axis (`/Size` 1) **multiplies and overwrites**, discarding earlier axes | `cpdf_sampledfunc.cpp:165-168` — `encoded = encoded_input[j] * sample;` | §7.10.2 — weight on a one-element axis is 1, i.e. identity | `function.js:231-236` clamps `e` to 0, giving `n0=0, n1=1`: no scaling, no clobber | **BUG** | ≥29 | §3 (A4) |
| **A5** | page/function | `/Order` never read; cubic-spline silently downgraded | `grep '"Order"' core/ fpdfsdk/` → **no hits** | §7.10.2 table 38 | `function.js:180-185` reads it, logs, **ignores it too** | **SPEC** | — | keep; both independents agree |
| **A6** | render/shading | LUT sampled at `i/256` but indexed at `s*255` | `cpdf_rendershading.cpp:81` `diff*i/kShadingSteps` vs `:160` `(int32_t)(scale*(kShadingSteps-1))` | §8.7.4.5 — the ramp is `t` over `[t₀,t₁]`; a 1/256 skew is unspecified | delegates to the canvas gradient; no equivalent | **BUG** (cosmetic) | ≥42 | §3 (A6) |
| **A7** | render/shading | radial branch 1 (`b≈0`) has **no** discriminant, root-selection, radius or `a==0` guard; branch 2 (`a≈0`) has no radius check | `cpdf_rendershading.cpp:236-259` — `if (FXSYS_IsFloatZero(b)) s = sqrt(-c/a);` … `else if (a_is_float_zero) s = -c/b;` | §8.7.4.5.4: choose the **larger** s with r(s) ≥ 0 | `pattern.js:328-347` + `pattern_helper.js:115-131` → `createRadialGradient`; no root selection of its own | **UNKNOWN** | ≥42 | §5 (A7) |
| **A8** | render/shading | the "decreasing" test **truncates the hypotenuse to an integer** | `cpdf_rendershading.cpp:220` — `bool bDecreasing = dr < 0 && static_cast<int>(hypotf(dx,dy)) < -dr;` | no spec basis: §8.7.4.5.4 has no integer step | none | **BUG** (cosmetic) | ≥42 | §3 (A8) |
| **A9** | render | `/TR` array loaded **reversed**: `array[2]` drives red; `array[3]` (gray) never read | `cpdf_docrenderdata.cpp:90` `pFuncs[2-i] = Load(array[i])`; `:113-114` `samples[0]` is `samples_r`; no second reversal downstream | §8.6.5.9 / table 58: `[red green blue gray]`, so `array[0]` is red | order-preserving: `evaluator.js:944-959` in-order `push`, `filter_factory.js:212` `[tableR,tableG,tableB] = …` | **BUG** | 3 (2 pass) | §3 (A9) |
| **A10** | render | transfer sample stored **unclamped**: negative folds to the top of the byte range | `cpdf_docrenderdata.cpp:124` `size_t o = FXSYS_roundf(output[0]*255); samples[i][v] = o;` | §8.6.5.9, and §7.10.1 requires clipping to `/Range`; also UB in C++ | clamps to `/Range` first (`function.js:265`), then `evaluator.js:888-896` | **BUG** | 3 (2 pass) | §3 (A10) |
| **A11** | render | single-function `/TR` with `OutputCount() > 16` reads an **unwritten** `output[0]` | `cpdf_docrenderdata.cpp:132-137` — `Call` guarded, the read is not; the **array** branch at `:119-122` handles the same case correctly with `samples[i][v] = v` | §8.6.5.9 | n/a | **BUG** | 0 | **already declined** (page D7). Relabel |
| **A12** | render | non-isolated groups **double-count their backdrop** | `cpdf_renderstatus.cpp:673-683` seeds the group buffer with `GetDIBits(backdrop,…)`; `:740-751` composites it back with no removal step anywhere between | §11.4.6 / §11.6.6 require removing the initial backdrop: `C = Cn + (Cn−C0)×(α0/αgn − α0)` | `canvas.js:3205-3237` draws a non-isolated group **directly onto the parent** — no copy, no double-count (cites bug 1873345) | **BUG** | ≥11 (1 fail) | §3 (A12) |
| **A13** | render | `/K` (knockout) **never honoured**: the only `/K` read in `core/fpdfapi` is CCITT's | `fpdf_parser_decode.cpp:330` is the sole hit; `RenderDeviceDriverIface::SetGroupKnockout` is an empty body (`renderdevicedriver_iface.cpp:132`), unoverridden by AGG | §11.6.6 table 147 | **fully implements it**: `evaluator.js:523-524` reads `/I` and `/K`; `canvas.js:499-534`, `:3310-3318` | **BUG** | ≥11 (1 fail) | §3 (A13) |
| **A14** | render | AGG's ±32000 `HardClip` **distorts** rather than clips — each coordinate clamped independently, applied **after** transform and separately to Bézier control points | `cfx_agg_devicedriver.cpp:53-58`, applied `:946` and `:968-970` | spec silent on device coordinate range | n/a | **ROBUSTNESS** | ≥0 | keep (render D10): the artefact is what the oracle emits |
| **A15** | page | dash entry `<= 1e-6` substituted with **0.1**, turning a zero-length dash into a visible one | `cfx_agg_devicedriver.cpp:370-372` | §8.4.3.6: a zero-length dash with a round cap is a **dot** | not implemented as such | **BUG** (minor) | ≥2 | §3 (A15) |
| **A16** | page | dash cycle `< 0.1` device px renders **solid** | `cfx_agg_devicedriver.cpp:363-365` | §8.4.3.6 silent on device thresholds | n/a | **ROBUSTNESS** | ≥2 | keep |
| **A17** | page | the two backends disagree on dash normalization (AGG cycles an odd array; Skia doubles it) | `cfx_agg_devicedriver.cpp:336-386` | §8.4.3.6: an odd-length array **cycles** — AGG is right | n/a | **SPEC** | — | keep AGG (page D6): both the oracle and the spec |
| **A18** | page | `/ExtGState /Font` looks its **name** up in the `/Font` resources, so the spec's `[<ref> size]` form never resolves and silently falls back to Helvetica | `cpdf_allstates.cpp:87-89` `FindFont(font->GetByteStringAt(0))`; `cpdf_streamcontentparser.cpp:1239` returns the stock font on failure | table 58: `/Font` is `[font size]` where font is an **indirect reference to a font dictionary** | not implemented (pdf.js ignores ExtGState `/Font`) | **BUG** | ≥0 | §3 (A18) |
| **A19** | page | `/TR` skipped when `/TR2` present; `/BG` when `/BG2`; `/UCR` when `/UCR2` | `cpdf_allstates.cpp:92-97`, `:137-143` | table 58 — `TR2` **supersedes** `TR`, `BG2` supersedes `BG` | same rule, same citation: `evaluator.js:1189-1194` | **SPEC** | — | keep; this is the spec, not a quirk |
| **A20** | page | `/OP` also sets the non-stroking overprint flag unless `/op` is present | `cpdf_allstates.cpp:125-131` | §8.6.7 — exactly this rule | n/a | **SPEC** | — | keep |
| **A21** | page | `J`/`j` operand `static_cast` into a 3-valued enum, so `5 J` stores `LineCap(5)`; only the AGG `default:` launders it | `cpdf_streamcontentparser.cpp:989-997`; `cfx_agg_devicedriver.cpp:305-328`; enums `cfx_graphstatedata.h:18-20` | §8.4.3.3 / §8.4.3.4 tables 54–55: 0/1/2 only | n/a | **ROBUSTNESS** | — | keep our clamp (page D5): observably identical, and the invalid enum otherwise survives `q`/`Q` |
| **A22** | page | colour-key `/Mask` too short leaves the ranges **at `[0,0]`** while still setting the flag, so **every all-zero pixel is masked out** | `cpdf_dib.cpp:447-459` — the `size() >= components_*2` test guards only the loop; `color_key_ = true` at `:458` is outside it | §8.9.6.4 requires `2 × n` integers; enabling masking from a short array is unsanctioned | n/a | **ROBUSTNESS**→see §6 | ≥0 | our `0/0` default matches; but the *reason* recorded is wrong — see §6 |
| **A23** | page | the mesh bbox helper **permanently shrinks** its point/colour counters after the first flagged patch | `cpdf_streamcontentparser.cpp:94-123` — `point_count -= 4; color_count -= 2;` mutate loop-**invariants**; the renderer's own copy at `cpdf_rendershading.cpp:900-917` uses per-iteration `iStartPoint`/`iStartColor` and is correct | §8.7.4.5.5–7 | n/a | **BUG** | ≥0 | **already declined** (page D18). Relabel |
| **A24** | page | pattern cache keyed on object identity **alone**, so the second user inherits the first's `parent_matrix`; `GetPattern` and `GetShading` also **share one map** with different `bShading` flags | `cpdf_docpagedata.cpp:388`, `:406`, `:415-422` | §8.7.3 | n/a | **BUG** | ≥0 | **already declined** (page D15). Relabel |
| **A25** | page/DCT | scaled decode refused for non-MCU-aligned images (crbug 890745); 4-component JPEGs never channel-reduced; CMYK inversion applied in the **colour space**, not the decoder | `libjpeg_scanline_decoder.cpp:117-123`, `:128-157`; `cpdf_devicecs.cpp:104-136` | §7.4.8 silent on scaling | `jpg.js:1292-1322` uses the APP14 `transformCode`; PDF-sourced CMYK inversion left to `/Decode` (`:1264-1284`) | **ROBUSTNESS** | 6 | keep. Note PDFium treats *any* Adobe marker as "transformed", ignoring the transform byte — a smaller BUG folded in here |
| **A26** | crypt | `/EFF` **never read anywhere** — embedded files decrypt as streams | `cpdf_security_handler.cpp:303-311`; `grep '"EFF"' core/ fpdfsdk/` → **zero hits** | §7.6.5 table 20 defines `/EFF` as a distinct default | **implements it**: `crypto.js:1120`, `:1206`, `:1336` | **BUG** | 0 | §3 (A26) |
| **A27** | crypt | a document whose `/StmF` and `/StrF` **name** different filters is **refused**; and because the check precedes the default, V≥4 with *neither* present also fails to load | `cpdf_security_handler.cpp:305`, `:325` — `if (stmf_name != strf_name) return false;` | §7.6.5: two independent entries, each defaulting to `Identity`; they **may** differ | `crypto.js:1116-1120` stores and consults them independently, applies the `Identity` default, no check | **BUG** | 2 | §3 (A27) |
| **A28** | crypt | `/Length` under 40 reinterpreted as **bytes** and multiplied by 8 | `cpdf_security_handler.cpp:271` | §7.6.3.2 table 20: bits, 40–128 — so `16` is non-conformant | `crypto.js:1092-1093` has the **same fixup**, commented as a producer bug | **ROBUSTNESS** | 7 | keep: both independents recover the same way |
| **A29** | crypt | AES padding **stripped without validation**; `back()==16` drops a whole block; `back()==0` emits all 16 as data | `cpdf_crypto_handler.cpp:217-224` — only `block_buf.back()` is inspected | §7.6.2 requires PKCS#5 padding | `crypto.js:436-441` **checks every pad byte**; on mismatch keeps all 16 (`psLen = 0`) | **ROBUSTNESS** | 7 | keep (crypt D6): reading a malformed file; both recover leniently |
| **A30** | crypt | a trailing **partial block is silently discarded** and the decrypt still reports success; a full block completing exactly at a chunk end is held back one call | `cpdf_crypto_handler.cpp:191`, `:217` | §7.6.2: ciphertext is always a multiple of 16 | pdf.js retains the tail in `this.buffer` but never emits it either | **ROBUSTNESS** | 7 | keep |
| **A31** | crypt | password retried in the **other encoding** (Latin-1↔UTF-8) when non-ASCII | `cpdf_security_handler.cpp:425-455` | §7.6.4.3.3 mandates **SASLprep** + UTF-8 + 127-byte truncation for R6 — PDFium does none | `sasl_prep.js:27` via `crypto.js:1142`; truncation `:896-897`; prepped-then-raw retry `:1178-1180` | **BUG** | 2 | §3 (A31) |
| **A32** | crypt | short `/O` out-of-bounds read (crbug.com/42270437) | **fixed at this pin**: `cpdf_security_handler.cpp:512-515` early-returns, `:532` clamps the copy | — | n/a | **SPEC** | — | nothing to do; our guard matches the fixed oracle |
| **A33** | filters | RunLengthDecode rejects at **20 MiB** (`>=`), hard | `fpdf_parser_decode.cpp:41`, applied `:277-280` (**not** in `core/fxcodec/`) | no size limit in the spec | **no cap** (`run_length_stream.js`, 57 lines) | **ROBUSTNESS** | 1 | keep (filters D2) |
| **A34** | filters | **no** output cap on Flate or LZW; `kMaxTotalOutSize` is an arithmetic clamp, not a limit | `flatemodule.cpp:58`, consumed only by `FlateGetPossiblyTruncatedTotalOut` | — | no cap | **ROBUSTNESS** | 0 | keep our 1 GiB cap — but see §6, the stated justification is wrong |
| **A35** | filters | a **starved scanline is zero-filled**, so a truncated image renders black to the bottom | `flatemodule.cpp:101` — `std::ranges::fill(dest_span.subspan(post_pos-pre_pos), 0);`, unconditional | spec silent | **opposite**: `flate_stream.js:286-289` sets `eof`, emitting a **short** buffer | **ROBUSTNESS** | ≥0 | keep (filters D3 obligation): malformed input, the oracle's recovery is the tie-break |
| **A36** | filters | LZW **freezes** the table at 4094 and keeps reading 12-bit codes (the early `return` at `:156-158` never touches `code_len_`); a **zero-byte decode is a failure** | `flatemodule.cpp:156-158`, `:306` `return dest_byte_pos_ != 0;` | §7.4.4.2 puts the clear-code duty on the **encoder**; nothing makes an empty decode an error | **no freeze** — `nextCode++` unconditional, runs off the typed arrays into garbage; empty decode fine | **ROBUSTNESS** | 3 | keep the freeze (safer than pdf.js). The zero-byte half is **UNKNOWN** — §5 |
| **A37** | parser | `VerifyCrossRefTable` spot-checks **one** entry (`break` at `:390`) and rejects the whole table; classic tables only | `cpdf_parser.cpp:384-385`, called `:443` under `xref_list.front() > 0` | spec silent on recovery | — | **ROBUSTNESS** | 1 | keep |
| **A38** | parser | the recursion budget is a **global non-atomic static** shared across nested parser instances | `cpdf_syntax_parser.cpp:79`, used `:541` | spec silent | — | **ROBUSTNESS** | ≥0 | keep the *shared-budget semantics* without the global (parser D4); already done |
| **A39** | parser | a node listed twice among one parent's `/Kids` is **counted twice**, and the inflated count is written back into the in-memory `/Count` | `cpdf_document.cpp:88-101`, `:110`; `scoped_set_insertion.h:30` erases on return | §7.7.3.2: the page tree is a **tree**; a repeated kid is not legal | **permanent** visited set, throws `FormatError` (`catalog.js:1327-1331`) | **ROBUSTNESS** | 2 | keep; see §5 |
| **A40** | text | a whitespace-only page yields **zero** characters (`crbug.com/40643656` says 1) | **root cause found**: `cpdf_textpage.cpp:881` and `:1076` reject a text object on glyph-**bbox** width (`cpdf_textobject.cpp:305-331`) rather than **advance** width; a space's bbox is empty | §9.4.3: `Tj` advances by `w0`, which for a space is non-zero; §9.2.2 keeps displacement and bbox distinct | drops it too, but by a different mechanism and **opt-outable**: `evaluator.js:3012` is bypassed by `keepWhiteSpace: true` | **BUG** | 1 | §3 (A40) |
| **A41** | text | soft hyphen at a line break is `U+0002` in `chars` and `U+FFFE` in `text` | `cpdf_textpage.cpp:1360-1361` — `set_unicode(0x2)` then `AppendChar(0xfffe)` | no spec basis; U+FFFE is a permanent Unicode **noncharacter**, forbidden in interchange (Unicode §23.7) | no sentinel: U+00AD normalised to `-` (`unicode.js:57-58`); join is a reversible query-time transform (`pdf_find_controller.js:131`, `:290-307`) | **BUG** (cosmetic) | 3 | §3 (A41) |
| **A42** | text | a word split by a hyphen **cannot be found by search** (`crbug.com/431824298`) | `cpdf_textpagefind.cpp:209-211`, `:262` search the text buffer containing `￾`; **`cpdf_linkextract.cpp:154-155` repairs the same sentinel and find does not** | §9.10 | joins across the break and finds it (`pdf_find_controller.js:302-307`) | **BUG** | 2 | §3 (A42) |
| **A43** | text | a space that should be generated is not (`crbug.com/444176962`, `"localact"`) | **not a heuristic failure**: there is a real space glyph (`bug_444176962.pdf` code `0x0005`, width 448/1000) killed by the same bbox gate as A40, `cpdf_textpage.cpp:1076-1078` | §9.4.3 requires the advance | keeps it two independent ways (`evaluator.js:3079-3084`; `:2924-2939`) | **BUG** | 1 | §3 (A43) |
| **A44** | text | characters **dropped** by the overlap/dedup logic (`crbug.com/42270780`, `"wo d wo d"`) | `cpdf_textpage.cpp:1437-1458` — a **7-entry** lookback (`:1438`) with a `0.07 × fontsize` threshold; `add_unicode = false` at `:1455` | no spec basis; §8.2 simply composites coincident glyphs | **no dedup at all** — grep for overlap/dedup in `evaluator.js` finds only font-state caching (`:3246`) | **BUG** | 2 (**both fail**) | §3 (A44) |
| **A45** | text | `CountRects` returns 12 where the TODO says 4 (`crbug.com/40448046`) | `cpdf_textpage.cpp:465` — the **only** split rule is `text_object != charinfo.text_object()`; no geometric merge exists | §9.10 | no rect-count API | **BUG** (cosmetic) | 2 | §3 (A45) |
| **A46** | text | link extraction slices a **text-buffer**-indexed string with **char-list**-indexed offsets | `cpdf_linkextract.cpp:123`/`:126` (char list) vs `:148` `page_text.Substr(start,nCount)`; PDFium **owns the converter it fails to call** — `CharIndexFromTextIndex`, `cpdf_textpage.cpp:409` | §12.5.6.5 defines `/Link` annotations; auto-detection is unspecified | `autolinker.js:147`, `:176-180` carries an explicit `diffs` reverse map — exactly the conversion PDFium skips | **BUG** | 4 | §3 (A46) |
| **A47** | text | `TrimBackwardsToChar` decrements a `size_t` past 0 | `cpdf_linkextract.cpp:77-79` — `for (size_t pos = *end; pos >= start; pos--)` | — | n/a | **ROBUSTNESS** | 4 | keep our `checked_sub` (text D6): the C++ path is UB |
| **A48** | text | every character above `U+FFFF` mirrors to `)` | `fx_unicode.cpp:48` returns **`0`** on out-of-range where the in-table "no mirror" sentinel is `0x1ff` (`kMirrorMax`, `:27`); `:147` tests for the sentinel and misses; `kFXTextLayoutBidiMirror[0] == 0x0029` (`:87-88`). `fx_ucddata.inc:41` shows index 0 belongs to `'('` | UAX #9 rule L4 mirrors only characters **possessing** `Bidi_Mirrored`; `BidiMirroring.txt` has **no** mappings above the BMP | **does not mirror at all**, with a correct rationale: `bidi.js:441` "don't mirror as characters are already mirrored in the pdf" | **BUG** | 2 | §3 (A48) |
| **A49** | text | `U+0093`–`U+0098` treated as control characters and stripped from `text` | `cpdf_textpage.cpp:136-144` | these are C1 controls in Unicode | n/a | **SPEC** | — | keep: they are controls by Unicode |
| **A50** | text | the hyphen path dereferences an **empty** container | text brief D4 | — | n/a | **ROBUSTNESS** | 0 | keep our decline (text D4); the C++ crashes and a crash writes no golden |
| **A51** | font | substitution yields weight **700** where the test's own TODO says 400 (`crbug.com/500640684`) | `cfx_fontmapper.cpp:644` — `if (nStyle == pdfium::kFontStyleNormal)` fails for an *italic* standard face, so `:645`'s reset to 400 never runs; a `!FontStyleIsForceBold(nStyle)` test would give 400 | §9.8.1 `/FontWeight`; Annex D makes `Helvetica-Oblique` a regular-weight face | weight is a property of the **resolved face**: `font_substitutions.js:32-35` `ITALIC = { style:"italic", weight:"normal" }`, `:129-133` | **BUG** | 2 | §3 (A51) |
| **A52** | font | `kOutOfSpecBFLimit = 160000` cap on ToUnicode sections; exceeding it **silently discards the whole section**, as does a count mismatch | `cpdf_tounicodemap.cpp:25-29`, applied `:187`, `:244` | §9.10.3 sets the limit at **100** — PDFium exceeds the spec by 1600× | no count cap; a **span** cap `MAX_MAP_RANGE = 2²⁴−1` that **throws** (`cmap.js:197-199`, `:226-258`), degrading to dropping the whole map | **ROBUSTNESS** | ≥0 | keep: strict-to-spec (100) would reject real files the oracle accepts |
| **A53** | font | GSUB vertical: only **single** substitutions; script-reachable set first with a whole-list fallback; a substitution to glyph 0 means "none" | `cfx_cttgsubtable.cpp:266-268`, `:91-93`, `:28-53`; the `optional` is flattened to `0` at `:82` and re-tested at `cpdf_cidfont.cpp:646` | OpenType (ISO/IEC 14496-22), not ISO 32000-1; GID 0 is a **legal** substitution target, and `vrt2` should supersede `vert` (PDFium iterates an unordered set) | **no GSUB at all** — vertical handled via CMap `WMode` and `/W2` | **ROBUSTNESS** | ≥0 | keep the policy. The glyph-0 sentinel collision is the same shape as A48 — see §5 |
| **A54** | font | unpaired surrogates survive to text output as lone `0xD800..DFFF` | font D3 | §7.9.2.2: a lone surrogate is malformed UTF-16 | JS strings are UTF-16, so they survive there too | **ROBUSTNESS** | 0 | keep our `U+FFFD` + diagnostic (font D3) |
| **A55** | doc/vt | `IsPunctuation`'s Latin-1 arm has `word <= 0x0094` in a chain of `==` tests, so **all of U+0080–U+0094** is punctuation and the six preceding equality tests are dead | `cpvt_section.cpp:84` | UAX #14: these are C1 controls; the *intended* characters include Lu letters (Œ, Š, Ž) which are not punctuation either | breaks **only at U+0020** (`annotation.js:3107-3134`); no punctuation rule at all | **BUG** | ≥0, up to 14 | §3 (A55) |
| **A56** | doc | `Bookmark::GetColor` dereferences null on a 3-element `/C` of non-numbers | `cpdf_bookmark.cpp:65-67` — the `size() != 3` guard at `:62` checks arity only, and `GetNumberAt` returns null for a non-number | §12.3.3 table 153: `/C` is three numbers | validates **type and arity**, falls back to black: `catalog.js:421-431` `isNumberArray(color, 3)` | **ROBUSTNESS** | 2 | keep our `None` (doc D2); the C++ crashes |
| **A57** | doc | font index 1 (the "system font") is permanently absent on Linux | `cpvt_fontmap.cpp:32-51` via `GetNativeFontName` (`cpdf_interactiveform.cpp:98-137`) **and** `AddNativeFont` (`:183-198`), both `#if BUILDFLAG(IS_WIN)`-only | — | n/a | **SPEC** | — | keep: platform fact. Note it is retried, not memoised, on every call |
| **A58** | doc | the `/Next` action chain has **no** depth cap (a `visited` set bounds it only by distinct-object count) | `cpdfsdk_formfillenvironment.cpp:1062-1098`, `:989-1019`, `:1021-1051` | — | pdf.js does not execute chained actions | **ROBUSTNESS** | 0 | keep our cap. **But the name-tree half of doc D4 is false** — see §6 |
| **A59** | doc | `--annot` writes `colour*255.f` into an `unsigned int`; a negative is **UB**, and `write.cc` then prints it with `%d` | `fpdf_annot.cpp:801-823`; `write.cc:420-423`, `:426` | — | n/a | **ROBUSTNESS** | 0 | keep our saturation (doc D14). To byte-match, the oracle emits `-2147483648` |
| **A60** | doc | wide-string output truncates at the first unpaired surrogate **and** at the first embedded NUL | `fx_string_testhelpers.cpp:48-66` — `push_back(ptr[0] + 256*ptr[1])` with no surrogate decoding; consumed by `%ls` at `write.cc:445`, `:450` | — | n/a | **ROBUSTNESS** | 0 | keep the truncation point (doc D10): it is the golden. Harness-only, not a library defect |
| **A61** | doc | `IsUpdateAPEnabled` is a **process-global, non-atomic** `static`, default `true`, saved/cleared/restored around one constructor | `cpdf_interactiveform.cpp:613-623`; written only at `cpdfsdk_pageview.cpp:596-600`; read at `cpdf_annotlist.cpp:209-213` | §12.7.2 table 218 puts the decision **per-document** in `/NeedAppearances`; PDFium's global is unrelated to it and keys on a *missing* `/AP` instead | reads it where the spec puts it: `annotation.js:210-211`, `:2270-2284` | **SPEC**→ see §5 | — | keep as an option defaulting to the oracle's value; the global-vs-per-document question is separable |
| **A62** | doc | `/UF` name-typed reads back as empty (the `crbug.com/959183` fix), then **falls through to `/F`** | `cpdf_filespec.cpp:103-113` — `ToString()` checked downcast, then `if (csFileName.IsEmpty())` | §7.11.3 table 44: `/F` and `/UF` are typed `string` | same `typeof item === "string"` guard (`file_spec.js:55`) but **no fall-through** (`:95-105` returns the first *present* key) | **SPEC** | 0 | keep: the oracle is right, and its fall-through is the more useful reading |
| **A63** | form | a **failing validate loses focus** — `CommitData` returns `true` on the revert path, so rejection is indistinguishable from success | `cffl_formfield.cpp:525-531` — `ResetPWLWindow(); return true;`; the caller's only guard is `:307`, so `:311 KillFocus()` and `:325 EscapeFiller()` run | §12.7.5.3: the Validate event's `event.rc` rejects the value; the field should retain focus so the user can correct it | **implements the rule, with the comment to prove it**: `event.js:277` `focus: true, // Stay in the field.` | **BUG** | 0 today | §3 (A63) |
| **A64** | form | tab-order row banding **spins forever** when every remaining annot has a non-positive top | `cpdfsdk_annotiterator.cpp:137` `float fTop = 0.0f;` + `:140` strict `>` + `:145-147` `continue` inside `while (!sa.empty())` with nothing erased | §12.5.5 | no `/Tabs` at all — DOM order via uniform `tabIndex = 0` | **BUG** | 2 | **already diverging correctly** (form D10). Relabel — §6 |
| **A65** | form | a row band's seed is the **rightmost** of the annots tied for topmost | `cpdfsdk_annotiterator.cpp:133` sorts left-ascending, `:138` scans high→low, `:140` strict `>` | §12.5.5 silent on ties | n/a | **ROBUSTNESS** | 2 | keep (M14): the tie-break is unspecified |
| **A66** | form | the column pass's "nothing chosen" guard is `fLeft < 0`, seeds **index zero** while recording `fLeft` from a **different** element, and **re-fires** while `left` stays negative | `cpdfsdk_annotiterator.cpp:170-180` | §12.5.5 | n/a | **BUG** (cosmetic) | 2 | §3 (A66). Note the column pass's own `continue` is **dead code** |
| **A67** | form | pressing Return on a **push button fires no action** | `fpdf_formfill_embeddertest.cpp:3658-3667` asserts `DoURIAction` `.Times(0)` and `ASSERT_FALSE(FORM_OnChar(...))`, both `TODO(crbug.com/1028991)`; the adjacent `LinkActionInvokeTest` (`:3670-3690`) asserts the **opposite** for links | §12.6.3 table 196: an annotation's `/A` is performed when it is **activated** | renders push buttons as `<a>` (`annotation_layer.js:2129-2137`), so the browser activates on Return | **BUG** | 2 | §3 (A67) |
| **A68** | form | document-wise Home/End gated on the **Control** bit on every platform | `fpdf_formfill_embeddertest.cpp:937`, `:965` — `TODO(448699368): This should work with the meta key on macos` (the sites are in the **tests**, not `cpwl_edit.cpp`) | — | n/a | **ROBUSTNESS** | 0 | keep: platform convention, no spec content |
| **A69** | form | `FORM_OnKeyUp` is a documented permanent no-op returning false | form D7 | — | n/a | **SPEC** | — | keep the omission |
| **A70** | form | the `.evt` mouse-verb arity guards are unsatisfiable, so a short mouse line reads out of bounds | form D8 | — | n/a | **ROBUSTNESS** | 0 | keep our skip+diagnostic (form D8); the C++ path is UB |
| **A71** | edit | the content generator emits only `rg`/`RG`; every other colour space is dropped | edit D5 | §8.6 defines all of them | pdf.js does not regenerate content | **BUG** (fidelity) | ≥0 | keep (edit D5): Tier-B scores us against the oracle's *regenerated* page, so a fix would score as a regression |
| **A72** | edit | two paths emit a prefix then bail, leaving `q` (and `BT`) unclosed | edit D6 | §7.8.2 requires balanced content | n/a | **ROBUSTNESS** | 0 | **already diverging correctly**. Relabel |
| **A73** | edit | four import-path bugs (hardcoded object 4, partial failure, source mutation, N-up XObject registration) | edit D13–D16 | — | n/a | **BUG** | 0 | **already fixed** (E10 ruling). Relabel |
| **A74** | edit | `Format("%010d", FX_FILESIZE)` truncates an `int64_t` through `%d` | edit D3 | §7.5.4 | n/a | **BUG** | 0 | **already fixed** (edit D3), unreachable on the corpus. Relabel |
| **A75** | cmap | `char_size` and `append_char` disagree for a sub-`0x100` code whose value is a lead byte under a mixed-two-byte scheme | cmap status "New, documented" | §9.7.5.2 codespace ranges | decodes by codespace range | **ROBUSTNESS** | 0 | keep: pinned, unreachable, and the original disagrees the same way |
| **A76** | doc/vt | `GetAutoFontSize` returns **4pt even when 4pt overflows** (`it == begin()`), and a multi-line field is hard-capped at **12pt** by `kQuarterSize` | `cpvt_variabletext.cpp:873-874`, `:861` | §12.7.3.3: auto-size shall be computed so the text fits | continuous closed form, no ladder, no floor, no multiline ceiling: `annotation.js:2694-2758` | **BUG** (minor) | ≥14 | §3 (A76). **Replaces doc D16, which described a different, non-existent defect** — §6 |

---

## 2. Counts by verdict

| verdict | count |
|---|---|
| **BUG** | **30** |
| ROBUSTNESS | 28 |
| SPEC | 13 |
| UNKNOWN | 5 |
| ACROBAT (as such) | 0 |
| **total rows** | **76** |

Of the 30 BUG rows, **eight are already implemented correctly** by pdfrum and
need only an `[oracle-bug]` relabel with both citations (§6). Twenty-two need
a ruling; five of those cost no golden at all.

Notably **no row classified as ACROBAT.** The one candidate — A63, a failing
validate losing focus — turned out to be contradicted by pdf.js at a
commented line, so it moved to BUG. The AF* negative-style-1 case PLAN.md
cites as the ACROBAT archetype is M15's and is not in Phases 1–3.

---

## 3. The BUG items, with the evidence

**A1 — `usecmap`, and `/UseCMap`, in an embedded CMap.**
`cpdf_cmapparser.cpp:61` is `} else if (word == "usecmap") {` with an **empty
body** — recognised only so it does not fall through into the data handlers,
then discarded; the referenced name sits unread in `last_word_`. A tree-wide
grep for `usecmap|UseCMap` under `core/fpdfapi/` returns that one line, so the
**dictionary `/UseCMap` key is unimplemented too** — both channels §9.7.5.3
sanctions. The parser runs only from the embedded-data constructor
(`cpdf_cmap.cpp:303-314`), so this is exactly the embedded case. pdf.js
implements both with the right precedence: `cmap.js:608-613` captures the
operator, `:639-648` lets an explicit `/UseCMap` win, and `extendCMap`
(`:650-669`) inherits the parent's codespace ranges when the child has none
and merges mappings under `if (!cMap.contains(key))` — child-wins, as the spec
requires. PDFium's result is not degradation but **silent total loss** of every
inherited code. **Zero rows** — no corpus file contains `usecmap`. We parse and
discard it with `DiagKind::CMapUsecmapIgnored` (`parser.rs:167`), pinned by
`usecmap_is_recognised_and_ignored`.

**A2/A3/A4 — the three sampled-function items**
(`cpdf_sampledfunc.cpp`). One 40-line function, three defects, all
pixel-visible through shadings and `Separation` colour; rule on them together.
A2: `:128` clamps *after* the `uint32_t` cast, so `-0.5` becomes ~4·10⁹ and
lands on the **top** cell, where §7.10.2 states `e'ᵢ = min(max(eᵢ,0),
Sizeᵢ−1)` on the real value and `function.js:231` does exactly that, landing
on cell 0. A3: `:163` seeds `encoded` from a **single** base sample and `:184`
adds `(eⱼ − ⌊eⱼ⌋)·(Sⱼ − S₀)` per axis — `m+1` samples read, never the `2^m`
corners, so every cross term is dropped and the result is the tangent plane at
the base corner; pdf.js builds the real hypercube with product weights
(`function.js:205-260`). A4: `:165-168` fires when `sizes == 1` and executes
`encoded = encoded_input[j] * sample` — an **assignment**, discarding every
axis already folded in, and a multiplication where the correct weight on a
one-element axis is 1; pdf.js's general path gives `n0=0, n1=1`, no scaling.
**≥29 rows** carry a `/FunctionType` visibly, and the true figure is higher
since shadings are usually compressed.

**A6 + A8 — two shading arithmetic defects.** A6:
`cpdf_rendershading.cpp:81` fills the ramp at `diff * i / kShadingSteps`,
dividing by **256** so `t_max` is never evaluated, while `:160` indexes with
`(int32_t)(scale * (kShadingSteps - 1))`, multiplying by **255** — a skew of
one part in 256, visible on smooth gradients. A8: `:220`
`bDecreasing = dr < 0 && static_cast<int>(hypotf(dx,dy)) < -dr` quantises the
centre distance to a whole device unit, so `dx=1.5, dr=-1.2` classifies as
decreasing where the true `1.5 < 1.2` does not. That flag selects between
quadratic roots (`:251-255`), so it changes *which* gradient is drawn, not
merely its shading. Neither has any basis in §8.7.4.5.4. Ported verbatim
(page D16; `radial.rs:95`). **≥42 rows each.**

**A9/A10/A11 — the three `/TR` items** (all in `cpdf_docrenderdata.cpp`).
A9: `:90` `pFuncs[2 - i] = Load(array[i])` while `:113-114` names `samples[0]`
as `samples_r`, so **`array[2]` drives red**. The consumption side was traced
end to end — there is no second reversal, so PDFium renders `/TR [fR fG fB]`
as `[fB fG fR]`, invisible whenever the three functions are equal, which is
why it has survived. Table 58 also specifies **four** functions; PDFium
requires `size() >= 3` and never reads `array[3]`. A10: `:124` stores
`FXSYS_roundf(output[0]*255)` into a `uint8_t` unclamped, so a `/Range`
admitting negatives folds its lower half onto the **top** of the byte range —
the type 4 fixture gives `-121.26` at `0xCC`, stored as `0x87` — and a
negative float→unsigned conversion is UB, so it is a bug twice over; §7.10.1
requires clipping to `/Range` and pdf.js clamps there (`function.js:265`).
A11: `:132-137` guards the `Call` but not the read, and since `OutputCount()`
is loop-invariant `output[0]` is never written at all, giving an **all-black**
curve — while the array branch at `:119-122` handles the identical condition
correctly with `samples[i][v] = v`, which marks the single-function path as an
oversight rather than a policy. **3 rows** — `transfer_function` ×2 (passing)
and `path_7.pdf` (already failing): the smallest non-zero radius of any
pixel-visible item here.

**A12 — non-isolated groups double-count the backdrop.**
`cpdf_renderstatus.cpp:673-679` fills a fresh bitmap with the page's current
pixels via `GetDIBits` whenever `!transparency.IsIsolated()`, hands it to
`CreateForNewBitmapWithBackdrop` (`:680-683`), and `:740-751` composites the
group back over the same pixels with **no removal step between** — a grep of
`core/` finds no implementation of §11.4.6's `C = Cn + (Cn−C0)×(α0/αgn − α0)`
at all. The backdrop is counted twice wherever group alpha < 1 or the blend is
non-Normal. pdf.js draws a non-isolated group **directly onto the parent
canvas** (`canvas.js:3205-3237`), arithmetically correct and commented as
such; it does log `"TODO: Fully support non-isolated non-knockout groups."`
(`:3243-3244`), but only on the slow path, and still does not double-count.
**≥11 rows**, one already failing.

**A13 — `/K` is never honoured.** The only `/K` read under `core/fpdfapi` is
CCITT's (`fpdf_parser_decode.cpp:330`). Knockout plumbing exists in
`core/fxge/`, but `RenderDeviceDriverIface::SetGroupKnockout` is an empty body
(`renderdevicedriver_iface.cpp:132`) and the AGG driver does not override it,
so on the oracle's configuration knockout is unimplemented. §11.6.6 table 147
defines `/K`; pdf.js reads it (`evaluator.js:523-524`) and implements it in
earnest (`canvas.js:499-534`, `:3310-3318`). Same blast radius as A12.

**A15 — the zero-length dash substitution.**
`cfx_agg_devicedriver.cpp:370-372`: `if (dash_len <= 0.000001f) dash_len =
0.1f;`. §8.4.3.6 makes a zero-length dash a **dot** under a round cap and
nothing under a butt cap; a 0.1-unit dash is neither. Distinct from A16, which
is legitimate DoS avoidance. **≥2 rows** (`dashed_lines`).

**A18 — `/ExtGState /Font` never resolves the specified form.**
`cpdf_allstates.cpp:87-89` calls `FindFont(font->GetByteStringAt(0))` — it
reads the array's first element **as a byte string** and looks it up in the
page's `/Font` resources. Table 58 specifies `[font size]` where *font* is an
**indirect reference to a font dictionary**. On a reference,
`GetByteStringAt(0)` yields `""`, the resource lookup fails, and
`cpdf_streamcontentparser.cpp:1239` substitutes stock Helvetica — so the
spec's own form silently produces the wrong font. Pinned by
`a_font_array_looks_its_name_up_in_the_resources` (`extgstate.rs:367`).
**Blast radius: ≥0** — no corpus file uses it. Very cheap to rule on.

**A26 — `/EFF` never read.** `grep '"EFF"' core/ fpdfsdk/` returns **zero
hits**; `LoadCryptInfo` takes one filter name and builds one
`CPDF_CryptoHandler`. §7.6.5 table 20 defines `/EFF` as the default for
embedded file streams, independent of `/StmF`. pdf.js carries it separately:
`crypto.js:1120`, used at `:1206` and `:1336`. We collapse
`CryptClass::Embedded` onto `Stream` deliberately (crypt D1). **Zero rows.**

**A27 — `/StmF ≠ /StrF` refused.** `cpdf_security_handler.cpp:305` and `:325`
both `return false` on a raw **name** inequality, so
`/StmF /StdCF /StrF /StdCF2` is rejected even when both `/CF` entries are
identical, and a conformant file with encrypted streams and plaintext strings
will not open. Because the check runs *before* the `Identity` default is
applied, a V≥4 document with **neither** entry present also fails to load,
where §7.6.5 says both default to `Identity`. pdf.js applies the defaults and
consults the two independently with no check (`crypto.js:1116-1120`).
Reproduced at `standard.rs:226-230`. **2 rows** (`bug_644`).

**A31 — password preparation.** `cpdf_security_handler.cpp:425-455` tries the
bytes as given and, only for a non-ASCII password, retries a Latin-1↔UTF-8
transcode whose direction depends on the revision. §7.6.4.3.3 (Algorithm 2.A)
requires **SASLprep** (RFC 4013), then UTF-8, then truncation to 127 bytes for
R6; PDFium implements none of the three, and a Latin-1↔UTF-8 transcode is not
PDFDocEncoding either (they differ across 0x80–0x9F). pdf.js has real SASLprep
(`sasl_prep.js:27` via `crypto.js:1142`), the truncation (`:896-897`), and its
own prepped-then-raw retry (`:1178-1180`). The two retry axes do not subsume
one another. **2 rows** (`encrypted_hello_world_r5`, `_r6`), both using ASCII
passwords — so the difference is unreachable on the corpus today.

**A40 + A43 — one root cause, two pinned bugs.** `cpdf_textpage.cpp:881`
and `:1076` reject a text object when
`fabs(text_obj->GetRect().Width()) < kSizeEpsilon`, and that rect is built
from the glyph **bounding box** (`cpdf_textobject.cpp:305-331`, `GetCharBBox`)
rather than the **advance** width — so any text object consisting only of
spaces vanishes before extraction. For A40 (`whitespace.pdf`, one `( ) Tj`)
that gives zero characters where `crbug.com/40643656` says one. A43 is more
interesting than the brief suggests: `bug_444176962.pdf` draws nine separate
`Tj`s, one per glyph, and the sixth is a **real space** (code `0x0005`,
ToUnicode U+0020, `/W` advance 448/1000) — so this is not the space
*generation* heuristic failing, it is an explicit space discarded as its own
single-glyph object. §9.4.3 requires the position to advance by `w0`, non-zero
for that space; using the bbox to decide the object exists is the divergence.
pdf.js keeps the character two independent ways (`evaluator.js:3079-3084` and
the positional `addFakeSpaces` at `:2924-2939`), and its whitespace drop is an
**opt-out** (`keepWhiteSpace: true`), not a loss. **1 row each**; the shared
fix addresses both.

**A41 + A42 — one design decision, two symptoms.**
`cpdf_textpage.cpp:1360-1361` writes `set_unicode(0x2)` into the CharInfo and
`AppendChar(0xfffe)` into the text buffer for the *same* position. Neither has
a spec basis, and U+FFFE is a permanent Unicode noncharacter forbidden in
interchange (Unicode §23.7). A42 follows: search runs over the text buffer
(`cpdf_textpagefind.cpp:209-211`, `:262` plain `Find`), so a hyphen-split word
can never match. What makes it a bug rather than a trade-off is the
**asymmetry** — `cpdf_linkextract.cpp:154-155` repairs the same sentinel
(`Replace(L"\xfffe", L"-")`) for link detection and search does not. pdf.js
keeps a real hyphen (`unicode.js:57-58`) and joins across the break at query
time with a reversible index map (`pdf_find_controller.js:131`, `:290-307`).
**A41: 3 rows. A42: 2 rows.**

**A44 — the overlap dedup drops characters.**
`cpdf_textpage.cpp:1437-1458` suppresses a character (`add_unicode = false`,
`:1455`) when an earlier entry within a **7-entry lookback** (`:1438`) shares
its char code and font and sits within `0.07 × fontsize` on both axes.
`bug_1769.in` draws one form XObject twice, at 1000× and 1×; in the 1×
instance the word collapses below the threshold and `r` and `l` fall inside
the window, giving `"wo d wo d"`. The spec has no notion of duplicate-glyph
suppression — §8.2 composites coincident glyphs, and drawing one twice is how
faux-bold is done. pdf.js has **no dedup at all** (the only "identical" check
in `evaluator.js` is font-state caching, `:3246`), so it duplicates where
PDFium deletes: neither matches the eye, but deletion is unrecoverable.
**2 rows, both already failing** — we keep the glyphs the oracle drops, so
this is a request to re-derive the golden, not to change code.

**A45 — `CountRects` splits per text object.** `cpdf_textpage.cpp:465`: the
**only** split rule is `text_object != charinfo.text_object()`, and `Union`
(`:477`) accumulates within one object — no adjacency, baseline or font-size
test exists. `bigtable_mini.in` alternates `/F2` and `/F1` on one line at
constant size, so each `Tj` is its own object and its own rect: 12. Note for
whoever implements it that the TODO wants **4**, not 1 — the intended
predicate merges runs per font, which must be designed rather than inferred.
**2 rows.**

**A46 — the link-extraction index mismatch.** `cpdf_linkextract.cpp:123`
and `:126` walk the **char list** (`CountChars`, `GetCharInfo`) while `:148`
cuts the candidate out of the **text buffer** (`page_text.Substr(start,
nCount)`). The two spaces diverge whenever a character is in one and not the
other — `AddCharInfo` (`cpdf_textpage.cpp:783-786`) pushes a non-normal char
into `char_list_` without touching `text_buf_`, and the normalization path at
`:808-813` appends several chars for one input. PDFium **owns the converters
that would fix it** — `CharIndexFromTextIndex` (`cpdf_textpage.cpp:409`) and
its inverse — and `ExtractLinks` calls neither. It is observable: those
offsets flow unchanged to `FPDFLink_GetTextRange` (`fpdf_text.cpp:599`), whose
header documents them as **char** indices. Two further consequences in the
same function — `nCount--` at `:165` decrements a text-buffer-derived count
against a char-list `start`, and the `\xfffe` replacement at `:155` changes
the length relative to `nCount`. The verdict rests on internal inconsistency,
pdf.js having no comparable auto-linker; but `autolinker.js:147`, `:176-180`
carries exactly the reverse map PDFium skips. **4 rows.**

**A48 — supplementary characters mirror to `)`.** The sentinel collision is
exact and verifiable in the data: `fx_unicode.cpp:48` returns **`0`** for any
`wch >= 0x10000`, while the in-table "no mirror" value is `0x1ff`
(`kMirrorMax`, `:27`) — so `GetMirrorChar`'s sentinel test at `:147`
(`idx == kMirrorMax`) misses, and `idx = 0` indexes
`kFXTextLayoutBidiMirror[0] == 0x0029` (`:87-88`). `fx_ucddata.inc:41` shows
index 0 legitimately belongs to `'('`. U+0000 itself is unaffected — it is
in-table with the proper `0x1ff`. UAX #9 rule L4 mirrors only characters
*possessing* `Bidi_Mirrored`, and `BidiMirroring.txt` has **no** mappings
above the BMP, so the correct answer for every input PDFium mishandles is the
identity. Reachable from `cpdf_textpage.cpp:1000` under `if (is_rtl)`, so any
RTL run containing an astral character (Cypriot, Rumi numerals, astral math
alphanumerics) is corrupted. pdf.js declines to mirror at all and explains why
at `bidi.js:441` — "characters are already mirrored in the pdf" — which is the
right call for extraction. A one-line fix exists (`return kMirrorMax <<
kMirrorBitPos;` at `:48`). **2 rows** (`hebrew_mirrored`); real exposure is
wider than the corpus shows.

**A51 — substitution weight 700.** Tracing `"Arial-ItalicMT"` at weight 700
with `kFontUseExternAttr`: `GetSubstName` canonicalises to
`Helvetica-Oblique`; `:519-522` does not reset the weight because the extern
flag is set; `:558-561` gives `nStyle = kFontStyleItalic`; `:583-586` leaves
weight at 700 since ForceBold is not set; and the fix-up that would correct it
fails at **`cfx_fontmapper.cpp:644`** — `if (nStyle ==
pdfium::kFontStyleNormal)` is false for an italic face, so `:645`'s reset to
400 never runs. The test conflates "has no style" with "has no *bold*"; a
`!FontStyleIsForceBold(nStyle)` test would give the 400 the TODO asks for, and
Annex D makes `Helvetica-Oblique` a regular-weight face. pdf.js gets it right
structurally rather than by luck: weight is a property of the **resolved
face**, `font_substitutions.js:32-35` defining `ITALIC = { style: "italic",
weight: "normal" }` and `:129-133` binding `Helvetica-Oblique` to it, with a
name-suffix fallback at `:783-789` reaching the same answer. **2 rows**
(`font_weight`).

**A55 — `IsPunctuation`'s `<= 0x0094`.** `cpvt_section.cpp:84` reads
`word == 0x0093 || word <= 0x0094 || word == 0x0096` — a `<=` inside a chain
of `==` tests, making the six preceding equality tests dead and classifying
**all 21 code points U+0080–U+0094** as punctuation. Those are C1 controls in
Unicode, and the characters actually *intended* (the cp1252 mappings) include
Œ, Š and Ž, which are `Lu` **letters**. Line-break opportunities therefore
move for any Latin-1 text in variable-text layout — every form field and
free-text annotation. Reproduced at `classify.rs:125` and pinned. pdf.js
breaks only at U+0020 (`annotation.js:3107-3134`), so both diverge from UAX
#14 in different directions. **≥0 up to 14 rows** (`text_form*`), but only for
text containing U+0080–U+0094 — the effective radius is plausibly zero and
should be measured before ruling.

**A63 — a failing validate loses focus.** The mechanism is worse than "the
focus path forgets to check": `cffl_formfield.cpp:525-531` runs
`ResetPWLWindow(pPageView); return true;` when `OnValidate` refuses, so
`CommitData` reports **success** — its only `false` returns (`:516`, `:522`,
`:527`) mean "the widget was destroyed". `KillFocusForAnnot`'s sole guard is
`:307`, which therefore cannot see a rejection, so control reaches `:311
KillFocus()` and `:325 EscapeFiller()`: a user whose value a script rejected
loses both the value and the caret. §12.7.5.3 gives `event.rc` as the
rejection mechanism and expects the field to retain focus; **pdf.js implements
exactly that, with the comment to prove it** — `event.js:277`,
`focus: true, // Stay in the field.` (it clears rather than reverts the field,
a separate divergence). That closes the question the brief left open: our
`commit.rs:14-29` calls the behaviour "not what other viewers do" and the
independent implementation confirms it, so this is **BUG**, not ACROBAT.
**Zero rows today** — the branch is unreachable until M15 wires scripts, which
makes it the ideal item to rule on before it becomes load-bearing.

**A66 — the column-pass seed.** `cpdfsdk_annotiterator.cpp:170-180`:
`if (fLeft < 0) { nLeftTopIndex = 0; fLeft = rcAnnot.left; }`. Three defects
in three lines — the "nothing chosen yet" question is asked as `fLeft < 0`,
conflating it with "the best so far has a negative left"; the index seeded is
**0** while `fLeft` is taken from `sa[i]`, a *different* element (on the first
pass `i == size-1`, so the chosen index is the topmost annotation and the
recorded left belongs to the bottom-most); and because `fLeft` may itself stay
negative, the guard **re-fires** on later iterations. §12.5.5 defines `/Tabs
C` as column order and specifies no seed rule, but an order computed from a
mismatched index/coordinate pair has no geometric meaning. One useful
side-finding: because this guard always fires on the first iteration, the
column pass's own `continue` at `:182-184` is **dead code** — only the row
pass can hang. **2 rows** (`annotiter`), and only for pages with negative x.

**A67 — Return on a push button fires nothing.**
`fpdf_formfill_embeddertest.cpp:3658-3667` asserts `DoURIAction` `.Times(0)`
and `ASSERT_FALSE(FORM_OnChar(..., kReturn, 0))`, both marked
`TODO(crbug.com/1028991)` saying they should be one and true. The adjacent
`LinkActionInvokeTest` (`:3670-3690`) asserts the **opposite** for a link
annotation — `.Times(4)` and `ASSERT_TRUE` — so Return-activates-URI is
implemented for links and simply missing for push-button widgets
(`CFFL_PushButton` has no `OnChar` override, where `CFFL_TextField` does at
`cffl_textfield.cpp:116-140`). §12.6.3 table 196 performs an annotation's `/A`
when it is *activated*, and a keyboard activation of a focused button reached
by tab navigation is an activation. pdf.js gets it free by rendering push
buttons as `<a>` elements (`annotation_layer.js:2129-2137`). **2 rows.**

**A76 — auto font size floor and ceiling.** Two real defects, neither the
one doc D16 describes (§6). `cpvt_variabletext.cpp:873-874`: when
`lower_bound` lands on `begin()` — every step overflows, the smallest
included — the function returns `*it`, i.e. **4pt on a plate too small for
4pt**. And `:861`, `span_size = IsMultiLine() ? kQuarterSize : kFullSize` with
`kQuarterSize = 25/4 = 6`, caps a multi-line field at **12pt** however large
its box. §12.7.3.3 asks the size be computed so the text fits; the 25-step
ladder has no spec basis, and pdf.js solves it continuously —
`min(height/LINE_FACTOR, width/textWidth)` for one line
(`annotation.js:2707-2709`), growing `numberOfLines` for multiline
(`:2748-2758`) — with no floor and no ceiling. Reproducing the ladder is
required for parity; the two boundary behaviours are the arguable part.
**≥14 rows** (`text_form*`).

**A71 — the content generator's colour loss.** Regenerating a page emits only
`rg`/`RG`; §8.6 defines the rest. Recorded as BUG so the record is honest,
with the recommendation to **keep reproducing**: Tier-B compares our
regenerated page against *the oracle's regenerated page*, so implementing the
other colour spaces would score as a regression against the thing we are
measured on.

---

## 4. Where pdf.js is silent

Named explicitly rather than read as agreement. pdf.js does **not** implement:
automatic web/mail link extraction from page text as PDFium does it (A46 — it
has `autolinker.js`, but over a single flat index space, which is why it is
cited as the reference design rather than as a matching behaviour); the
`.evt` event model and `/Tabs` tab order (A64–A68 — `grep '"Tabs"' src/`
returns nothing; pdf.js gives every widget `tabIndex = 0` and lets the DOM
decide, which is PDFium's `kStructure` applied unconditionally); page content
**regeneration** (A71, A72); a rect-count API (A45); GSUB vertical
substitution (A53 — no GSUB parser exists in `src/core/`; vertical writing is
handled through CMap `WMode` and `/W2` alone); the mesh-shading bbox helper
(A23) and the pattern cache (A24), which have no counterpart in a
canvas-backed renderer; and per-pixel radial shading (A7 — it delegates to
`createRadialGradient`, so it inherits correct root selection from the browser
without implementing §8.7.4.5.4 itself).

Where pdf.js *is* cited, it was read at the line. Two rows where that changed
the verdict: A63, where `event.js:277` carries a literal
`focus: true, // Stay in the field.`, and A5, where `function.js:180-185`
shows pdf.js ignoring `/Order` exactly as PDFium does — which is why A5 is
SPEC rather than BUG.

---

## 5. The UNKNOWN items, and what would settle them

**A7 — the radial solver's under-guarded branches.** Confirmed uneven:
branch 1 (`b ≈ 0`, `:236-237`) has no discriminant check, no root selection,
no radius check and no `a == 0` guard, so `sqrt(-c/a)` can be NaN; branch 2
(`a ≈ 0`, `:238-239`) has no radius check; only branch 3 (`:240-259`) is
complete. What is *not* settled is reachability: branch 1's NaN needs `b ≈ 0`
and `a ≈ 0` simultaneously, a degenerate geometry, and the NaN then casts to
`INT_MIN` at `:260` and takes the `index < 0` path — degrading to the
start-extend colour rather than crashing. So the observable damage may be nil.
**What would settle it:** instrument our own port (which already carries all
three branches, `radial.rs`) over the ≥42 shading rows, recording which branch
each pixel takes and whether any yields a non-finite parametric value. Half a
day, and the data is one counter away.

**A36(b) — LZW zero-byte decode treated as failure.**
`flatemodule.cpp:306` returns `dest_byte_pos_ != 0`, so a stream that
legitimately decodes to zero bytes fails the whole decode. §7.4.4 does not
make an empty result an error and pdf.js accepts it. But the failure
propagates to `std::nullopt` and the caller falls back to the raw bytes, which
for a zero-length stream is also empty — so the observable difference may be
nil. **What would settle it:** a one-object PDF whose content stream is an
empty LZW stream, compared `--txt`/`--png` against the oracle. Cheap; do it
before spending a ruling.

**A39 — a duplicate kid counted twice.** §7.7.3.2 specifies the page tree as a
*tree*, so a repeated kid is malformed and the spec is silent on recovery.
PDFium counts it twice and **writes the inflated count back** into the
in-memory `/Count` (`cpdf_document.cpp:110`); leaves are worse, never entering
`visited_pages` at all, so a page listed twice in one `/Kids` is counted twice
with no guard. pdf.js's set is permanent and it **throws**
`FormatError("Pages tree contains circular reference.")`
(`catalog.js:1327-1331`). Ours reproduces PDFium, pinned by
`a_subtree_listed_twice_counts_twice` — `no_page_count.pdf` reports six pages,
not three. Refusing to open the file is arguably worse for a library, which is
why this stays ROBUSTNESS. **What would settle it:** whether any real producer
emits a shared page subtree deliberately (an N-up or booklet imposition
would); if so, six is the answer a user wants.

**A53(b) — GSUB substitution to glyph 0.** The same sentinel-collision shape
as A48, and notable because PDFium *had* the right type and discarded it: the
internal chain is `std::optional`-typed (`cfx_cttgsubtable.cpp:85`, `:104`),
`GetVerticalGlyph` flattens `nullopt` to `0` at `:82`, and
`cpdf_cidfont.cpp:646` then cannot distinguish a genuine substitution to
`.notdef` from a miss. GID 0 is a legal OpenType substitution target. But no
real font deliberately maps a vertical form to `.notdef`, so the practical
exposure may be zero. **What would settle it:** a scan of the corpus's CJK
fonts for a `vert`/`vrt2` single-substitution whose target is GID 0. Related:
PDFium iterates `feature_set_` as an **unordered set** (`:75`), so `vert` and
`vrt2` compete on iteration order where OpenType expects `vrt2` to win —
worth measuring in the same pass.

**A61 — `IsUpdateAPEnabled` as a process global.** Classified SPEC because
our side already models it as an option defaulting to the oracle's effective
value, so nothing observable turns on it. But two things about the oracle are
worth a ruling: the flag is a **non-atomic mutable static** with a
save/clear/restore around one constructor (`cpdfsdk_pageview.cpp:596-600`), so
any concurrent `CPDF_AnnotList` construction silently loses AP generation; and
it is **unrelated to `/NeedAppearances`** — `cpdf_annotlist.cpp:209-213`
regenerates on a *missing* `/AP`, where §12.7.2 table 218 puts the decision
per-document in the AcroForm dictionary and pdf.js reads it there
(`annotation.js:210-211`, `:2270-2284`). **What would settle it:** whether any
corpus file sets `/NeedAppearances true` and would render differently if we
honoured it.

---

## 6. Already correct-and-diverging, and five stale records

### Eight sites to relabel `[oracle-bug]` retroactively

No code change. These are places pdfrum already implements the correct
behaviour where the record calls it a "divergence" without naming the oracle's
defect.

| id | site | what we already do |
|---|---|---|
| **A64** | `crates/pdfrum-form/src/tab.rs:249` | banding terminates where `cpdfsdk_annotiterator.cpp:145-147` spins forever — verified: `fTop` starts at `0.0f`, the test at `:140` is a strict `>`, and the `continue` at `:146` skips the only `erase`, so loop state is bit-identical on re-entry. This is M14's ruling and PLAN.md §226 names it as the precedent the new rule generalises, so it should be the first to carry the label. |
| **A11** | `pdfrum-page` transfer | identity where `cpdf_docrenderdata.cpp:132-137` reads an unwritten `output[0]` (page D7) |
| **A23** | `pdfrum-page` shading bbox | the correct bbox where `cpdf_streamcontentparser.cpp:94-123` mutates its loop invariants (page D18) |
| **A24** | `pdfrum-page` pattern cache | keyed `(ObjRef, parent_matrix)` where `cpdf_docpagedata.cpp:388` keys on identity alone (page D15) |
| **A72** | `pdfrum-edit` content generator | an unrepresentable object contributes nothing rather than an unclosed `q` (edit D6) |
| **A73** | `pdfrum-edit` import | the four import-path bugs fixed, not ported (E10), each pinned in `tests/import.rs` |
| **A74** | `pdfrum-edit` writer | offsets from `u64` where `%010d` truncates an `int64_t` (edit D3) |
| **A50** | `pdfrum-text` hyphen path | we decline the empty-container dereference (text D4); the C++ crashes |

### Five stale records — the docs are wrong, not the code

Found while verifying. Each should be corrected in its source document.

1. **`docs/design/pdfrum-doc.md` D16 describes a defect that does not exist.**
   It says `GetAutoFontSize` "returns the step *before* the first fitting one,
   i.e. a size that overflows the plate". It does not:
   `cpvt_variabletext.cpp:865-867` runs `lower_bound` with the comparator
   `!IsBigger(font_size)` over an **ascending** ladder, so `it` lands on the
   first size that *overflows*, and `:876` returns `font_span[it - begin - 1]`
   — the largest that **fits**. Our code was corrected on 2026-08-29
   (`autosize.rs:12-22` records the correction and the damage: every
   auto-sized field was being set at 4). D16 should be **struck and replaced
   by A76**, which names the two defects that are really there — the `4pt
   floor` at `:873-874` and the `12pt multiline ceiling` at `:861`.
   `docs/status/pdfrum-doc.md` repeats D16 and should be corrected with it.
2. **`docs/design/pdfrum-doc.md` D4 is half wrong.** It calls the name tree
   and the `/Next` chain both "unbounded recursion in the oracle". The **name
   tree has a cap**: `cpdf_nametree.cpp:26`
   `constexpr int kNameTreeMaxRecursion = 32;`, enforced at all five recursive
   entry points (`:67`, `:112`, `:241`, `:358`, `:417`) *plus* an
   object-number cycle set. Our cap is also 32, so we agree with the oracle by
   coincidence rather than by the stated reasoning — and pdf.js caps at **10**,
   iteratively (`name_number_tree.js:91-97`), so a tree 11–32 levels deep
   resolves in PDFium and ours and fails in pdf.js. The `/Next` half **is**
   right: no depth cap, only a `visited` set
   (`cpdfsdk_formfillenvironment.cpp:1069-1074`), bounding depth by
   distinct-object count.
3. **`docs/design/pdfrum-filters.md` D2's premise about `kMaxTotalOutSize`.**
   It reads as though 1 GiB is "`kMaxTotalOutSize`'s effective ceiling".
   `flatemodule.cpp:58` is a **saturation clamp** consumed only by
   `FlateGetPossiblyTruncatedTotalOut` for overflow-safe pointer arithmetic;
   nothing compares a decoded size against it and nothing rejects. PDFium has
   **no** Flate/LZW output cap at all. Our 1 GiB decision still stands and is
   strictly safer; only the justification needs rewording. Note the asymmetry
   this exposes: RunLength is capped at 20 MiB and Flate — with a far higher
   achievable expansion ratio — is not capped at all.
4. **The RunLength cap's location.** `docs/design/pdfrum-filters.md` and the
   crate docs place `kMaxStreamSize` in `core/fxcodec/`. It is in
   `core/fpdfapi/parser/fpdf_parser_decode.cpp:41`, applied at `:277-280`, and
   it is a hard rejection (`>=`, returning `FX_INVALID_OFFSET`) of the whole
   stream rather than a truncation.
5. **`docs/design/pdfrum-page.md` D19 misstates the colour-key `/Mask` case.**
   It says a short `/Mask` leaves `color_key_min/max` *uninitialized* and
   justifies our `0/0` as "a difference from *undefined* behaviour". The values
   are **deterministically zero**: `comp_data_` is a
   `std::vector<DIB_COMP_DATA>` sized by `resize()` (`cpdf_dib.cpp:407`),
   value-initialising a POD aggregate (`cpdf_dib.h:27-32`). Our `0/0` matches
   the oracle exactly — but the shared behaviour is worse than "arbitrary": the
   effective range is `[0,0]` on every component, so per
   `IsPixelColorKeyRangeOutOfBounds` (`cpdf_dib.cpp:103-110`) **every all-zero
   pixel is masked out** — blacks silently turn transparent on any malformed
   `/Mask`. §8.9.6.4 requires `2 × n` integers, so the correct handling rejects
   the array and leaves `color_key_` false. A22 is ROBUSTNESS on the strength
   of matching the oracle, but the reasoning must be replaced and the ruling is
   arguably BUG.

---

## 7. Prioritised BUG list — smallest, safest first

Ordered so each can be ruled on independently. "Rows lost" is the number of
scoreboard rows that would move from **pass** to
**not-achievable-by-construction**.

| # | id | behaviour | rows lost | why here / caveat |
|---|---|---|---|---|
| 1 | **A64, A11, A23, A24, A72, A73, A74, A50** | the eight already-correct sites | **0** | pure relabelling; no code, no goldens. Do these regardless. |
| 2 | **A1** | `usecmap` and `/UseCMap` in embedded CMaps | **0** | no corpus file reaches it; the static chain is already implemented so the fix is a merge over the same data; pdf.js is unambiguous and PDFium's loss is total, not partial. |
| 3 | **A26** | `/EFF` | **0** | no corpus file has it; the `Embedded` variant already exists in our enum with a comment reserving it. Pure addition. |
| 4 | **A18** | `/ExtGState /Font`'s indirect form | **0** | no corpus file uses it; try resolving a reference before falling back to the resource-name lookup, keeping the fallback for the oracle's form. |
| 5 | **A63** | a failing validate loses focus | **0** | unreachable until M15 wires scripts, so it costs nothing *now* and becomes load-bearing later. pdf.js settles it at a commented line. Rule on it before it has a blast radius. |
| 6 | **A31** | password SASLprep / 127-byte truncation | **0** | both R5/R6 rows use ASCII passwords so the corpus cannot tell; keep PDFium's transcode retry as a second candidate. |
| 7 | **A9 + A10** | `/TR` array order and the unclamped sample | **2** | `transfer_function.in`/`.pdf`. Self-contained, table 58 is explicit, and A11 is already declined — ruling here completes `/TR`. |
| 8 | **A40 + A43** | the glyph-bbox object gate | **2** | one root cause, two upstream bugs, two rows (`whitespace.pdf`, `bug_444176962.pdf`). The fix is to gate on advance width; §9.4.3 is explicit. Best value-per-row on the list. |
| 9 | **A66** | column-pass seed index zero | **2** | `annotiter`; only pages with negative x reach the guard, so the effective loss is probably 0. Measure first. |
| 10 | **A67** | Return on a push button | **2** | one-line branch; upstream itself calls it wrong, and the sibling link test asserts the opposite behaviour. |
| 11 | **A48** | supplementary characters mirror to `)` | **2** | `hebrew_mirrored`. UAX #9 is unambiguous, pdf.js declines to mirror at all, and the oracle-side fix is one line. |
| 12 | **A42 + A41** | the hyphen sentinel and unfindable split words | **2 + 3** | rule together; A42 alone is defensible as "repair the sentinel in find as link-extract already does", which is a smaller change than removing the sentinel. |
| 13 | **A45** | `CountRects` splits per text object | **2** | note the TODO wants **4**, not 1 — the merge predicate is per-font runs, which must be designed rather than inferred. |
| 14 | **A46** | link-extraction index mismatch | **4** | `weblinks` ×2, `weblinks_across_lines` ×2. No independent implementation to check against; the verdict rests on PDFium indexing one array with another's offsets while owning the converter. |
| 15 | **A15** | zero-length dash → 0.1 | **≥2** | `dashed_lines`; pixel-visible and §8.4.3.6 is explicit about the dot. |
| 16 | **A76** | auto-size 4pt floor and 12pt multiline ceiling | **≥0 up to 14** | the ladder itself must stay for parity; only the two boundary behaviours are arguable. Measure how many `text_form*` rows actually auto-size. |
| 17 | **A55** | `IsPunctuation`'s `<= 0x0094` | **≥0 up to 14** | **measure the effective radius first** — it needs text containing U+0080–U+0094 and is plausibly zero, which would move it up to position 5. |
| 18 | **A27** | `/StmF ≠ /StrF` refused | **2** | `bug_644`. Accepting differing filters means also applying the `Identity` default for absent entries — two behaviour changes in one function. |
| 19 | **A6 + A8** | shading LUT off-by-one; radial integer truncation | **≥42 at risk** | Tier-B pixel changes across every shading file. The LUT skew is one part in 256 so most rows should stay inside SSIM — but this is the first item whose radius is a *measurement*, not a count. Run it before ruling. |
| 20 | **A2 + A3 + A4** | the three sampled-function items | **≥29 at risk** | one function, three defects. A3 (multilinear) is the largest single behavioural change on this list — it alters every multi-input sampled function everywhere. All three together or none. |
| 21 | **A12 + A13** | non-isolated backdrop double-count; `/K` unhonoured | **≥11 at risk** | both touch the compositing core, and A12 interacts with render D6's collapse of the five-armed compositor into one arm. Highest risk, lowest urgency: last. |
| — | **A44** | text overlap dedup drops characters | **0 lost** | both `bug_1769` rows **already fail** — we keep the characters the oracle drops. This is a request to re-derive the golden, not to change code. Handle separately. |
| — | **A71** | regenerated-page colour loss | **n/a** | keep reproducing: Tier-B scores us against the oracle's own regenerated page, so a fix reads as a regression. Revisit only if the scoring changes. |
| — | **A22** | colour-key `/Mask` masks blacks | **≥0** | our output already matches; only §6's stale justification needs replacing. Reclassify to BUG if the user wants malformed `/Mask` rejected outright. |

**Recommended first tranche** (zero rows lost, six independent rulings):
items 1–6 — the eight relabels, `usecmap`, `/EFF`, `/ExtGState /Font`, the
validate-focus rule, and password preparation. Together they cost no golden,
close six `[oracle-bug]` sites, and settle A63 while its blast radius is still
zero.
