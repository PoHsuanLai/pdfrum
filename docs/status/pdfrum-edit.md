# `pdfrum-edit` status

**Updated:** 2026-08-29 · **State:** the document writer, page import, N-up
and CID font subsetting are implemented and verified against the oracle;
content regeneration is implemented as *emitters* and is not yet wired to a
page-object holder (see "What is not here" below).

Contract: SPEC.md §11 (including the 2026-08-29 rulings E1–E4, E7, E10);
behavior: `docs/design/pdfrum-edit.md`.

## M7 verification

`cargo run --release -p conformance -- save-round-trip` performs the two-step
check SPEC §11's ruling E7 describes: `pdfium_test` cannot save a document, so
"the oracle re-opens our saved files cleanly" is a sequence rather than a
flag diff. pdfrum saves every corpus file, the **oracle** reopens and renders
what pdfrum wrote, pdfrum re-renders the saved file against the *original's*
golden, and the incremental path is checked for the append discipline.

Over the full 1,675-file corpus, with pixels compared on a 233-file sample
(stride-selected, so the sample spans the corpus's directory groups rather
than one feature cluster):

| Metric | Result |
|---|---|
| Files saved | 1638 (37 skipped: the tool could not open them, which Tier B already scores) |
| **Oracle reopened what we wrote** | **1638 / 1638 — 100.00%** |
| **Saved renders as well as the original** | **233 / 233 — 100.00%** |
| Saved clears the Tier-B floor outright | 179 / 233 — 76.82% |
| **Incremental append discipline held** | **1638 / 1638 — 100.00%** |

The first, third and fourth are M7's exit criteria and all three are met. The
absolute Tier-B rate is the *renderer's* standing figure, not the writer's,
and the next section is why.

The one incremental failure the first sweep reported turned out to be a
harness bug rather than a writer bug, and it is worth recording because the
shape recurs. `resources/pixel/bug_440028542.pdf` ends in a malformed `%EOF`
rather than `%%EOF`, so requiring the output to hold *two* `%%EOF`s failed a
save that had done everything right — the prefix contributed none. The check
now counts **relative to what the original held**: the append must add one
`startxref` and one `%%EOF`, whatever the original spelled. The same
correction was applied to the crate's own round-trip test, and the case is
pinned there as `a_file_ending_in_a_malformed_eof_still_appends_correctly`.

### The two pixel numbers, and why there are two

The absolute number diffs our render of the *saved* file against the golden
render of the *original*. On a file our renderer already gets slightly wrong
it reports the renderer's gap rather than the writer's — to the digit. The
first corpus files checked make this concrete:
`FRC_3.5_AuthEvent_EFOpen.pdf` renders at SSIM 0.985592 in the ordinary
Tier-B sweep, and its saved copy renders at 0.985592 as well. Counting that
as a save failure would be a lie about what the writer did.

So the sweep reports a second number: whether the saved file's SSIM **matches
the original's**. That isolates what M7 is about — the writer preserving what
the reader saw — and it is the number the failure list is filtered on. The
absolute number stays alongside it because it is comparable with the ordinary
sweep and because a file that clears the floor after a save is worth knowing
about too.

`hello_world.pdf` is the tightest available demonstration: the oracle renders
the original and our saved copy to the **same MD5**,
`fabeb951a76eab6288ddbfc01db64b70`.

## What landed

| module | contents |
|---|---|
| `doc` | `EditDoc`, the overlay over an immutable `Document` — the third sanctioned `Resolve` impl (STYLE §2b) |
| `write/mod` | `save`: the six-step sequence, the full/incremental split, and the two downgrades that keep an append sound |
| `write/header` | the version ladder and the binary comment |
| `write/object` | the per-type serializer, the no-separator rule, the signature-`/Contents` exemption |
| `write/stream` | the four-row flate decision table, the metadata exemptions, `/Length` correctness |
| `write/reach` | the objnum-keyed reachability sweep with its two reference filters, and the eleven-key trailer suppression list |
| `write/xref` | the classic full table, the delta table, the xref-stream record |
| `write/trailer` | the trailer body, the xref-stream object, `startxref` |
| `write/id` | the five-branch `/ID` construction and `IdSource` |
| `content/num` | the four content-stream number writers |
| `content/path` | path construction, the `re` fast path, the paint-operator matrix |
| `content/text` | text emission, the transposed `Tm`, the font-subtype classification |
| `content/emit` | `ProcessGraphics` and the per-kind dispatch, including the deliberate losses |
| `content/marks` | marked-content diffing, inline vs named property lists |
| `import/copy` | the deep copy: memoized remap, skipped back-pointers, the dict-prune / array-abort asymmetry |
| `import/inherit` | the `/Parent` walk with all four of its entry conditions |
| `import/nup` | the slot geometry, the one-axis centering, the fragment layout |
| `import/range` | the page-range grammar, spaces-inside-numbers and all |
| `import/viewer` | the viewer-preference filter, whose reference rejection is the whole circular-graph defence |
| `import/mod` | `init_dest`, `import_pages`, `n_page_to_one` |
| `font` | `subset`, the `GidMap` contract, subset naming, the OpenType-CFF test |
| `font/widths` | `/W`, re-keyed to new glyph IDs |
| `font/tounicode` | the `/ToUnicode` CMap's three buckets and its 256-code boundary rule |

