# Design brief — `pdfrum-edit`

Behavior source: `pdfium-c++/core/fpdfapi/edit/` in full
(`cpdf_creator.*`, `cpdf_pagecontentgenerator.*`, `cpdf_pagecontentmanager.*`,
`cpdf_contentstream_write_utils.*`, `cpdf_stringarchivestream.*`,
`cpdf_pageorganizer.*`, `cpdf_pageexporter.*`, `cpdf_npagetooneexporter.*`,
`cpdf_fontsubsetter.*`, `cpdf_font_util.*`), plus the serialization half of the
object classes (`CPDF_*::WriteTo` in `core/fpdfapi/parser/`),
`core/fpdfapi/parser/object_tree_traversal_util.*`,
`core/fpdfapi/parser/cpdf_flateencoder.*`, `core/fpdfapi/parser/cpdf_encryptor.*`,
the archive primitives in `core/fxcrt/fx_stream.cpp`, and the page-range /
viewer-preference logic in `fpdfsdk/fpdf_ppo.cpp`. Shape contract: **SPEC.md §11**
(binding). All C++ paths below are relative to `/mnt/data2/pdfium/pdfium-c++/`.

This is the last brief of the program, and it is the one crate that *writes*.
Everything before it derives values from bytes; this crate turns values back
into bytes that the oracle must be able to reopen (PLAN.md M7 exit). Three
independent machines live here and share only the serializer:

1. **The document writer** (`save`) — object enumeration, xref emission,
   trailer construction, incremental append. `cpdf_creator.cpp`.
2. **The content-stream generator** — page-object graph back to operator bytes.
   `cpdf_pagecontentgenerator.cpp` + `cpdf_pagecontentmanager.cpp`.
3. **Page import / N-up** — cross-document deep copy with reference remapping.
   `cpdf_pageorganizer.cpp` + friends.

Font subsetting hangs off (1) as an object-override pass.

**A standing note on byte-exactness.** SPEC.md §2 rules that dict serialization
order is insertion order, permanently diverging from the C++'s sorted
`std::map`. Every "exact bytes" claim in this brief is therefore exact *up to
dict key order*: the operator streams of §2 are byte-exact (they contain no
dicts except inline mark parameters), and the file layout of §1 is
structure-exact but not byte-identical. The oracle target is semantic —
reparse and re-render — not a `cmp`.

---

## 1. Behavior inventory — the document writer

### 1.1 Entry shape and flags (`cpdf_creator.h:31-38`, `cpdf_creator.cpp:605-634`)

The C++ exposes one call, `CPDF_Creator::Create(Mask<CreateFlags>, int32_t
file_version)`, driven by four flags:

| Flag | Value | Meaning |
|---|---|---|
| `kIncremental` | `1<<0` | append to the original bytes instead of rewriting |
| `kNoOriginal` | `1<<1` | full rewrite; do **not** copy the original prefix |
| `kRemoveSecurityDeprecated` | `3` | the *combination* 1\|2, kept as a legacy spelling |
| `kRemoveSecurity` | `1<<2` | drop the security handler and `/Encrypt` |
| `kSubsetNewFonts` | `1<<3` | run the font subsetter over newly added objects |

Flag normalization, in order (`:606-621`):

1. Any bit outside `kAllValidFlags` (the four above) ⇒ **all flags are cleared**
   to `kNone` (`:606-608`). Pinned by `SaveSimpleDocBadFlags`
   (`fpdfsdk/fpdf_save_embeddertest.cpp:95-100`): flags `999999` behaves exactly
   like flags `0`.
2. `flags == kRemoveSecurityDeprecated` (exactly the value 3) **or** the
   `kRemoveSecurity` bit ⇒ `RemoveSecurity()` (`:610-613`).
3. If **both** `kIncremental` and `kNoOriginal` are set, both are cleared
   (`:615-617`) — this is what makes the deprecated value 3 degrade to a plain
   full save.
4. `is_incremental_ = kIncremental`; `is_original_ = !kNoOriginal` (note the
   inversion: `is_original_` is true by default);
   `subset_new_fonts_ = kSubsetNewFonts` (`:619-621`).
5. `file_version` is honored only when `10 <= v <= 17` (`:623-625`); anything
   else leaves `file_version_ = 0`, meaning "use the parser's". Pinned by
   `SaveSimpleDocWithBadVersion` (`fpdf_save_embeddertest.cpp:49-61`): −1, 0
   and 18 all produce `%PDF-1.7` for a 1.7 input.

`RemoveSecurity()` (`:712-717`) resets the handler, sets `security_changed_`,
and nulls both encrypt-dict pointers. `security_changed_` matters at
`:247-249`: an incremental save of a document whose security changed is
silently downgraded to a full save, because the original bytes are encrypted
under a key the new file no longer uses.

### 1.2 The stage machine (`cpdf_creator.h:47-62`, `:682-710`)

`Stage` is a numbered enum driven by `Continue()`'s `while (stage_ <
kComplete100)` loop, dispatching to `WriteDoc_Stage{1,2,3,4}` by range. It
exists purely to support pausable saving through the public API — a facility
we do not need. The stage numbers are a linear program:

```
0 Init → 10 WriteHeader → 15 WriteIncremental → 20 InitWriteObjs
→ 21 WriteOldObjs → 25 InitWriteNewObjs → 26 WriteNewObjs
→ 27 WriteEncryptDict → 80 InitWriteXRefs
→ {81 XrefsNotIncremental | 82 XrefsIncremental} → 90 TrailerAndFinish → 100
```

A stage function returning `kInvalid` (−1) makes `iRet < stage_` and breaks the
loop, which is the failure path. **We port the sequence, not the machine**
(STYLE §2): `save` is a straight-line function calling six private steps.

### 1.3 Stage 1 — header and the incremental prefix (`:244-295`)

- **Init0** (`:246-252`): if there is no parser (a document built from
  scratch) **or** security changed on an original-preserving save,
  `is_incremental_ = false`.
- **WriteHeader10, non-incremental** (`:253-270`): write `"%PDF-1."`, then the
  version's units digit as `version % 10`, then `"\r\n%\xA1\xB3\xC5\xD7\r\n"`.
  The version is `file_version_` if set, else `parser_->GetFileVersion()`, else
  the literal **7**. So a header-less/version-less document saves as 1.7.
  Note `version % 10` on a two-digit version: 17→7, 14→4. A version of 20 would
  emit `%PDF-1.0`; unreachable given the 10..17 clamp and the parser's own
  range.
- **WriteHeader10, incremental** (`:271-274`): write nothing;
  `saved_offset_ = parser_->GetDocumentSize()`.
- **WriteIncremental15** (`:276-292`):
  - If `is_original_` and `saved_offset_ > 0`, copy the **entire original file**
    byte-for-byte to the output (`parser_->WriteToArchive`,
    `cpdf_parser.cpp:1398-1420`, a 4096-byte-block loop from position 0).
    *The original bytes are never rewritten* — this is the append discipline.
  - If `is_original_` and `parser_->GetLastXRefOffset() == 0` (a document whose
    table was **rebuilt**, so there is no real previous xref to chain from),
    pre-seed `object_offsets_` with every non-free object's *original* position
    (`:282-290`). This converts the incremental save into a full-table rewrite
    that happens to sit after the original bytes.
- **`InitNewObjNumOffsets()` runs unconditionally at the end** (`:293`),
  even on the non-incremental path.

`GetDocumentSize` is the *syntax parser's* size, i.e. the file measured **from
the `%PDF` header onwards** (`cpdf_parser.cpp:1168-1170`;
`CPDF_SyntaxParser` is constructed over the header-offset slice). Junk before
the header is therefore dropped by an incremental save too — our `Document`
already stores exactly that slice, so `Document::bytes()` is the right prefix.

### 1.4 `InitNewObjNumOffsets` — what counts as "new" (`:228-242`)

Iterate the document's whole indirect-object map. Skip an entry whose object
carries `kInvalidObjNum`. Then:

```
if (!is_incremental_ && parser_ && parser_->IsValidObjectNumber(objnum)
    && !parser_->IsObjectFree(objnum))  continue;    // :235-238
new_obj_num_array_.insert(lower_bound(new_obj_num_array_, objnum), objnum);
```

So:

- **Full save:** "new" = an in-memory object whose number is beyond the xref's
  last object number, **or** whose xref slot is Free. Objects that exist in the
  xref and are live are *not* new — they go down the "old objects" path even if
  they have been mutated in memory, because that path re-fetches them from the
  document's object map first (§1.5).
- **Incremental save:** *every* object currently materialized in the document's
  map is "new", i.e. gets rewritten in the appended section. This is why an
  incremental save's size grows with how much of the document has been touched
  — pinned by `SavedDocsAreEqualAfterParse`
  (`cpdf_creator_embeddertest.cpp:21-45`) which asserts the opposite for a
  *full* save: rendering a page first (which materializes many objects) must
  **not** change the saved size.

`new_obj_num_array_` is kept sorted ascending (`cpdf_creator.h:96`); the
subsetter and the incremental xref emitter both depend on that.

### 1.5 Stage 2a — old objects (`:174-201`, `:152-172`)

```
nLastObjNum = parser_->GetLastObjNum();
if (!parser_->IsValidObjectNumber(nLastObjNum)) return true;   // empty xref
if (cur_obj_num_ > nLastObjNum) return true;
objects_with_refs = GetObjectsWithReferences(document_);        // §1.6
for (objnum = cur_obj_num_ .. nLastObjNum):
    if (objnum not in objects_with_refs) continue;              // GC!
    WriteOldIndirectObject(objnum);
    last_object_number_written = objnum;
if (new_obj_num_array_.empty())  last_obj_num_ = last_object_number_written;
```

Two behaviors matter enormously:

- **Unreferenced objects are dropped.** A full save garbage-collects: only
  objects reachable from the trailer (§1.6) survive. This is what makes
  `Bug1409` (`fpdf_save_embeddertest.cpp:185-210`) work — after removing every
  page object and regenerating content, the saved file contains no `/Image`
  and is under 600 bytes.
- **`last_obj_num_` is walked back** when there are no new objects (`:195-199`),
  so `/Size` reflects the highest object actually written, not the xref's
  nominal last number. `SaveLinearizedDoc` (`fpdf_save_embeddertest.cpp:149-183`)
  pins the interaction: `/Size 37`, objects 35 and 36 present, 37 and 38 absent.

`WriteOldIndirectObject(objnum)` (`:152-172`):

1. `parser_->IsObjectFree(objnum)` ⇒ return without writing (and without an
   offset entry).
2. **Record the offset first**: `object_offsets_[objnum] = CurrentOffset()`.
3. `bExistInMap = !!document_->GetIndirectObject(objnum)` — was it already
   materialized?
4. `pObj = document_->GetOrParseIndirectObject(objnum)`; on failure, **erase the
   offset entry** and return success (`:161-164`). A broken object silently
   vanishes from the output and from the xref.
5. `WriteIndirectObj(pObj->GetObjNum(), pObj)` — note it writes the *object's
   own* number, which equals `objnum` by construction of the store.
6. If the object was **not** previously in the map, `DeleteIndirectObject` it
   again (`:168-170`) — the save does not permanently grow the document's
   memory. This is the mechanism behind `SavedDocsAreEqualAfterParse`.

### 1.6 The reachability sweep — `GetObjectsWithReferences`

`core/fpdfapi/parser/object_tree_traversal_util.cpp`. A breadth-first walk from
the trailer (or, absent one, the catalog), collecting every object number that
is the target of at least one reference:

- **Root selection** (`:29-44`): `root = parser->GetTrailer()` if there is one,
  else `document->GetRoot()`. `root_object_number` is
  `parser->GetTrailerObjectNumber()` for a trailer (0 for an inline `trailer`
  keyword, non-zero for an xref-stream dictionary) or the catalog's own number.
  If that number is non-zero it is **seeded into the result with count 1** —
  which is why an xref-stream document's trailer object appears in the output
  set (`bug_1399.pdf`'s object 16, below).
- **Queue** (`:59-116`): arrays and dictionaries enqueue their values; a stream
  enqueues its dictionary's values (with `CHECK(dict->IsInline())` — a stream's
  dict never has its own number); a reference resolves its target and, if
  non-null, records `(ref_object_number, referenced_object_number)` and
  enqueues the target.
- **`object_number_map_`** (`:139-157`) maps every visited `CPDF_Object*` to the
  nearest enclosing object number, so an inline sub-object's references are
  attributed to their top-level container. This is what makes the
  self-reference test below meaningful.
- **Seen-set** (`:145-148`): a pointer entering the queue twice is dropped, so
  the traversal terminates on cycles.
- **Counting** (`:118-137`): a reference is skipped when
  `referenced == ref_object_number` (self-reference) or when **both** endpoints
  are already in `seen_ref_objects` (the circular-reference filter). Otherwise
  the target's count increments and the source is added to `seen_ref_objects`.

`GetObjectsWithReferences` returns the key set (every reached target);
`GetObjectsWithMultipleReferences` returns those with count > 1 and is used by
the content manager's copy-on-write decisions (§2.8).

Golden sets (`object_tree_traversal_util_embeddertest.cpp:30-112`):

| Fixture | `WithReferences` | `WithMultipleReferences` |
|---|---|---|
| `hello_world.pdf` | `{1,2,3,4,5,6}` | `{}` |
| new empty document | `{1,2}` | `{}` |
| `circular_viewer_ref.pdf` | `{1}` | `{}` |
| `bug_1399.pdf` (xref stream) | `{1,2,3,4,5,12,13,14,16}` | — |
| `rectangles_object_zero.pdf` | `{1,2,3,4}` | `{}` |
| `hello_world_2_pages.pdf` | — | `{5,6,7}` |

`bug_1399` is the load-bearing case: object 16 is the `/XRef` stream itself
(seeded as the trailer's object number), and object 3 is referenced *only* by
the trailer.

### 1.7 Stage 2b — new objects and the subsetter hook (`:203-226`)

```
font_obj_overrides = subset_new_fonts_
    ? CPDF_FontSubsetter(document_).GenerateObjectOverrides(new_obj_num_array_)
    : {};
for (i = cur_obj_num_ .. new_obj_num_array_.size()):
    objnum = new_obj_num_array_[i];
    obj = document_->GetIndirectObject(objnum);
    if (!obj) continue;                          // no offset recorded either
    object_offsets_[objnum] = CurrentOffset();
    obj_to_write = font_obj_overrides.contains(objnum) ? override : obj;
    WriteIndirectObj(objnum, obj_to_write);
```

Note the asymmetry with §1.5: here the offset is recorded **after** the
null check, so a missing new object leaves no stale entry. New objects are
**not** filtered through `objects_with_refs` — an unreferenced newly created
object is still written.

**Wired 2026-09-03.** `write::subset_fonts` is the hook and
`write::write_override` the branch: with `subset_new_fonts` set,
`font::overrides::build` produces the map and each new object is written
through it, exactly as above. Two details are ours rather than the C++'s, both
because our subsetter renumbers glyphs where HarfBuzz's `RETAIN_GIDS` does not
(§5's D1):

- the pass may **mint** an object — the `/CIDToGIDMap` that absorbs the
  renumbering — so its numbers are appended to `new_obj_num_array_`'s
  equivalent before the loop runs, taken from one past everything in play;
- the map is keyed by object number and consulted with `get`, so an override
  of an object the loop was already going to visit costs one lookup and one
  branch. A save with the option off builds an empty map and does not walk
  the page tree at all.

### 1.8 Stage 2c — the inline encrypt dictionary (`:326-340`)

If the encrypt dict exists and `IsInline()` (it was written as a direct
dictionary in the trailer rather than as an indirect object), it is promoted to
a fresh indirect object:

```
last_obj_num_ += 1;
saveOffset = CurrentOffset();
WriteIndirectObj(last_obj_num_, encrypt_dict_);
object_offsets_[last_obj_num_] = saveOffset;
if (is_incremental_)  new_obj_num_array_.push_back(last_obj_num_);
```

Two quirks to port verbatim: the offset is captured *before* the write but
stored *after* it, and the incremental push_back appends **without** re-sorting,
which is safe only because `last_obj_num_` exceeds everything already there.

### 1.9 `WriteIndirectObj` and the non-re-encryption rule (`:135-150`)

```
archive->WriteDWord(objnum);  archive->WriteString(" 0 obj\r\n");
encryptor = (GetCryptoHandler() && pObj != encrypt_dict_)
              ? CPDF_Encryptor(GetCryptoHandler(), objnum) : nullptr;
pObj->WriteTo(archive, encryptor);
archive->WriteString("\r\nendobj\r\n");
```

- **Generation is always 0 on output.** Every object is written as `N 0 obj`
  and every reference as `N 0 R` (`cpdf_reference.cpp:84-89`), regardless of
  the generation the xref recorded. The xref entries likewise all say `00000`
  (§1.11). Generations are read but never written — this is the single most
  important round-trip simplification.
- **The encrypt dictionary is never encrypted** (`pObj != encrypt_dict_`),
  matching ISO 32000-1 §7.6.1.
- Encryption uses `gennum = 0` unconditionally (`cpdf_encryptor.cpp:21-27`),
  which is consistent because the output generation is 0.

**The "security of not re-encrypting when copying".** Read the paths together:

- A full save of an encrypted document re-encrypts every object with the
  *same* crypto handler the file was opened with, because `security_handler_`
  is the parser's (`:129`). Objects were decrypted on fetch and are
  re-encrypted on write; the file key is unchanged, so the result opens with
  the same password.
- The **only** case where a new key is derived is `InitID`'s R2/R3 branch
  (§1.10), which rebuilds the handler from the *newly generated* `/ID` because
  R2/R3 key derivation mixes the first `/ID` element in.
- `RemoveSecurity()` drops the handler entirely, so nothing is encrypted and
  no `/Encrypt` is emitted — but note it does **not** rewrite any
  `/Perms`/`/P`-bearing structure; it simply omits the dictionary.
- Incremental save of an encrypted document with an unchanged handler appends
  freshly-encrypted objects after the untouched original ciphertext, which is
  sound only because the key did not change. `security_changed_` forcing a full
  save (§1.3) is exactly the guard that keeps this sound.

There is no path that copies still-encrypted bytes into a differently-keyed
file. There is also no path that writes plaintext into a file that still
declares `/Encrypt` — because `WriteIndirectObj` consults the live handler for
every object.

### 1.10 `/ID` construction (`:636-680`)

Runs once per `Create`, before any bytes are written.

```
id_array_ = new Array;
pOldIDArray = parser_ ? parser_->GetIDArray() : null;     // trailer /ID
pID1 = pOldIDArray ? pOldIDArray[0] : null;
id_array_[0] = pID1 ? pID1->Clone() : Str(random 16 bytes, hex);

