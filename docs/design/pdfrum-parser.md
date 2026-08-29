# Design brief — `pdfrum-parser`

Behavior source: `pdfium-c++/core/fpdfapi/parser/` — `cpdf_syntax_parser.*`
(lexer + object grammar), `cpdf_parser.*` (xref orchestration + recovery),
`cpdf_cross_ref_table.*`, `cpdf_object_stream.*`, `cpdf_stream_acc.*`,
`fpdf_parser_decode.*` (filter hook only), `fpdf_parser_utility.*`,
`cpdf_indirect_object_holder.*`, `cpdf_document.*` (pages tree),
`cpdf_security_handler.*` + `cpdf_crypto_handler.*` (interface surface only —
crypt gets its own brief), `cpdf_read_validator.*`, `cpdf_linearized_header.*`.
Shape contract: SPEC.md §5 (binding). All C++ paths relative to
`/mnt/data2/pdfium/pdfium-c++/`.

**This is THE fidelity-critical crate.** PDFium's value is that it opens
broken files; every heuristic below must survive, and every recovery emits a
`Diagnostic` (STYLE §3). "Port the behavior, never the shape" — the C++
classes named here decompose per §3 of this brief.

---

## 1. Behavior inventory

### 1.0 Character classes (fpdf_parser_utility.cpp:32-77)

One 256-entry classifier table drives everything:

- **Whitespace (`W`)**: `0x00 0x09 0x0A 0x0C 0x0D 0x20` **and `0x80`, `0xFF`**
  (the last two are a PDFium quirk — keep them). Note `0x0B` (VT) is *not*
  whitespace.
- **Numeric (`N`)**: `0-9 + - .`
- **Delimiter (`D`)**: `% ( ) / < > [ ] { }`
- **Other (`R`)**: everything else.
- Line-ending predicate: `\r` or `\n`.

Implement as a `const [CharClass; 256]` + predicate fns in `lexer.rs`.

### 1.1 Header and version (fpdf_parser_utility.cpp:79-94, cpdf_parser.cpp:198-233)

- `GetHeaderOffset`: scan byte offsets **0..=1024** for the 4 bytes `%PDF`;
  first match is the header offset; none → not a PDF (`LoadError::NotPdf`).
  Everything before the header is invisible: all further positions are
  relative to it, and "document size" = file size − header offset
  (`cpdf_syntax_parser.cpp:149-151`).
- File must be at least `header_offset + 9` bytes (`kPDFHeaderSize = 9` =
  `"%PDF-1.7\n"`, cpdf_parser.cpp:52).
- Version: byte at (header-relative) offset 5 = major digit ×10, offset 7 =
  minor digit; non-digits contribute 0; stored, never validated
  (cpdf_parser.cpp:212-233). Keep as `Document::version: u8`-ish info only.

### 1.2 Lexer — word scanning (cpdf_syntax_parser.cpp:180-252, 398-430)

`ToNextWord` (:398-430): skip whitespace; on `%` skip to line ending;
repeat. Comments are invisible everywhere except inside strings.

`GetNextWordInternal` (:180-252) returns a word + `is_number` flag:

- Word buffer is **257 bytes**; chars beyond 256 stored are silently dropped
  while still being consumed (`word_size_ < sizeof(word_buffer_) - 1`,
  :205, :234). Net: keywords/names/numbers observably truncate at 256 bytes
  (255 payload chars after `/` for names). Port the truncation (do not error).
- First char delimiter ⇒ word type `kWord`:
  - `/`: keep appending chars while class is Other **or** Numeric (stop on
    whitespace *and* on delimiter, pushing the stopper back). So `/` alone is
    a valid (empty) name token.
  - `<`: if next is `<` → token `<<`, else push back → token `<`.
  - `>`: same pairing → `>>` or `>`.
  - Every other delimiter is a 1-char token (`[ ] ( ) { } %`… note `%` can
    only reach here via callers that bypass `ToNextWord`; in practice comments
    are consumed).
- Else: accumulate chars until whitespace/delimiter (pushed back). The word
  `is_number` iff **every** char is in the Numeric class (`0-9+-.`) — so
  `--37`, `1.2.3`, `+-.` all count as "numbers" at the token level (the value
  parse then applies FX_Number rules, object brief §1.2).
- EOF mid-word returns what was accumulated.

`GetDirectNum` (:898-905): next word must be a number token, else 0; value via
`FXSYS_atoui` — decimal accumulate, **overflow clamps to `u32::MAX`**
(fx_system.cpp `FXSYS_StrToInt`), sign handling: leading `-` on an unsigned
parse yields two's-complement negation (`~num + 1`) — in practice xref uses
only nonnegative counts; port clamp-at-max + wrapping-negate exactly.

Zero-copy note: SPEC pins `Token<'a>` borrowing from the file. The C++
truncation rule applies to the *materialized* word; a borrowed token can carry
the full slice, but keyword/name comparison and storage must use at most the
first 256 bytes to stay observably identical.

### 1.3 Literal strings (cpdf_syntax_parser.cpp:254-343)

State machine (Normal / Backslash / Octal / FinishOctal / CarriageReturn),
entered after the `(` token:

- Parenthesis nesting: unescaped `(` increments, `)` decrements; the string
  ends at depth-0 `)`. Nested parens are kept literally in the output.
