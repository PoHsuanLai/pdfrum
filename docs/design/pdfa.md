# PDF/A — the checker, the conversion, and what neither claims

All four parts of M26. `crates/pdfrum-doc/src/pdfa/` reports what a document
fails (§§1-7); `crates/pdfrum/src/pdfa/` repairs it (§§8-11). Both are scored
against veraPDF over the corpus, with the disagreements and the shortfalls
written down rather than tuned away.

## 1. Report before repair

The two halves landed in that order and the ordering was the point: a converter
built on an unexamined checker inherits every one of its mistakes and hides
them behind a file that now claims to be archival. A checker nobody has
cross-examined is a claim; one scored against an independent implementation is
a result. §6 lists what the oracle caught on the checker's first scored run —
34 disagreements, every one a defect on our side — which is the argument for
having spent a pass on the checker alone.

The reading half is one method:

```rust
let report = doc.check_pdfa(PdfaLevel::A2b);
for violation in &report.violations {
    println!("{} — {}", violation.clause.iso(report.level), violation);
}
```

## 2. Where the checker lives, and why it took no feature flag

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
  in the tree and could do it. Still unimplemented *for reading* — but the
  conversion now writes a profile rather than only reading someone else's, and
  the one it writes comes out of `moxcms` and is asserted well-formed in
  `pdfrum-page`'s own test (§8).
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

## 7. The checker's run

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


## 8. The conversion: `to_pdfa`, and the shape that makes lossiness loud

M26 parts 3 and 4, in `crates/pdfrum/src/pdfa/`. Where the checker reports,
this repairs — and the roadmap's fourth item names the failure mode it is
built against: *silent lossy conversion*. A converter that quietly drops a
font, deletes an annotation or rasterizes a page has produced a file that
passes a validator and is not the document the caller handed in.

Two things prevent that, and neither is a convention someone has to remember:

**One pass decides, a second writes.** `convert.rs` surveys the object graph
first, deciding every repair and — where the repair changes what the document
means — asking `Policy` whether it is allowed. Nothing is written during the
survey. Only if the survey collects no `Refusal` does the applying pass run,
and by then every decision is made, so the writer cannot discover a compromise
it has to make up its mind about mid-file. The same survey entry produces both
the edit and the `Compromise`, so a compromise that reaches the file has a
report entry *by construction* rather than by discipline.

**A refusal writes nothing.** `to_pdfa` returns `(Conversion, Option<Vec<u8>>)`
internally, and the bytes are `None` exactly when `Conversion::converted()` is
false. There is no file for a caller to mistake for a conversion.

### The policy is a struct of enums

```rust
let outcome = doc.to_pdfa("out.pdf", PdfaLevel::A2b, &PdfaPolicy::lossy())?;
for compromise in &outcome.compromises {
    eprintln!("cost: {compromise}");   // and match on it, which is the point
}
```

`Policy` has one `Concession` per *kind* of compromise the pipeline can face —
`unembeddable_font`, `unrepresentable_content`, `forbidden_feature` — each
`Refuse` by default, because a caller who has not thought about a compromise
has not agreed to it. `Policy::strict()` is that default named; `lossy()` is
the archivist's answer, "produce a conforming file and tell me what it cost".

Three things are deliberate here and each removes a check or a comment, which
is STYLE.md §2's test for whether a type is earning its place:

- **`Concession` is not `bool`.** "Rasterize it" and "refuse" are different
  answers, and a future third answer ("substitute and mark it") is a variant
  rather than a second boolean nobody can order against the first.
- **The axes are separate** because a caller's answers genuinely differ between
  them. An archive may accept a substituted font — the text stays text, and
  stays searchable — while refusing a rasterized page, where the text stops
  being text at all.
- **`Refusal` names the concession, not the problem.** A caller reads a
  refusal, widens one named field, and runs again;
  `widening_the_policy_a_refusal_names_makes_the_conversion_succeed` asserts
  that loop actually closes over the corpus.

`Policy` is `#[non_exhaustive]` so a fourth concession is not a breaking
change, which means a caller cannot write a struct literal — hence the three
`const fn` setters, chained off `strict()` or `lossy()`.

### `Compromise` is the complete list of ways the file can differ

`FontSubstituted`, `ActionRemoved`, `AnnotationRemoved`,
`AnnotationFlagsChanged`, `FormFlattened`, `PageRasterized`,
`EmbeddedFileRemoved`, `OptionalContentRemoved`, `ObjectMetadataRemoved`. Each
carries the `ObjRef` or page index it happened to, for the same reason
`Subject` does (§3): a caller showing a user what changed has to reach the
thing, and a sentence cannot be turned back into a reference.

A repair that changes nothing a reader would notice is **not** a compromise and
does not appear: minting a `/ID`, writing the output intent, rewriting the XMP
packet, dropping an `/Interpolate` hint or a `/TR` transfer function. The list
is about meaning, not about bytes.