Roughly 8,000 lines. **264 tests** plus **7 doctests**.

The ordinary corpus sweep is unaffected by this crate — nothing here runs on
a read — and `run --check-regressions` against the committed scoreboard
reports **no regressions**. (It reads 1242/1675 passing against the
scoreboard's 1086, but that gain is the render work landing in parallel, not
this crate's; the scoreboard is left for that work to update.)

Three fuzz targets: `edit_save_roundtrip` (a file that opened must save to a
file that opens), `edit_subset` (the glyph map is contiguous and names only
requested glyphs), `edit_import` (the destination stays a document, and a
failed import changes nothing).

## `[spec]` changes made

**§5 — `Document` gains three exposures** (the ruling's E1/E2, landed with
this work). `last_xref_offset`, `main_xref_is_stream` and `encrypt_dict`.
All three were already computed on the load path and discarded: `chain.rs`
knew the startxref offset and the table-vs-stream answer as locals, and
`build_security` consumed the encryption dictionary and kept only the
handler. The `bool` `read_xref_full` returned for "was it rebuilt" became an
`XrefShape` record carrying all three — a rebuilt table reports offset **0**,
and that zero is a value rather than an absence: it is what tells an
incremental save it has no previous section to chain from.

**§4 — `pdfrum-filters` gains `encode_flate`.** The writer flate-encodes an
unfiltered stream, which is the only compression in the workspace. It belongs
beside `decode_flate` — same codec, same `miniz_oxide` dependency — rather
than making `pdfrum-edit` a second place that knows what `/FlateDecode`
means.

**§11 — `subset` returns the map.** SPEC's `subset(font_bytes, gids) ->
Vec<u8>` is not enough once glyphs are renumbered; the signature is
`subset(font_bytes, gids) -> Result<Subsetted, Error>` with `Subsetted {
bytes, gid_map }`. This was the ruling's E4 and is already recorded in §11.

## Divergences honoured

The four import-path bugs are **fixed, not ported** (the ruling's E10), each
pinned by its own test in `tests/import.rs`:

- **D13** — a source `/Type /Pages` resolves to the destination's real pages
  node, not the hardcoded object 4 that is right only for a freshly created
  document.
- **D14** — a failed import leaves the destination untouched, rather than a
  stray blank page and the earlier pages of the batch.
- **D15** — importing never mutates the source, which our shared immutable
  bytes make structural rather than a discipline.
- **D16** — an N-up form reused on a later sheet is registered in *that*
  sheet's `/XObject`, so it renders instead of vanishing. The test walks each
  sheet's operators and requires every name invoked to be present.

Also: **D6**, an object we cannot express contributes nothing rather than an
unclosed `q`; **D11**, the xref stream's `/Length` is the true byte count;
**D4**, determinism is a parameter rather than a process-global RNG.

## Errata against the design brief

**R12 is stated too strongly.** The brief says an untouched page's
`/Contents` is "byte-identical" after a save. That holds only for a stream
that arrived *with* a `/Filter` — the writer's verbatim row. A stream stored
uncompressed takes row 3 and is flate-encoded on the way out, which changes
its bytes while changing nothing a reader sees, and that is the C++'s own
behavior. `hello_world.pdf` is the second kind. The invariant as it actually
holds: the *decoded* content survives a save, and the raw bytes survive for
an already-compressed stream (which is what keeps R14's image checksums
stable). Both halves are tested separately.

**The circular-reference filter makes `multiply_referenced` order-dependent.**
§1.6's filter drops a reference when both endpoints are already recorded as
referencing sources. Over object numbers that cannot distinguish a cycle from
a diamond, so whether a shared font reads as shared depends on the traversal
order — breadth-first (which we use, and which the C++ uses) reaches both
pages before descending into either's font, so it counts. Recorded because
the under-report is in the safe direction: the content generator treats
"shared" as the reason to copy, so missing one costs an unnecessary copy,
never a mutation that was not allowed.

**`subsetter`'s `variable-fonts` feature is off**, per the brief's E4
secondary note. Verified at implementation time: `cargo tree -p pdfrum-edit`
resolves exactly one `skrifa` (0.46.2) and one `kurbo` (0.13.1).

## What is not here

**Content regeneration is emitters only.** Everything that turns a page
object into operator bytes is implemented and pinned against the C++'s
verbatim goldens — the graphics frame, the paint matrix, the `re` fast path,
the transposed `Tm`, the mark diffing, the colour and text losses. What is
*not* implemented is the holder-mutation half: the dirty-set bookkeeping,
`/Contents` array surgery, and the `FXF1`/`FXX1`/`FXE1` resource sweep.

That half needs page-crate infrastructure that does not exist:
`PageObject` has no `dirty` or `active` field, `content_stream` is present
but hardcoded to `0` at its single construction site, and there is no
per-stream CTM facility (`GetCTMAtBeginningOfStream`'s equivalent). The
brief's E6 proposes adding the first two as additive `[spec]` changes to §7;
the third is new work in `pdfrum-page`'s interpreter. None of it is reachable
without touching another crate's contract, and M7's exit criteria do not
depend on it — an ordinary save regenerates nothing, which is why the
fidelity number is what it is.

The emitters are public and tested, so wiring them up is a `pdfrum-page`
change plus a driver, not a rewrite.

**Preserve-encryption save landed in M10** (2026-08-30), superseding SPEC
§11's ruling E3. An encrypted document now saves encrypted under the handler
its password opened, and the output opens with that same password;
`remove_security` is the explicit opt-out. `Error::EncryptedSaveUnsupported`
survives with a narrower meaning: the document declares `/Encrypt` but this
reader holds no key for it — an `/Identity` crypt filter, or a handler we
answered with `SecurityHandler::Identity` — so re-declaring a cipher over
plaintext would produce a file nothing could open.

Three things the next person should not have to rediscover:

1. **The `/Encrypt` dictionary is written from the trailer lookup's plaintext
   copy, not through the object store.** The store deciphers every string it
   hands out and has no exemption for this object, so fetching `/Encrypt`
   through it yields `/O` and `/U` run through a cipher keyed by the very
   material they carry — a saved file that looks perfectly well-formed and
   opens for nobody. The C++ never meets this because it keeps the dictionary
   in a field beside the handler. The `write::mod` stage that emits it says so
   at the site.
2. **Metadata encryption follows `/EncryptMetadata`** (divergence D17). The
   C++ writer skips the cipher for a metadata stream unconditionally, leaving
   a file whose `/Encrypt` claims enciphered metadata and whose metadata is
   plaintext; PDFium's own reader then deciphers it into rubbish. We follow
   the flag, so the packet survives a save. Both writers' output opens in both
   readers — the oracle's *reader* honours the flag too.
3. **Initialisation vectors come from the document's bytes and a counter**
   (`encrypt::IvSource`), so two saves of one document are byte-identical.
   That is what lets a whole encrypted file be snapshot-tested; the C++ cannot
   be, since its vectors come from a process-global Mersenne Twister.

The M7 `security_changed_` / `/ID`-rekey / full-save interlock is unchanged
and still authoritative: an R2/R3 document with no `/ID` is rekeyed and forced
full, and so is a `remove_security` save.

Oracle round trip at the milestone: all seven encrypted corpus fixtures
(`/R` 2, 3, 4, 5 and 6) save, the oracle reopens each with the same password,
refuses each without one, and renders MD5-identically to the original. Five of
the six that have a chainable cross-reference also hold the incremental append
discipline; `bug_644.pdf`'s table is rebuilt, so its save correctly downgrades
to a full one.

**Subsetting is not wired into `save`.** `subset`, the `GidMap` contract,
`/W` re-keying and `/ToUnicode` re-keying are all implemented and tested;
what is missing is the candidate-collection pass that walks pages for text
objects and the content-stream re-keying that D1.5 makes inseparable from it
— which is the same content-generation infrastructure above.