- Backslash escapes: `\n \r \t \b \f` → control chars; `\` + octal digit
  starts an octal escape of **up to 3 digits**, value accumulated base-8 and
  emitted as a single byte (mod 256 via `char` cast — `\777` → 0xFF);
  `\<CR>` and `\<CR><LF>` and `\<LF>` → line continuation (nothing emitted);
  `\` + anything else → that char literally (so `\(` `\)` `\\`).
- EOF before the closing `)` → return what was accumulated (no error), and
  one extra `GetNextChar` is attempted (harmless; don't port the quirk, port
  the resulting position: after EOF, pos = EOF).
- No length cap.

### 1.4 Hex strings (cpdf_syntax_parser.cpp:345-379)

After `<`: accept hex digits pairwise; **every non-hex, non-`>` char is
skipped silently** (including whitespace, `\0`, high bytes); `>` terminates;
EOF terminates; a trailing lone nibble emits `nibble * 16` (i.e. padded with
0). Assertion goldens in `cpdf_syntax_parser_unittest.cpp:19-133` (e.g.
`"z12b"` → `[0x12, 0xb0]`, `"*<&*#$^&@1"` → `[0x10]`, `"1A2b>abcd"` →
`[0x1a,0x2b]` with pos after `>`).

### 1.5 Object grammar — `parse_object` (cpdf_syntax_parser.cpp:537-664)

`GetObjectBodyInternal(parse_type)`; recursion depth capped at
**`kParserMaxRecursionDepth = 64`** (cpdf_syntax_parser.h:41) — exceeding it
returns null. In C++ the depth counter is a **global static**
(cpdf_syntax_parser.cpp:79) shared across nested parsers (an object-stream
parse nested inside a file parse shares the budget); we pass depth explicitly
(see Divergences #4) but keep 64 as the total budget along any nested chain by
threading the running depth through object-stream parsing.

Grammar, in order:

1. Empty word → null (parse failure).
2. Number token: peek two more words. If next is number and the one after is
   the word `R` → **Reference** with `refnum = atoui(word)` (clamped at
   u32::MAX); `refnum == 0xFFFF_FFFF` → parse failure (:565-568). Note
   `refnum == 0` is *allowed* here (fetch later fails). Otherwise rewind to
   after the first number and return **Number** (FX_Number parse of the
   token).
3. `true` / `false` → Bool; `null` → Null.
4. `(` → Str (literal machine §1.3); `<` → Str (hex, §1.4, marked hex).
5. `[` → Array: repeatedly `parse_object(Loose)` until failure; **stream
   elements are dropped** (ISO 32000-1 §7.3.8.1; :590-596). Termination: the
   element parse fails on `]` (falls through all cases). Strict mode then
   requires that the failing word was `]` (first byte of word buffer, :597) —
   else the whole array fails; Loose returns the partial array. Note what
   this implies: in Loose mode an unclosed array swallows until something
   unparseable (EOF, `>>`, keyword) and succeeds.
6. Word starting `/` → Name (payload = word after `/`, `name_decode`d;
   empty payload = valid empty name).
7. `<<` → Dict:
   - Loop words: empty → whole dict fails (EOF). `>>` → done. `endobj` →
     **push back and stop** (missing `>>` repair; :621-624).
   - Word not starting with `/` → skipped silently (Loose repair for junk
     keys; in Strict mode C++ *also* skips — the strict/loose branch only
     differs for empty decoded keys, :630-632).
   - Key = `name_decode(word)` **including the leading `/`** then stripped:
     stored key is `key[1..]`, and requires decoded length > 1 (a bare `/`
     key is dropped, :645-648).
   - Value = `parse_object(Loose)` (always Loose for values). Failure: Loose
     → skip the pair; Strict → `ToNextLine()` and fail the dict (:634-643).
   - **Stream values are dropped** (must arrive via reference; :647).
   - Duplicate keys: last wins (map overwrite).
   - After the dict, if the next word is `stream` → §1.6 stream reading,
     else return the dict (position restored before the peeked word).
8. `>>` alone → rewind to before it, fail (:659-661) — this is what
   terminates dict loops cleanly.
9. Anything else (keywords like `endobj`, `xref`, `}`…) → fail with position
   after the word.

`GetIndirectObject` frame (:666-699): `num num obj` — both words must be
number tokens; the keyword must be exactly `obj`; failure rewinds to the
saved start. Body via the grammar above with the caller's strictness; result
carries `(obj_num = atoui(first), gen_num = atoui(second))`.

`PeekNextWord`/keyword scanning as needed (:513-520).

### 1.6 Stream payloads — `ReadStream` (cpdf_syntax_parser.cpp:770-896)

Entered with the dict parsed and the word `stream` consumed:

1. Read `/Length` via `GetDirectObjectFor` — a **literal number or an
   indirect ref** are both honored (ref resolves through the object store);
   missing/unresolvable/non-number → `len = -1` (:772-774).
2. `ToNextLine()` after the `stream` keyword (:777): skips to just past the
   next `\n` or lone `\r` (a `\r\n` pair consumed together, :381-396). Data
   starts here (`streamStartPos`).
3. If `len > 0` but `start + len >= document size` → `len = -1` (:780-786).
4. If `len >= 0` (zero-length streams are legal — needed for page import):
   take the `len` bytes, then **verify**: skip 1-2 EOL bytes
   (`ReadEOLMarkers` :701-717: `\r\n`→2, `\r` or `\n`→1, else 0), read the
   next word and compare its first 9 bytes to `endstream` (**prefix**
   compare, :810-827). Mismatch ⇒ the declared length was wrong: discard,
   `len = -1`, rewind to `streamStartPos`. (Diagnostic: `LengthMismatch`.)
5. If `len < 0` — keyword scan repair (:829-852): `FindStreamEndPos`
   (:733-768) finds the next **whole-word** occurrence of `endstream` and of
   `endobj` scanning forward from `streamStartPos` (naive byte search
   `FindTag` :977-1006 + `IsWholeWord` :911-937 which checks the chars
   before/after are not Numeric/Other — delimiters and whitespace both count
   as boundaries here since `checkKeyword=true` for FindWordPos… careful:
   `FindWordPos` passes `checkKeyword=true`, so a delimiter adjacent to the
   keyword *disqualifies* the match, e.g. `>>endstream` does NOT match —
   port exactly). Take whichever keyword occurs first (if only one exists,
   use it for both); then back off the EOL immediately before it (2 bytes for
   `\r\n`, else 1 for a single `\r`/`\n`, else 0); if the resulting end <
   start → the stream fails (null). `len = end − start`.
6. Stream data = the byte range [start, start+len). C++ copies; we take a
   `ByteSpan` of the backing `Arc<[u8]>`.
7. Post-amble resync (:873-895): record `end_stream_offset = pos`; read the
   next word (the presumed `endstream`); consume trailing non-EOL whitespace;
   then if that word was actually `endobj` **and** an EOL follows → rewind to
   `end_stream_offset` (so the caller's `endobj` handling sees it — repairs a
   missing `endstream` keyword). Otherwise leave `endstream` consumed.

Related repair: `/Length` values are read via the **object store**, which can
recurse into the same object being parsed (self-referential `/Length`); the
holder's in-progress guard (§1.13) breaks the cycle and yields
missing-Length ⇒ keyword scan.

### 1.7 startxref discovery (cpdf_parser.cpp:315-340)

- Position at `document_size − 9` and **search backwards** for the whole word
  `startxref` within the last **4096** bytes (`BackwardsSearchToWord`,
  cpdf_syntax_parser.cpp:939-975; whole-word check with `checkKeyword=false`
  — delimiters neighboring the word are acceptable boundaries). **The byte at
  that starting position is inside the search — see correction C1 (§6).**
- Then skip the keyword, read one word: must be a number token, value via
  `atoi64` (clamps at i64::MAX), must be `< document size` → the xref offset.
  Any failure → 0.
- `StartParseInternal` (cpdf_parser.cpp:246-268): offset `>= 9`
  (kPDFHeaderSize) → try the normal chain load, on failure → full rebuild
  (§1.11); offset `< 9` (incl. "not found") → straight to rebuild. If rebuild
  also fails → `FORMAT_ERROR` (`LoadError::Broken`).

### 1.8 Classic xref tables (cpdf_parser.cpp:531-705)

`ParseCrossRefTable` grammar (:626-666), at a claimed table offset:

- First keyword must be `xref` else fail.
- Loop subsections: read a word; empty → fail; not a number → push back and
  stop (this is how `trailer` ends the loop). `start_objnum = atoui(word)`;
  `start_objnum > kMaxObjectNumber` → fail. `count = GetDirectNum()`;
  `ToNextWord()` (position at first entry).
- Subsection body (`ParseAndAppendCrossRefSubsectionData` :531-624): entries
  are **exactly 20 bytes** (`kEntrySize = 20`, :540 — `"0000000000 00007 f\r\n"`),
  read in blocks of ≤1024 entries. Caps before reading: cumulative entry
  count ≤ `kMaxXRefSize` and ≤ `document_size / 20` (:561-568) else fail.
  In skip mode (pre-scan) just seek `count*20` forward.
- Per entry: byte[17] == `'f'` → free entry, pos 0. Else: offset =
  `atoi64` of the entry start (reads the 10-digit field; embedded spaces
  terminate the number naturally); if the *parsed offset* is 0, all first 10
  chars must be decimal digits, else the whole table load fails (:603-609 —
  garbage detector). Generation = integer parse of bytes [11..] (`StringToInt`
  int32) then truncated into `u16` storage (C++ implicit narrowing,
  :612-618 — port as `as u16`). Type = normal. Note byte[17] anything but
  `'f'` (including garbage) means "in use" — there is no `'n'` check.
- Merge step (`MergeCrossRefObjectsData` :678-705) applies entries to the
  table via §1.10 primitives; **free entries are only recorded when
  `gen > 0`** (:682-687 — the classic `0000000000 65535 f` head entry lands
  as Free(65535); a free entry with gen 0 is ignored, leaving any prior state).

Verification heuristic `VerifyCrossRefTable` (:370-393, crbug 602650): after
loading the **oldest** table in the chain (only that one — see §1.10), find
the **first** entry with pos > 0, seek there, read one word: it must be a
number equal to the entry's object number. Any mismatch fails the whole
chain-load → triggers full rebuild. Only the first such entry is checked
(`break` at :390).

### 1.9 Xref streams (cpdf_parser.cpp:841-999)

`LoadCrossRefStream(pos, is_main_xref)`:

1. Parse the indirect object at `pos` (Loose); must be a Stream with a
   nonzero object number (:842-846).
2. Dict reads — **all non-resolving** (`GetIntegerFor`/`GetDirectIntegerFor`
   distinction matters; here `/Prev` and `/Size` use `GetIntegerFor` which
   delegates through refs one level — port as written):
   `/Prev < 0` → fail; `/Size < 0 || /Size > kMaxXRefSize` → fail (:849-863).
   Output `*pos = Prev` (0 ends the caller's walk).
3. Trailer handling: a new table is created with **a clone of the stream's
   dict** as trailer, `trailer_object_number = stream's objnum`. Main xref →
   this *replaces* the current table and `SetObjectMapSize(Size)`; non-main →
   `merge_up(new, current)` (current wins; §1.10) (:867-876).