if (pOldIDArray) {
    pID2 = pOldIDArray[1];
    if (is_incremental_ && encrypt_dict_ && pID2) { id_array_[1] = pID2->Clone(); return; }
    id_array_[1] = Str(random 16 bytes, hex);
    return;
}
// no old /ID at all:
id_array_[1] = id_array_[0]->Clone();
if (encrypt_dict_) {
    revision = encrypt_dict_->GetIntegerFor("R");
    if ((revision == 2 || revision == 3) && encrypt_dict_["Filter"] == "Standard") {
        new_encrypt_dict_ = encrypt_dict_->Clone();
        encrypt_dict_ = new_encrypt_dict_;
        security_handler_ = new CPDF_SecurityHandler();
        security_handler_->OnCreate(new_encrypt_dict_, id_array_, parser_->GetEncodedPassword());
        security_changed_ = true;
    }
}
```

Rules restated:

1. **ID[0] is preserved** when the source had one — it is the document's
   permanent identity. `Bug873` (`cpdf_creator_embeddertest.cpp:47-63`) pins
   this for `embedded_attachments.pdf`: the saved trailer contains
   `/ID[<D889EB6B9ADF88E5EDA7DC08FE85978B><` followed by exactly 32 hex
   characters and then `>]>>\r\n`.
2. **ID[1] is regenerated** on every save, except on an incremental save of an
   encrypted document with an existing ID[1], where it is preserved (changing
   it would invalidate the already-written ciphertext's key material for R2/R3).
3. **A document with no `/ID` at all** gets ID[1] = ID[0] (identical elements),
   and then — only then — the R2/R3 rekey. Since a fresh random ID[0] was just
   minted, the old key is no longer derivable, hence the new handler and
   `security_changed_ = true`, which in turn forces a full save (§1.3).
4. Randomness: `FX_Random::Fill` over a `uint32_t[4]` (16 bytes) using a
   Mersenne Twister seeded from a **process-global** counter
   (`core/fxcrt/fx_random.cpp:90-95`). The bytes are then written hex-encoded,
   so 32 hex characters — matching `kIdLen = 32`.

### 1.11 Stage 3 — cross-reference emission (`:344-454`)

`xref_start_ = CurrentOffset()` first (`:350`); this is the value `startxref`
will name. Then a three-way split:

```
bXRefStream = is_incremental_ && parser_->IsXRefStream();
if (!bXRefStream) {
    if (!is_incremental_ || parser_->GetLastXRefOffset() == 0)  → classic full table  (81)
    else                                                        → classic delta table (82)
} else                                                          → no table at all     (→90)
```

`IsXRefStream()` is true when the document's **main** (last-loaded) cross
reference was a stream (`cpdf_parser.cpp:459-468` sets it only for the
`is_xref_stream` main-load path). It is *not* "the chain contained a stream
anywhere". The `GetLastXRefOffset() == 0` case is the rebuilt-table document
of §1.3.

**Classic full table (`kWriteXrefsNotIncremental81`, `:352-362`, `:375-414`).**
Header: `"xref\r\n"` if object 1 has an offset, else
`"xref\r\n0 1\r\n0000000000 65535 f\r\n"` — i.e. when object 1 was dropped, a
standalone free-head subsection is emitted up front. Then `cur_obj_num_ = 1`
and a run-length scan over `object_offsets_`:

```
i = 1
while i <= dwLastObjNum:
    skip forward while i not in object_offsets_
    if i > dwLastObjNum: break
    j = i;  advance j while j in object_offsets_
    if i == 1:  emit "0 {j}\r\n0000000000 65535 f\r\n"
    else:       emit "{i} {j-i}\r\n"
    while i < j:  emit sprintf("%010d 00000 n\r\n", object_offsets_[i++])
```

Note `i == 1` emits a subsection **starting at 0** whose count is `j`
(= 1 free + (j−1) real entries), with the free head inline. `Bug342`
(`fpdf_save_embeddertest.cpp:223-228`) pins the free entry's generation as
`65535`, not `65536`.

**Classic delta table (`kWriteXrefsIncremental82`, `:363-370`, `:415-452`).**
Iterates `new_obj_num_array_` (sorted) in runs of consecutive object numbers.
The C++ here is genuinely convoluted (`:419-437`): the inner `while` advances
`j` to the end of the run, then `objnum` is **reassigned** back to
`new_obj_num_array_[i]`, so the subsection header is
`"{objnum} {j-i}\r\n"` — except when `objnum == 1`, where it is
`"0 {j-i+1}\r\n0000000000 65535 f\r\n"`. Then the run's offsets are emitted.
**There is a real bug here**: the outer `while (i < iCount)` never advances `i`
when the inner emit loop does not run, but the emit loop always runs
(`i < j` holds because `j > i` after the inner advance), so it terminates.
Port the *observable output*, not the shape.

**Xref stream (`bXRefStream`, no stage-3 output at all, `:371-373`).** The
whole cross-reference goes into the stage-4 trailer object (§1.12).

**Offset formatting.** `ByteString::Format("%010d", FX_FILESIZE)` where
`FX_FILESIZE` is `int64_t` (`core/fxcrt/fx_types.h:16`) — a `%d`/`int64_t`
mismatch. In practice offsets under 2 GiB format correctly. See Divergence D3.

### 1.12 Stage 4 — trailer, xref stream body, `startxref` (`:456-603`)

Opening (`:459-469`):
- Not an xref stream: `"trailer\r\n<<"`.
- Xref stream: `"{document_->GetLastObjNum() + 1} 0 obj <<"` — note this uses
  the *document's* last object number, **not** `last_obj_num_`, so the trailer
  object's number can differ from `/Size − 2`.

Trailer body, when a parser exists (`:471-489`): iterate
`parser_->GetCombinedTrailer()` (a **clone** of the merged trailer,
`cpdf_parser.cpp:1049-1053`) and copy every key **except** this exact
suppression list:

> `Encrypt`, `Size`, `Filter`, `Index`, `Length`, `Prev`, `W`, `XRefStm`,
> `ID`, `DecodeParms`, `Type`

Each surviving key is written as `"/" + PDF_NameEncode(key)` followed by
`value->WriteTo(archive, nullptr)` — **with a null encryptor**, so trailer
strings are never encrypted (correct: the trailer is not an indirect object).
`Bug1328389` (`fpdf_save_embeddertest.cpp:238-242`) pins that a malformed
trailer key survives: the output contains `/Foo/`.

With no parser (a from-scratch document, `:490-503`):
`"\r\n/Root {n} 0 R\r\n"` and, if an Info dict exists,
`"/Info {n} 0 R\r\n"`.

Then, in order:

- `/Encrypt` (`:504-517`): `"/Encrypt"` then `" {objnum} 0 R "`, where objnum is
  the encrypt dict's own number or `document_->GetLastObjNum() + 1` when it was
  inline (matching §1.8's promotion).
- `/Size` (`:519-522`): `"/Size "` + `last_obj_num_ + (bXRefStream ? 2 : 1)`.
  The +2 accounts for the trailer object itself.
- `/Prev` (`:523-530`): only when incremental and
  `parser_->GetLastXRefOffset()` is non-zero. `IncrementalSaveWithModifications`
  (`fpdf_save_embeddertest.cpp:244-317`) pins exactly one `/Prev`, two
  `trailer`s, two `startxref`s and two `%%EOF`s after one incremental save.
- `/ID` (`:531-536`): `"/ID"` then the array's own `WriteTo` (which emits
  `[` … `]` with no separator beyond what each element writes).
- Close: `">>"` for a classic trailer, or the xref-stream body below.

**Xref-stream body (`:541-593`).** `"/W[0 4 1]/Index["` then a list of
`"{objnum} 1 "` pairs, `"]/Length {count*5}"`, `">>stream\r\n"`, then
`count` 5-byte records, then `"\r\nendstream"`. Each record is
`OutputIndex` (`:114-120`): the offset's four big-endian bytes followed by a
zero byte. With `/W [0 4 1]` the type field is width 0 (defaulting to type 1 =
in-use), field 2 is the 4-byte offset, field 3 is the 1-byte generation — which
is always 0, consistent with §1.9.

Which objects go in depends on the rebuilt-table case again (`:545-589`):
rebuilt (`GetLastXRefOffset() == 0`) ⇒ every `i < last_obj_num_` present in
`object_offsets_`, and `/Length` is `last_obj_num_ * 5` **even though fewer
records may be written** — a genuine `/Length` overstatement bug in a corner
case. Otherwise ⇒ the `new_obj_num_array_` entries, with `/Length` = `count*5`,
which is correct.

Tail, always (`:595-599`):
`"\r\nstartxref\r\n" + xref_start_ + "\r\n%%EOF\r\n"`.

### 1.13 Object serialization — `WriteTo`

The per-type writers, exactly (`core/fpdfapi/parser/`):

| Type | Bytes | Source |
|---|---|---|
| Null | `" null"` | `cpdf_null.cpp:25-28` |
| Bool | `" "` + `"true"`/`"false"` | `cpdf_boolean.cpp:41-46` |
| Number | `" "` + `fmt_int`/`fmt_number` | `cpdf_number.cpp:66-70` |
| Name | `"/"` + `PDF_NameEncode(name)`; an empty encoding writes just `/` | `cpdf_name.cpp:46-55` |
| String | `PDF_EncodeString` or `PDF_HexEncodeString` of the (possibly encrypted) bytes | `cpdf_string.cpp:70-83` |
| Reference | `" "` + objnum + `" 0 R "` | `cpdf_reference.cpp:84-89` |
| Array | `"["` + elements (no separators!) + `"]"` | `cpdf_array.cpp:284-297` |
| Dict | `"<<"` + per key `"/" + PDF_NameEncode(key)` + value + `">>"` | `cpdf_dictionary.cpp:355-376` |
| Stream | see below | `cpdf_stream.cpp:177-203` |

**Arrays and dicts write no separators.** Tokens self-delimit because numbers,
booleans, nulls and references all carry a *leading* space and references a
trailing one; names and strings are self-delimiting. So `[1 2 3]` comes out as
`[ 1 2 3]` and `<</A 1/B/C>>` as `<</A 1/B/C>>`. This is why the golden
`"trailer\r\n<</Info 9 0 R /Root 11 0 R /Size 36/ID[<…"` has a space before
`/Root` (the trailing space of `9 0 R `) but none before `/ID`.

**Signature dictionaries** (`cpdf_dictionary.cpp:360`, `:371-373`): when
`CPDF_CryptoHandler::IsSignatureDictionary(this)` — the dict's `/Type` or,
absent that, its `/FT`, has the string value `Sig`
(`cpdf_crypto_handler.cpp:48-59`) — the value of key `Contents` is written with
a **null encryptor**. Everything else in the dict is still encrypted. This is
mandatory: a signature's `/Contents` covers a byte range of the file and must
not be re-enciphered.

**Streams** (`cpdf_stream.cpp:177-203`):

```
is_metadata = ValidateDictType(dict, "Metadata") && dict["Subtype"] == "XML";
encoder = CPDF_FlateEncoder(stream, /*bFlateEncode=*/!is_metadata);
data = encoder.GetSpan();
if (encryptor && !is_metadata)  data = encryptor->Encrypt(data);
encoder.UpdateLength(data.size());
encoder.WriteDictTo(archive, encryptor);
archive->WriteString("stream\r\n");
archive->WriteBlock(data);
archive->WriteString("\r\nendstream");
```

Metadata streams are neither re-compressed nor encrypted — ISO 32000-1
§14.3.2's requirement that XMP be readable without decryption.

**`CPDF_FlateEncoder`** (`cpdf_flateencoder.cpp:23-53`) is a three-way choice
over `(has_filter, want_flate)`:

| `has_filter` | `want_flate` | Action |
|---|---|---|
| true | false | raw bytes; **clone the dict and remove `/Filter`** — note it loads the *filtered* data into a scratch accumulator but then still writes `acc_` (the raw span). The removed `/Filter` therefore lies about the payload. Only reachable for metadata streams, which the caller has already decided not to touch. |
| true | true | raw bytes, **original dict unchanged** — an already-compressed stream is copied verbatim, filters and all. |
| false | true | `FlateModule::Encode(raw)`; clone the dict, set `/Length` to the encoded size, set `/Filter /FlateDecode`, **remove `/DecodeParms`**. |
| false | false | raw bytes, original dict. |

`UpdateLength(size)` (`:57-67`) is a no-op when the dict's `/Length` already
equals `size`; otherwise it clones the dict (if not already cloned) and sets
`/Length`. `Bug905142` (`fpdf_save_embeddertest.cpp:230-234`) pins the
consequence: `bug_905142.pdf` has an empty stream declaring
`/Filter /FlateDecode /Length 0`; it takes row 2 (filter present, flate wanted),
copies zero bytes and the original dict, `UpdateLength(0)` matches, and the
output still says `/Length 0`.

### 1.14 The parser surface the writer consumes

Exhaustive list, with the pdfrum status of each:

| C++ | Used for | pdfrum status |
|---|---|---|
| `GetLastObjNum()` | old-object loop bound, trailer objnum, `/Encrypt` objnum | `Xref::last_object_number()` ✅ |
| `IsValidObjectNumber(n)` | ditto | `Xref::is_valid_object_number` ✅ |
| `IsObjectFree(n)` | skip free slots; classify "new" | `Xref::entry(n) == Some(Entry::Free)` / `None` ✅ (note C++ treats *absent* as free too) |
| `GetObjectPositionOrZero(n)` | rebuilt-table pre-seed | `Xref::entry(n)` → `Entry::Offset` ✅ |
| `GetFileVersion()` | header | `Document::version()` ✅ |
| `GetDocumentSize()` | incremental prefix length | `Document::bytes().len()` ✅ |
| `WriteToArchive(archive, n)` | copy the original prefix | `Document::bytes()` ✅ |
| `GetTrailer()` / `GetCombinedTrailer()` | trailer body | `Document::trailer()` ✅ |
| `GetTrailerObjectNumber()` | reachability root | `Document::trailer_object_number()` ✅ |
| `GetIDArray()` | `/ID` | `trailer().raw(names::ID)` ✅ |
| `GetLastXRefOffset()` | `/Prev`, rebuilt detection | ❌ **not tracked** — escalation E1 |
| `IsXRefStream()` | classic-vs-stream xref emission | ❌ **not tracked** — escalation E1 |
| `GetEncryptDict()` | `/Encrypt`, non-encryption of itself, inline promotion | ❌ **not exposed** — escalation E2 |
| `GetSecurityHandler()` / `GetCryptoHandler()` | encrypting output | ❌ **no encrypt side at all** — escalation E3 |
| `GetEncodedPassword()` | R2/R3 rekey | ❌ (crypt brief §1.8 notes it) — part of E3 |
| document object map iteration, `AddIndirectObject`, `DeleteIndirectObject`, `GetOrParseIndirectObject` | everything | ❌ **`Document` is immutable** — see §3.1's `EditDoc` |

---

## 2. Behavior inventory — content-stream generation

Source: `cpdf_pagecontentgenerator.cpp` (1017 lines) and
`cpdf_pagecontentmanager.cpp` (253). The full inventory is reproduced here
because these bytes *are* the round-trip: a page whose objects were touched is
rewritten from the object graph, and any divergence shows up as pixels.

### 2.1 The dirty set and the early-out (`:332-358`)

`GenerateContent()` computes the modified streams and **returns immediately if
none** (`:336-338`) — an untouched page's `/Contents` and `/Resources` are not
rewritten at all.

The dirty set is the union of:
- `pPageObj->GetContentStream()` for every page object with
  `IsDirty()` = `dirty_ || matrix_dirty_` (`cpdf_pageobject.h:78`) —
  **including inactive objects** (`:349-351`), because an inactive object's
  stream must still be regenerated (without it) to make it disappear;
- `obj_holder_->TakeDirtyStreams()`, which moves and clears the holder's own
  set (`cpdf_pageobjectholder.cpp:87-91`).

The set is `std::set<int32_t>`, so `kNoContentStream = −1`
(`cpdf_pageobject.h:38`) sorts first — brand-new streamless objects are
processed before stream 0.

### 2.2 Per-stream frame (`:367-382`, `:404-446`)

Prologue, per dirty stream:

```
q\n
[ WriteMatrix(GetCTMAtBeginningOfStream(s).GetInverse()) " cm\n"  if not identity ]
0 0 0 RG 0 0 0 rg 1 w 0 J 0 j\n/{default_gs} gs
```

Epilogue:

```
[ FinishMarks — one "EMC\n" per open mark ]
Q\n
[ WriteMatrix(prev_ctm.GetInverse() * ctm) " cm\n"  if the stream affects the CTM ]
```

`affects_ctm` (`:405-421`): stream 0 ⇒ `GetCTMAtEndOfStream(0)` is not
identity; stream `s > 0` ⇒ end-CTM of `s−1` differs from end-CTM of `s`;
`kNoContentStream` ⇒ false (it is appended last, nothing follows).

A stream that produced no objects **and** does not affect the CTM has its
buffer reset to `""` (`:426-430`), which is the deletion sentinel the content
manager acts on (§2.8). A stream that produced nothing but *does* affect the
CTM keeps the full frame so downstream streams still see the right CTM.

`GetCTMAtBeginningOfStream` / `GetCTMAtEndOfStream`
(`cpdf_pageobjectholder.cpp:123-151`) read the `all_ctms_` map that
`pdfrum-page` already carries (page brief §1.6).

### 2.3 Object emission order (`:385-401`)

Objects are visited in **document order** (the holder's deque), each appended
to its own stream's buffer — so the several open buffers interleave. Inactive
objects are skipped here (`:386-388`); objects whose stream is not dirty are
skipped (`:391-394`). Content marks are diffed per stream via
`current_content_marks[stream_index]`.

### 2.4 Per-object graphics frame — `ProcessGraphics` (`:816-905`)

Every object opens with `"q "` and closes with `" Q\n"`
(`EndProcessGraphics`, `:81-83`). **There is no inter-object state diffing at
all**: each object is self-contained inside its own `q`/`Q`, and each piece of
state is compared against the *hardcoded PDF default*, never against the
previously emitted object. In emission order:

| Step | Condition | Bytes |
|---|---|---|
| open | always | `q ` |
| fill color | `WriteColorToStream` succeeded | `{r} {g} {b}` + ` rg ` |
| stroke color | ditto | `{r} {g} {b}` + ` RG ` |
| line width | `≠ 1.0` | `{w}` + ` w ` |
| line cap | `≠ kButt` (0) | `{int}` + ` J ` |
| line join | `≠ kMiter` (0) | `{int}` + ` j ` |
| dash | array non-empty | `[` + floats sep by ` ` + `] ` + `{phase}` + ` d ` |
| clip | `clip_path.HasRef()` | per subpath: points + ` W ` or ` W* `, then `n ` |
| ExtGState | any of fill α, stroke α, blend differs from default | `/{name} gs ` |

`WriteColorToStream` (`:64-78`) writes **only** when the color is non-null,
its colorspace is the stock DeviceRGB **or** DeviceGray, and `GetRGB()`
succeeds — and it always writes three components. Consequence: the only color
operators ever emitted are `rg` and `RG`. CMYK, ICCBased, Indexed, Separation,
DeviceN, Lab, CalRGB **and every pattern** silently emit nothing and inherit
whatever the default-graphics prologue set (black). This is a real fidelity
loss the C++ accepts; see Divergence D5.

Clip paths: `kNoFill` clip type is `NOTREACHED()` (`:862-863`); text clips
(`clip_path.GetTextObject()`), clip-path soft masks and shading clips are never
emitted.

ExtGState (`:872-904`) writes only `ca`, `CA`, `BM`, each only when it differs
from the default, and skips the whole `/gs` when all three are default
(`:876-879`). Dedup is a per-holder map keyed
`GraphicsData{fillAlpha, strokeAlpha, blendType}`
(`cpdf_pageobjectholder.cpp:24-32`). `SetGraphicsResourceNames` is called
**only on the cache-miss path** (`:901`) — an object reusing a cached name does
not record it for the resource sweep, which is why the default name is
force-inserted separately (`:505-507`).

`ProcessDefaultGraphics` (`:907-914`) emits the literal
`0 0 0 RG 0 0 0 rg 1 w 0 J 0 j\n` then `/{name} gs `, where the name comes from
`GetOrCreateDefaultGraphics` (`:916-936`) — a dict with all three of `ca 1`,
`CA 1`, `BM /Normal` set unconditionally. It resets neither the dash array,
the miter limit, nor any text state.

### 2.5 Per-kind emission

**Path** (`:785-804`): `ProcessGraphics`; `{matrix} cm ` if not identity;
`ProcessPathPoints`; the paint operator; ` Q\n`.

Paint operator matrix (`:796-802`, leading space included):

| fill type | stroke = false | stroke = true |
|---|---|---|
| `kNoFill` | ` n` | ` S` |
| `kWinding` | ` f` | ` B` |
| `kEvenOdd` | ` f*` | ` B*` |

No `b`/`b*`, no `W n` (clipping comes from `ProcessGraphics`).

**Path points** (`:741-780`): a `CFX_Path::IsRect()` path takes a fast path
emitting `{p0.x} {p0.y} {p2.x−p0.x} {p2.y−p0.y} re` (`:744-749`) — the width
and height may be negative. Otherwise, points separated by a single space,
with ` m`, ` l`, and beziers as `{c1} {c2} {end} c` (consuming three points).
A malformed bezier run emits ` h` and **breaks out of the loop, dropping every
remaining point** (`:763-770`). `close_figure_` appends ` h`. `v`/`y`
shorthands are never emitted.

**Text** (`:943-1017`): `ProcessGraphics`; `BT `;
`{matrix} Tm ` if the text matrix is not identity;
`/{fontname} `; `{size} Tf `; `{int render mode} Tr `; `[`;
per char-code run, hex strings and kerning floats; `] TJ ET`; ` Q\n`.

`GetTextMatrix()` (`core/fpdfapi/page/cpdf_textobject.cpp:198-202`) is
`Matrix(tm[0], tm[2], tm[1], tm[3], pos.x, pos.y)` — note b and c are
**transposed** relative to the stored array.

Font classification (`:958-971`): Type1 ⇒ `"Type1"` + encoding, TrueType ⇒
`"TrueType"` + encoding, CID ⇒ `"Type0"` with **no** encoding; anything else
(Type3!) ⇒ `return` — after `q ` and `BT ` have already been written, leaving
**unbalanced output**. See Divergence D6.

Text state operators never emitted: `Td`, `TD`, `T*`, `Tj`, `'`, `"`, `Tc`,
`Tw`, `Tz`, `TL`, `Ts`. All positioning collapses into `Tm`; character and word
spacing are lost.