One entry is worth calling out because it *adds* rather than removes.
`AnnotationFlagsChanged` is PDF/A requiring every annotation be visible and
printable: an annotation hidden by its `/F` flags has them cleared and becomes
visible, which changes what the page draws. It is gated on
`forbidden_feature` and reported. That gating was not in the first draft — the
test `the_strict_policy_refuses_what_the_lossy_policy_compromises` caught a
compromise being made under a policy that authorized none, which is exactly the
class of bug this design exists to make loud.

`Conversion::rasterized_pages()` is the roadmap's "say which", kept as its own
accessor because it is the compromise whose *extent* a caller almost always
wants separately.

### Where the code lives, and why it is behind `edit`

The facade, not `pdfrum-doc`. The checker reads the object graph, which
`pdfrum-doc` already has; the conversion **writes**, which needs
`pdfrum-edit`'s overlay and the writer, and needs the facade's own `Metadata`.
Making the checker depend on the writer to serve the caller who wants to repair
would be the wrong edge — the checker ships to the caller who only wants to
know. So the conversion sits beside `flatten` and `stamp`, the other write-side
document operations, behind the `edit` feature that gates saving. The checker
stays in the default set (§2) and is unaffected.

## 9. The run: what veraPDF says about the output

The checker's exit criterion was agreement. The conversion's is stricter and
is a *difference* — the same 44 corpus files, through the same veraPDF at the
same flavour, before and after:

```
PDFRUM_VERAPDF=~/verapdf/verapdf \
  cargo nextest run -p pdfrum --test pdfa_convert --no-capture
```

> `veraPDF A-2b over 44 corpus files: 0 passed before conversion, 10 after`

**0 to 10 of 44.** Not one corpus document is a PDF/A file to begin with — they
are rendering fixtures, and veraPDF fails every one — so the before-number is
asserted to be zero rather than assumed. That is what makes the after-number a
conversion result rather than a statement about the corpus. `A2B_PASS_FLOOR`
pins it: the number may rise freely and may not fall without someone editing
the constant.

All 44 convert (none refuses under `lossy`), all 44 reopen in our own parser,
and one file — `shading_type4_5` — that veraPDF previously **declined to parse
at all** comes out of the rewrite as a file it validates and passes.

The repairs, and the rule each answers, counted over the 41 files veraPDF has
an opinion on:

| Repair | veraPDF rule | Failed before |
|---|---|---|
| the XMP packet, with the PDF/A identification schema | 6.6.4-1 | 27 |
| the packet rewritten so it parses, as UTF-8 | 6.6.2.1-1/-4/-5 | 14/11/11 |
| the sRGB output intent | 6.2.4.3-2/-4 | 33/28 |
| a trailer `/ID` | 6.1.3-1 | 11 |
| legal object spacing | 6.1.9-1 | 7 |
| JavaScript and the forbidden actions stripped | 6.5.1-1, 6.4.1-1/-2 | 1, 2/2 |
| annotation flags corrected or supplied | 6.3.2-1/-2 | 4/5 |
| `/Interpolate` and `/TR` dropped, recursively | 6.2.8-3, 6.2.5-1 | 3, 2 |
| a button widget's `/N` made a state subdictionary | 6.3.3-3 | 3 |
| `/Metadata` on non-catalog objects removed | 6.6.2.3.1-1/-2 | 1/5 |

Two of those cost no code at all. The writer already mints an `/ID` on every
save and emits its own object frames, so a **full rewrite** — never an
incremental save — answers 6.1.3 and 6.1.9 as a side effect of writing the file
at all. That is also why `remove_security` is set unconditionally: the trailer
is the writer's to build, and it is the only way to drop the `/Encrypt` PDF/A
forbids.

### Three findings the oracle forced, which reading the standard would not have