4. `/Index`: pairs `(start, count)` via `GetNumberAt` (type-filtered,
   non-resolving); pairs with missing/mistyped members are skipped; `start <
   0 || count <= 0` skipped; empty result → default `[{0, Size}]`
   (:109-140).
5. `/W`: all elements via `GetInteger` (refs delegate); **need ≥ 3 fields**
   (`kMinFieldCount = 3`, :56) else fail; fields may be >3 (extras shift
   nothing — only the first three are used via prefix offsets); each field
   width is a u32, sum checked for overflow (:881-895). Width 0 for field 1
   ⇒ type defaults to **1** (normal) per ISO table 17 (:963-967). Width 0
   for fields 2/3 ⇒ that value reads as 0 (`GetVarInt` of empty span).
6. Data: **fully decoded** stream bytes (filters applied — the §1.16 hook).
7. Segments: for each index pair, the segment spans
   `[segindex*total_width, (segindex+count)*total_width)` in the decoded
   data; if that range overflows or exceeds the data length the pair is
   **skipped and `segindex` does not advance** (:901-906 `continue` before
   the `segindex += count` at :943) — subsequent pairs then read the bytes
   the skipped pair would have used. Port this quirk exactly.
8. Growing the map: `new_size = min(start+count, kMaxXRefSize)`; grow via
   `SetObjectMapSize(new_size)` only when it exceeds the current
   `last_obj_num+1` (:917-931). (`SetObjectMapSize` also *truncates*: entries
   with objnum ≥ size are erased, and entry `size-1` is created free if
   absent — cpdf_cross_ref_table.cpp:115-126.)
9. Entries are `total_width` bytes; each field is a big-endian variable-width
   unsigned int (`GetVarInt` :82-88). Per entry (obj_num = start + i;
   `obj_num > kMaxObjectNumber` → **break** out of this pair):
   - type field value 0 → Free: gen = field3; recorded only if gen fits u16.
   - 1 → Normal: offset = field2 (must fit the file-size type), gen = field3
     (must fit u16) → AddNormal.
   - 2 → Compressed: archive_obj_num = field2 — must satisfy
     `IsValidObjectNumber` i.e. **≤ current last object number** (this is why
     a bogus archive number like 0xFF in a Size-3 table is skipped —
     `ParserXRefTest.XrefHasInvalidArchiveObjectNumber`); index = field3 →
     AddCompressed.
   - any other type value → entry skipped (:954-961).

### 1.10 The cross-ref table, chains, hybrids, and trailer merging

Table entry model (cpdf_cross_ref_table.h:21-41): type Free/Normal/Compressed,
`gennum: u16`, `is_object_stream_flag: bool`, and pos-or-(archive num, index).
Primitives (cpdf_cross_ref_table.cpp):

- `AddNormal(obj, gen, is_objstm, pos)` (:65-83): rejects
  `obj > kMaxObjectNumber`; **ignored if the existing entry's gen is greater**
  (`info.gennum > gen_num` → keep old); otherwise overwrite, OR-ing
  `is_object_stream_flag`.
- `AddCompressed(obj, archive, index)` (:38-63): rejects obj/archive >
  kMaxObjectNumber; ignored if existing entry has `gennum > 0` or is itself a
  known object stream; sets gen 0; **marks `archive`'s entry as
  `is_object_stream_flag = true`** (creating it if absent).
- `SetFree(obj, gen)` (:85-95): unconditional overwrite, pos 0.
- `SetObjectMapSize(size)` (:115-126): 0 → clear; else erase all entries with
  objnum ≥ size and ensure entry `size−1` exists (free, pos 0). This is why a
  trailer `/Size` materializes a phantom last entry.
- `Update` / `merge_up(current, top)` (:14-27, 109-161): applies `top` onto
  `current` and keeps the result. Entry merge (`UpdateInfo` :128-161):
  top's entries win on conflicts, current's non-conflicting entries are
  retained; when both sides have a *Normal* entry for the same objnum,
  current's `is_object_stream_flag` is propagated onto the winner.
  Trailer merge (`UpdateTrailer` :163-179): if current has no trailer, take
  top's. Else: move **current's** `/XRefStm` and `/Prev` values into top's
  trailer (overwriting), then copy every key of top's trailer into current's
  (overwriting). Net effect: newer (top) keys win, except `/Prev` and
  `/XRefStm` which stay those of the older (current) trailer — the walk's
  continuation pointer is preserved. `trailer_object_number` keeps current's
  when current has a trailer (top's number is dropped) — it is 0
  (`kNoTrailerObjectNumber`, cpdf_parser.cpp:59) for inline `trailer`
  dictionaries and the xref-stream objnum for stream trailers.

Chain orchestration `LoadAllCrossRefTablesAndStreams` (cpdf_parser.cpp:395-472):

1. Probe the startxref target: `LoadCrossRefTable(skip=true)` — success ⇒
   classic table; failure ⇒ assume xref stream.
2. Classic main: after skip-scan, `LoadTrailer` (:1181-1187 — keyword
   `trailer` then a Loose dict via `GetObjectBody`); table gets that trailer
   (objnum 0). Trailer `/Size` (via **`GetDirectIntegerFor`** — an indirect
   Size is ignored) in `1..=kMaxXRefSize` → `SetObjectMapSize(Size)`.
   Stream main: `LoadCrossRefStream(is_main_xref=true)` did trailer+size.
3. Build the update-section lists by walking `/Prev`
   (`FindAllCrossReferenceTablesAndStream` :707-759): maintain
   `seen_offsets = {main}`; loop while `prev_offset > 0`: **revisit of a seen
   offset fails the whole load** (→ rebuild); try
   `LoadCrossRefStream(non-main)` at the offset — success means that section
   was a stream (entries already merged, next Prev returned); failure →
   classic: skip-scan the table, `LoadTrailer`, record the table offset and
   the trailer's `/XRefStm` (via `GetIntegerFor` here — resolving!, :748),
   merge the trailer (`merge_up(older-as-current, accumulated-as-top)` —
   accumulated wins), next = trailer's `/Prev` via `GetDirectIntegerFor`.
   Sections are recorded **front = oldest**.
   Non-number/missing `/Prev` reads as 0 → clean stop (:713-715).
4. Load the oldest classic table for real (`skip=false`) and run
   `VerifyCrossRefTable` (§1.8) — verification applies to the oldest table
   only; failure fails the load (→ rebuild).
5. For every newer section (index ≥ 1): first the stream at
   `xref_stream_list[i]` (if > 0), then the classic table at `xref_list[i]`
   (if > 0) — **table entries take precedence over stream entries** in a
   hybrid section (ISO 32000-1 §7.5.8.4). Index 0's XRefStm is deliberately
   skipped for non-linearized files (:448-455): hybrid-file XRefStm belongs
   to update sections only.
6. If the main was an xref stream: clear the object-stream cache and mark
   `xref_stream_ = true` (:466-469; drives `IsXRefStream()` which pdfium_test
   does not print — keep as an internal flag).

Entry-precedence subtlety: merges use `merge_up(new_section_as_current,
accumulated_as_top)` — since **top wins**, later-loaded (older) sections never
override already-present entries; combined with `AddNormal`'s gen check inside
a single section, "newest wins" holds throughout. **This describes
cross-reference *streams* only; classic tables apply their entries directly —
see correction C2 (§6).**

### 1.11 RebuildCrossRef — the recovery scan (cpdf_parser.cpp:761-839)

Full-file forward scan reconstructing the xref from object headers. Port
faithfully; every entry it creates is a `Diagnostic` (`XrefRebuilt` once, not
per object).

Algorithm:

- Read buffer set to 4096 (from the default `CPDF_Stream::kFileBufSize = 512`,
  cpdf_stream.h:25); position 0 (header-relative). (Buffer size is a C++
  I/O detail — irrelevant for us, noted for provenance.)
- Stream words via `GetNextWord` until EOF, maintaining `numbers`: the last
  **two** number tokens seen, each with the file position of its first byte
  (`pos_after_word − word_len`). A non-number word clears `numbers` after
  processing. More than two numbers in a row: keep the latest two (FIFO
  evict).
- On word `(` → consume a literal string body (§1.3); on `<` → consume a hex
  string (§1.4). This skips string contents so `obj`/`trailer` inside strings
  can't confuse the scan. (`<<` is its own token and is NOT skipped — dict
  bodies are scanned word-by-word.)