**Image** (`:663-701`): degenerate matrix (`(a==0&&b==0)||(c==0&&d==0)`),
inline image, or missing stream ⇒ the object is dropped
(`:665-678`). Otherwise `ProcessGraphics`; `{matrix} cm ` if not identity;
`RealizeResource(stream, "XObject")`; `/{name} Do`; ` Q\n`.

**Form** (`:703-731`): if the form holder has dirty streams, **recurse** —
a nested `CPDF_PageContentGenerator(form).GenerateContent()` (`:705-711`), with
no depth guard and no cycle guard. Then the same degenerate-matrix drop;
`CHECK(pStream)` on a null form stream (a crash, unlike the image path);
`RealizeResource` **before** `ProcessGraphics` (the reverse of the image order,
which changes resource-name allocation order but not output bytes);
`{matrix} cm `; `/{name} Do`; ` Q\n`.

**Shading**: `ProcessPageObject`'s dispatch (`:649-661`) has **no shading
branch** — a shading page object emits nothing and is silently dropped, though
it is still marked clean.

### 2.6 Content marks (`:587-647`)

`FindFirstDifference` against the previous object's marks; emit one `EMC\n`
per mark being closed, then per newly opened mark:
`/{PDF_NameEncode(tag)} ` followed by

- no param ⇒ `BMC\n`;
- direct dict param ⇒ the dict's `WriteTo` (through
  `CPDF_StringArchiveStream`, a trivial ostream adapter whose `CurrentOffset()`
  is `NOTREACHED()`) then ` ` then `BDC\n`;
- properties-dict param ⇒ `/{PDF_NameEncode(propname)} ` then `BDC\n`.

`FinishMarks` closes the remainder. The `/Properties` resource referenced by
the third form is neither created nor preserved by the resource sweep — a
latent inconsistency we inherit.

### 2.7 Resource naming — `RealizeResource` (`:513-552`)

1. If the holder has no `/Resources`, create one indirectly and attach it to
   the page dict as a reference (`:517-522`). (Contrast `UpdateResourcesDict`,
   which silently no-ops when resources are null, `:479-482`.)
2. `GetOrCreateDictFor(type)` for the sub-dictionary.
3. Name search from `idnum = 1`: `format!("FX{}{}", type[0], idnum)`, rejecting
   a name that exists in the sub-dict **or** in the holder's saved
   `all_removed_resources_map()[type]` (removed entries may be restored later),
   incrementing until free.
4. `SetNewFor<Reference>(name, objnum)`; return the name.

Prefixes: `/Font` → `FXF1…`, `/XObject` → `FXX1…` (images *and* forms),
`/ExtGState` → `FXE1…`. Numbering restarts at 1 on each call and takes the
first free slot; it is not a monotonic counter.

`kResourceKeys` (`:57`) is exactly `{"ExtGState", "Font", "XObject"}`, with an
explicit TODO to add ColorSpace/Pattern/Shading. Those three, plus
`/Properties` and `/ProcSet`, are never created, swept, or cloned.

### 2.8 Resource dict maintenance — `UpdateResourcesDict` (`:478-511`)

- Null resources ⇒ return.
- `IsPageResourceShared` (`:221-271`): true if the resources dict's number is in
  `GetObjectsWithMultipleReferences`, **or** if a sweep of every page in the
  document sees the same page dict or the same resources dict twice (O(pages),
  constructing a `CPDF_Page` each time). If shared ⇒ deep-clone the resources,
  add indirectly, repoint the page's `/Resources`.
- `CloneResourcesDictEntries` (`:273-316`): for each `kResourceKeys` entry that
  is a reference to a multiply-referenced object, clone and repoint. Mutation
  is deferred until the dictionary locker drops (`:307`).
- Usage sweep over **active** objects only (`:498-504`) plus the default
  ExtGState name (`:505-507`), then
  `RemoveOrRestoreUnusedResources` (`:169-219`): unused entries are removed and
  parked in `all_removed_resources_map`, and entries needed again are restored
  from it.
- Latent bug at `:186`: `current_resource_dict->GetKeys()` is called when the
  dict may be null (the guard at `:183` only fires when the in-use set is
  *also* absent).

### 2.9 `/Contents` shape — `CPDF_PageContentManager`

Constructor (`cpdf_pagecontentmanager.cpp:29-68`) resolves `/Contents` into a
`variant<RetainPtr<Stream>, RetainPtr<Array>>` where the stream alternative may
be null:

- direct inline array ⇒ `CHECK(IsInline())`, use it;
- reference to an array ⇒ if that array is multiply referenced, **clone it**
  and repoint `/Contents`; else use it;
- reference to a stream ⇒ use it;
- dangling reference or anything else ⇒ null stream (treated as "no contents").

`GetStreamByIndex` (`:78-97`): the single-stream case answers only for index 0;
the array case requires the element to be a reference resolving to a stream —
anything else reads as absent.

`AddStream` (`:99-134`):

| current `/Contents` | action | returns |
|---|---|---|
| single stream | build an indirect array `[old new]`, repoint `/Contents` at it | **1** |
| array | append a reference | `size() − 1` |
| nothing | create the stream, point `/Contents` at it | **0** |

`UpdateStream` (`:136-172`): an empty buffer schedules removal; otherwise, if
the stream is not multiply referenced, replace its data in place
(`SetDataFromStringstreamAndRemoveFilter`, `cpdf_stream.cpp:123-131` — the
filter is stripped); if it *is* shared, copy-on-write into a new indirect
stream and repoint either `/Contents` or the array element's reference.

`ExecuteScheduledRemovals` (`:178-239`) runs in the **destructor** (`:70-72`),
i.e. at the end of `UpdateContentStreams`'s scope and *before*
`UpdateResourcesDict`:

- single stream ⇒ removing index 0 deletes the whole `/Contents` key;
- array ⇒ remove in **descending** index order, then build
  `old → new` index mapping over the survivors and rewrite
  `SetContentStream` on **every** object in the holder (active or not).
  `std::map::operator[]` default-inserts 0 for an unmapped index, so an object
  whose stream was removed — and any object still at `kNoContentStream` —
  collapses to index 0 (`:229-234`).
- Explicit note at `:236-238`: a single-element array stays an array.

`UpdateContentStreams` (`:451-476`) forces `default_graphics_name_` to exist
(`:456`), then, because the map is ordered, handles `kNoContentStream` **first**
so brand-new objects get the lowest available index, calling
`UpdateStreamlessPageObjects` (`:574-585`) to stamp the new index onto every
**active** streamless object.

### 2.10 `ProcessPageObjects` — the form/test path (`:554-572`)

A second, simpler entry: no `q\n` prologue, no default graphics, no trailing
`Q\n`. For a **form** (`IsPage()` false) *every* object is emitted; for a page,
only dirty **and** active ones. Returns whether anything was emitted. This is
what the unit tests exercise, which is why their goldens start with `q ` (from
`ProcessGraphics`) rather than `q\n`.

### 2.11 Number formatting on write

`cpdf_contentstream_write_utils.h:12-18` — four helpers, all returning the
stream so they chain:

- `WriteFloat` → `FloatToDecimal` into a 49-byte buffer. **No leading space,
  no trailing space, no sign for positives.** The algorithm is exactly the
  `fmt_number` already implemented in `pdfrum-object`
  (`docs/design/pdfrum-object.md` §1.2; `crates/pdfrum-object/src/number.rs:164`).
- `WriteMatrix` → `a b c d e f`, single spaces, no brackets, no trailing space.
- `WritePoint` → `x y`.
- `WriteRect` → `left bottom width height` (**not** x0 y0 x1 y1). Unused by the
  content generator — the rect fast path uses two `WritePoint`s.

Callers always supply their own separator: `WriteMatrix(buf, m) << " cm\n"`,
`<< " cm "`, `<< " Tm "`, `WriteFloat(...) << " w "`, `<< " d "`, `<< " Tf "`.

---

## 3. Behavior inventory — page import, N-up, subsetting, trailer ends

### 3.1 Destination setup — `InitDestDoc` (`cpdf_pageorganizer.cpp:40-76`)

Idempotent repair of the destination catalog, in this order:

1. No mutable root ⇒ **fail** (`:41-44`) — the only hard failure.
2. If the destination has an `/Info` dict, set `/Producer` to the string
   `"PDFium"` (`:46-49`). No Info dict ⇒ silently skipped
   (`ImportIntoDestDocWithoutInfo`, `fpdfsdk/fpdf_ppo_embeddertest.cpp:639`).
3. Catalog `/Type` **empty or missing** ⇒ set `/Catalog`. A *wrong* non-empty
   `/Type` is left alone (`:51-53`).
4. `/Pages`: fetch the raw object then `GetMutableDirect()` — so a direct dict
   *or* a reference is accepted. If it does not resolve to a dictionary
   (missing, dangling, or a non-dict), **create a fresh indirect Pages dict and
   overwrite `/Pages`** (`:55-65`).
5. Pages `/Type` empty or missing ⇒ set `/Pages` (`:66-68`).
6. **If `/Kids` is not an array**, create an indirect empty array, set
   `/Count` to `0`, and point `/Kids` at the array by reference (`:70-74`).
   Both keys are written together, so a document with a non-array `/Kids` has
   its `/Count` force-zeroed.

Neither `/MediaBox` nor `/Rotate` is touched here — they are per-page concerns
(§3.3). A freshly created destination (`CPDF_Document::CreateNewDoc`,
`cpdf_document.cpp:493-506`: `/Type /Catalog`, a Pages dict with `/Type
/Pages`, `/Count 0`, a **direct** empty `/Kids` array, plus an Info dict)
passes step 6 untouched.

### 3.2 The deep copy — `GetNewObjId` + `UpdateReference`

`GetNewObjId(ref)` (`cpdf_pageorganizer.cpp:127-166`) is the whole copier:

```
obj_num = ref.target;
if object_number_map_[obj_num] is set → return it            // memoized
direct = ref.resolve();  if none → return 0                  // DANGLING
clone = direct.clone();                                      // refs stay pointing at the source
if clone is a dict with /Type:
    if /Type ==NoCase "Pages" → return 4                     // (!!) hardcoded
    if /Type ==NoCase "Page"  → return 0                     // refuse to copy another page
new_num = dest.add(clone);
object_number_map_[obj_num] = new_num;                       // BEFORE recursing
if !UpdateReference(clone) → return 0
return new_num;
```

- **Memoization is the cycle-breaker.** The mapping is recorded *before* the
  recursive walk (`:160` before `:161`), so a subtree cycling back to
  `obj_num` hits the map instead of recursing. Together with the
  `Parent`/`Prev`/`First` skip below, that is the entire cycle story
  (`BadCircularViewerPref` `:373`, `ImportWithSelfReferentialPageParent`
  `:620`).
- **`return 4` for `/Type /Pages` is a hardcoded magic destination object
  number** (`:153`), not a lookup. It happens to be the Pages dict of a
  `FPDF_CreateNewDocument` document and is silently wrong for any other
  destination. `EqualNoCase` means `/PAGES` and `/pages` hit it too.
  Divergence D13.
- **`return 0` for `/Type /Page`** (`:155`) shares the dangling sentinel, so
  a reference to another source page is *pruned* by `UpdateReference` — this
  is how cross-page `/Annots` destinations get dropped.
- On a `UpdateReference` failure (`:161-163`) the function returns 0 but
  **leaves the map populated and the partial clone registered** in the
  destination.
- `Clone()` is `CloneObjectNonCyclic(bDirect = false)`, so references clone as
  references **still naming the source document's object numbers**
  (`cpdf_reference.cpp:52-64`) — which is exactly why the walk must re-point
  them. `CPDF_Dictionary::CloneNonCyclic` (`cpdf_dictionary.cpp:54-71`) drops
  any key whose child is already in the visited set, and passes a *copy* of
  that set down each branch so siblings do not poison each other.

`UpdateReference(obj)` (`:78-125`) rewrites the clone in place:

- **Reference**: `GetNewObjId`; 0 ⇒ return false, else re-point at the
  destination.
- **Dictionary**: iterate, **skipping the keys `Parent`, `Prev`, `First`
  entirely** (`:96-98`) — the page-tree and outline back/sibling/child
  pointers, whose omission both breaks cycles and stops the copy dragging in
  the whole source page tree. Keys whose recursion failed are collected and
  **removed after the iteration** (`:105-107`); the dictionary itself still
  succeeds.
- **Array**: the **first** failing element aborts the whole array (`:110-118`)
  — no per-element removal, unlike dictionaries.
- **Stream**: recurses into its dictionary only; **stream bytes are never
  walked**.
- Everything else: unchanged, success.

**Streams are copied raw** in this path: `CPDF_Stream::CloneNonCyclic`
(`cpdf_stream.cpp:100-115`) uses `LoadAllDataRaw()`, so the encoded bytes and
the `/Filter`/`/DecodeParms` travel together, unre-encoded. "Raw" means
undecoded by `/Filter`; the crypto layer was already peeled off at parse time,
so an encrypted source yields decrypted-but-still-compressed bytes. §3.6.

### 3.3 `CPDF_PageExporter::ExportPages` (`cpdf_pageexporter.cpp:20-92`)

Signature `ExportPages(page_indices: &[u32], index: i32) -> bool`, both 0-based.
Per source page, with `curpage` starting at `index` and **incrementing by one**
so a batch lands as a contiguous run in source order:

1. `dest.CreateNewPage(curpage)` — allocates an indirect dict with
   `/Type /Page` and inserts it into the destination tree, which is what
   writes the correct destination `/Parent` and bumps `/Count` along the path
   (`cpdf_document.cpp:508-517`, `:519-606`). Note this runs **before** the
   source page is fetched.
2. `src.GetPageDictionary(pageIndex)`; either null ⇒ **return false**,
   leaving the already-inserted blank page behind (§3.6).
