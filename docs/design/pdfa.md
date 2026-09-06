# PDF/A — the checker, and what it does not claim

`crates/pdfrum-doc/src/pdfa/`, M26 parts 1 and 2. What `check_pdfa` looks at,
what it does not, and how we know — which is veraPDF, scored over the corpus
with the disagreements written down rather than tuned away.

## 1. Report before repair

M26 has four parts and this pass implements the first two. There is no
`to_pdfa` and no conversion policy, deliberately: the roadmap orders them that
way because a converter built on an unexamined checker inherits every one of
its mistakes and hides them behind a file that now claims to be archival. A
checker nobody has cross-examined is a claim; one scored against an
independent implementation is a result.

So the shipped surface is one method that only reads:

```rust
let report = doc.check_pdfa(PdfaLevel::A2b);
for violation in &report.violations {
    println!("{} — {}", violation.clause.iso(report.level), violation);
}
```

## 2. Where the code lives, and why it took no feature flag

`pdfrum-doc`, next to the annotation model, the AcroForm reader and the
structure tree. Three reasons, in the order they decided it:

- **Every input is already there.** The checks read the object graph:
  `/FontDescriptor`, `/OutputIntents`, `/Annots` and its flag word, the
  `/ExtGState` and `/ColorSpace` resource dictionaries, the `/Metadata`
  stream. `pdfrum-doc` already depends on `pdfrum-object`, `pdfrum-filters`
  and `pdfrum-page`, which is the entire input set. A new crate would have
  taken the same dependencies and added an edge.
- **The vocabulary is already there.** `AnnotFlags` is `pdfrum-doc`'s type,
  and the annotation-flag requirement is one line of it. Re-deriving the flag
  word in a new crate to avoid a dependency, or depending on `pdfrum-doc` from
  a new crate to reuse it, both cost more than a module.
- **It carries no dependency, so there is nothing to gate.** The facade's
  feature flags — `markdown`, `svg`, `svg-ingest`, `javascript`, the codecs —
  each exist because the subsystem behind them adds crates a caller who never
  uses it should not compile. This one adds none. `cargo tree` over a default
  `pdfrum` is byte-identical before and after. So `check_pdfa` is in the
  default set, and the rule the other features follow is satisfied by not
  needing one.

**No XMP dependency was taken**, and none is requested. See §5.

## 3. What is checked

`Clause` has one variant per requirement enforced. The table is the
requirement set, not a sample of it.

| Clause | What it means | How it is found |
|---|---|---|
| `Encrypted` | PDF/A forbids encryption: an archive must open without a key | `/Encrypt` in the trailer |
| `FontNotEmbedded` | every font's program must be in the file | no `/FontDescriptor`, or no `/FontFile{,2,3}` on it; Type 3 exempt, Type 0 checked through its descendant |
| `FontSubsetIncomplete` | **A-1 only:** a CIDFont subset must declare its coverage | an `ABCDEF+` base name on a Type 0 font whose descriptor has no `/CIDSet`. A-2's counterpart is conditional and unimplemented (§4) |
| `JavaScript` | no script, anywhere | a `/JavaScript` name tree, or a `/JavaScript` action in `/OpenAction`, an `/AA`, or an annotation |
| `ForbiddenAction` | no `/Launch`, `/Sound`, `/Movie`, `/ResetForm`, `/ImportData` | the same action walk, including `/Next` chains |
| `EmbeddedMultimedia` | no embedded audio or video | `/Movie`, `/Sound` or `/Screen` annotations |
| `AnnotationSubtypeForbidden` | subtypes outside the permitted table | `/FileAttachment`, `/3D`, `/RichMedia` |
| `AnnotationFlagsIllegal` | flags must be legal | `/Hidden`, `/Invisible` or `/NoView` set, or `/Print` clear; `/Popup` exempt |
| `AnnotationAppearanceMissing` | an annotation must draw the same way everywhere | no `/AP` `/N`; `/Link` and `/Popup` exempt |
| `XmpMissing` / `XmpMalformed` | an XMP packet, and one that is RDF | the catalog's `/Metadata`, decoded |
| `XmpIdentificationMissing` / `Mismatch` | the packet must say which PDF/A this is | `pdfaid:part` and `pdfaid:conformance` against the level asked for |
| `XmpInfoMismatch` | the two metadata stores must agree | `/Title`, `/Author`, `/Creator`, `/Producer`, `/Keywords` against `dc:title`, `dc:creator`, `xmp:CreatorTool`, `pdf:Producer`, `pdf:Keywords` |
| `OutputIntentMissing` / `ProfileMissing` | an `/OutputIntents` array that exists must hold a usable PDF/A intent | an array with no `S` of `GTS_PDFA1`, or one whose entry has no `/DestOutputProfile` stream. A *wholly absent* array is not reported here — see the `DeviceColor` row |
| `DeviceColorWithoutOutputIntent` | device colour needs an intent to define it | a named `/DeviceGray`, `/DeviceRGB` or `/DeviceCMYK` resource when no intent was found. This is the *only* route by which a missing intent becomes a violation, at either level |
| `ExternalContentReference` | nothing may live outside the file | a reference XObject's `/Ref`, or `/F` on a stream |
| `Transparency` | A-1 only | a non-`/None` `/SMask` or a non-`Normal` `/BM` in an `/ExtGState`, a form XObject `/Group`, an image `/SMask`. Constant alpha is **not** checked: 6.4 does not mention it (§6) |
| `OptionalContent` | A-1 only: PDF 1.5 feature | `/OCProperties` |
| `EmbeddedFile` | A-1 only | an `/EmbeddedFiles` name tree |
| `LzwFilter` | no LZW-compressed stream, either level | `/Filter` on any XObject stream |
| `JpxFilter` | A-1 only: its PDF 1.4 base has no JPEG 2000 | the same walk. veraPDF's A-1 profile has no such rule, so this one is unadjudicated (§6) |