- On word `trailer`: parse a Loose object body. If it yields a dict →
  `merge_up(rebuilt_so_far_as_current, {trailer:dict, objnum:0}_as_top)` — a
  later `trailer` overrides earlier keys (except Prev/XRefStm per §1.10).
  If it yields a *stream* (a trailer keyword followed by a stream-dict…) its
  dict is used (:788-799).
- On word `obj` with exactly two pending numbers (`N G obj`):
  - `obj_pos` = position of `N`; `obj_num = N` (atoui-clamped);
    `gen_num = G`.
  - Seek back to `obj_pos` and parse a full indirect object **Strict**
    (:805-807). Strictness matters: a malformed body yields null, but the
    entry below is still added.
  - If the parsed object is a stream whose dict has `/Type /XRef`: merge its
    dict as a trailer (top wins) with `trailer_object_number = stream objnum`
    (:809-815) — recovers files whose xref stream is intact but whose
    startxref is broken.
  - `AddNormal(obj_num, gen_num as u16, false, obj_pos)` (respects the
    existing-gen rule). If it was accepted **and** the object is a valid
    object stream (§1.12 validation): enumerate its member table and
    `AddCompressed(member_obj_num, obj_num, i)` for each member index i
    (:817-828) — objects living only inside object streams are recovered.
  - Parsing continues **after the object** (the Strict parse consumed
    through `endobj`/stream end); note the scan does NOT rewind to just after
    `obj`, so nested `N G obj` sequences inside a broken object body are
    skipped — port by continuing from the parser's position.
- Numbers are cleared after every non-number word (:830).
- Finish: `cross_ref_table_ = merge_up(previous_table_as_current,
  rebuilt_as_top)` — the rebuild *overlays* whatever partial table existed
  (:833-834). Success iff a trailer exists and the entry map is non-empty
  (:838) — note the trailer may come from a `/Type /XRef` stream found
  during the scan.

Also: rebuilt tables have `trailer_object_number` per the last contributing
trailer; `RebuildCrossRefCorrectly` expects 0 for inline trailers.

### 1.12 Object streams (cpdf_object_stream.cpp)

Validation (`IsObjectStream` :24-52): dict `/Type` must be the **Name**
`ObjStm` (a String fails — `ValidateDictType`); `/N` must be a literal
integer Number (float fails) in `0..=kMaxObjectNumber`; `/First` a literal
integer ≥ 0. All non-resolving reads.

Member table (`Init` :97-117): over the **fully decoded** stream data, read
`N` pairs `(obj_num, offset)` with `GetDirectNum` (§1.2; non-number → 0);
stop early if position ≥ data size; pairs with `obj_num == 0` are skipped
(garbage tolerance) but their offset token is still consumed — pair
alignment shifts exactly as C++ (`StreamDictGarbageObjNum` golden:
`"10 0 hi 14 12 21..."` → members `(10,0), (12,21)`). Offsets are raw u32s
(negative source text → wraps via atoui-negate to huge values, recorded
as-is — `StreamDictNegativeObjectOffset` expects `(11, 4294967295)`).

Member fetch (`ParseObject` :76-95): by `(obj_num, index)` — index in range
AND `table[index].obj_num == obj_num` (both from the xref's compressed
entry); parse a Loose object body at `First + offset` in the decoded data
(overflow-checked; `>= data size` → null). Objects get `obj_num` and
**gen 0**. Duplicate obj_nums in the table are legal; the xref's index
disambiguates.

Parser-side cache and gating (cpdf_parser.cpp:1108-1144): object streams are
decoded and indexed **once**, cached by archive objnum (SPEC: store cache).
`GetObjectStream` requires the xref entry for the archive number to have
`is_object_stream_flag` (set by type-2 entries or the rebuild), be Normal
with pos > 0, and not be mid-parse (cycle guard). The whole cache is cleared
when the main xref turns out to be a stream (§1.10 step 6) and when a
linearized main table is (re)loaded.

### 1.13 The object store — fetch semantics

C++ splits this between `CPDF_IndirectObjectHolder` and `CPDF_Parser`:

- Holder fetch (`GetOrParseIndirectObjectInternal`,
  cpdf_indirect_object_holder.cpp:58-82): objnum 0 or `kInvalidObjNum` →
  null. **Insert a placeholder before parsing** — a re-entrant fetch of the
  same objnum during its own parse sees the placeholder and returns null
  (recursion guard #1); parse failure removes the placeholder (so later
  fetches retry!). Parse success caches forever and bumps `last_obj_num`.
- Parser-level parse (`ParseIndirectObject`, cpdf_parser.cpp:1071-1106):
  objnum must be ≤ last object number in the xref (`IsValidObjectNumber`) —
  **numbers beyond the table are unfetchable even if present in the file**;
  a second recursion guard (`parsing_obj_nums_` set) covers the objstm path;
  Free → null; Normal → `pos > 0` and parse-at-offset; Compressed → via the
  object-stream cache.
- Parse-at-offset (`ParseIndirectObjectAt` :1146-1166): Loose indirect parse
  at pos; **if the parsed header's objnum ≠ the requested objnum → null**
  (stale/shifted xref detector; feeds the retry ladder). Then decryption of
  the whole object tree (strings + stream bytes) unless the objnum is the
  cached Metadata objnum (§1.15). Position is save/restored around the parse.
- `ReplaceIndirectObjectIfHigherGeneration`
  (cpdf_indirect_object_holder.cpp:97-115) — edit-crate concern; noted only.

Rust: `ObjectStore = HashMap<ObjRef, OnceLock<Arc<Object>>>` per SPEC, keyed
by objnum for lookup (see object brief §1.1: resolution ignores gen — key the
map by `num: u32`, keep gen in the xref). The in-progress set is explicit
(`Mutex<HashSet<u32>>`) making `Document: Sync`; the C++ "failed parses
retry" behavior falls out of not caching failures (OnceLock left unset — but
beware: OnceLock cannot distinguish "in progress"; the Mutex set does).

### 1.14 Load ladder (cpdf_parser.cpp:246-313) and error mapping

`StartParseInternal` in full — port as `load()`'s spine:

1. `ParseStartXRef`; if offset ≥ 9: chain-load (§1.10); on failure →
   `RebuildCrossRef` (fail → FORMAT_ERROR), mark `xref_table_rebuilt`.
   Offset < 9 → rebuild immediately.
2. `SetEncryptHandler` (:342-364): no trailer → FORMAT_ERROR. Trailer
   `/Encrypt` present (dict directly or via one ref, :1011-1031): `/Filter`
   must be the Name `Standard` else **HANDLER_ERROR**
   (`LoadError::UnsupportedEncryption`); `SecurityHandler::from_encrypt_dict`
   failure → **PASSWORD_ERROR** (`LoadError::WrongPassword`).
3. Root check: trailer `/Root` must be a **Reference** (:1063-1069 — a
   direct dict `/Root` is invalid!) and fetch+init must succeed
   (`TryInit` = root fetchable as dict AND page count > 0, §1.17). If not:
   - already rebuilt → FORMAT_ERROR;
   - else drop the security handler, `RebuildCrossRef`, re-run step 2,
     retry root/init; still no root → FORMAT_ERROR.
4. If `/Root` is missing/not-a-ref (`GetRootObjNum == kInvalidObjNum`):
   drop handler, rebuild (again — even after a prior rebuild), re-run step 2;
   still invalid → FORMAT_ERROR. (Yes, C++ can rebuild twice; keep the exact
   ladder — it changes outcomes on some corpus files.)