- **The XMP packet is generated, not patched.** Splicing the identification
  schema into the document's existing packet is the obvious conversion and it
  is wrong: 11 corpus files fail 6.6.2.1-4 ("serialized incorrectly and can not
  be parsed") and 11 fail -5 ("encoding null different from UTF-8") *before*
  anything is spliced. A packet that does not parse does not start parsing
  because something was added to it. So the packet is built from the
  information dictionary, which is the store PDF/A requires it to agree with
  anyway (6.6.2.3.1) — making that agreement true by construction rather than
  checked afterwards.
- **`Dict::dict` answers for a stream.** A stream *has* a dictionary, so
  "is this `/N` a state subdictionary?" asked as `ap.dict("N").is_none()` is
  always false — it passes on exactly the shape it is looking for. Asked as
  `ap.stream("N").is_some()` it works. The 6.3.3-3 repair was silently a no-op
  until veraPDF said the count had not moved.
- **`/Type` is optional on a graphics-state dictionary.** The `/TR` repair was
  gated on `/Type /ExtGState` and the corpus file carrying the offending `/TR`
  omits the key entirely, so the repair never fired. It now keys off `/TR`
  itself, which no other PDF object type defines.

### The scored test's gating

Identical to §6's, and for the same reason: `$PDFRUM_VERAPDF` unset **skips
with a printed note** and the four unscored tests still convert all 44 files;
set-but-broken **fails**. A silently skipping oracle is a false green. One
veraPDF invocation validates the whole output directory, so the scored test
costs one JVM start rather than 44 and runs in about eight seconds.

## 10. What the conversion does not repair

The honest half, and the reason 10 of 44 is the number rather than 44 of 44.
Every item is a rule veraPDF still fails converted files on.

- **Fonts that are not embedded — 6.2.11.4.1, 21 files.** The single largest
  remaining blocker, and the one the roadmap anticipated: `Policy` has an
  `unembeddable_font` concession and the substitution behind it is **not
  implemented**. Doing it means finding a system font of the right metrics,
  embedding and subsetting it, and rewriting the font dictionary's `/Widths`
  and encoding to match — a font problem, not a PDF/A one. Until it lands, a
  document with an unembedded font converts (the other repairs still apply) but
  does not pass.
- **Non-UTF-8 resource names — 6.1.8-1, 13 files.** A font resource named with
  bytes that are not valid UTF-8. Renaming it means rewriting every content
  stream that names it, which needs the interpreter §4's first bullet says this
  code does without. Not attempted rather than attempted badly.
- **DeviceCMYK without a CMYK output intent — 6.2.4.3-3, 5 files.** The sRGB
  intent we write covers RGB and Gray; CMYK needs a CMYK profile, and `moxcms`
  has a built-in sRGB profile but no built-in CMYK one. A correct CMYK profile
  is LUT data, not a formula, so this is a **dependency or a vendored asset
  question and is left for the user** rather than decided here. Nothing else in
  the pass needs one.
- **Rasterizing a page — the `unrepresentable_content` concession.** The
  concession is live and refuses, naming the page; the *repair* does not exist.
  Two reasons it was not written: A-2b permits transparency, so no corpus file
  needs it at this level, and the facade's `edit` feature does not imply a
  rasterizer, so `to_pdfa` would have to take a `RasterBackend` type parameter
  to have one. That is a signature change worth making when the repair is real
  and not before. `Compromise::PageRasterized` and `RasterCause` are in the
  vocabulary anyway, so a caller writes that match arm once.

Both of the first two concessions are **live rather than reserved**, and that
distinction was forced by the dead-code sweep at the end of this pass. As first
written, `unembeddable_font` and `unrepresentable_content` were never read: the
fields existed, the `Refusal` variants existed, and nothing produced either —
a dead option of exactly the kind STYLE.md §4 forbids, wearing documentation
that claimed a caller "still gets a refusal". They now drive a real refusal,
detected through **the checker** (`survey_unrepairable`) rather than through a
second implementation of the same walk — which is `Subject` carrying an
`ObjRef` (§3) being used for the purpose it was designed for. `Accept` on
either currently means "convert as far as you can": every other repair applies
and no `Compromise` is reported, because with no substitution performed nothing
was in fact compromised.
- **Glyph widths, `.notdef` references, CIDSet completeness, CIDSystemInfo** —
  6.2.11.5, 6.2.11.8, 6.2.11.4.2, 6.2.11.3.1, one or two files each. All need
  the embedded font program parsed and compared against the dictionary, which
  is the checker's §4 gap seen from the writing side.
- **LZW re-encoded as Flate — 6.1.7.2-1, one file.** Genuinely in reach: decode
  the stream and hand the writer raw bytes, which it flates. Not done because
  the one file that needs it fails four other rules, so it cannot change the
  number; recorded here rather than left as a surprise.
- **A direct annotation or resource inside a dictionary we do not rewrite.**
  The repairs work by replacing indirect objects. A `/Annots` array or an
  `/ExtGState` written inline in a page rather than as its own object is
  reached only when the page itself is being rewritten for another reason.

None of these is a case where the conversion writes something wrong. Each is a
case where it writes less than a complete repair and the file still fails
validation — which is the direction that is safe, and it is visible because
veraPDF says so.

## 11. Named debt

For the roadmap, and each one is a decision rather than an oversight:

1. **Font embedding and substitution** — the `unembeddable_font` concession
   detects and refuses; the substitution behind `Accept` is unwritten. 21 of 44
   corpus files are blocked on it. The largest single item.
2. **Page rasterization** — likewise for `unrepresentable_content`, and wiring
   the repair needs `to_pdfa` to take a backend.
3. **A CMYK output intent** — needs a profile `moxcms` does not carry.
   **The user's call**, per M26's "no new colour dependency is taken".
4. **The checker gaps §4 lists are inherited**, and two are now load-bearing
   rather than theoretical: the conversion writes an ICC profile without
   validating it (it comes from `moxcms`, so it is well-formed by
   construction — asserted in `pdfrum-page`'s own test), and the
   content-stream gap is what blocks the UTF-8 resource-name repair.
