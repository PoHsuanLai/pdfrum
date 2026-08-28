# `pdfrum-parser` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green

The fidelity-critical crate of M1. Contract: SPEC.md §5; behavior:
`docs/design/pdfrum-parser.md`.

Everything a PDF file has to be read *as* before it means anything: bytes to
tokens, tokens to objects, objects located by cross-reference information that
may be wrong or absent, and objects fetched lazily through a store that
decrypts and refuses to loop. The crate's job is not to read well-formed files
— it is to read the other kind, and every heuristic below exists because a
real file needs it.

## What is implemented

Module plan as the brief §3 has it, with `Trailer` promoted to a public type
because `trailer_object_number` is information the edit crate needs and
nothing else can re-derive:

`lib.rs` · `lexer.rs` · `syntax.rs` · `objstm.rs` · `store.rs` · `doc.rs` ·
`decode.rs` · `error.rs` · `xref/{mod,classic,stream,chain,rebuild}.rs`

- **`lexer.rs`** — the byte classifier (§1.0) as a `const fn match` rather than
  a table, so its two deviations from ISO 32000-1 §7.2.2 (`0x80`/`0xFF` are
  whitespace, `0x0B` is not) read as code. `Lexer<'a>`/`Token<'a>` are
  zero-copy; literal strings borrow unless an escape forces a rewrite. Word
  truncation at `Limits::max_word_len`, `atoui`'s saturate-and-negate, the two
  whole-word rules (`checkKeyword` strict for `endstream`, loose for
  `startxref`), and the backwards search.
- **`syntax.rs`** — the grammar (§1.5) in two strictnesses, with nested values
  always loose. `read_stream` carries the `/Length` repair in full (§1.6): the
  nine-byte `endstream` **prefix** compare, the whole-word fallback scan taking
  whichever of `endstream`/`endobj` comes first, the end-of-line back-off, and
  the post-amble resync that hands back an `endobj` standing in for a missing
  `endstream`. An indirect `/Length` is chased through the store; a
  self-referential one meets the cycle guard and degrades to the scan.
- **`xref/`** — classic tables with twenty-byte entries and the garbage
  detector (§1.8); cross-reference streams with the `/W[0] == 0` default-type-1
  rule and the skipped-`/Index`-pair cursor quirk (§1.9); the merge primitives
  and the asymmetric trailer merge that keeps the *older* `/Prev` and
  `/XRefStm` (§1.10); the `startxref` chase, `/Prev` walk with loop detection,
  and hybrid ordering (§1.7, §1.10); and `rebuild.rs`, the full-file scan
  (§1.11) with the last-two-numbers memory, string-body skipping, `/Type /XRef`
  trailer harvesting and object-stream enumeration.
- **`objstm.rs`** — validation exact on type (§1.12), the header read that
  overruns past `/First` when `/N` is too large, the garbage-object-number rule
  that drops one pair while still consuming its offset, and member lookup
  requiring index *and* object number to agree.
- **`store.rs`** — `HashMap<u32, OnceLock<Arc<Object>>>` with the in-progress
  set as the cycle guard, the object-number-at-offset check, the
  past-the-table-end refusal, and the decryption walk including the signature
  `/Contents` deferral the crypt brief §3.4 assigns here. Failures are not
  cached, matching the C++.
- **`doc.rs`** — the load ladder (§1.14) with its rebuild retry, page counting
  that trusts `/Count` (§1.17), the incremental in-order page walk behind a
  `Mutex` so `page()` takes `&self`, and `PageDict::inherited` for the
  `/Resources` `/MediaBox` `/CropBox` `/Rotate` chain.

## Divergences from the brief, and one correction to it

All of the brief's divergences D1–D9 are implemented as written. Two notes:

- **D4 (shared recursion budget) is implemented through the store's
  in-progress set.** The C++ shares its 64-deep budget through a global static;
  we thread the count of fetches currently in flight into each nested parse, so
  a chain of `/Length` references spends one budget across the whole chain
  rather than resetting per object. Pinned by
  `store::tests::nested_fetches_share_one_nesting_budget`.