`Subject` names the object, not a sentence about it: `Object(ObjRef)`,
`Page(u32)`, `Resource { page, name }`, `Catalog`, `Document`. The converter
that comes later needs the reference to reach the object, and a formatted
string cannot be turned back into one.

## 4. What is **not** checked

This is the important half of the table, and it is why a passing report says
"we found nothing" and not "this file is PDF/A".

- **Content-stream operands.** Every check reads the object graph. A colour
  set by `1 0 0 rg` rather than through a named `/ColorSpace` resource, and
  transparency reached without a `gs`, are invisible to it. This is the
  largest gap and the source of most of §6's "theirs only" clauses — veraPDF
  interprets the streams and we do not. The direction matters: it makes us
  report **fewer** violations, never a violation veraPDF does not see.
- **CMap embedding** (ISO 19005-1 6.3.3.3). A non-`Identity` CMap must itself
  be embedded. Not implemented.
- **Glyph-level checks** (6.3.7, 6.3.8): that every glyph a stream shows is
  actually present in the embedded program, and that widths in the font
  program match the `/Widths` array. Both need the interpreter.
- **ICC profile validity.** The profile stream's presence is checked; that it
  parses as a valid ICC profile of the right class is not. `moxcms` is already
  in the tree and could do it — deferred with the conversion, which is the
  pass that has to write one.
- **A-2's `/CIDSet` completeness rule** (6.2.11.4.2). Where A-1 requires a
  CIDFont subset to *have* a `/CIDSet`, A-2 instead requires that one which is
  present list every CID in the program. Checking that means parsing the
  embedded font, so A-2 has no check here at all rather than a check that
  would guess.
- **A-2's embedded-file rule.** A-2 permits an embedded file if it is itself
  PDF/A, which means recursing into it. A-1's blanket ban is checked; A-2's
  conditional permission is not, so an A-2 check silently permits every
  embedded file.
- **XMP schema validity beyond identification.** See §5.
- **The `a` conformance levels.** A-1a, A-2a and A-3a need a tagged structure
  tree with a defined reading order. `Level` has no variant for them, so a
  caller cannot ask for a check that would be weaker than its name.
- **PDF/A-3, PDF/A-4, PDF/UA.** Out of M26's scope by the roadmap.

## 5. XMP without an XML parser, and what it costs

`pdfa/xmp.rs` reads five scalar properties out of the packet with string
scanning: `pdfaid:part`, `pdfaid:conformance`, and the three `dc:`/`xmp:`/
`pdf:` properties the information dictionary mirrors. Each appears either as
an element or as an attribute on an `rdf:Description`, and both spellings are
about two dozen lines.

**No dependency was added and none is being requested.** STYLE.md §5 is the
rule ("write the thirty lines rather than take a dependency for them") and
this is squarely the case it describes: the alternative was a full XML stack
to read five strings. `cargo tree` over a default `pdfrum` is unchanged.