5. Metadata skip-list: if encrypted and **metadata is not encrypted**
   (`!IsMetadataEncrypted()` — encrypt dict `/EncryptMetadata`), record the
   catalog's `/Metadata` reference objnum; that object is fetched without
   decryption (:305-311). (The linearized path at :1287-1293 tests the
   *opposite* condition — a long-standing C++ inconsistency; we follow the
   non-linearized variant. Diagnostic-worthy? No — just documented.)

Error enum mapping (cpdf_parser.h:44-50 → SPEC `LoadError`):
`FORMAT_ERROR → Broken`, `PASSWORD_ERROR → WrongPassword`,
`HANDLER_ERROR → UnsupportedEncryption`, `FILE_ERROR → NotPdf/io`. Success
with a rebuilt table is still success (+ `Diagnostic`), and
`has_valid_cross_reference_table = !rebuilt` (cpdf_document.cpp:346-351) —
expose as `Document::xref_was_rebuilt()` (pdfium_test reports load errors
only, but the flag gates `--save` paths later).

### 1.15 Encryption surface (interface only — crypt brief owns the math)

What the parser must do (and only this):

- Extract `/Encrypt` dict (§1.14.2) and trailer `/ID` first element; build
  `SecurityHandler::from_encrypt_dict(dict, file_id, password)`. The
  handler internally: tries the password as owner (only if non-empty), then
  as user (`CheckSecurity`, cpdf_security_handler.cpp:213-219); on failure
  retries with Latin1↔UTF-8 re-encodings of the password (:428-455; R≥5
  tries Latin1→UTF8, R<5 tries UTF8→Latin1). Distinct `WrongPassword`.
- Per-object decryption at fetch (§1.13): the crypto handler decrypts the
  whole object tree — every String's bytes and every Stream's payload —
  keyed by `(objnum, gennum)` of the *top-level* indirect object
  (cpdf_crypto_handler.cpp:247-330). Behaviors the store must preserve:
  - **Signature `/Contents` deferral**: while walking, a String value under
    key `Contents` whose parent dict has `/Type` or `/FT` is skipped;
    after the walk, each such candidate is decrypted anyway **unless** the
    (now decrypted) parent proves to be a signature dict (`/Type` or `/FT`
    resolves to the name/string `Sig`, :48-59) — signature blobs stay raw.
  - AES stream shorter than 16 bytes → decrypts to **empty** (:295-298).
  - Decryption *replaces* the stream bytes (ByteSpan re-backed by the
    decrypted buffer) and String bytes.
- Metadata-objnum exemption (§1.14.5).
- Permissions passthrough: unencrypted → `0xFFFFFFFF`
  (cpdf_parser.cpp:1189-1192); Standard handler masks
  `(P & 0xFFFFFFFC) | 0xFFFFF0C0` (cpdf_security_handler.cpp:221-231);
  owner-unlocked + owner query → `0xFFFFFFFF`. `pdfium_test`'s metadata dump
  prints permissions — Tier A depends on the mask.
- Crypt filter class routing (`/StmF /StrF`, `CryptClass`) is inside
  `pdfrum-crypt`; the store passes `CryptClass::{Stream,String}` per node.
- The `Crypt` filter appearing in a stream's `/Filter` array is a **no-op in
  the decode pipeline** (already handled at object level;
  fpdf_parser_decode.cpp:463-465).

### 1.16 Filter hook (fpdf_parser_decode.cpp — boundary with `pdfrum-filters`)

The parser needs decoded bytes in exactly two places (xref streams, object
streams) plus on-demand for consumers. Semantics owned here (the codec math
lives in `pdfrum-filters`):

- `GetDecoderArray(dict)` (:393-426): `/Filter` direct object; absent → empty
  pipeline; neither Name nor Array → error (undecodable). Array form pairs
  with `/DecodeParms` array *by index* (`params[i]` may be absent/null);
  Name form takes `/DecodeParms` dict directly. `/F`(file) is ignored by C++
  here — streams with external `/F` simply decode their embedded data.