- **Brief correction — §1.2's word scanning pushes back its terminator,
  whitespace included.** The brief describes delimiters being pushed back and
  is silent on whitespace; `cpdf_syntax_parser.cpp:246-248` pushes back *both*
  (`pos_--` on `PDFCharIsDelimiter(ch) || PDFCharIsWhitespace(ch)`). This is
  load-bearing, not cosmetic: `ReadStream` calls `ToNextLine` after the
  `stream` keyword, so a reader that swallowed the newline as a separator
  starts every stream payload one line late. Found by
  `syntax::tests::a_stream_reads_its_declared_length` failing, and worth
  adding to the brief if it is ever revised.
- **Brief clarification — §1.17's page-tree visited set guards ancestors, not
  every node seen.** `cpdf_document.cpp:94` inserts into `visited_pages` with
  a `ScopedSetInsertion`, so the entry is removed when the descent returns. A
  node listed twice among one parent's `/Kids` is therefore counted twice. The
  brief's phrase "visited-set of dicts (cycle guard)" reads as a permanent set;
  implementing it that way makes `no_page_count.pdf` report three pages instead
  of six. Pinned by `real_files::a_subtree_listed_twice_counts_twice`.
- **Brief clarification — §1.10's `AddNormal`/`AddCompressed` return true when
  they decline.** `cpdf_cross_ref_table.cpp:47`, `:53` and `:74` return `true`
  on the generation and object-stream declines; only the object-number cap
  returns `false`. This matters at exactly one call site: the recovery scan
  gates object-stream enumeration on `AddNormal`'s result
  (`cpdf_parser.cpp:817`), so an object whose entry was declined *still* has
  its members enumerated. The brief describes the declines as "ignored", which
  reads as a failure. Our accessors return "the object number was usable".
- **Brief clarification — §1.10 step 5 fails the load when an update section
  will not load.** `cpdf_parser.cpp:456-464` returns false, sending the file to
  the rebuild; keeping a partially-merged table instead leaves the reader with
  less than the scan would have found.
- **Brief clarification — §1.10's `merge_up` model does not describe classic
  tables.** `LoadCrossRefTable` applies its entries through
  `MergeCrossRefObjectsData` (`cpdf_parser.cpp:678-705`) *directly against the
  accumulated table*, so an equal-generation entry overwrites and a free entry
  clears — which is how a section revises the one before it. Only
  cross-reference *streams* go through `MergeUp`. Reading the brief's model
  literally makes a trailer's `/Size` phantom entry win over the real object
  the table names at that number.
- **Brief clarification — §1.2's word budget counts the name's slash.** A name
  keeps 255 payload bytes where a keyword keeps 256
  (`cpdf_syntax_parser.cpp:193` stores `/` into the buffer first). The brief
  says this in passing; the consolidated limits table §1.20 does not, and the
  table is what an implementer reaches for.
- **Brief clarification — §1.5's depth cap refuses the nested object, not its
  container.** `GetObjectBodyInternal` increments on entry and returns null
  (`cpdf_syntax_parser.cpp:540`), and its caller treats that like any element
  that would not parse. So a file nested past 64 loses its *innermost*
  contents and keeps everything above them; the brief's "exceeding it returns
  null" reads as failing the whole parse.
- **Brief clarification — §1.14's rebuild retry asks only for a catalog.** The
  first attempt requires `GetRoot() && TryInit()` (catalog *and* pages), but
  after the rebuild the gate is `if (!GetRoot()) return FORMAT_ERROR`
  (`cpdf_parser.cpp:305`). A rebuilt document whose catalog is reachable but
  describes no pages opens, with a page count of zero. Two corpus files
  (`circular_viewer_ref.pdf`, `repeat_viewer_ref.pdf`) depend on this: the
  oracle opens both and renders nothing.