What that costs, stated plainly: the reader is lenient where a validator is
strict. It does not verify that the RDF is well-formed, that namespace
prefixes are bound to the URIs they should be, or that a property appears
exactly once. A packet we read a `part` out of may still be rejected by a real
XMP parser. That is a real divergence source and it is recorded here rather
than hidden — and again it errs toward under-reporting, never toward
inventing a violation.

## 6. veraPDF, the oracle

`crates/pdfrum/tests/pdfa_oracle.rs`. Every check is scored against veraPDF's
verdict on the same file at the same flavour, over the 44-document benchmark
corpus, at both levels — 88 file-level pairs.

### How it runs, and what a missing oracle means

`$PDFRUM_VERAPDF` names the launcher. The two cases are kept apart on purpose,
because collapsing them is what produces a false green:

- **Unset, or naming no file**: the oracle half skips with a printed note. The
  other two tests in the file still check all 44 documents at both levels.
- **Set, but producing no usable report**: **fail**. The operator asked for
  this half to run; a broken oracle passing vacuously is the trap.
- **Set, and veraPDF declines the file** ("doesn't appear to be a valid PDF"):
  neither. Counted and named separately, because it is the oracle having no
  opinion rather than agreeing — collapsing it into either bucket would be the
  same false green one level down.

veraPDF is a **Java** tool, which is a new category for this repository — no
other tool here needs a JVM. DEPS.md records it in the tools table and says
so explicitly. CI cannot run it: GitHub Actions is blocked on account billing,
unrelated to this work.

### What the test asserts, and what it deliberately does not

It does **not** assert agreement. §4 says the checker is a deliberate subset
of ISO 19005, so veraPDF legitimately reports clauses we do not, and asserting
equality would either be false or would force the checker to claim coverage it
does not have.

It asserts the one direction that is a defect: **no clause we report that
veraPDF does not see on the same file at the same flavour**. That is us
inventing a violation, and it is the failure that matters — a checker that
over-reports makes conforming files look broken, which is worse than one that
under-reports and says so.

### What the oracle found, and what it changed

The first scored run disagreed on 34 file-level clauses. **Every one was a
defect on our side**, and the checker was corrected rather than the test
loosened. That is the value the oracle bought, and it is worth listing what it
caught, because none of it would have been visible from reading the standard
alone:

| What was wrong | How it was found | Fix |
|---|---|---|
| Ten `Clause::iso` citations named the wrong ISO paragraph — A-2 font embedding is 6.2.11.4.1 not 6.2.11.4, A-1 output intent is 6.2.3.3 not 6.2.2, A-2 actions are 6.5.1 not 6.6.1, and six more | the comparison is *by clause number*, so a wrong citation showed up as a clause veraPDF never reports | corrected against veraPDF's own rule table |
| A `HeaderVersion` clause that fired on any PDF above 1.4 at A-1 | veraPDF has no such rule: a header version ceiling is not an ISO 19005 conformance requirement | clause and check deleted, `Level::max_version` with it |
| "Missing output intent" reported as a violation in its own right | veraPDF exempted the 8 of 44 corpus files whose colour is not device colour | both parts phrase it as a *condition on uncalibrated colour*; the absence is now only reported through the colour check |
| `/CA` and `/ca` below 1.0 reported as A-1 transparency | veraPDF passes a corpus file carrying `/CA 0.498` | 6.4 bans a non-`/None` `/SMask` and blend modes, and says nothing about constant alpha; the check was following the popular summary rather than the clause |
| A `/CIDSet` demanded of every subset at A-2 | veraPDF's 6.2.11.4.2 is *conditional* — if a CIDSet is present it must be complete | the requirement is A-1's 6.3.5 only, and only for CIDFont subsets |
| One `ForbiddenFilter` clause covering both LZW and JPEG 2000 | a JPX stream cited the LZW rule | split into `LzwFilter` and `JpxFilter`, which is what the enum is for |

### The divergence table

Scored over 88 file-level pairs (44 documents × 2 levels), after those fixes.
Not one corpus document is a PDF/A file — they are rendering fixtures — so
veraPDF fails every one, as do we; the interesting quantity is which *clauses*
each side names.