- `ValidateDecoderPipeline` (:95-122): every element of a `/Filter` array
  must be a (possibly ref'd) Name; a pipeline of length > 1 requires every
  filter **except the last** to be from
  {FlateDecode, Fl, LZWDecode, LZW, ASCII85Decode, A85, ASCIIHexDecode, AHx,
  RunLengthDecode, RL} — i.e. image/fax/crypt codecs may only appear last.
  Violation → error → **raw bytes are used undecoded**
  (cpdf_stream_acc.cpp:146-151 fallback: any pipeline error yields the raw
  data, not a hard failure — port this, with a `Diagnostic`).
- `PDF_DataDecode` (:446-534): apply filters left-to-right; `Crypt` skipped;
  aliases `DCT→DCTDecode`, `CCF→CCITTFaxDecode` (:514-518); reaching an
  image codec (anything not in the basic set) stops the chain and returns the
  bytes-so-far + `(image_encoding, image_params)` — SPEC's
  `DecodeOutput::Image(NeedsImageCodec)`. A basic filter reporting failure →
  whole decode fails → raw-bytes fallback (StreamAcc rule above).
- For xref/object streams the parser calls the full pipeline and **requires**
  actual bytes (an image-codec terminator or failure means the xref stream is
  unusable → that load path fails → rebuild can still win).
- RunLength output cap `kMaxStreamSize = 20MB` (:41,279) and Flate/LZW
  predictor param validation (`CheckFlateDecodeParams` :43-56) are
  `pdfrum-filters` behaviors — cross-referenced in that crate's brief.

### 1.17 Pages tree (cpdf_document.cpp) — `Document::page_count/page`

- `TryInit` (:203-214): fetch `/Root` (must yield a dict); build the page
  index; success iff root exists and `page_count > 0`.
- Page count (`RetrievePageCount` :461-473): no `/Pages` in catalog → 0;
  `/Pages` without `/Kids` → **1** (the Pages node itself is treated as the
  single page); else `CountPages`.
- `CountPages` (:69-113): **trusts `/Count`** when `0 < Count <
  kPageMaxNum` without walking (broken counts accepted as-is!); else walks
  `/Kids` with a visited-set of dicts (cycle guard), classifying nodes by
  `GetNodeType` (:47-62): `/Type /Pages` → branch, `/Type /Page` → leaf,
  missing/wrong `/Type` → **repaired in memory**: has `/Kids` ⇒ branch (and
  `/Type` set to `Pages`), else leaf (`/Type` set to `Page`). Count ≥
  `kPageMaxNum = 0xFFFFF` (cpdf_document.h:84) → error → treated as 0.
  Walks write the corrected `/Count` back into the node. (We do not mutate
  parsed objects — see Divergences #6; we compute the same numbers.)
- Page lookup (`GetPageDictionary` :367-394 + `TraversePDFPages` :259-333):
  a `page_list_: Vec<objnum>` cache sized to the page count; lookups first
  try the cached objnum; otherwise resume an **incremental in-order
  traversal** of the tree (persistent stack of (node, child_index), resuming
  where the last lookup stopped). Traversal behaviors to preserve:
  - Depth cap `kMaxPageLevel = 1024` (:38); exceeding it poisons all further
    traversal (`reached_max_page_level_` — every later lookup fails).
  - A kid that fails to load as a dict **consumes a page slot** (`nPagesToGo`
    decrements; :292-295) — missing kids shift nothing.
  - A kid equal to its parent node is skipped (self-loop guard, :297-300).
  - A node without `/Kids` is a leaf even mid-tree; a leaf that claims
    `/Type /Pages` at the *root without Kids* fails the lookup (:271-277).
  - Branch descent uses `/Count` to skip whole subtrees when resuming
    (`FindPageIndex` :115-172 uses the same trust in `/Count`).
  - C++ converts inline kids to indirect objects during traversal
    (`ConvertToIndirectObjectAt` :290) so the cache can hold objnums; we
    instead cache `Option<ObjRef>` + parsed dicts for inline kids (see
    Divergences #6).
- `GetPageIndex(objnum)` (:424-455): linear scan of the cache; else a
  counted tree search seeded with the number of already-cached leading
  entries; out-of-range result → −1; only stores the found index when the
  object is a *valid page object* (`/Type` Name == `Page`, :194-197).
- Linearized fast path (`LoadPages` :238-257): header's `/O` (first page
  objnum) pre-seeds `page_list_[first_page_no]` when the object validates as
  a page. Deferred with linearization (Divergences #2) but the page-count
  source (`/N`) must NOT be used in the non-linearized path.

### 1.18 GetTrailerEnds (cpdf_parser.cpp:1350-1396, cpdf_syntax_parser.cpp:443-501)

Traverses the whole file recording byte offsets just past each `%%EOF` that
follows a trailer-ish region; used by `pdfrum-edit` for incremental-update
safety checks. The EOF detector is a `% E O F <eol>` state machine that runs
*inside whitespace skipping* while a recording hook is set. Not needed for
M1; the brief for `pdfrum-edit` should port it (noted here because the C++
hangs it off the parser).

### 1.19 Linearized load & progressive reading — deferred

`StartLinearizedParse` (cpdf_parser.cpp:1198-1343), `CPDF_LinearizedHeader`
(header dict at fixed offset 9 with `/Linearized /L /H /O /E /N /T`
validation, cpdf_linearized_header.cpp:24-119), `CPDF_ReadValidator`,
`CPDF_DataAvail`, hint tables: all serve incremental/streamed loading. SPEC
pins whole-file `Arc<[u8]>` for v1 (post-M8 work). Behavior consequence to
verify at M1 (see Open questions #1): for a *linearized file loaded whole*,
the C++ non-linearized path produces the same final xref except for the
first-page-table ordering subtleties in `LoadLinearizedAllCrossRefTable`
(:474-529, first XRefStm IS processed there); corpus diffing decides whether
any divergence is observable.

### 1.20 Consolidated hard-limit table

| Constant | Value | C++ source |
|---|---|---|
| `kMaxObjectNumber` | 25_165_824 (24·2²⁰) | cpdf_parser.h:64 |
| `kMaxXRefSize` | 25_165_825 (max objnum + 1) | cpdf_parser.cpp:49 |
| `kPDFHeaderSize` | 9 | cpdf_parser.cpp:52 |
| header scan range | offsets 0..=1024 | fpdf_parser_utility.cpp:83 |
| startxref back-scan | 4096 bytes | cpdf_parser.cpp:320 |
| `kParserMaxRecursionDepth` | 64 | cpdf_syntax_parser.h:41 |
| word buffer (token cap) | 256 bytes stored (257 buf) | cpdf_syntax_parser.h:132 |
| xref entry size | 20 bytes | cpdf_parser.cpp:540 |
| xref entries per read block | 1024 | cpdf_parser.cpp:572-577 |
| xref table entries cap | ≤ kMaxXRefSize and ≤ filesize/20 | cpdf_parser.cpp:561-568 |
| `/W` minimum fields | 3 | cpdf_parser.cpp:56 |
| xref-stream gen cap | fits u16 else entry dropped | cpdf_parser.cpp:971-985 |
| `kInvalidObjNum` | 0xFFFF_FFFF | cpdf_object.h:52 |
| `kPageMaxNum` | 0xFFFFF (1_048_575) | cpdf_document.h:84 |
| `kMaxPageLevel` | 1024 | cpdf_document.cpp:38 |
| objstm `/N` cap | ≤ kMaxObjectNumber | cpdf_object_stream.cpp:35-41 |
| RunLength decode cap | 20 MB (`kMaxStreamSize`) | fpdf_parser_decode.cpp:41 (filters crate) |
| rebuild scan read buffer | 4096 (I/O detail) | cpdf_parser.cpp:764 |
| default read buffer | 512 (`kFileBufSize`, I/O detail) | cpdf_stream.h:25 |

`pdfrum-common::Limits` defaults (values SPEC §1 delegates to this brief):
`max_object_nesting: 64` (`kParserMaxRecursionDepth`);
`max_string_len: usize::MAX` and `max_array_len: usize::MAX` (C++ has no such
caps; fields exist for future hardening and fuzz budgets — outputs are
already O(file size)); `max_xref_size: 25_165_825`; add (field list is
*(abridged)*): `max_object_number: 25_165_824`, `header_scan: 1024`,
`startxref_scan: 4096`, `max_word_len: 256`, `max_page_tree_depth: 1024`,
`max_page_count: 0xFFFFF`.

### 1.21 Diagnostics mapping

Every recovery gets a `Diagnostic` (severity Recovered unless noted):

| Event | DiagKind (proposed) |
|---|---|
| header found at offset > 0 | `HeaderOffset` |
| startxref missing/bad → rebuild | `BadStartXref` |
| chain load failed → rebuild ran | `XrefRebuilt` |
| circular `/Prev` chain | `XrefPrevLoop` (then rebuild) |
| VerifyCrossRefTable mismatch | `XrefEntriesShifted` (then rebuild) |
| xref stream invalid field skipped (bad type / gen > u16 / segment overflow) | `XrefStreamEntryDropped` (Suspicious) |
| trailer missing `/Root` ref / root refetch ladder step | `RootRecovered` |
| `/Length` mismatch → keyword scan | `LengthMismatch` |
| missing `endstream`/`endobj` resync | `KeywordResync` |
| dict closed by `endobj` / junk key skipped / bad value skipped | `MalformedDict` (Suspicious) |
| array element unparsable (loose partial) | `MalformedArray` (Suspicious) |
| stream dropped from dict/array slot | `StreamInCompositeDropped` |
| objnum-at-offset mismatch → fetch null | `ObjNumMismatch` |
| objstm garbage pair skipped | `ObjStmEntryDropped` |
| decode pipeline invalid → raw bytes | `UndecodableStream` |
| page-tree `/Type` guessed / `/Count` wrong / self-loop kid / depth cap | `PageTreeRepaired` (`PageTreeDepthExceeded` Suspicious) |
| password accepted after re-encoding | `PasswordReencoded` |

`Diagnostic.at` = header-relative byte offset where known.

---

## 2. Divergences

1. **Whole file in memory (`Arc<[u8]>`), no `CPDF_ReadValidator`.** The
   validator's read-error/has-unavailable-data plumbing
   (cpdf_read_validator.*, the `ScopedSession` guards in
   `GetNextWord`/`GetObjectBody`/`GetIndirectObject`) exists for progressive
   loading; with the whole file resident, "unavailable" is impossible and
   "read error" becomes bounds-checked slicing. The observable rule that
   survives: a truncated read yields token/parse failure, never partial junk.
2. **Linearized-specific load path deferred** (§1.19). Linearized files load
   through the normal path; conformance decides if any file notices (Open
   question #1).
3. **Diagnostics channel**: recoveries that C++ performs silently are
   recorded (§1.21). Never changes control flow.
4. **Recursion depth as threaded parameter, not a global static.** The C++
   global (cpdf_syntax_parser.cpp:79) accidentally shares the 64-budget
   across nested parser instances (file parser → objstm parser). We thread
   `depth` through `parse_object` and pass the current depth into
   object-stream member parsing to preserve the shared budget.
5. **The store caches decrypted objects; no mutation-in-place.** C++ parses
   then destructively decrypts the tree (cpdf_crypto_handler.cpp:247-330).
   We build the decrypted `Object` during fetch (strings/streams rewritten
   before the value is frozen into `Arc<Object>`); observable results
   identical, including the signature-`/Contents` deferral rules (§1.15).
6. **Pages tree: no in-memory repair mutation.** C++ patches `/Type` and
   `/Count` into the parsed dicts (cpdf_document.cpp:58-62, 110-112) and
   converts inline kids to indirect objects so its objnum cache works. Our
   parsed objects are immutable; the same classification/count logic runs on
   a side structure (`PageIndex`: `Vec<Option<PageSlot>>` where `PageSlot` is
   `ObjRef` or an owned inline dict). The *saved-file* consequence of C++'s
   normalization is an edit-crate concern (its brief must re-derive the
   normalized values when writing).
7. **`CPDF_SimpleParser`** (cpdf_simple_parser.*, a lightweight
   word-splitter used by content-stream & doc code) is **not** part of this
   crate — its behavior belongs to `pdfrum-page`'s content tokenizer /
   `pdfrum-doc`. Recorded so nobody looks for it here.
8. **Failed fetches are not cached** — same as C++ (placeholder removed on
   failure). But C++ retries the *entire* parse on every access to a broken
   object; we do too (no negative cache) for behavioral parity, accepting the
   re-parse cost.
9. **`GetString()`-style virtual dispatch on the trailer for `/Root`**: C++
   requires `/Root` to be a Reference *before* fetch (§1.14.3); we keep that
   exact check rather than "resolving" a direct dict — it drives rebuild
   outcomes.

---

## 3. Module plan

```
crates/pdfrum-parser/src/
  lib.rs        // re-exports: Lexer, Token, parse_object, Xref, Entry,
                // read_xref, Document, LoadOptions, LoadError, load, Error
  lexer.rs      // char classes (§1.0), Lexer<'a>, Token<'a>, word rules,
                // literal/hex string readers, to_next_line, back_search,
                // find_tag/is_whole_word
  syntax.rs     // parse_object (grammar §1.5), indirect frame, read_stream
                // (§1.6); pure over (Lexer, &Limits, &mut Diagnostics,
                // optional &dyn StoreForLength for /Length refs)
  xref/
    mod.rs      // Xref map (BTreeMap<u32, XEntry>), Entry per SPEC,
                // XEntry { kind, gen: u16, objstm_flag }, set/add/merge
                // primitives (§1.10) as functions on the record
    classic.rs  // table grammar + subsection reader + verify heuristic
    stream.rs   // xref-stream validation + entry decoding
    chain.rs    // startxref, /Prev walk, hybrid ordering, trailer merge
    rebuild.rs  // the recovery scan (§1.11)
  objstm.rs     // validation, member table, member parse (§1.12)
  store.rs      // ObjectStore (fetch + cycle guards + decrypt hook + objstm
                // cache); impl Resolve
  doc.rs        // Document, load() ladder (§1.14), page index (§1.17)
  decode.rs     // GetDecoderArray/pipeline-validate/apply (§1.16), calling
                // pdfrum-filters; raw-bytes fallback rule
  error.rs      // Error + LoadError (thiserror)
```

Decomposition per STYLE §1: `CPDF_Parser` (1400 lines, 20 fields) becomes the
`xref/*` function modules + a small `LoadState` record used only inside
`load()`; `CPDF_SyntaxParser` becomes `Lexer` (pos + bytes, zero-copy) +
free-function grammar in `syntax.rs`. No struct holds both "where am I in the
bytes" and "what does the document mean".

Key internal types beyond SPEC:

- `XEntry { kind: Entry, gen: u16, objstm_flag: bool }` — SPEC's `Entry` is
  the public shape; gen + flag are xref-internal (flag gates §1.12).
- `Trailer` = `Dict` + `trailer_object_number: u32` (0 = inline) — needed by
  edit; `merge_trailers(older, newer)` implements §1.10 exactly.
- `PageIndex { slots: Vec<Option<PageSlot>>, cursor: TraverseState, poisoned: bool }`.
- `read_xref(file, limits, diags) -> Result<(Xref, Dict)>` per SPEC is the
  chain+rebuild composite; `load()` builds `Document` on top adding
  encryption + store + pages.

Data flow: `load(bytes)` → header scan → `read_xref` (chain else rebuild) →
security handler → store construction → root/TryInit ladder (may loop back to
rebuild) → `Document`. Fetches flow `store.fetch(ObjRef)` → xref lookup →
`syntax::parse_indirect_at` / objstm member → decrypt → `Arc<Object>` cached.

Concurrency: `Document: Send + Sync`; store uses `OnceLock` cells +
`Mutex<HashSet<u32>>` in-progress set; object-stream cache
`Mutex<HashMap<u32, Arc<ObjStm>>>`. Loading itself is single-threaded.

Fuzz targets (SPEC): `fuzz_lexer` (token stream to EOF), `fuzz_parse_object`,
`fuzz_read_xref`, `fuzz_load` (full `load` with and without password
`"password"`). Seeds: `pdfium-c++/testing/fuzzers/` corpora +
`testing/resources/*.pdf`.

---

## 4. Test plan

Ported unittest assertions (restated over our types; file:line = C++ source):

- `cpdf_parser_unittest.cpp`:
  - `RebuildCrossRefCorrectly` (:146-168) using
    `testing/resources/parser_rebuildxref_correct.pdf`: offsets
    `[0,15,61,154,296,374,450]`, gens `[0,0,2,4,6,8,0]`, trailer objnum 0.
  - `RebuildCrossRefFailed` (:170-178) with
    `parser_rebuildxref_error_notrailer.pdf`.
  - `LoadCrossRefTable` five inline-buffer cases (:180-331) including the
    1024-entry-multiple regression (crbug 945624) — expected
    offset/type vectors as written.
  - `ParseStartXRef` + `WithHeaderOffset` (:333-367): offset 100940, object
    75 at that offset, header offset 765 variant.
  - `BadStartXrefShouldNotBuildCrossRefTable` (:391-406): FORMAT_ERROR with
    empty table.
  - Full `ParserXRefTest` suite (:429-850): `XrefObjectHighestIndex`
    (objnum == kMaxObjectNumber accepted), `XrefObjectIndicesTooBig`
    (past-max → FORMAT_ERROR), `XrefHasInvalidArchiveObjectNumber` (skip
    entry, keep rest; trailer objnum 7), `XrefHasInvalidObjectType` (dict
    not stream → FORMAT_ERROR), `XrefHasInvalidPrevValue` (/Prev −1),
    `XrefHasInvalidSizeValue` (/Size −1), `XrefHasZeroSizeValue` (success,
    empty table), `XrefHasInvalidWidth` (2-element /W → rebuild succeeds),
    `XrefFirstWidthEntryIsZero` (default type 1), `XrefWithValidIndex`,
    `XrefIndexWithRepeatedObject` (later section wins: pos 18),
    `XrefIndexWithOutOfOrderObjects`, `XrefWithIndexAndWrongSize` (Size
    smaller than Index range still loads all three).
- `cpdf_syntax_parser_unittest.cpp` (:19-149): all 13 `ReadHexString` cases
  with exact bytes + end positions; `GetInvalidReference`
  (`4294967295 0 R` → failure); `PeekNextWord` position-neutrality.
- `cpdf_object_stream_unittest.cpp` (:34-509) — the whole file: normal
  member table `(10,0),(11,14),(12,21)` with index/objnum cross-checks; all
  Create-rejection cases (empty dict, missing/String/misnamed `/Type`,
  missing/float/negative/too-big `/N`, missing/float/negative `/First`);
  `First` beyond data (table parses, members don't); `N` too small / too
  large (the `(2,3)` phantom pair golden); garbage objnum / garbage offset /
  negative offset (`(11, 4294967295)`) / offset too big; duplicate objnums
  addressable by index; unordered objnums and offsets accepted.
- `cpdf_document_unittest.cpp` (:161-309): `GetPages` in-order,
  `GetPageWithoutObjNumTwice` (inline kid cached consistently),
  `GetPagesReverseOrder`, `GetPagesInDisorder`, `IsValidPageObject` (Name
  `Page` required; String `Page` invalid), `CountGreaterThanPageTree`
  (tree size 10 vs 7 real pages: 0-6 found, 7-9 fail, then 6 still works),
  `PagesWithoutKids` (Count 3, no Kids → all lookups fail),
  `UseCachedPageObjNumIfHaveNotPagesDict` → restated as: manually seeded
  page-slot is honored without a pages dict.
- `cpdf_indirect_object_holder_unittest.cpp`: `RecursiveParseOfSameObject`
  (fetch during own parse → null, no infinite loop), `ParseInvalidObjNum`,
  `GetObjectMethods` semantics.
- `fpdf_parser_decode_unittest.cpp:46-266`: `ValidateDecoderPipeline` (all
  20 cases incl. only-last-image rule and ref'd names),
  `ValidateDecoderPipelineWithIndirectObjects`, `GetDecoderArray` (name vs
  array vs invalid). (A85/Hex decode value tests port to `pdfrum-filters`.)
- `fpdf_parser_utility_unittest.cpp:42-127`: `ValidateDictType`,
  `ValidateDictOptionalType` semantics (used by objstm/page checks).
- From embedder tests (`cpdf_parser_embeddertest.cpp`) port as *conformance
  expectations*, not unit tests (they load corpus files through the API).

Additional hand-written unit tests pinning heuristics with no C++ unittest:
`/Length` too long / too short / indirect / self-referential; `endstream`
prefix-compare acceptance (`endstreamXYZ`); missing `endstream` with
`endobj` resync; dict closed by `endobj`; junk dict keys; stream in
array/dict dropped; 64-deep nesting cap (65 `[` fails, 63 succeeds);
name truncation at 255 payload bytes; `0x80`/`0xFF` as whitespace;
`VerifyCrossRefTable` off-by-one rejection; `/Prev` loop; hybrid XRefStm
precedence (table beats stream in same section); free-entry gen-0 ignored;
`SetObjectMapSize` phantom last entry; rebuild-scan strings containing
`" 1 0 obj "`; trailer-in-rebuild override order; metadata-objnum decryption
skip (with crypt crate stub); page-slot consumption by null kid.

Snapshot tests (insta): for ~10 crafted broken files (checked into
`tests/files/`), snapshot `(xref summary, trailer keys, diagnostics list)`;
plus the rebuild result for `parser_rebuildxref_correct.pdf`.

Fuzzing: the four targets (§3) clean for 24h = M1 exit criterion (PLAN §6).

Conformance clusters (M1 exits, PLAN §6): 100% corpus+resources load without
crash/panic; page counts Tier-A; `--show-metadata` dumps Tier-A (exercises
trailer/Info/encryption permissions). Encrypted-file clusters gate on
`pdfrum-crypt` landing; until then those files must fail with
`WrongPassword`/`UnsupportedEncryption`, never a crash.

---

## 5. Open questions

1. **Linearized files through the non-linearized path** (§1.19): C++ takes
   `StartLinearizedParse` for these when loaded via DataAvail (as
   `pdfium_test` does), whose xref assembly differs subtly (first XRefStm
   processed; first-page table loaded first; `Size`-vs-last-objnum rebuild
   trigger at cpdf_parser.cpp:1238-1246). Action: at M1, diff page counts +
   metadata dumps for every linearized corpus file; if any mismatch, port
   the linearized xref-assembly subset (not progressive loading) behind the
   same `load()`.
2. **`Document::page(i) -> PageDict`** (SPEC §5): the incremental-traversal
   cache is inherently `&mut`/interior-mutable. Proposal: `PageIndex` behind
   a `Mutex` inside `Document` so `page()` takes `&self` (rayon-friendly per
   STYLE §4). Confirm in review.
3. **Failed-fetch retry cost** (Divergence #8): a hot loop fetching a broken
   object re-parses each time, same as C++. Accept for parity, revisit only
   with a benchmark (would need a `[spec]`-visible behavior note if we add
   negative caching, since retry-after-better-xref is observable).
4. **`GetTrailerEnds`** (§1.18): confirm it lands in `pdfrum-edit`'s brief;
   nothing in M1 needs it.
5. **Object numbers > table size present in file**: C++ makes them
   unfetchable (§1.13). RebuildCrossRef would find them — but rebuild only
   runs when the ladder triggers. Keep exact parity; flag because it
   surprises people debugging "object exists but fetch fails".
6. **`Limits` field for max in-memory objstm cache** — C++ caches every
   object stream forever (per document). A 38MB objstm decodes once and
   stays; acceptable v1 (matches C++ memory behavior). No limit proposed.

---

## 6. Corrections (post-implementation, 2026-08-29)

Two statements above are **wrong**, not merely underspecified. Both were
found while implementing the crate, both changed which files the reader
opens, and both are recorded here rather than edited in place so that a
future port checked against this brief sees the defect and its resolution
together. The implementation follows the corrected reading; the sections
they correct are otherwise unchanged.

(Underspecifications that cost implementation time but state nothing false —
the name/keyword word-budget asymmetry, the depth cap refusing the nested
object rather than its container, the page-tree visited set being
ancestor-scoped, and the rebuild retry's weaker gate — are catalogued in
`docs/status/pdfrum-parser.md` instead.)

### C1 — §1.7: the backwards-scan origin byte is **inside** the window

§1.7 says to "position at `document_size − 9` and search backwards … within
the last 4096 bytes" without saying whether the byte *at* that position takes
part in the comparison. It does.

`GetCharAtBackward(pos, &ch)` (cpdf_syntax_parser.cpp:153-168) reads
`file_buf_[pos - buf_offset_]` — the byte **at** `pos`. Its name refers to the
direction the 512-byte block is loaded from, not to an index offset;
everything else about the function (`pos += header_offset_`, the
`pos >= file_len_` guard) reads as an off-by-one and is not one.
`BackwardsSearchToWord` (:946-958) then opens with `pos = pos_`,
`offset = taglen - 1`, so its first comparison is the keyword's **last**
character against `bytes[pos_]`.

Net: a match may occupy `[origin − 8, origin]` **inclusive** for a
nine-character keyword. Implementing the exclusive reading loses exactly one
position — a `startxref` followed by one separator and a seven-digit offset
and nothing else, i.e. a file truncated with no trailing end-of-line or
`%%EOF`. That is precisely the damage this scan exists to rescue, and the
exclusive reading sends such a file to a full rebuild instead of reading the
table sitting in it.

No corpus file lands on the boundary, so it is held only by
`lexer::tests::search_back_includes_the_byte_under_the_cursor` and
`xref::chain::tests::a_start_xref_ending_at_the_search_origin_is_still_found`;
both fail against the exclusive arithmetic.

Note for anyone constructing a test here: `"…startxref12345678"` does **not**
exercise this. `IsWholeWord` (:921-937) rejects it on the right-hand check —
the byte after the keyword is Numeric — in the C++ as well as in our port, so
both sides agree and nothing is proved. The separator is what makes the two
readings diverge.

### C2 — §1.10: classic tables do **not** merge via `merge_up`

§1.10's closing paragraph ("Entry-precedence subtlety: merges use
`merge_up(new_section_as_current, accumulated_as_top)`") describes *all*
sections. It is true only of cross-reference **streams**.

`LoadCrossRefTable` (cpdf_parser.cpp:668-676) hands its parsed entries to
`MergeCrossRefObjectsData` (:678-705), which applies each one **directly
against the accumulated table** through `SetFree` / `AddNormal` /
`AddCompressed`. The consequences differ from `merge_up` in three ways that
files depend on:

- an entry of **equal** generation overwrites (`AddNormal` only declines on
  `info.gennum > gen_num`), where `merge_up` would keep what is present;
- `SetFree` overwrites **unconditionally**, clearing an entry a newer
  section recorded;
- a rejected object number (past `kMaxObjectNumber`) returns false and
  **fails the whole table load** → rebuild, where a merge would silently drop
  the entry.

This is load-bearing for `/Size` ordering, which is how it surfaced. §1.10
step 2 has the main classic trailer's `/Size` call `SetObjectMapSize` before
any section is read; `SetObjectMapSize` materializes a phantom free entry at
`size − 1` (§1.10, `cpdf_cross_ref_table.cpp:115-126`). Under the merge
reading that phantom **wins** over the real object the table names at that
number, so the document's last object becomes unfetchable. Applied directly,
the real entry overwrites the phantom, which is what the C++ does. Getting
either half wrong alone hides the other: implementing the merge reading with
`/Size` applied last (also wrong, per §1.10 step 2) happens to produce the
right answer on well-formed files.

Both halves are pinned by `doc::tests::reads_pages_in_order` and the corpus
page-count comparison; the phantom-entry interaction specifically is what
`xref::tests::resizing_truncates_and_materializes_the_last_slot` guards.