- **Brief clarification — §1.17's `CountPages` has no depth cap**, only the
  visited set (`cpdf_document.cpp:69-113`), and its `kPageMaxNum` overflow
  returns `nullopt` that propagates all the way out, making the whole document
  report zero rather than a partial count. The page *lookup* does have a cap,
  checked only on the branch path. Separately, `TraversePDFPages` refuses a
  `/Kids`-less node whose `/Type` says `Pages` (`:271-277`), so such a
  document reports one page and cannot produce it.

## `[spec]` changes

None to SPEC.md §5 — every signature it pins is implemented as written.

One additive change to `pdfrum-object`: four inheritable page-attribute names
(`RESOURCES`, `MEDIA_BOX`, `CROP_BOX`, `ROTATE`) added to `names`, which SPEC
§2 describes as growing per crate.

## Tests

172 in-crate, plus 7 doctests. Ported assertion sets:

- `cpdf_syntax_parser_unittest.cpp` — the hexadecimal-string cases with their
  exact bytes and end positions, the invalid-reference rejection, and the
  position-neutrality of peeking.
- `cpdf_parser_unittest.cpp` — the five `LoadCrossRefTable` tables including
  the 2048-entry block-boundary regression, the `startxref` cases, and the
  whole `ParserXRefTest` suite restated over `load`/`read_xref` in
  `tests/xref_streams.rs`.
- `cpdf_object_stream_unittest.cpp` — all twenty-two cases, including the
  `(2, 3)` phantom pair an over-large `/N` produces, the `(11, 4294967295)`
  wrapped negative offset, and the garbage-object-number alignment golden.
- `cpdf_document_unittest.cpp` — pages in order, reverse, and out of order; a
  `/Count` larger than the tree; a `/Pages` node without `/Kids`.
- Cross-reference merge asymmetry has no upstream unittest (the file does not
  exist in this checkout); the semantics are pinned directly from
  `cpdf_cross_ref_table.cpp` in `xref::tests`.

Hand-written coverage for the heuristics with no upstream test: the
`/Length` repairs, the `endstream` prefix acceptance, dictionaries closed by
`endobj`, junk keys, streams dropped from composites, the nesting cap, name
truncation, the high-byte whitespace quirk, `/Prev` loops, free entries with
generation zero, the phantom last entry, and object headers inside strings.
Never-panic sweeps sit on `lexer`, `parse_object`, `read_xref`, the rebuild
scan, `objstm`, and `load`.

`tests/real_files.rs` reads the oracle's own corpus: a plain file, four whose
`startxref` is gone, the rebuild offsets and generations from
`parser_rebuildxref_correct.pdf`, encrypted files under both passwords and all
four handler revisions, and a sweep that opens every `.pdf` in
`testing/resources` twice. It skips silently when the corpus is absent.

## Measured against the oracle

Against every corpus file with a recorded page count (902 files):
**902 agree, 0 load failures.** The comparison allows for the goldens counting
pages the oracle *rendered* rather than what `FPDF_GetPageCount` reports, so a
document may hold more pages than the golden names; a golden of zero means the
oracle opened nothing, and those files are refused here too.

A fidelity review against the C++ found eighteen further divergences, all
fixed here; the ones that change which files open are the inverted trailer
merge during the `/Prev` walk, classic tables merging instead of applying
directly, the `/Size` ordering on the classic-main path, and the rebuild
retry's gate. Three earlier bugs came from the corpus comparison: the ancestor-scoped
page-tree guard described above; an indirect `/Encrypt`, which most encrypted
files use and which needs a throwaway plaintext store to read; and the
main cross-reference stream applying `/Size` *before* reading its entries, so
that an entry whose object number equals `/Size` survives.

## Not implemented, deliberately

- **Linearized loading** (§1.19, D2) — linearized files load through the normal
  path, as the brief directs. The corpus comparison above found no file that
  notices, which resolves the brief's open question 1 in the negative for M1.
- **`GetTrailerEnds`** (§1.18) — belongs to `pdfrum-edit`.
- **Fuzz targets** — the four the brief names are designed for but land with
  the separate fuzz workspace; the never-panic sweeps stand in until then.