| Bucket | Count | What it is |
|---|---|---|
| **Agreed** | 74 of 82 pairs | every clause we report, veraPDF reports too |
| **Ours right** | 1 file, 1 pair | `forms_number` at A-1, clause 6.6.1. The file carries 174 `/S/JavaScript` actions, which 6.6.1 forbids by name. veraPDF's A-1 profile does not report them; at A-2 it catches the same actions under 6.4.1 (a Field with `/A` or `/AA`), which the test's alias table treats as the same finding. |
| **Theirs right** | 3 files, 6 pairs | `text_foxittext`, `vector_paths_1751` and `image_jbig2_880920`, each at both levels — 6.3.4 under A-1, 6.2.11.4.1 under A-2. Both parts scope font embedding to fonts *used within* the file; we have no interpreter and report every unembedded font in `/Resources`, so a listed-but-unused one is an over-report. This is the **only** place the checker is not strictly under-reporting, and §4's first bullet is why. |
| **Not determined** | 1 file, 1 pair | `image_jpx_123` at A-1, clause 6.1.3. veraPDF's A-1 profile has no JPXDecode rule at all, so it can neither confirm nor deny. A-1's PDF 1.4 base does not define JPEG 2000, which is why the check stays — but with no oracle opinion it is unadjudicated rather than proven. |
| **Theirs only** | 18 (flavour, clause) pairs | requirements veraPDF enforces and we do not. A-1: 6.1.4 xref streams, 6.1.7 stream `/Length`, 6.1.8 object-number spacing, 6.2.4 `/Interpolate`, 6.2.8 the `/TR` key, 6.3.3.1–3 CIDSystemInfo / CIDToGIDMap / CMap embedding, 6.6.2 a Field with `/AA`, 6.7.9 XMP schema validity. A-2: 6.1.7.1, 6.1.8, 6.1.9, 6.2.5, 6.2.8, 6.2.11.3.1, 6.2.11.8 `.notdef`, 6.4.1 a Field with `/A` or `/AA`. Every one is §4's list seen from the oracle's side — expected, not a defect. |
| **Oracle declined** | 3 files, 6 pairs | `shading_coons`, `shading_gouraud`, `shading_type4_5`. veraPDF reports each as "doesn't appear to be a valid PDF" and produces no verdict; our parser opens all three and renders them on the conformance board. Not scored either way, and counted separately so it cannot be mistaken for agreement. |

The test asserts only the direction that would be a defect — a clause we
report that veraPDF does not see — with the eight adjudicated pairs above
enumerated in the source and their total pinned, so the accepted set cannot
grow without someone editing it.

Three of the four buckets are one clause each and the fourth is one
requirement seen at two levels. That is the number worth quoting: after the
corrections in the table above, **the checker and veraPDF disagree about four
distinct requirements across 82 scored file-level pairs**, and every one of
the four is explained rather than outstanding.

## 7. The run

veraPDF 1.30.2 (greenfield, built 2026-06-03) under openjdk 21.0.12, over the
44-document benchmark corpus at flavours `1b` and `2b`:

```
PDFRUM_VERAPDF=~/verapdf/verapdf \
  cargo nextest run -p pdfrum --test pdfa_oracle --no-capture
```

- **88** file-level pairs attempted, **82** scored, **6** declined by the
  oracle (the three `shading_*` files at both levels).
- **8** clause-pairs on our side alone, all four requirements adjudicated in
  §6's table; **0** unadjudicated.
- The remaining tests in the file — which need no oracle — check all 44
  documents at both levels and pass with veraPDF absent.

Each invocation starts a JVM, so the scored run takes roughly two minutes.
That is the reason the oracle is a separate test rather than part of the
per-crate suite, and the reason a missing veraPDF skips rather than fails.

## 8. What comes next

Parts 3 and 4 of M26 — `to_pdfa(level, policy)` and the policy that decides
what to do with a document that cannot be converted faithfully. Nothing in
this pass anticipates them beyond one thing: `Subject` carries the `ObjRef`
rather than a formatted string, because a converter has to reach the object it
is about to rewrite, and a report that had thrown that away would have had to
be redesigned first.

The debt the conversion inherits is §4's list. The two items most likely to
matter there are the content-stream gap — a converter that rewrites colour
must see the operands, so it will need the interpreter this checker does
without — and ICC profile validation, which becomes load-bearing the moment we
are the ones writing the profile rather than reading someone else's.