3. Copy every source key **except `/Type` and `/Parent`** as a shallow clone
   (`:35-45`) — `/Type` was already written by step 1, `/Parent` must stay
   pointing at the destination.
4. Inherited-attribute flattening, **exactly four keys in this order**
   (`:47-81`), each via `CopyInheritable` (which is a no-op when the key was
   already copied in step 3, giving correct page-over-ancestor precedence):
   - **`/MediaBox`** — on failure, fall back to the inheritable **`/CropBox`**;
     on failure again, the default **US Letter** rect, written by `SetRectFor`
     as `[0 0 612 792]` in `left bottom right top` order (`:51-66`).
   - **`/Resources`** — on failure, an **empty direct dict** (`:68-74`).
   - **`/CropBox`** — result ignored (`:76-78`).
   - **`/Rotate`** — result ignored (`:79-81`). No normalization, no default:
     the value is inherited verbatim or left absent. (Normalization to
     `(v/90) % 4` happens only at render time,
     `cpdf_page.cpp:230-232`.)
   `/BleedBox`, `/TrimBox`, `/ArtBox` are **not** in the list.
5. `AddObjectMapping(src_page_objnum, dest_page_objnum)` **before**
   `UpdateReference(dest_page_dict)` (`:83-88`), so a self-reference from
   inside the page (an annotation's `/P` back-pointer) resolves to the new page
   rather than being pruned by the `/Type /Page → 0` rule. The walk's return
   value is ignored here.

`CopyInheritable` (`:169-185`) returns early-true when the destination already
has the key; otherwise `PageDictGetInheritableTag` and a plain `Clone()`.

`PageDictGetInheritableTag(dict, tag)` — the `/Parent` walk (`:188-232`) —
requires, all returning "not found": a non-null dict and non-empty tag; **both**
`/Parent` and `/Type` present (`:194-196`); `/Type` resolving to the Name
`Page` **case-sensitively** (contrast `GetNewObjId`'s `EqualNoCase`,
`:199-203`); `/Parent` resolving to a dictionary (`:205-209`). Then: **the
dict's own value wins** (`:211-213`); otherwise walk parents with a
pointer-keyed visited set, and on a **revisit return "not found" outright**
rather than any value found so far (`:215-230`). Ancestor `/Type` is not
checked.

**`object_number_map_` is NOT cleared between pages.** A font, image XObject or
shared `/Resources` referenced by two exported pages is copied into the
destination **exactly once** and both destination pages point at it. That is
the deduplication guarantee, and it is why the map lives on the organizer
rather than the per-page loop.

### 3.4 N-up — `CPDF_NPageToOneExporter`

`ExportNPagesToOne(page_indices, dest_page_size, x, y)`
(`cpdf_npagetooneexporter.cpp:155-211`). Overflow-guards `x * y`; clears
`object_number_map_` and `src_page_xobject_map_` **once**; builds one `NupState`
for the whole run; `curpage` **always starts at 0** — there is no insertion
index. Per output sheet: clear `xobject_name_to_number_map_`, `CreateNewPage`,
set `/MediaBox` to `[0 0 width height]` (always the caller's size, no
inheritance, no `/Rotate`), accumulate the sub-page fragments, `FinishPage`.
Output page count is `ceil(len / (x·y))`; the last sheet is short.

**The geometry** (`NupState`, `:36-129`). `sub_page_size = dest_page_size / (x, y)`.
`ConvertPageOrder` (`:88-96`):
`sub_x = i % x`, `sub_y = y − (i / x) − 1` — fill order is left-to-right,
top-to-bottom in visual terms, converted to a bottom-origin slot index.
`CalculatePageEdit` (`:98-118`), exactly:

```
start.x = sub_x * sub_w;   start.y = sub_y * sub_h;
scale   = min(sub_w / page_w, sub_h / page_h);          // uniform, aspect-preserving
if (sub_w / page_w > sub_h / page_h)  start.x += (sub_w - page_w * scale) / 2;
else                                  start.y += (sub_h - page_h * scale) / 2;
```

Centering is on **exactly one axis** — the slack one; the strict `>` sends the
exact-tie case down the `else` branch, which adds zero. **No clamping**: a
source page smaller than its slot is scaled *up*.

`src_page->GetPageSize()` is the *cropped, rotation-swapped* size
(`cpdf_page.cpp:266-298`): MediaBox defaulting to `(0,0,612,792)` when empty,
intersected with CropBox when present, with width and height **swapped for
rotations 1 and 3**.

**The sub-page content fragment** — `GenerateSubPageContentStream`
(`:131-145`), exact bytes:

```
q\n{a} {b} {c} {d} {e} {f} cm\n/{Name} Do Q\n
```

with the matrix `[scale, 0, 0, scale, start.x, start.y]` (scale then
translate). Note the space before `cm`, the newline *after* it, and that `Q`
sits on the same line as `Do`. Fragments are concatenated with **no separator**
— each fragment's trailing `\n` is the only delimiter.

**The Form XObject** — `MakeXObjectFromPageRaw` (`:231-276`):

- **Only `/Resources` is carried over** from the source page (via
  `CopyInheritable`, defaulting to an empty dict); every other page key —
  `/Annots`, `/Group`, `/CropBox`, … — is dropped.
- `AddObjectMapping(src_page_objnum, new_xobject_objnum)` then
  `UpdateReference`, so anything inside the resources that referenced the page
  gets re-pointed at the form.
- **`/Type /XObject`, `/Subtype /Form`, `/FormType 1` are set *after* the
  reference walk** — deliberately, so the walk sees a `/Type`-less dict and
  does not trip `GetNewObjId`'s Pages/Page special cases.
- `/BBox` = `src_page->GetBBox()` (the CropBox∩MediaBox rect), written
  `[left bottom right top]`.
- `/Matrix` = `src_page->GetPageMatrix()` (`cpdf_page.cpp:282-297`), one of:
  rot 0 `[1 0 0 1 −left −bottom]`, rot 1 `[0 −1 1 0 −bottom right]`,
  rot 2 `[−1 0 0 −1 right top]`, rot 3 `[0 1 −1 0 top −left]`. This is what
  makes rotated and offset source pages land correctly inside the form.
- **Content is decoded, not copied raw** (`:255-275`) — the one place in the
  module that re-encodes. A single `/Contents` stream is
  `LoadAllDataFiltered()` then `SetDataAndRemoveFilter`; an array has each
  element decoded and appended with a `"\n"` after **every** element including
  the last (`:272`), which is the fix for content split mid-token across
  elements. No `/Contents` ⇒ an empty form.

**Naming**: `format!("X{}", ++object_number_)` → `X1`, `X2`, … (`:278-287`),
a never-reset pre-incremented counter, with an explicit
`TODO(xlou)` acknowledging the collision risk. `AddSubPage` (`:220-229`) reuses
an existing XObject by *source page object number*, so importing the same page
twice creates one form and two invocations.

`FinishPage` (`:304-319`) writes `/Resources` and `/Resources /XObject` as
**direct** dicts holding indirect references to the forms, and the page's
`/Contents` as a single **indirect, uncompressed** stream.

### 3.5 Page ranges and viewer preferences (`fpdfsdk/fpdf_ppo.cpp`)

The public shims are not ported (PLAN §1: no C ABI), but two pieces of core
logic are:

**`ParsePageRangeString(range, count)`**
(`fpdfsdk/cpdfsdk_helpers.cpp:538-577`). Exact grammar:

1. The only legal characters are space, `0`-`9`, `-`, `,`; anything else ⇒
   **total failure** (empty result).
2. **All spaces are stripped globally, including inside numbers** —
   `"5  0, 1-2"` becomes `"50,1-2"` ⇒ `{49, 0, 1}`. The C++ unit test flags
   this with a literal `// ???` comment.
3. Split on `,`; each entry splits on `-` into at most 2 parts; **3+ parts ⇒
   total failure**.
4. Single entry: `n == 0 || n > count` ⇒ failure. `atoi` of an empty entry is
   0, so `",1"`, `"1,"` and `",,"` all fail.
5. Range `a-b`: `a == 0`, `b == 0`, `a > b`, or `b > count` ⇒ failure. `a` is
   never bounds-checked directly; it is transitively bounded by `a ≤ b ≤ count`.
   Both ends **inclusive**, expanded elementwise.
6. Numbers are 1-based; output indices are 0-based.
7. **Duplicates and out-of-order entries are legal**: `"1-4,3-6"` ⇒
   `{0,1,2,3,2,3,4,5}`; `"2,1"` ⇒ `{1,0}`.
8. **Failure is all-or-nothing** — one bad entry discards everything.
9. An **empty** range string parses to an empty result, but the *caller*
   (`fpdf_ppo.cpp:35-45`) short-circuits it to "all pages in order" before ever
   calling the parser.

**`CopyViewerPreferences`** (`fpdf_ppo.cpp:247-281`): read the source catalog's
`/ViewerPreferences` **as a dict** (a non-dict or dangling value reads as
absent ⇒ fail); build a **direct** dict on the destination catalog holding a
filtered shallow clone of each entry; **unconditionally replace** any existing
destination entry, returning true even when the filter dropped everything.
The filter, `IsValidViewerPreferencesObject` (`:59-72`), rejects dictionaries,
nulls, references and streams outright; an array is rejected if **any** element
is an array, dictionary, reference or stream (`:47-57`) — one level deep, no
nesting, no references. **That reference rejection is the entire defense
against repeated and circular preference graphs** (`BadRepeatViewerPref`,
`BadCircularViewerPref`).

### 3.6 Damage tolerance and encryption in the import path

- **Nothing is transactional.** A missing source page returns false *after*
  the destination page was already created and inserted, and after earlier
  pages of the batch were fully exported. Ported as-is would leave a stray
  blank page; see Divergence D14.
- **Missing geometry is defaulted, never fatal**: absent MediaBox ⇒ letter,
  absent Resources ⇒ empty dict, absent/zero-length `/Contents` ⇒ an empty
  form (`ImportWithZeroLengthStream`).
- **Dangling references are pruned, not fatal** (§3.2).
- **A cyclic `/Parent` chain** makes the attribute simply not inheritable
  (`ImportWithSelfReferentialPageParent`).
- **`CPDF_Page`'s constructor `CHECK`s** `IsValidPageDictLoose`
  (`cpdf_page.cpp:31`) — a hard crash on a page dict whose `/Type` is present
  and is not `Page` — and, when `/Type` is *absent*, **writes `/Type /Page`
  into the source document's dict** (`:33-36`). So the N-up path is not
  read-only with respect to the source. Both become `Result` + no-mutation in
  our port (D10, D15).
- **Encryption is entirely a save-time concern.** Nothing in the organizer,
  exporter or N-up exporter mentions encryption. Objects live in memory
  decrypted (the source's handler peeled it off at parse time), the source's
  `/Encrypt` dictionary is **never** copied into the destination, and the
  destination's own handler re-encrypts everything at save. Net: importing
  from an encrypted source into an unencrypted destination **strips the
  encryption**; into an encrypted destination, it re-encrypts under the
  *destination's* key. No permission bit is consulted anywhere in the import
  path.

### 3.7 Font subsetting (`cpdf_fontsubsetter.cpp`, `cpdf_font_util.cpp`)

`cpdf_font_util.cpp` is **not** part of the import path — its only includers
are the subsetter and `fpdfsdk/fpdf_edittext.cpp`. It belongs here.

Runs only under `kSubsetNewFonts`, only over `new_obj_num_array_`, and
**creates only new objects** — it never mutates the document
(`cpdf_fontsubsetter.h:25-29`). Result is a `map<objnum, Object>` of overrides
consumed by §1.7.

**Candidate collection** (`:233-301`). For every page, parse content, and for
every *text* object:

1. `font->GetFontDict()` must have a number in `new_obj_nums`
   (`binary_search`, hence the sortedness requirement) — otherwise skip.
2. CID font ⇒ `cid_font = root["DescendantFonts"][0]` (with `CHECK`s),
   `descriptor = cid_font["FontDescriptor"]`; simple font ⇒
   `descriptor = root["FontDescriptor"]`. No descriptor ⇒ skip.
3. `font_stream = descriptor["FontFile2"]`. **`FontFile` (Type 1) and
   `FontFile3` are skipped** — HarfBuzz cannot subset Type 1, and PDFium only
   ever writes new embedded fonts as `FontFile`/`FontFile2` internally.
4. Keyed by the font stream's object number, a `SubsetCandidate` accumulates:
   the subset name (minted once), the four object handles, the used GID set,
   a charcode→width map (only when the CID font has a `/W` array), and a
   charcode→unicode multimap.

`AddUsedText` (`:303-328`): per char code, `GlyphFromCharCode` (≠ −1 ⇒ insert
the GID), `GetCharWidth` (≥ 0 ⇒ record), and `UnicodeFromCharCode`'s **first**
code unit (non-empty ⇒ record).

**The subset name** (`:105-120`): strip an existing prefix via
`MaybeRemoveSubsettedFontPrefix` (`core/fxge/fx_font.cpp:217-223`: length > 6,
`name[6] == '+'`, and the first six bytes are all uppercase), then six random
uppercase letters (`'A' + (rand % 26)`), `'+'`, the base name. Pinned by
`ReplaceExistingPrefix` (`cpdf_fontsubsetter_embeddertest.cpp:514-551`):
`AAAAAA+Arimo-Regular` must come back as `XXXXXX+Arimo-Regular`, six
uppercase letters and exactly one `+`.

**The subset call** (`:59-99`): HarfBuzz with
`HB_SUBSET_FLAGS_RETAIN_GIDS | HB_SUBSET_FLAGS_NOTDEF_OUTLINE`, the GID set,
face index 0. **`RETAIN_GIDS` is the load-bearing flag**: glyph IDs are *not*
renumbered, so `/Widths`, `/W`, the CMap and `/ToUnicode` all stay valid
without touching the charcode→GID mapping at all. Divergence D1 is entirely
about this.

**The overrides produced** (`:138-226`), per candidate:

| Object | Override |
|---|---|
| the font file stream | a **new stream** carrying the subset bytes with a fresh dict: `/Subtype /OpenType` if the *original* bytes start with `OTTO`, else `/Length1 = subset size`. Nothing else — no `/Filter`, so it is written uncompressed and then flate-encoded by the stream writer (§1.13). |
| root font dict | clone + `/BaseFont = subset name` |
| CID font dict (if any) | clone + `/BaseFont = subset name`; `/Subtype /CIDFontType0` if OpenType-CFF |
| the `/W` array (if any) | rebuilt from `char_code_to_width` by `CreateWidthsArray` |
| font descriptor | clone + `/FontName = subset name`; if OpenType-CFF also `Flags |= 0x04` (symbolic), `Flags &= ~0x20` (clear nonsymbolic), remove `/FontFile2`, set `/FontFile3` → the stream |
| `/ToUnicode` stream (if any) | rebuilt by `LoadUnicode` |

If the subset came back empty, the candidate is skipped entirely — the
original font survives unmodified.

**What we implement, against this inventory (2026-09-03).** The collection
ladder is ported rung for rung, with the candidate keyed by the font-program
object number and the used-glyph set accumulated across every page that draws
with it. Four deliberate differences, each a consequence of §5's D1:

1. **Candidates are narrower.** A candidate must be `/Type0` with a
   `CIDFontType2` descendant. The C++ admits a *simple* font with
   `/FontFile2` too; a subsetted simple font would render nothing here,
   because the `subsetter` crate removes the `cmap` its codes reach glyphs
   through. `/FontFile` (Type 1) is skipped by both.
2. **An `OTTO` program is skipped rather than converted.** The C++ subsets it,
   switches the descendant to `/CIDFontType0` and moves the stream to
   `/FontFile3`. A `CIDFontType0` reaches glyphs with the CID *as* the glyph
   index and never consults `/CIDToGIDMap` (`cpdf_cidfont.cpp:508-518`), so
   the renumbering would have nowhere to go but the content streams.
   `IsOpenTypeCFF` is still ported, as the test that recognises the case.
3. **`/W` and `/ToUnicode` are carried through untouched**, so neither
   `CreateWidthsArray` nor `LoadUnicode` is ported — the two modules that held
   them are deleted. Both are keyed by CID, and the CID space does not move.
   The C++'s rebuild of them is a *pruning* to the codes still drawn; a
   correct reader cannot observe the difference, and R15's text-extraction
   obligation is met trivially rather than by reconstruction.
4. **One object is added**: the `/CIDToGIDMap` stream, a big-endian `u16` per
   CID sized to the highest CID drawn (ISO 32000-1 §9.7.4.2). It is what makes
   items 1 and 3 possible.

5. **Every object in the chain must be new, not just the root font.** The
   C++ tests `binary_search(new_obj_nums, root_font->GetObjNum())` and nothing
   else (`:262-265`), which is safe *for it* because it only runs over fonts
   PDFium itself created, where the whole chain is new by construction and the
   1:1 mapping its header promises holds. We run over whatever a caller
   imported, so an old `/Font` dictionary sharing the same `/FontFile2` would
   be left pointing at a subset built for somebody else's glyph set. The
   descendant, the descriptor and the program are all checked. Costs nothing:
   the import path copies the whole chain into fresh numbers.

A candidate whose subset is not *smaller* than the original is also skipped,
which the C++ does not check — it costs five rewritten objects to find out,
and the option exists to make files smaller.

**What can produce a new embedded font today, audited 2026-09-03.** Exactly
one path: **page import**. `import_pages` and `n_page_to_one` share
`import/copy.rs`'s `copy_object`, which gives every object it follows a fresh
destination number, and a `/FontFile2` inside a `/FontDescriptor` matches none
of the pruning rules — so the whole `/Type0` → `/DescendantFonts` →
`/FontDescriptor` → `/FontFile2` chain is copied new. That is what §3.7's
divergence 5 relies on, and it is a real path with real files.

Nothing else reaches this stage, and the audit is worth recording because the
list is shorter than it looks:

- **`pdfrum::edit`'s `TextBuilder` embeds nothing.** It takes an `ObjRef` to a
  `/Font` the page's resources can already reach and sets `font_source`; the
  only way a caller obtains one is `PageEdit::font_of`, reading it back off an
  existing text object.
- **Content regeneration mints no font.** `ResourceTable::realize` allocates a
  *name* for an object that already exists; `realize_dict`, which can mint an
  inline dictionary, is called only for `/ExtGState`.
- **Appearance generation embeds no program.** `ap::font_map`'s
  `substitute_font_dict` does build a `/FontDescriptor`, but only to carry the
  non-symbolic flag — `/BaseFont` names a face the substitution machinery
  resolves at render time, and there is no `/FontFile*` at all. Nothing to
  subset.

**`EditDoc::embed_font` / `standard_font` is the `FPDFText_LoadFont` equivalent
(landed 2026-09-03).** It constructs the same four-object chain
`collect::admit` already recognises (`/Type0` → `/DescendantFonts` →
`/FontDescriptor` → `/FontFile2`, or `/FontFile3` for `OTTO`). A font
embedded this way and used by a show operator on a regenerated page is a
subset candidate with **no collector change** — the audit's prediction that
the stage would subset its output unchanged was correct.

Three corrections against the oracle's writer (`fpdfsdk/fpdf_edittext.cpp`),
each marked `// [oracle-bug]` at the site with both a PDFium citation and a
pdf.js citation (PLAN.md oracle-bug rule). pdf.js is a reader, not a writer,
so the pdf.js lines are the ones that *depend* on the value:

- **OTTO** is `/FontFile3` `/Subtype /OpenType` and a `/CIDFontType0`
  descendant, not `/FontFile2` (`LoadFontDesc` `:171-174`). This crate's
  subsetter already treated OTTO that way (`is_opentype_cff`). pdf.js:
  `isOpenTypeFile` sniffs `OTTO` (`src/core/fonts.js:319`) and
  `getFontFileType` classifies it `"OpenType"` (`:357`); `checkAndRepair`
  requires `fontFileN === "FontFile3"` for an OTTO CFF CID (`:2761-2763`).
  `translateFont` walks FontFile/2/3 (`src/core/evaluator.js:4633`) and
  reads the stream dict `/Subtype` (`:4668-4670`).
- **Type 1 `/FontFile`** is *unwrapped out of its PFB container* and stored
  raw, with `/Length1` `/Length2` `/Length3` partitioning what was stored
  (ISO 32000-1 §9.9 table 127). The oracle stores the caller's bytes verbatim
  and leaves all three off (`:166` TODO), so a PFB reaches `/FontFile` with
  its `[0x80, type, len:u32le]` record headers and `80 03` end marker intact
  and nothing describing them. A PFB is a container, not a program:
  concatenating its record bodies in order yields exactly the PFA-shaped raw
  program table 127 describes — clear text, `eexec` binary, the
  512-zeros-plus-`cleartomark` trailer — and the three lengths are those
  bodies' sizes. `FoxitSerifMM.pfb` is 113417 B in, 113397 B stored
  (10710 + 102155 + 532), the 20 bytes of framing gone. A PFA or a bare
  program has no wrapper and is stored as-is, its private portion left in
  whatever form it arrived in — hexadecimal stays hexadecimal, which §9.9
  permits, and `/Length2` counts the hex digits. `pdfrum_type1::font_file`
  returns the bytes and the three lengths *together* (`FontFile`), which is
  what makes the disagreement unrepresentable rather than merely fixed.
  pdf.js: `translateFont` pulls the three lengths off the stream dict
  (`src/core/evaluator.js:4672-4674`); `Type1Font.#parseType1` splits the
  header and eexec blocks with `properties.length1` / `properties.length2`
  (`src/core/type1_font.js:195-201`) — given the oracle's output it would
  slice PFB headers as font data.
- **`/CapHeight`** uses OS/2 `sCapHeight` when present, else the oracle's
  ascent fallback (`:160-161`). pdf.js: `translateFont` reads
  `descriptor.get("CapHeight")` (`src/core/evaluator.js:4731`); `Font`
  stores `this.capHeight = properties.capHeight / PDF_GLYPH_SPACE_UNITS`
  (`src/core/fonts.js:1123`).

Unmappable `encode` codes are 0, matching `CharCodeFromUnicode`.

The brief's claim that "`TextBuilder` embeds nothing" is now only half
true: `TextBuilder` still takes an `ObjRef`, but `DocEdit::embed_font`
is how a caller obtains one that did not already live on the page.
`ImageBuilder` still has the same "must already exist" limitation.

**Latent emitter bug.** `emit_text_body` used to return `false` whenever
`text.font` was `None`, and whenever `font_subtype` was `None` (Type 3).
That refusal made `TextBuilder` non-functional for new text: a constructed
object names the dict through `font_source` and stores size only in the
glyph matrix, so there is no `Font` to classify. Loading a Helvetica
stand-in on `TextBuilder::build` just to pass the check was the wrong kind
of fix — a text object whose `font_source` names an embedded Roboto must
not carry a Helvetica `Font`. The emitter now takes the constructed path:
skip the Type 3 refusal (the caller named the dict), write `Tf` with the
matrix scale and `Tm` with the scale divided out (cleaner than `Tf 1`
with the scale left in the matrix).

`IsOpenTypeCFF` (`core/fxge/fx_font.cpp:225-231`) is a four-byte `OTTO` tag
test on the **original** (filtered) font bytes.

**`CreateWidthsArray`** (`cpdf_font_util.cpp:82-118`) emits the two ISO
32000-1 §9.7.4.3 forms from an ordered charcode→width map:
`c_first c_last w` when two-or-more consecutive codes share a width, else
`c [w1 w2 …]` gathering the consecutive-code run. Order is the map's, i.e.
ascending charcode.

**`LoadUnicode`** (`:120-273`) builds a CMap from the multimap in three
buckets — singletons (`beginbfchar`), consecutive-code/non-consecutive-unicode
runs (`beginbfrange` with a `[…]` array), and consecutive-code/consecutive-unicode
runs (`beginbfrange` with a single start value) — each chunked at
`kMaxBfCharBfRangeEntries = 100` per block (`:29`). Runs stop at a 256-byte
boundary (`max_extra = 255 - (code % 256)`, `:155`) because ISO 32000-1
§9.10.3 only permits the low byte to vary within a range; a run starting at
`code % 256 == 0` is degraded to two singletons (`:150-154`). Surrogate
halves are written as `<0000>` (`:65-78`). The literal prologue and epilogue
(`kToUnicodeStart` `:31-44`, `kToUnicodeEnd` `:46-50`) are fixed text
declaring `Adobe-Identity-H`, `/CMapType 2`, and codespace `<0000> <FFFF>`.

### 3.8 `GetTrailerEnds` — *not* an edit concern

The parser brief's open question 4 asked whether `GetTrailerEnds`
(`cpdf_parser.cpp:1350-1396`) lands here. **It does not.** Its only consumer is
the public C API `FPDF_GetTrailerEnds` (`fpdfsdk/fpdf_view.cpp:1528-1545`); no
code in `core/fpdfapi/edit/` calls it, and the incremental-save path uses
`GetDocumentSize()`, not trailer ends. It belongs to the facade if we ever
expose the equivalent, and the golden values live with the parser
(`fpdfsdk/fpdf_view_embeddertest.cpp:1879-1961`: `two_signatures.pdf` →
`{633, 1703, 2781}`, `hello_world.pdf` → `{840}`,
`annotation_stamp_with_ap.pdf` → `{441, 7945, 101719}`,
`linearized.pdf` → `{474, 11384}`,
`trailer_end_trailing_space.pdf` → `{1193}`). Recorded here so the question
closes.

---

## 4. Round-trip obligations

The invariants that must hold for PLAN.md M7's two exit criteria. Each is a
property test in §6.

### 4.1 For "the oracle reopens our saved files"

| # | Invariant | Why it is load-bearing |
|---|---|---|
| R1 | Every `%010d` xref offset equals the byte offset of the `N` in that object's `N 0 obj` header, measured from the start of the emitted file (including the copied prefix on an incremental save). | PDFium's `ParseIndirectObjectAt` rejects an object whose parsed header number ≠ the requested one (parser brief §1.13), so a shifted offset makes the object unfetchable, not merely slow. |
| R2 | `startxref` names the byte offset of the `xref` keyword (or of the xref stream's `N 0 obj`). | `ParseStartXRef` requires `offset < document size` and the chain load fails otherwise, forcing a rebuild — recoverable but a silent fidelity change. |
| R3 | Every stream's `/Length` equals the number of bytes between `stream\r\n` and `\r\nendstream`. | PDFium tolerates a wrong `/Length` (it re-scans for `endstream`) but records a diagnostic and may truncate; the oracle's own writer guarantees it, so must ours. |
| R4 | `/Size` ≥ (largest object number written) + 1. | `SetObjectMapSize` is driven by `/Size`; an understated value makes high objects unfetchable. |
| R5 | `/Root` is a **reference**, and its target is a fetchable dict with a non-empty page tree. | Parser brief §1.14 step 3: a direct-dict `/Root` is invalid and a failed `TryInit` triggers a rebuild. |
| R6 | Every reference written names an object that has an xref entry (or is deliberately absent, in which case it must read as null). | The reachability sweep of §1.6 guarantees this for a full save *provided* we sweep the same graph the writer walks. |
| R7 | On an incremental save, byte `0..original_len` of the output is identical to `Document::bytes()`. | The whole point of the append discipline: signatures, byte-range digests and the `/Prev` chain all depend on it. |
| R8 | On an incremental save, `/Prev` names the *original* file's last xref offset, and the appended xref covers exactly the appended objects. | Two `startxref`s / two `%%EOF`s / one `/Prev` is the pinned shape (`fpdf_save_embeddertest.cpp:296-302`). |
| R9 | Generation numbers are 0 everywhere on output — in `N 0 obj`, in `N 0 R`, and in the `00000` xref field — with the sole exception of the free head's `65535`. | §1.9; also `Bug342`. |
| R10 | An encrypted output declares `/Encrypt` **and** every string/stream in every non-exempt indirect object is enciphered with the same handler. Metadata streams and signature `/Contents` are exempt. | §1.9, §1.13. A mixed file is unopenable. |

### 4.2 For "save → reload → re-render matches Tier-B"

| # | Invariant |
|---|---|
| R11 | Reloading the output yields the same page count and the same page dictionaries' semantic content (boxes, rotation, resources reachable). |
| R12 | For every page whose objects were **not** dirtied, `/Contents` is byte-identical to the input's (the early-out of §2.1 guarantees this). |
| R13 | For every regenerated page, the reparsed page-object list is equivalent to the pre-save one **modulo the documented losses**: non-RGB/Gray colors, patterns, shading objects, Type 3 text, `Tc`/`Tw`/`Tz`/`TL`/`Ts`, miter limit, SMask. These are C++ behaviors (§2.4, §2.5) and are therefore *expected* Tier-B differences, not bugs — the conformance harness must treat "regenerated page" as a distinct cluster with its own threshold. **(M11, 2026-08-30: measured, and the cluster needed no threshold of its own.** `conformance mutate-round-trip` compares the *oracle's* render of a page we regenerated against *ours* of the same file, so the documented losses fall on both sides and cancel: whatever the emitter dropped, it dropped for both readers. The floor is therefore the ordinary Tier-B 0.99, and 287 of 287 applied mutations reach it. The one thing that had to be added is not a threshold but a second question — see the harness's `mutation` module on why SSIM alone cannot separate a mutation's loss from a disagreement that predates it.) |
| R14 | Decoded stream bytes survive: a stream that already had a filter is copied verbatim (§1.13 row 2), so `--save-images` md5s must be unchanged for untouched images. |
| R15 | Text extraction over the reloaded document matches, for pages whose fonts were not subsetted. With subsetting, extraction must still match because `/ToUnicode` is regenerated to cover exactly the used codes (`fpdf_save_embeddertest.cpp:362-383` asserts the extracted text of a subsetted save char-for-char). |

### 4.3 Determinism

The C++ output is **not** reproducible: `/ID[1]` and the six-letter subset tag
come from a process-global Mersenne Twister. STYLE.md §1 forbids global state,
and a non-reproducible writer cannot be snapshot-tested. Design decision:
`SaveOptions` carries an explicit `id_source: IdSource` and the same seed
threads into subset-tag generation, with `IdSource::Random` (from
`std::hash`/time, no global) as the default and `IdSource::Fixed([u8; 16])` for
tests and for byte-reproducible output. See Divergence D4.

---

## 5. Divergences

**D1 — `subsetter` renumbers GIDs; HarfBuzz's `RETAIN_GIDS` does not. This is
the single largest behavioral divergence in the crate.**

DEPS.md pins `subsetter = 0.2.6` (typst's). Its API is
`subset(data: &[u8], index: u32, mapper: &GlyphRemapper) -> Result<Vec<u8>>`
with `GlyphRemapper::new_from_glyphs_sorted(&[u16])`, and it **always** produces
a contiguous GID space starting at 0 with `.notdef` at 0 — there is no
retain-GIDs mode (verified by reading the fetched crate source: `remapper.rs`'s
doc comment states "a font needs to have a contiguous sequence of glyph IDs
that start from 0, so we cannot just reuse the old ones"). It also **removes
`cmap` unconditionally** (`lib.rs`'s `_subset`: "CID fonts in PDF define their
own cmaps, so we don't need to include them"), and drops `OS/2`, `gasp`,
`VORG`. Surviving tables: TrueType ⇒ `glyf`+`loca`, plus `cvt `/`fpgm`/`prep`
when not instancing; CFF ⇒ `CFF `; always ⇒ `head`, `hmtx`, `maxp`, `name`,
`post`. CFF2 is unsupported (`Error::Unimplemented`) unless the
`variable-fonts` feature is on, which converts it to TrueType via skrifa.

Consequences we must handle and PDFium does not:

1. **The PDF-side glyph mapping must be rewritten.** With `RETAIN_GIDS`,
   PDFium changes nothing but the name. With `subsetter`, GID *g* becomes
   `remapper.get(g)`, so for a **CID font** we must emit a `/CIDToGIDMap`
   stream (or, since the subsetter documents an identity GID↔CID mapping in
   the output, re-key the whole font to use the new GIDs as CIDs and rewrite
   the encoding CMap accordingly). For a **simple font** the `/Widths` array
   is indexed by char code, not GID, so it is unaffected — but the font's own
   `cmap` is gone, which for a simple TrueType font is how the viewer maps
   codes to glyphs at all.
2. **Therefore: v1 subsets only CID (Type 0) fonts**, matching the corpus the
   C++ tests use (every `cpdf_fontsubsetter_embeddertest.cpp` case loads with
   `cid=true`) and matching the subsetter crate's stated scope ("You must write
   your fonts as a CID font"). A simple font with `/FontFile2` is **skipped**,
   exactly as Type 1 already is. This is a *narrowing* of the C++'s behavior,
   not a fidelity change to any file the C++ subsets in practice.
3. **`/W` must be re-keyed.** PDFium's `CreateWidthsArray` is keyed by char
   code and, under Identity-H with retained GIDs, char code = CID = GID. Under
   our remapper, CID becomes the *new* GID, so the widths map must be built as
   `new_gid → width`, not `char_code → width`. The `CreateWidthsArray` run
   encoding itself is unchanged.
4. **`/ToUnicode` must be re-keyed the same way**, since it maps the *code*
   used in the content stream, which after re-keying is the new CID.
5. **The content stream's char codes change.** Under Identity-H the code *is*
   the CID; re-keying CIDs means rewriting the `TJ` hex strings of every text
   object using that font. This makes subsetting inseparable from content
   regeneration for those pages — a coupling the C++ does not have. `SaveMode`
   therefore gains the rule: subsetting implies regenerating the content of
   every page containing a subsetted font.
6. **Size expectations shift.** The C++ tests assert subset sizes as a fraction
   of the original (2–3.5%); ours will differ because a different set of tables
   survives. Those assertions port as *ranges with our own bounds*, recorded
   with the measured value.

This is a large enough behavioral delta that it is called out as an
**escalation** (E4) as well as a divergence: an alternative is to write our own
retain-GIDs subsetter, which DEPS.md's closed set forbids without a `[spec]`
change.

**Revised 2026-09-03, when the stage was wired. Items 3, 4 and 5 above
describe a design that was not built, and item 1's parenthetical is the one
that was.** The renumbering is absorbed in a rewritten **`/CIDToGIDMap`**, not
by re-keying. ISO 32000-1 §9.7.4.2 already defines a per-CID glyph index for a
`CIDFontType2`, so writing one that sends each CID to its *new* glyph leaves
every other thing that named a glyph alone. What that changes about the four
items:

- **3 and 4 are wrong as written.** `/W` and `/ToUnicode` are keyed by CID,
  the CID space does not move, and both are carried through **untouched**.
  `font/widths.rs` and `font/tounicode.rs`, which implemented the re-keying,
  are deleted.
- **5 is wrong, and it was the expensive one.** No content stream is
  regenerated and no char code changes, so subsetting is *not* coupled to
  content regeneration and `SaveMode` gains no rule. That coupling was never
  merely awkward: `crate::content`'s emitter drops character spacing, word
  spacing, shadings, text clips and soft masks, so a page regenerated to save
  a few kilobytes of font would have come back visibly changed. The wired
  stage's proof is the opposite assertion — a subsetted save renders
  **pixel-identically** to the same save without it
  (`crates/pdfrum/tests/subset.rs`).
- **2 stands, and narrows once more.** Only CID fonts are subsetted, and among
  those only `CIDFontType2`: an OpenType-CFF program's descendant is a
  `CIDFontType0`, where the CID *is* the glyph index and `/CIDToGIDMap` is
  never read (`cpdf_cidfont.cpp:508-518`), so it is skipped rather than
  converted. §3.7 lists all four differences against the C++ inventory.
- **6 stands.** The measured numbers on `latin_extended.pdf`, whose page draws
  most of Latin Extended through a 1294-glyph Roboto: the program goes from
  35636 to 20424 bytes uncompressed, 13358 to 12970 compressed, and the
  `/CIDToGIDMap` costs 410. That fixture is close to the worst case for the
  ratio; the C++'s 2–3.5% figures are for a CJK face showing one character.

**E4's escalation is therefore narrower than it was.** The `[spec]` widening
of `subset(font_bytes, gids)` to return `Subsetted { bytes, gid_map }` is
still needed — the map is what the `/CIDToGIDMap` is built from. The sentence
recording that subsetting covers CID fonts only still stands. But the
re-keying contract E4 proposed to add is not part of it, and no first-party
retain-GIDs subsetter is needed to avoid regenerating content.

**D2 — no pause/resume stage machine.** `CPDF_Creator::Stage` and `Continue()`
exist for the public API's incremental-save-with-pause facility. `save` is one
straight-line function; the stage numbers survive only as the order of its
steps. STYLE §1 (no state-machine-shaped god objects) and §7.

**D3 — offsets are formatted from `u64`, correctly.** The C++'s
`Format("%010d", FX_FILESIZE)` truncates an `int64_t` through `%d`. We format
`u64` with `{:010}`, which produces identical bytes for every offset under
2³¹ and correct bytes above it. Unobservable on the corpus; a fix, not a
behavior change. Similarly `WriteDWord`'s `FXSYS_itoa(uint32_t → int)` would
misprint object numbers above 2³¹, which `kMaxObjectNumber = 24 · 1024 · 1024`
(`cpdf_parser.h:64`) makes unreachable.

**Relabelled 2026-09-02 (oracle-divergence audit, A74): this is an oracle
bug, so the fix is obligatory rather than optional.** Verified:
`cpdf_creator.cpp:404` and `:445` pass `object_offsets_[…]` — a
`std::map<uint32_t, FX_FILESIZE>` (`cpdf_creator.h:95`) whose value type is
`int64_t` (`fx_types.h:16`) — through a `%d` conversion that reads 32 bits.
Past 2 GiB the printed offset is the low half read as signed, and §7.5.4
requires the entry to give the object's byte offset from the file's
beginning, so the table names the wrong bytes and no reader can open the
file. A format-string type mismatch, not a decision. pdf.js has no comparable
serializer path. `write/xref.rs` carries `// [oracle-bug]`; 0 rows, since no
fixture is 2 GiB.

**D4 — determinism is a parameter, not a global.** §4.3. `SaveOptions` carries
`id_source` and the subset-tag seed. Default behavior matches the C++
(fresh randomness per save); tests pin a seed. This is *required* by STYLE §1's
no-global-state rule, and it is what makes snapshot tests of whole files
possible.

**D5 — the content generator's color loss is ported verbatim.**
**Superseded 2026-09-02 by A71 — kept here, corrected, rather than deleted.**

As written, D5 said: *"Only `rg`/`RG` are emitted; CMYK, ICC, Indexed,
Separation, DeviceN, Lab, CalRGB and all patterns are dropped (§2.4). This is a
genuine fidelity loss in PDFium, but it is *the* behavior of regenerated pages,
and Tier-B comparison of a regenerated page against the oracle's regenerated
page requires matching it exactly."*

The first sentence describes `WriteColorToStream`'s gate
(`cpdf_pagecontentgenerator.cpp:64-78`) accurately. The justification is
**false at three lines**: `pdfium_test` has no save flag, so the oracle's
regenerated page does not exist (`conformance/src/saveroundtrip.rs:6-8`, SPEC
§11 ruling E7); Tier-B compares our render against a golden of the *original*
(`conformance/src/run.rs:339-361`); and the one sweep that regenerates compares
two renders of the **same** regenerated bytes, where a content loss "dropped
for both" cannot score (`conformance/src/mutation.rs:12-17`, `:41-44`).

**D5 now reads:** only `rg`/`RG` are emitted, and **every colour space converts
to them** via `ColorValue::to_rgb` — mirroring `CPDF_Color::GetRGB`
(`cpdf_color.cpp:116-127`), which delegates to `cs_->GetRGB(buffer)` for any
space and is the conversion PDFium has and never reaches. Only a **pattern**
still emits nothing, because a pattern paints through a resource no `rg` can
name. Carried `[oracle-bug]` at `content::emit::expressible_rgb` with §8.6;
pdf.js does not regenerate page content and so is silent. Measured cost: 0
rows. Full reasoning in `docs/status/reopened-declines.md` §2.6 and
`docs/status/oracle-divergence-audit.md` §10.

**D6 — the unbalanced-output paths become clean drops.** Two C++ paths emit a
prefix and then bail, leaving `q ` (and for text `BT `) unclosed:
`ProcessText`'s unsupported-font-class return (`:968-970`) and — arguably —
`ProcessPathPoints`'s bezier `break`. Emitting syntactically broken content is
not something we will do: the Rust generator builds each object's bytes into a
scratch buffer and commits it only on success, so an unsupported object
contributes *nothing* rather than an unclosed `q`. The rendered result is the
same for every well-formed input (the C++'s trailing `q`/`BT` are closed by the
stream-level `Q` and by `ET`-less end-of-stream, both of which PDFium tolerates
on re-parse); the difference is only that our output stays parseable. Recorded
as a deliberate divergence with a diagnostic.

**Relabelled 2026-09-02 (oracle-divergence audit, A72): an oracle bug, so
declining it is obligatory rather than a preference.** Verified:
`cpdf_pagecontentgenerator.cpp:945-946` writes the graphics prefix and then
`*buf << "BT ";`, and `:968-970` is a bare `return` for any font class that is
neither Type1, TrueType nor CID — a Type 3 font, most obviously. §7.8.2
requires a content stream's operators to be balanced, `BT` with `ET` (§9.4.1)
and `q` with `Q` (§8.4.4); an unmatched `BT` leaves every later operator
inside a text object never meant to hold them. pdf.js does not regenerate page
content, so the reading rests on the spec and on PDFium writing a file it must
itself re-parse. `content/emit.rs` carries `// [oracle-bug]`. 0 rows.

**D7 — `Document` stays immutable; edits live in an overlay.** The C++
`CPDF_Document` is a mutable indirect-object holder that the writer walks,
mutates (`DeleteIndirectObject` after writing an old object!) and re-reads.
`pdfrum_parser::Document` is an immutable, `Sync`, lazily-caching reader over
`Arc<[u8]>`, which is the right shape for the other eleven crates. Rather than
make it mutable (which would poison `Send + Sync`, the `OnceLock` store and
every downstream borrow), `pdfrum-edit` owns an `EditDoc` overlay: a base
`&Document` plus a `BTreeMap<u32, Object>` of added/replaced objects and a
`next_obj_num`. It implements `Resolve` (overlay first, base second), which
STYLE §2b explicitly sanctions as the third real `Resolve` impl ("the editor's
flattened view"). The C++'s write-then-delete memory dance disappears: our
overlay never materializes an object it did not need.

**D8 — reachability is computed over object *numbers*, not pointers.** The C++
traversal is pointer-keyed (`seen_objects_`, `object_number_map_`) because a
`CPDF_Object*` identity distinguishes two structurally equal inline dicts.
Our objects are values without identity. The equivalent is a visited set of
`ObjRef` for indirect objects plus a recursion depth cap for inline structure.
The two differ in exactly one situation: the C++'s pointer set stops a
*shared inline sub-object* — the same `CPDF_Object` reachable through two
parents, which happens when a dictionary was cloned by reference rather than
by value — from being walked twice, whereas ours walks it once per path. The
only observable effect would be double-counting a reference in
`GetObjectsWithMultipleReferences`, and the `seen_ref_objects` filter
(§1.6) already suppresses the second count whenever both endpoints have been
seen. All six golden sets of §1.6 are reproduced either way; the
multiply-referenced sets are the ones to watch, so `hello_world_2_pages.pdf`
→ `{5,6,7}` is the test that pins it. The self-reference and
circular-reference filters port directly, since they are already expressed in
object numbers.

**D9 — `/Contents` shape decisions become an explicit enum.** The C++'s
`std::variant<RetainPtr<Stream>, RetainPtr<Array>>` where the stream may be
null is three states spelled as two. Ours is
`enum Contents { None, Single(ObjRef), Array(ObjRef) }` with the same
transitions, which makes the `AddStream` return values (0 / 1 / len−1) fall out
of the match instead of being three separate `return`s.

**D10 — no `CHECK`/`NOTREACHED` aborts.** Every one enumerated in §2 becomes a
diagnostic plus a skip: `CHECK(pStream)` on a null form stream, `NOTREACHED()`
on a `kNoFill` clip type, `CHECK(!name.IsEmpty())` in the usage sweep,
`CHECK(contents_array->IsInline())`, `CHECK(existing_stream)`,
`CHECK(!node.empty())`, `CurrentOffset()`'s `NOTREACHED()`. STYLE §3.

**D11 — the `/Length` overstatement in the rebuilt-table xref stream is
fixed.** §1.12: the C++ writes `/Length = last_obj_num * 5` while emitting only
the present entries. We write the true byte count. A wrong `/Length` there
would make the file unopenable by a strict reader; PDFium survives it only
because its own reader re-scans. This is a correctness fix, and the case is
vanishingly rare (an incremental save of a rebuilt-table xref-stream document).

**D12 — resource-name allocation order is stabilized.** The C++ allocates the
form XObject's name before `ProcessGraphics` but the image's after (§2.5),
which changes which `FXX{n}` a given object gets when the two interleave. We
allocate uniformly *before* graphics for both. The emitted bytes for any single
object are unchanged; only the numeric suffix assignment across a mixed page
can differ, which is not observable after re-parse.

**D17 — metadata encryption follows `/EncryptMetadata`; the C++ writer skips
it unconditionally.** `CPDF_Stream::WriteTo` (`cpdf_stream.cpp:177-190`) tests
only `IsMetaDataStreamDictionary` before dropping the encryptor, never
`CPDF_SecurityHandler::IsMetadataEncrypted()` — whose only callers are in the
*parser* (`cpdf_parser.cpp:305`, `:1287`). So the C++ writes a plaintext XMP
packet into a file whose `/Encrypt` says metadata is enciphered, and PDFium's
own reader then deciphers that plaintext into rubbish. Reproducing it would
mean knowingly destroying a document's metadata on every save of every
`/EncryptMetadata true` document, which is most of them.

We consult the flag instead: `true` writes an enciphered packet, `false` a
plaintext one, matching ISO 32000-1 §7.6.1 and matching what the oracle's
reader expects to find. Neither writer's output is unopenable by either
reader — the difference is only whether the metadata survives, and ours does.
The `/Type /Metadata /Subtype /XML` stream is still never *compressed* in
either implementation (§14.3.2), so that half of the exemption is unchanged.

**D13 — `/Type /Pages` resolves to the destination's real pages node, not the
hardcoded object 4.** `GetNewObjId`'s `return 4` (`cpdf_pageorganizer.cpp:153`)
is correct only because `FPDF_CreateNewDocument` happens to number its Pages
dict 4. Any other destination — an existing document being imported into, one
whose catalog was built differently — gets a reference to whatever object 4
happens to be. We return the destination's actual `/Root /Pages` object number.
This *changes output* for non-fresh destinations, in the direction of
correctness; `ImportIntoDestDocWithoutInfo` and
`ImportIntoDocWithWrongPageType` are the two tests that exercise a non-fresh
destination, and both assert page counts and rendered pixels rather than object
numbers, so both still pass. Recorded as a divergence rather than silently
fixed. The case-insensitive `/Type` comparison (`EqualNoCase`) is kept, since
it is a real damage-tolerance behavior.

**D14 — import is transactional.** The C++ leaves a stray blank page in the
destination when a source page turns out to be missing, and leaves earlier
pages of a failed batch in place (§3.6). Our `import_pages` stages every
destination mutation in the `EditDoc` overlay and commits only on success, so a
failed import leaves the destination untouched. The success path is
byte-identical; only the failure path differs, and no test asserts on the
destination after a failed import (`BadIndices` and `BadRanges` assert only the
`false` return).

**D15 — importing never mutates the source.** `CPDF_Page`'s constructor writes
`/Type /Page` into a source page dict that lacked one
(`cpdf_page.cpp:33-36`), so PDFium's N-up path is not read-only with respect to
the source document. Our source is a `&Document` over an `Arc<[u8]>` and
cannot be mutated; the missing `/Type` is supplied on the *copy*. The only
observable consequence would be re-saving the source in the same session, which
we cannot do anyway.

**D16 — the N-up XObject-name reuse bug is fixed.** `AddSubPage`
(`cpdf_npagetooneexporter.cpp:220-229`) reuses a name from
`src_page_xobject_map_` (never cleared) but does not re-register it in
`xobject_name_to_number_map_` (cleared per output sheet). A source page reused
on a *later* sheet therefore emits `/Xn Do` against a `/Resources /XObject`
dict that has no `Xn` entry, and the sub-page silently renders blank. We
register the name on every use. Also, `FinishPage` allocates `X1, X2, …`
without consulting the destination page's existing `/XObject` namespace and
overwrites a same-named entry; our naming starts from the first free slot in
that namespace, the same rule `RealizeResource` already uses (§2.7).

---

## 6. Module plan

```
crates/pdfrum-edit/src/
  lib.rs          // public surface: save, SaveMode, SaveOptions, IdSource,
                  // EditDoc, generate_content, import_pages, ImportOptions,
                  // NUpOptions, subset_font, Error

  doc.rs          // EditDoc: the mutable overlay over &Document (D7).
                  // - base: &Document, overlay: BTreeMap<u32, Arc<Object>>,
                  //   deleted: BTreeSet<u32>, next_num: u32
                  // - impl Resolve (overlay, then base)
                  // - add(Object) -> ObjRef, replace(ObjRef, Object),
                  //   remove(ObjRef), get(ObjRef), materialized() iterator
                  // No behavior beyond bookkeeping.

  write/
    mod.rs        // save(): the §1.2 sequence as six calls, no stage enum
    header.rs     // §1.3 header + incremental prefix
    enumerate.rs  // §1.4 new/old partition; §1.5 old-object loop
    reach.rs      // §1.6 GetObjectsWithReferences / WithMultipleReferences,
                  // objnum-keyed (D8). Returns ReachCounts { counts: BTreeMap<u32,u32> }.
    object.rs     // §1.9 WriteIndirectObj + §1.13 the per-type serializer.
                  // Free functions: write_object(&Object, &mut W, Option<&Encryptor>).
    stream.rs     // §1.13's flate-encoder decision table + metadata/signature rules
    xref.rs       // §1.11 classic full table, classic delta table, xref-stream body
    trailer.rs    // §1.12 trailer construction, the suppression list, /ID, /Prev
    id.rs         // §1.10 InitID + IdSource

  content/
    mod.rs        // generate_content(page) -> Option<ContentUpdate>; §2.1 dirty set,
                  // §2.2 per-stream frame, §2.3 emission order
    emit.rs       // §2.4 ProcessGraphics + §2.5 per-kind emitters. One function per
                  // page-object variant, each returning Option<Vec<u8>> (D6).
    path.rs       // §2.5 point emission incl. the rect fast path
    text.rs       // §2.5 text emission incl. font classification + TJ assembly
    marks.rs      // §2.6 content-mark diffing
    resources.rs  // §2.7 RealizeResource + §2.8 UpdateResourcesDict
    contents.rs   // §2.9 the Contents enum + AddStream/UpdateStream/removals (D9)
    num.rs        // thin wrappers over pdfrum_object::fmt_number:
                  // write_float / write_matrix / write_point / write_rect (§2.11)

  import/
    mod.rs        // init_dest (§3.1), import_pages (§3.3), n_page_to_one (§3.4),
                  // copy_viewer_preferences (§3.5)
    copy.rs       // §3.2 deep copy: the objnum remap, the skipped-key set,
                  // the dict-prune / array-abort asymmetry
    inherit.rs    // §3.3 CopyInheritable + the /Parent walk with its cycle rule
    nup.rs        // §3.4 NupState geometry, the Form XObject, FinishPage
    range.rs      // §3.5 the "1,3-5" page-range grammar
    viewer.rs     // §3.5 the viewer-preference filter

  font/
    mod.rs        // the `subsetter` call, the GID remap (D1), subset naming
    collect.rs    // §3.7 candidate collection, over operators rather than
                  // page objects
    overrides.rs  // build(doc, new_nums, id_source, &mut next) -> Overrides,
                  // incl. the `/CIDToGIDMap` that absorbs the remap (D1)
    // `widths.rs` / `tounicode.rs` are gone: D1's revision carries `/W` and
    // `/ToUnicode` through untouched, so neither array is ever rebuilt.

  error.rs        // Error (thiserror)
```

### 6.1 Public surface (`lib.rs`)

```rust
#![forbid(unsafe_code)]

/// How a document is written back out (SPEC §11).
pub enum SaveMode { Full, Incremental }

pub struct SaveOptions {
    pub mode: SaveMode,
    /// Keep the original bytes as the file's prefix. Ignored for `Full`.
    pub keep_original: bool,          // C++ !kNoOriginal
    /// Drop the security handler and `/Encrypt`.
    pub remove_security: bool,
    /// Subset newly embedded fonts.
    pub subset_new_fonts: bool,
    /// Header version to declare, `10..=17`; `None` keeps the document's.
    pub version: Option<u8>,
    /// Where `/ID` and subset tags come from (D4).
    pub id_source: IdSource,
}
impl Default for SaveOptions { /* Full, keep_original, no subset, Random */ }

pub enum IdSource { Random, Fixed([u8; 16]) }

pub fn save(doc: &EditDoc, opts: &SaveOptions, out: &mut impl io::Write)
    -> Result<(), Error>;

/// A document plus the edits made to it. Implements `Resolve`.
pub struct EditDoc<'a> { /* §6 doc.rs */ }
impl<'a> EditDoc<'a> {
    pub fn new(base: &'a Document) -> Self;
    pub fn add(&mut self, obj: Object) -> ObjRef;
    pub fn replace(&mut self, r: ObjRef, obj: Object);
    pub fn remove(&mut self, r: ObjRef);
}

/// Regenerate the content streams of a page whose objects were modified.
/// Returns `None` when nothing was dirty (the C++'s early-out).
pub fn generate_content(doc: &mut EditDoc, page: &mut Page)
    -> Result<Option<ContentUpdate>, Error>;

pub fn import_pages(dest: &mut EditDoc, src: &Document, pages: &PageRange, at: u32)
    -> Result<(), Error>;
pub fn n_page_to_one(dest: &mut EditDoc, src: &Document, opts: &NUpOptions)
    -> Result<(), Error>;

/// Subset one font program to the given glyphs (SPEC §11).
pub fn subset(font_bytes: &[u8], gids: &[u16]) -> Result<Subsetted, Error>;
pub struct Subsetted { pub bytes: Vec<u8>, pub gid_map: Vec<(u16, u16)> }
```

Note `subset` returns the **map** as well as the bytes — SPEC §11's
`fn subset(font_bytes, gids) -> Vec<u8>` is not enough once GIDs are
renumbered (D1). That is a `[spec]` change (escalation E4).

### 6.2 Data flow

```
save:  EditDoc ──reach.rs──▶ ReachCounts
           │                     │
           ├── enumerate.rs ─────┴──▶ (old_nums, new_nums)
           │        │
           │        └── font/ (if subset_new_fonts) ──▶ Overrides
           │
           ├── header.rs ──▶ prefix bytes
           ├── object.rs ×N ──▶ body bytes, recording ObjectOffsets
           ├── xref.rs ──▶ table or stream body
           └── trailer.rs ──▶ trailer + startxref + %%EOF
```

`ObjectOffsets` is `BTreeMap<u32, u64>` — the C++'s `object_offsets_` with the
same insert/erase discipline (§1.5 step 4's erase is load-bearing).

The writer is generic over `io::Write` per STYLE §2b's generics-are-plumbing
rule, and wraps it in a small `Counting<W>` that tracks the byte offset — the
C++'s `FileBufferArchive` minus the 32 KiB manual buffer (a `BufWriter` is the
caller's choice).

```
generate_content:  Page ──dirty set──▶ {stream_index: Vec<u8>}
                            │
                            ├── emit.rs per object
                            └── contents.rs ──▶ EditDoc mutations
                                       └── resources.rs
```

### 6.3 Types beyond SPEC

All small records:

- `ObjectOffsets(BTreeMap<u32, u64>)` — the xref bookkeeping.
- `ReachCounts { counts: BTreeMap<u32, u32> }` with
  `reachable()` / `multiply_referenced()` (§1.6).
- `Contents { None, Single(ObjRef), Array(ObjRef) }` (D9).
- `ContentUpdate { streams: BTreeMap<i32, Vec<u8>>, removed: BTreeSet<i32> }`.
- `GraphicsKey { fill_alpha: f32, stroke_alpha: f32, blend: BlendMode }` and
  `FontKey { base_font: Name, subtype: Name }` — the dedup map keys
  (`cpdf_pageobjectholder.h:37-50`).
- `font::collect::Candidate { root_font: ObjRef, cid_font: ObjRef,
  descriptor: ObjRef, base_name: Vec<u8>, used_gids: BTreeSet<u16>,
  cid_to_gid: BTreeMap<u16, u16> }` — keyed by the font-program object
  number. No `widths` or `unicode` map, because D1's revision rebuilds
  neither; `cid_to_gid` is what `/CIDToGIDMap` is built from, and `cid_font`
  is not optional because a simple font is never a candidate.
- `PageRange(Vec<RangeInclusive<u32>>)` with `PageRange::parse(&str)`.

No trait beyond the `Resolve` impl on `EditDoc` (STYLE §2b's closed seam list —
`EditDoc` is the sanctioned third implementation, so this adds no seam).

### 6.4 Concurrency

`save` is single-threaded and takes `&EditDoc`. `EditDoc` is `Send` but not
`Sync` (it holds `&Document`, which is `Sync`, plus owned mutable maps) — that
is fine: editing is not a parallel workload, and the facade's `rayon` page
rendering operates on `Document`, not `EditDoc`.

---

## 7. Test plan

### 7.1 Ported assertions — the writer

From `core/fpdfapi/edit/cpdf_creator_embeddertest.cpp`:

- `SavedDocsAreEqualAfterParse` (`:21-45`): save `annotation_stamp_with_ap.pdf`,
  render page 0, save again — **the two outputs must be the same size**. This
  pins §1.5's write-then-forget discipline. Restated as: `save` twice with the
  same `IdSource::Fixed` must produce byte-identical output even after a
  `Document::page()` walk in between.
- `Bug873` (`:47-63`): `embedded_attachments.pdf` saved contains
  `"trailer\r\n<</Info 9 0 R /Root 11 0 R /Size 36/ID[<D889EB6B9ADF88E5EDA7DC08FE85978B><"`
  followed by 32 hex chars then `">]>>\r\n"`. Ported **modulo dict order**
  (SPEC §2): assert `/Info 9 0 R`, `/Root 11 0 R`, `/Size 36`, and
  `/ID[<D889EB6B9ADF88E5EDA7DC08FE85978B><…32 hex…>]` as separate substrings,
  plus the trailer ending `>]>>\r\n` when `/ID` is last.
- `SaveLinearizedInfo` (`:65-90`): output contains `/Info`.

From `fpdfsdk/fpdf_save_embeddertest.cpp` (these are the richest goldens):

| Test | Assertion |
|---|---|
| `SaveSimpleDoc` (:35-40) | `hello_world.pdf` → starts `"%PDF-1.7\r\n"`, **805 bytes** |
| `SaveSimpleDocWithVersion` (:42-47) | version 14 → `"%PDF-1.4\r\n"`, 805 bytes |
| `SaveSimpleDocWithBadVersion` (:49-61) | −1, 0, 18 all → `"%PDF-1.7\r\n"` |
| `SaveSimpleDocIncremental` (:63-71) | incremental → starts with the **original** header bytes `"%PDF-1.7\n%\xa0\xf2\xa4\xf4"`, size > 985 |
| `SaveSimpleDocNoIncremental` (:73-78) | 805 bytes |
| `SaveSimpleDocRemoveSecurityDeprecated` (:80-86) | flag 3 → 805 bytes (degrades to plain full save) |
| `SaveSimpleDocRemoveSecurity` (:88-93) | 805 bytes |
| `SaveSimpleDocBadFlags` (:95-100) | flags 999999 → 805 bytes |
| `SaveLinearizedDoc` (:149-183) | `"%PDF-1.6\r\n"`; contains `/Root `, `/Info `, `/Size 37`, `35 0 obj`, `36 0 obj`; **not** `37 0 obj` or `38 0 obj`; **7884 bytes**; all 3 pages re-render to the original md5s |
| `Bug1409` (:185-210) | after removing all page objects + regenerating: renders blank; contains `/Root `; **not** `/Image`; size < 600 |
| `Bug342` (:223-228) | contains `"0000000000 65535 f\r\n"`, not `"…65536 f\r\n"` |
| `Bug905142` (:230-234) | contains `"/Length 0"` |
| `Bug1328389` (:238-242) | contains `"/Foo/"` |
| `IncrementalSaveWithModifications` (:244-317) | exactly 2 `trailer`, 1 `/Prev`, 2 `startxref`, 2 `%%EOF`; reload shows the added text object |
| `Bug42271133` (:114-147) | remove object 0, regenerate, save, reload → path fill color still `(180,180,180)`, not black |

The exact byte counts (805, 7884) are **not** portable as-is: they depend on
dict key order, which SPEC §2 diverges on. They port as *upper-bounded ranges
recorded with our measured value*, plus the structural substring assertions
verbatim. The 805-byte case additionally ports as an exact snapshot
(`insta`) with `IdSource::Fixed`, so any future change to our own output is
caught.

From `core/fpdfapi/parser/object_tree_traversal_util_embeddertest.cpp`
(`:30-112`) — the six reachability golden sets of §1.6, all portable verbatim
because they are object-number sets.

### 7.2 Ported assertions — content generation

From `cpdf_pagecontentgenerator_unittest.cpp`, verbatim golden strings:

- `ProcessRect` (:55-79): `"q 10 5 3 25 re B* Q\n"` and
  `"q 0 0 5.2 3.78 re n Q\n"` (the four-point closed path takes the `re` fast
  path).
- `Bug937` (:81-149): the two extreme-float goldens, notably
  `"q .000000000000000000001 .7 .35 rg .000000000000000000001 .7 .35 RG 200000000000000000000 w 1 0 0 1 .000000000000000000001 200000000000000 cm .000000000000000000001 .000000000000000000001 100 100 re f Q\n"`
  and the bezier variant ending `"53.4 5000000000000000000 c h f Q\n"`.
- `ProcessPath` (:151-186):
  `"q 3.102 4.67 m 5.45 .29 l 4.24 3.15 4.65 2.98 3.456 .24 c 10.6 11.15 l 11 12.5 l 11.46 12.67 11.84 12.96 12 13.64 c h f Q\n"`.
- `ProcessGraphics` (:188-259): prefix `"q .5 .7 .35 rg 1 .9 0 RG /"`, suffix
  `" gs 1 2 m 3 4 l 5 6 l h B Q\n"`; the named ExtGState has `ca 0.5`,
  `CA 0.8`; after `SetLineWidth(10.5)` the prefix becomes
  `"q .5 .7 .35 rg 1 .9 0 RG 10.5 w /"` and **the same gs name is reused**
  (the dedup assertion).
- `ProcessStandardText` (:261-326): `"q .5 .7 .35 rg 1 .9 0 RG /"`,
  `" gs BT 1 0 0 1 100 100 Tm /"`,
  `" 10 Tf 0 Tr [<48656C6C6F20576F726C64>] TJ ET Q\n"`; the synthesized font
  dict is `/Type /Font /Subtype /Type1 /BaseFont /Times-Roman`.
- `ProcessText` (:328-404): `"q 0 0 5 4 re W* n BT /"` and
  `" 15.5 Tf 4 Tr [<4920616D20696E646972656374>] TJ ET Q\n"` — the only clip
  golden; confirms the rect fast path applies to clips and the ` W* ` + `n `
  spelling.
- `ProcessEmptyForm` (:406-423): returns false, output `""`.
- `ProcessFormWithPath` (:425-448): the round-trip float golden — input
  `4.6700001` → `4.67`, `.28999999` → `.29`, `4.2399998` → `4.24`,
  `0.24` → `.24`, while `5.4500012` and `3.1499999` survive unchanged.
  Output:
  `"q 3.102 4.67 m 5.4500012 .29 l 4.24 3.1499999 4.65 2.98 3.456 .24 c 3.102 4.67 l h f Q\n"`.
- `ProcessContentMarksWithProperties` (:450-474): output contains
  `"/M1 /Property#20Name#20With#20Space BDC"` — the `PDF_NameEncode` `#20`
  escaping.

From `cpdf_contentstream_write_utils_unittest.cpp` (:26-69) — every value
verbatim. The `WriteFloat` cases duplicate `fmt_number`'s existing tests in
`pdfrum-object` and are restated here only for `write_float`'s *no-space*
contract; the three that are new to this crate:

- `WriteMatrix(Matrix(1,0,0,1,10.5,20.25))` → `"1 0 0 1 10.5 20.25"`.
- `WritePoint(Point(1,2.5))` → `"1 2.5"`.
- `WriteRect(Rect(1,2,10,20))` → `"1 2 9 18"` (left, bottom, **width**,
  **height**).

### 7.3 Ported assertions — subsetting

From `cpdf_fontsubsetter_embeddertest.cpp`:

- `NoNewText` (:332-348): empty new-nums ⇒ no overrides; a document with only a
  rect page object ⇒ no overrides.
- `StandardFont` (:350-363): a stock Helvetica text object ⇒ no overrides
  (no `/FontFile2`).
- `TrueType` (:398-428): `Arimo-Regular.ttf` (**436180** bytes), text
  `"Hello world"` ⇒ exactly **6** overrides: the font stream (with `/Length1`
  matching its raw size and no `/Subtype`), root font (`/Type /Font`,
  `/Subtype /Type0`, `/Encoding /Identity-H`, `/DescendantFonts` present,
  `/BaseFont` = `XXXXXX+Arimo-Regular`), CID font (`/Subtype /CIDFontType2`),
  descriptor (`/FontFile2` present, no `/FontFile3`), a non-empty `/W`, and a
  `/ToUnicode` containing `/CIDInit /ProcSet findresource begin`, `begincmap`
  and `endcmap`.
- `OpenType` (:365-396): `NotoSansCJKjp-Regular.otf` (**16427228** bytes),
  text `"这"` ⇒ 6 overrides with the OpenType-CFF shape: stream `/Subtype
  /OpenType` and **no** `/Length1`; CID font `/Subtype /CIDFontType0`;
  descriptor with `Flags & 0x04` set, `Flags & 0x20` clear, `/FontFile3`
  present, `/FontFile2` absent.
- `SingleFontMultipleTexts` (:430-462), `MultipleFontsMultipleTexts`
  (:464-512, `Lohit-Tamil.ttf` = **48908** bytes, 12 overrides),
  `ReplaceExistingPrefix` (:514-551, `AAAAAA+Arimo-Regular` → a fresh
  six-uppercase tag with exactly one `+`).
- The size-ratio assertions (2–3.5%, 5.5–6.5%) port as **our own measured
  ranges**, recorded with a comment that the C++ figures differ because a
  different table set survives (D1).

From `fpdf_save_embeddertest.cpp`'s `FPDFSaveWithFontSubsetEmbedderTest`
(:319-493): `SaveWithSubsetWithoutNewText` (subset flag with no new text ⇒
805 bytes, unchanged); the not-subset (5004) vs subset (4453) size comparison
ports as "subsetting strictly reduces the output"; and `TestExtractedFont`'s
char-by-char extraction of the saved document — that one ports **exactly** and
is the strongest guarantee that our GID re-keying (D1) preserves text.

**What was actually portable, 2026-09-03.** Every C++ case above builds its
document by calling `FPDFText_LoadFont` with the bytes of a `.ttf` from
`testing/resources`. **We have no such API** — §3.7's audit found page import
to be the only path that produces a new embedded font at all — so no case that
loads a font from bytes ports as written. `crates/pdfrum/tests/subset.rs`
reaches the same assertions through an import of `latin_extended.pdf`, whose
page already carries an embedded `Roboto-Regular`:

| C++ case | Ported as |
|---|---|
| `NoNewText` | `a_save_with_no_new_fonts_is_unchanged_by_the_option` — byte-identical output with the option on and off |
| `StandardFont` | the same test; `hello_world.pdf`'s fonts are stock Type 1 with no `/FontFile2` |
| `TrueType`'s override shape | `base_font_and_font_name_carry_a_six_letter_tag` and `the_cid_font_gains_a_cid_to_gid_stream`. **Five overrides, not six**: `/W` is not one of ours, and the `/CIDToGIDMap` is |
| `ReplaceExistingPrefix` | `font/mod.rs`'s `an_existing_prefix_is_replaced_not_stacked`, unchanged — it was always a unit test of the naming |
| `OpenType` | **not portable, and deliberately so.** §3.7's divergence 2: an `OTTO` program is skipped rather than converted, so there is no `/CIDFontType0` shape to assert |
| The size-ratio assertions | `the_embedded_program_is_smaller`, with the measured 35636 → 20424 recorded rather than a ratio |
| `TestExtractedFont` | `a_subsetted_save_extracts_the_same_text`. It is **weaker than the C++'s and stronger than planned**: there is no re-keying left to check, because `/ToUnicode` is carried through byte for byte |
| — | `a_subsetted_save_renders_identically`, which has no C++ counterpart. `pdfium_test --md5` agrees: the same digest for both pages of the subsetted and unsubsetted saves |

### 7.4 Ported assertions — page import and N-up

From `core/fpdfapi/edit/cpdf_npagetooneexporter_unittest.cpp:10-19` — the whole
file, one test, and it is the highest-value golden in the import path because
it pins four contracts at once:

```
GenerateSubPageContentStream("foo", { start: (0.000001, 1000000000000.0), scale: 0.5 })
  == "q\n.5 0 0 .5 .000001 1000000000000 cm\n/foo Do Q\n"
```

(the `q\n… cm\n/… Do Q\n` layout; leading zeros dropped; tiny values in full
positional notation, not scientific; huge values with all integer digits).
**Port this one first** — it is a `fmt_number` anchor as well as a layout
anchor.

From `fpdfsdk/fpdf_ppo_embeddertest.cpp` (26 cases). The public-API shims are
not ported, but every assertion restates over the core functions:

| Test | Assertion |
|---|---|
| `NoViewerPreferences` (:63) | `hello_world.pdf` has none ⇒ copy fails |
| `ViewerPreferences` (:71) | `viewer_ref.pdf` ⇒ copy succeeds |
| `CopyViewerPrefTypes` (:387) | `viewer_pref_types.pdf` ⇒ **6** surviving entries: `Bool` boolean, `Num` integer `1`, `Str` = `"str"`, `Name` = `name`, `EmptyArray` empty, `GoodArray` size 4 — the filter's exact accept set |
| `BadRepeatViewerPref` (:359), `BadCircularViewerPref` (:373) | copy succeeds and the result saves without hanging |
| `ImportNPages` (:108) | `rectangles_multi_pages.pdf` (5 pages): `(612,792,2,1)` ⇒ **3** sheets; `(612,792,5,1)` ⇒ **1**; `(792,612,8,1)` ⇒ **1**; `(792,612,128,1)` ⇒ **1** |
| `BadNupParams` (:129) | `x = 0`, `y = 0`, `width = 0`, `height = 0` each ⇒ failure |
| `NupRenderImage` (:148) | `(792,612,3,1)` ⇒ **2** sheets, each **792 × 612**, matching `rectangles_multi_pages_3_in_1` and `..._2_in_1` |
| `BadIndices` (:428) | on a 1-page document, `{-1}`, `{1}`, `{-1,0,1}`, `{42}` all fail |
| `GoodIndices` (:454) | `viewer_ref.pdf` (5 pages), all inserted at index 0: `{0,0,0,0}` ⇒ count **4**; `{0}` ⇒ **5**; `{4}` ⇒ **6**; `{1,2,3}` ⇒ **9**; then all-pages ⇒ **14** |
| `BadRanges` (:493) | on a 1-page document: `"clams"`, `"0"`, `"42"`, `"1,2"`, `"1-2"`, `",1"`, `"1,"`, `"1-"`, `"-1"`, `"-,0,,,1-"` all fail |
| `GoodRanges` (:513) | `"1,1,1,1"` ⇒ **4**; `"1-1"` ⇒ **5**; `"5-5"` ⇒ **6**; `"2-4"` ⇒ **9** |
| `Bug750568` (:546) | 4 source pages each rendering to `bug_750568_page1..4`; after import, all four imported pages render to **the same** four expectations — the core "import preserves rendering" test |
| `Bug40162073` (:576) | the imported page renders to `bug_40162073_wrong`, **not** `bug_40162073`, with a `TODO(crbug.com/40162073)` — a known-bad golden documenting a real import fidelity bug. Port it as an **expected-failure** case pinned to the same wrong output, so a future fix is visible as a deliberate change |
| `ImportWithZeroLengthStream` (:598) | `zero_length_stream.pdf` imports and renders identically |
| `ImportWithSelfReferentialPageParent` (:620) | `bug_517126568.pdf` — the `/Parent`-cycle guard; import once ⇒ 1 page, import again at index 1 ⇒ 2 pages |
| `ImportIntoDestDocWithoutInfo` (:639) | destination lacking `/Info`: 1 → 2 → 3 pages |
| `ImportIntoDocWithWrongPageType` (:661) | `bad_page_type.pdf`: after deleting page 0 and importing, 2 pages rendering to `bad_page_type_new_page1/2`, preserved across a save — the `InitDestDoc` `/Type` repair |
| `XFAImportTest` (:714) | the definitive insertion-index + order test: importing `"1, 4, 2"` (note the **spaces**) at index 4 into an 8-page result puts source pages 0, 3, 1 at destination indices 4, 5, 6 and shifts the old index-4 page to 7, verified by per-page render checksum |
| `ImportPageToXObject` (:166), `ImportPageToXObjectWithSameDoc` (:263) | a page-as-form-XObject renders identically and survives a save; bounds `(-1, -1, 201, 301)`; two form objects built from the same XObject **share one stream** |

From `fpdfsdk/cpdfsdk_helpers_unittest.cpp:51-93` — the page-range grammar, the
tightest specification available. All 24 failing cases and all 13 succeeding
ones port verbatim, notably `("1- 4", 4) → {0,1,2,3}`, `("1 -4", 4) →
{0,1,2,3}`, `("5  0, 1-2 ", 100) → {49,0,1}` (the space-stripping-inside-numbers
behavior the C++ test itself annotates `// ???`), `("1-4,3-6", 10) →
{0,1,2,3,2,3,4,5}` (duplicates legal), `("2,1", 10) → {1,0}` (order preserved),
and the failures `("1-2-", 10)`, `("1-2,,,,3-4", 10)`, `("1-5", 4)`.

### 7.5 Hand-written tests (no C++ counterpart)

Serializer, on values:

- Every `Object` variant's bytes, including the leading-space forms and the
  no-separator array/dict rules; a name needing `#` escapes; an empty name.
- A signature dict: `/Contents` unencrypted while a sibling string is
  encrypted; the `/FT /Sig` detection path as well as `/Type /Sig`.
- A metadata stream: not flate-encoded, not encrypted, dict untouched.
- The flate-encoder decision table's four rows, each asserted on the emitted
  dict and payload.
- `/Length` correctness for all four rows.

Xref emission:

- Object 1 present vs absent (the two `"xref\r\n"` headers).
- A gap in the middle producing two subsections.
- The `i == 1` subsection's `0 {j}` count including the free head.
- The incremental delta table over `[2,3,4,9,10]` → two subsections.
- The xref-stream body's `/W [0 4 1]` records, big-endian offset + zero byte.
- `/Size` = last written + 1 (classic) and + 2 (stream).

Round-trip properties (the crate's centre of gravity, per SPEC §11):

- **P1 (offsets)**: for every corpus file, `save` then re-`load` and assert
  that every xref offset resolves to an object whose parsed number matches.
- **P2 (idempotence)**: `save(load(save(load(f))))` equals `save(load(f))`
  byte-for-byte with a fixed `IdSource`.
- **P3 (append)**: incremental output's prefix equals the input bytes exactly.
- **P4 (reachability)**: every reference in the output resolves, or is
  deliberately dangling in a way the input also had.
- **P5 (semantic)**: page count, per-page media/crop box, rotation, and text
  extraction survive a full save.
- **P6 (Tier-B)**: re-render every page of the saved file; SSIM against the
  original render, with regenerated pages in their own threshold cluster
  (R13).

Content generation:

- The dirty-set early-out: a page with nothing dirty produces `None` and the
  document is not modified at all.
- Each `/Contents` transition of §2.9 (`None → Single`, `Single → Array`,
  `Array` append, removals with the index remap, single-element array
  preserved).
- The copy-on-write paths: a shared `/Contents` array, a shared stream, a
  shared `/Resources`, a shared resource sub-dict.
- `RealizeResource` collision handling against both the live dict and the
  removed-resources map.
- The clip-path `W`/`W*` spellings and the rect fast path inside a clip.

Import (beyond the ported grammar cases of §7.4):

- Inherited-attribute flattening: a page inheriting `/Resources` and
  `/MediaBox` two levels up; a page whose own value shadows an ancestor's; a
  page with no `/Parent` (⇒ nothing is inheritable at all); an ancestor chain
  where the `/Type` is missing on the page itself (⇒ nothing inheritable,
  case-sensitively).
- The `/MediaBox` fallback ladder: present → inherited → `/CropBox` →
  `[0 0 612 792]`, and `/Resources` → empty dict.
- Reference remapping: a cycle (terminates via the pre-recursion mapping),
  a dangling source reference (the owning **dict key is removed**, the owning
  **array is dropped whole** — the asymmetry of §3.2), a reference to another
  source page (pruned), and a self-reference from inside the page (preserved,
  because the page's own mapping is registered first).
- Object dedup across a multi-page import: two source pages sharing a font
  produce **one** destination font object.
- `InitDestDoc` on each of: a fresh document, a document with a non-dict
  `/Pages`, a document with a non-array `/Kids` (⇒ `/Count` force-zeroed), a
  document with a wrong-but-non-empty catalog `/Type` (⇒ left alone), a
  document with no `/Info`.
- N-up geometry: the `ConvertPageOrder` slot mapping for a 3×2 grid; the
  one-axis centering including the exact-tie case; scale > 1 for a source page
  smaller than its slot; a rotated source page's `/Matrix`.
- The N-up array-`/Contents` join emitting `"\n"` after **every** element.
- D16's regression: the same source page on two different sheets, asserting
  both sheets' `/Resources /XObject` name the form.

### 7.6 Snapshots and fuzzing

`insta` snapshots of the complete saved bytes for a handful of small fixtures
(`hello_world.pdf`, `bug_905142.pdf`, `rectangles.pdf`, a from-scratch
document) with `IdSource::Fixed` — these are the regression net for any change
to our own output shape.

Fuzz targets (SPEC §0's "fuzz target if it consumes untrusted bytes"):

- `fuzz_save_roundtrip`: `load` arbitrary bytes; if it opens, `save` and
  `load` again — neither may panic, and the second load must succeed whenever
  the first did.
- `fuzz_generate_content`: build a page from arbitrary content bytes,
  regenerate, re-parse — no panic, no unbounded growth.
- `fuzz_subset`: arbitrary bytes plus an arbitrary GID list into `subset` —
  no panic (the `subsetter` crate is `deny(unsafe_code)` but is young).

Conformance clusters (M7 exit): a `--save` mode in `pdfrum-tool` writing the
document back out, then (a) the *oracle* opening it — note `pdfium_test` has
**no save flag**, so the check is `pdfium_test --md5 our_output.pdf` succeeding
and matching the original render, not a byte diff of two writers' outputs; and
(b) our own re-render diffed Tier-B. `--save` is a new `pdfrum-tool` flag with
no oracle counterpart; that asymmetry is expected and documented.

---

## 8. Open questions and escalations

**E1 — `Document` does not expose `last_xref_offset` or `is_xref_stream`
(blocking).** `/Prev` (§1.12), the classic-vs-stream xref choice (§1.11) and
the rebuilt-table special cases (§1.3, §1.12) all read them. Neither is
currently tracked anywhere in `pdfrum-parser`
(`crates/pdfrum-parser/src/xref/`). Proposal: a `[spec]` change adding to
SPEC §5's `Document`:

```rust
/// Byte offset of the last cross-reference section the chain load used,
/// or 0 when the table was rebuilt. `/Prev` on an incremental save.
pub fn last_xref_offset(&self) -> u64;
/// Whether the document's main cross-reference was a stream.
pub fn main_xref_is_stream(&self) -> bool;
```

Both are one field each on the load path (`xref/chain.rs` already knows both
facts); neither changes any existing behavior. **Needs sign-off before
implementation.**

**E2 — the `/Encrypt` dictionary and its inline-ness are not exposed
(blocking for encrypted saves).** §1.8's promotion of an inline encrypt dict
to an indirect object, §1.9's "never encrypt the encrypt dict", §1.10's R2/R3
rekey and §1.12's `/Encrypt` trailer entry all need it. `build_security`
(`crates/pdfrum-parser/src/doc.rs:285-323`) already computes the dict and
discards it. Proposal: a `[spec]` change adding

```rust
/// The `/Encrypt` dictionary and whether the trailer held it directly.
pub fn encrypt_dict(&self) -> Option<(&Dict, bool /* inline */)>;
```

**E3 — `pdfrum-crypt` has no encryption side, by its own D2 ruling
(blocking for encrypted saves).** *(RESOLVED in M10, 2026-08-30: the encrypt
side landed and preserve-encryption save is the default. The scope question
below was answered "in scope, but for M10 rather than M7"; SPEC §11's M10
ruling supersedes the E3 deferral, and `remove_security` survives as the
explicit opt-out. The `EncryptContent` key rule the paragraph below proposes
was **not** adopted — see the crypt brief's revised D2: the object key is
derived the same way in both directions, because the two disagree only at key
lengths AESV2 cannot have, and sharing one derivation is what makes the round
trip exact. `OnCreate` and `GetEncodedPassword` were not needed either: v1
preserves passwords rather than setting them, so the R2/R3 `/ID` rekey still
resolves through the M7 interlock — it forces a full save — rather than
through a new key. Original text follows.)* `docs/design/pdfrum-crypt.md` D2 (:515-520)
explicitly defers `OnCreate`, `AES256_SetPassword`, `AES256_SetPerms` and
`EncryptContent` to M7 "as a `[spec]` change adding `encrypt` to this crate".
This brief is that trigger. Minimum needed:

```rust
impl SecurityHandler {
    fn encrypt(&self, obj: ObjRef, class: CryptClass, data: &[u8]) -> Vec<u8>;
}
```

using the `EncryptContent` key rule (crypt brief §1.10: `realkeylen =
min(key_len + 5, 16)` for RC4 *and* the non-32-byte AES case, which the brief
notes disagrees with the decrypt path for AESV2 with `key_len ≠ 16` — a case
that does not occur in practice). `OnCreate` (the R2/R3 rekey of §1.10) is
additionally needed for the no-`/ID` encrypted-document path, along with
`GetEncodedPassword`. **Scope question for the user: is "save an encrypted
document as encrypted" in M7 scope, or is `remove_security` the v1 answer?**
If the latter, `save` returns
`Error::EncryptedSaveUnsupported` unless `remove_security` is set, and E2/E3
shrink to just the `/Encrypt`-suppression logic. The corpus has ~20 encrypted
files; none of them is a save fixture in the C++ tests, so the conformance
cost of deferring is low.

**E4 — SPEC §11's `subset(font_bytes, gids) -> Vec<u8>` is insufficient, and
subsetting is narrower than the C++'s (blocking for `kSubsetNewFonts`).** D1
in full: `subsetter` renumbers GIDs and removes `cmap`, where HarfBuzz's
`RETAIN_GIDS` does neither. Consequences: the signature must return the GID
map; `/W` and `/ToUnicode` must be re-keyed; the content stream's char codes
must be rewritten for Identity-H fonts; and simple (non-CID) fonts cannot be
subsetted at all. Proposal: a `[spec]` change to §11 replacing the signature
with `subset(font_bytes, gids) -> Result<Subsetted, Error>` where
`Subsetted { bytes, gid_map }`, plus a sentence recording that subsetting
covers CID fonts only. The alternative — a first-party retain-GIDs subsetter —
is a new crate and a DEPS.md change, and is not proposed. **Needs sign-off.**

Secondary `subsetter` notes for the implementer: its default feature
`variable-fonts` pulls `skrifa 0.42`, `write-fonts 0.48` and `kurbo 0.13`,
while the workspace pins `skrifa 0.46.2` and `kurbo 0.13.1`. Turning the
feature **off** (`default-features = false`) drops all three extra deps and
loses only CFF2 support and variation instancing, neither of which PDFium's
subsetter does either. Recommend `default-features = false`; verify at
implementation time that the resolver does not still unify a second skrifa.

**E5 — `EditDoc` versus a mutable `Document` (design, non-blocking).** D7
proposes the overlay. The alternative is making `pdfrum_parser::Document`
mutable, which SPEC §5 does not describe and which would cost `Sync`. The
overlay costs one extra `Resolve` hop per fetch and requires every edit-aware
API to thread `&EditDoc` rather than `&Document`. Confirm the overlay in
review of this brief; it is the default.

**E6 — where does `Page` mutation live?** *(RESOLVED 2026-08-30 in M11. The
proposal below was accepted and grown by three fields the brief did not
foresee: `ImageObject`/`FormObject` gained `source: Option<ObjRef>` and
`TextObject` gained `font_source`, because a parsed object holding decoded
pixels or a loaded font cannot otherwise name the `/XObject` or `/Font` entry
a regenerated `Do` or `Tf` must spell. See SPEC §7's M11 entry and
`docs/status/pdfrum-page.md`.)* `generate_content` needs a *mutable*
page-object graph with dirty flags — SPEC §7's `Page`/`PageObject` are
described as records with no behavior, and the page brief carries
`content_stream: i32` explicitly "because `pdfrum-edit` needs it" but says
nothing about `dirty`. Proposal: `PageObject` gains
`pub dirty: bool` and `pub active: bool` (both defaulting to `false`/`true` on
parse), and `Page` gains `pub dirty_streams: BTreeSet<i32>` — three additive
fields, no behavior, matching `cpdf_pageobject.h:167-176` and
`cpdf_pageobjectholder.h:170-171`. That is a `[spec]` change to §7's
*(abridged)* field lists, which SPEC §0 permits briefs to grow. Flagged rather
than assumed because it touches another crate's contract.

**E7 — `pdfrum-tool` gains a `--save` flag with no oracle counterpart.**
`pdfium_test` has no document-save option (verified:
`testing/pdfium_test/pdfium_test.cc` has `--save-attachments`, `--save-images`,
`--save-rendered-images`, `--save-thumbs*`, and nothing else). So the M7 exit
"oracle re-opens our saved files cleanly" is necessarily a *two-step* harness
check — run `pdfrum-tool --save`, then run the oracle on the result — not a
like-for-like flag diff. Confirm that shape with the harness owner; it changes
`conformance/`'s driver, not this crate.

**E8 — the C++'s per-object dedup caches live on the page-object holder, which
we do not have.** *(RESOLVED 2026-08-30 in M11 as proposed: a `ResourceTable`
created fresh per `regenerate` call. The parking half — `all_removed_resources_map`
— is inside that table and so is likewise per-call, which means a name freed by
one regeneration is available to the next. Unobservable after re-parse, as
predicted.)* `graphics_map_`, `fonts_map_` and
`all_removed_resources_map_` (`cpdf_pageobjectholder.h:171-176`) persist across
`GenerateContent` calls on the same page. Our `Page` is a value. Proposal:
these live in a `ContentGenState` record owned by `generate_content`'s caller
(or created fresh per call), which changes only which `FX{X}{n}` number a
resource gets across *repeated* regenerations of the same page — unobservable
after re-parse, and no test depends on it. Recording it so the difference is
not discovered as a bug.

**E9 — nested form regeneration has no depth guard in the C++** *(RESOLVED
2026-08-30 in M11: not implemented, and not needed. A form page object is
written as the `Do` that draws it, so its own stream is never rewritten and
the recursion this escalation guards against does not arise. Should nested
regeneration ever be wanted, the proposal below still stands.)* (`:705-711`).
A form referencing itself would recurse forever. `pdfrum-page` already carries
a form-recursion guard for *parsing* (page brief §1.7, `kMaxFormLevel = 40`
plus a visited set). Proposal: reuse the same cap and visited-set shape for
regeneration, as an additive safety net with a diagnostic — the same shape as
the accepted caps in SPEC §7's ruling. Unobservable on any real file.

**E10 — four import-path bugs are fixed rather than ported (D13, D14, D15,
D16), which is a departure from this program's usual rule.** Every other brief
ports quirks verbatim on the principle that observable behavior wins. These
four are different in kind: each is a *bug with no file depending on it*, and
three of them produce output the oracle itself would then fail to render
correctly (a reference to the wrong object, a blank sub-page, a stray page).
The C++ tests that touch these paths assert page counts and rendered pixels,
never object numbers, so all of them still pass against the fixed behavior.
Two of the four (D13's hardcoded object 4, D16's name reuse) are annotated as
bugs in the C++ source itself. Confirm the ruling; the conservative
alternative is to port each verbatim behind a diagnostic and revisit if
conformance shows a difference, which costs nothing but leaves us shipping
known-broken output.

**Relabelled 2026-09-02 (oracle-divergence audit, A73): all four are oracle
bugs, which removes the departure this escalation describes.** PLAN.md
§212–229's rule, ruled 2026-09-02, makes implementing the correct behaviour
obligatory wherever the C++ is shown wrong against the specification — so E10
is no longer an exception to the program's usual rule but an instance of it.
Verified at the line: the hardcoded `return 4`
(`cpdf_pageorganizer.cpp:150-153`) against §7.7.3.2's `/Parent`; the
non-transactional import, which no clause sanctions; the source mutation
(`cpdf_page.cpp:33-36`); and the N-up name reuse, where
`cpdf_npagetooneexporter.cpp:224-228` takes a cached name while the registry
that must also carry it (`xobject_name_to_number_map_`) is cleared per sheet
at `:180` and written only at `:284-285` inside the path the cache hit skips,
so §8.10.1's requirement that an invoked name be present in the resources is
broken and the sub-page renders blank. pdf.js implements neither page import
nor N-up, so the reading rests on the spec and on three of the four producing
output PDFium itself would fail to render. `import/mod.rs` carries the
umbrella `// [oracle-bug]`; each fix stays pinned in `tests/import.rs`. 0
rows.

**E11 — `Bug40162073` is a known-bad import golden.** `fpdf_ppo_embeddertest.cpp:576`
asserts that importing a page from `bug_40162073.pdf` renders to
`bug_40162073_wrong`, with an explicit `TODO(crbug.com/40162073)` saying it
*should* render to `bug_40162073`. Our import is a rewrite, so it may well get
this right by accident. Proposal: port it as a pinned expectation of the
**wrong** output (matching the oracle, which is what conformance compares
against), with a comment naming the upstream bug — and if our output turns out
to match the *correct* rendering instead, record that as a documented
Tier-B waiver rather than deliberately reproducing the fault. Flagging because
it is the one place where "match the oracle" and "be correct" are known to
conflict in this crate.
