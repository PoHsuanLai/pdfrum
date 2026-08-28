# Design brief — `pdfrum-object`

Behavior source: `pdfium-c++/core/fpdfapi/parser/` object classes
(`cpdf_object.*`, `cpdf_boolean.*`, `cpdf_number.*`, `cpdf_string.*`,
`cpdf_name.*`, `cpdf_array.*`, `cpdf_dictionary.*`, `cpdf_stream.*`,
`cpdf_reference.*`, `cpdf_null.*`), the text/name codecs in
`fpdf_parser_decode.*` and `fpdf_parser_utility.*`, number semantics in
`core/fxcrt/fx_number.cpp`, float formatting in
`core/fpdfapi/edit/cpdf_contentstream_write_utils.cpp`, and the name-constant
headers in `constants/`. Shape contract: SPEC.md §2 (binding). All C++ paths
below are relative to `/mnt/data2/pdfium/pdfium-c++/`.

This crate is **values only**: construction and access. No parsing, no I/O, no
decoding of stream payloads. The parser crate builds these values; every other
crate reads them through the accessors defined here.

---

## 1. Behavior inventory

### 1.1 Type universe and object identity

- C++ has 9 concrete classes behind `CPDF_Object::Type`
  (`cpdf_object.h:53-63`): Boolean, Number, String, Name, Array, Dictionary,
  Stream, Null, Reference. Our `Object` enum splits Number into `Int`/`Real`
  (§1.2 explains why the split is observable and how to keep C++ semantics).
- Every object carries `(obj_num, gen_num)`; both are set only when the object
  was parsed as an indirect object body (`cpdf_syntax_parser.cpp:692-696`).
  `obj_num == 0` means "inline" (`cpdf_object.h:69`). In Rust, identity lives
  in the store key (`ObjRef`), not inside `Object` — inline objects simply are
  values with no key. Nothing in the accessor layer depends on an object
  knowing its own number; the two C++ uses (cache keys
  `cpdf_object.cpp:28-35`, decryption keying) are handled by the parser crate
  which always knows the `ObjRef` it fetched.
- `kInvalidObjNum = 0xFFFF_FFFF` (`cpdf_object.h:52`). The lexer-level
  consequence (a reference token `4294967295 0 R` parses to *failure*, not a
  reference — `cpdf_syntax_parser.cpp:565-568`) is the parser crate's job, but
  the constant belongs here next to `ObjRef`.
- `ObjRef.gen` exists in xref entries and in `N G obj` frames, but
  **references never store a generation**: `CPDF_Reference` holds only
  `ref_obj_num_` (`cpdf_reference.h`, `cpdf_reference.cpp:15-16`). Resolution
  is keyed by object number alone. Keep `ObjRef { num, gen }` per SPEC (xref
  and edit need `gen`), and document on `Resolve::fetch` that implementations
  must ignore `gen` when looking objects up (a `12 3 R` token in the wild
  resolves to whatever object 12 the xref delivers).

### 1.2 Numbers — `FX_Number` semantics (fx_number.cpp:23-115)

C++ `CPDF_Number` wraps `FX_Number`, a tri-state
`variant<uint32_t, int32_t, float>`. The tri-state is *observable* and must be
reproduced exactly, because `GetInteger` and `GetNumber` disagree on large
unsigned values.

Parse rules (`FX_Number::FX_Number(ByteStringView)`, fx_number.cpp:23-85) —
these run in the lexer (parser crate) but define what `Object::Int`/`Real`
must be able to represent:

1. Empty string → integer 0.
2. If the string **contains a `.` anywhere** → float, via `StringToFloat`
   (fx_string.cpp:124-140): leading `[ +-]*` skipped except a `-` directly
   before the digits is kept; then `fast_float` general parse; parse error →
   `0.0`; out-of-range → the clamped value fast_float produced (±inf becomes
   ±inf — `result_out_of_range` still returns the value). No `.` and no digits
   (e.g. `--`) → 0.
3. Otherwise integer: optional single leading `+`/`-`; then decimal digits
   accumulated into a checked **u32**; accumulation overflow ⇒ value becomes
   **0** (`ValueOrDefault(0)`, fx_number.cpp:58). Trailing garbage stops the
   scan (e.g. `12abc` → 12).
   - Unsigned (no sign char): stored as **u32** — `4294967295` stays
     4294967295.
   - Signed: if magnitude > `INT_MAX` (or > `INT_MAX+1` for negatives) ⇒ 0;
     else stored as **i32**, `-2147483648` handled without UB
     (fx_number.cpp:66-84).

Accessor semantics (fx_number.cpp:87-115):

- `GetSigned()` (= C++ `GetInteger()`): u32 → `as i32` (**wrapping**:
  `4294967295` → −1); i32 → itself; float →
  saturating cast to i32 (`pdfium::saturated_cast`: NaN → 0, ±inf/out-of-range
  clamp to i32::MIN/MAX).
- `GetFloat()` (= C++ `GetNumber()`): u32 → `as f32` (4294967295 →
  4294967296.0f); i32 → `as f32`; float → itself.

Rust mapping that preserves both observables with SPEC's `Int(i64)`:

- Integer parse stores the **mathematical value** in `i64`: unsigned parse
  result `u` → `Int(u as i64)` (range 0..=4294967295, overflow → 0); signed
  parse result `s` → `Int(s as i64)` (range −2147483648..=2147483647,
  overflow → 0). Every reachable `Int` therefore fits
  −2^31 ..= 2^32−1; the brief-level invariant is documented on the variant.
- `int()`-flavored accessors return C++ `GetInteger`: `(v as u32) as i32 as
  i64` reproduces the wrap for the unsigned range and is the identity for the
  signed range. Concretely: `fn as_c_int(v: i64) -> i64 { v as u32 as i32 as
  i64 }` for values known to be in the reachable range; implement as
  `v.wrapping_rem_euclid` — no: implement exactly as the double cast above and
  unit-test the corner (`4294967295 → -1`, `2147483648 → -2147483648`? note:
  2147483648 unsigned parse → u32 2147483648 → wraps to −2147483648 —
  test this).
- `number()`-flavored accessors return C++ `GetNumber`: `v as f32` on `Int`
  (i64→f32 rounds the same way u32→f32 does for the reachable range), or the
  stored `f32` for `Real`.
- `Real(f32)` per SPEC. `GetInteger` on a `Real` = saturating f32→i32 with
  NaN → 0.

Float → string (needed by any dump/serialize path and by `--annot`-style
output; keep it in this crate as `fmt_number`): port `FloatToDecimal`
(`cpdf_contentstream_write_utils.cpp:29-108`) exactly:

- NaN **and 0.0/−0.0** → `"0"`.
- +inf → f32::MAX's representation; −inf → f32::MIN's.
- Negative → leading `-`, format magnitude.
- Shortest-round-trip decimal digits + exponent (C++ uses dragonbox; `ryu`'s
  `d2s`-style shortest digits are identical — DEPS pins ryu). Then positional
  rendering, never scientific:
  - exponent ≥ 0: digits then `exponent` zeros (`340282350000000000000000000000000000000`).
  - digits reach across the point: `dd.dd` form.
  - value < 1: **`.` with no leading zero**, then `-places` zeros, then digits
    (`.000000000000000000000000000000000000011754944`); output is capped at 48
    chars + NUL (`kMaximumFloatToDecimalLength = 49`,
    cpdf_contentstream_write_utils.cpp:17) — the cap only binds for -FLT_MIN
    denormals; port the same truncation (stop emitting digits when 48 chars
    written).
- Integer → plain decimal `i32` formatting (through the C-int view above).

Golden values (from `cpdf_number_unittest.cpp:37-131`): `0.0f→"0"`,
`1.0f→"1"`, `-7.5f→"-7.5"`, `38.895285f→"38.895287"`,
`-77.037232f→"-77.03723"`, `f32::MAX→"340282350000000000000000000000000000000"`,
`f32::MIN_POSITIVE→".000000000000000000000000000000000000011754944"`.

### 1.3 Booleans and Null

- `CPDF_Boolean`: `GetString()` → `"true"`/`"false"`, `GetInteger()` → 0/1
  (`cpdf_boolean.cpp`). Dict/array boolean getters only accept an actual
  Boolean (see §1.6 matrix) — an `Int(1)` is *not* a boolean.
- Null: all accessors return defaults. Note for parser-crate readers: C++
  never *stores* nulls in dicts (a null value erases the key,
  `cpdf_dictionary.cpp:277-289`) but the syntax layer does produce
  `CPDF_Null` objects (e.g. array elements).

### 1.4 Strings — `PdfString`

- Storage is **raw bytes** exactly as the lexer produced them (escapes already
  processed, hex already paired). No encoding is assumed.
- C++ keeps a flag `output_is_hex_` recording whether the source syntax was
  `<...>` or `(...)` (`cpdf_string.h:47`, set in the two constructors
  `cpdf_string.cpp:20-34`). It is observable through `EncodeString()` /
  `WriteTo()` (round-trip serialization writes hex strings back as hex,
  `cpdf_string.cpp:70-87`) and through nothing else. See Open questions — SPEC
  currently pins `PdfString(pub Box<[u8]>)` without the flag.
- `as_text()` (C++ `GetUnicodeText` → `PDF_DecodeText`,
  fpdf_parser_decode.cpp:552-578):
  1. Bytes start `FE FF` → UTF-16BE of the rest; start `FF FE` → UTF-16LE
     (PDFium extension beyond ISO 32000-1); start `EF BB BF` → UTF-8
     (extension). Lone/unpaired surrogates survive as-is in C++'s wide string;
     in Rust map each unpaired surrogate to U+FFFD (see Divergences).
     Odd trailing byte in UTF-16 payloads is dropped (span of u16s).
  2. In the BOM paths only, strip *language-code regions*: every char equal to
     U+001B starts a region skipped through the next U+001B
     (`StripLanguageCodes`, fpdf_parser_decode.cpp:536-550); an unterminated
     region strips to end of string.
  3. No BOM → **PDFDocEncoding**, byte-per-char through the 256-entry table
     (fpdf_parser_decode.cpp:64-93). The table is identity-to-Unicode except:
     - `0x18..=0x1F` → U+02D8 U+02C7 U+02C6 U+02D9 U+02DD U+02DB U+02DA U+02DC
     - `0x7F` → U+0000
     - `0x80..=0x9E` → U+2022 U+2020 U+2021 U+2026 U+2014 U+2013 U+0192 U+2044
       U+2039 U+203A U+2212 U+2030 U+201E U+201C U+201D U+2018 U+2019 U+201A
       U+2122 U+FB01 U+FB02 U+0141 U+0152 U+0160 U+0178 U+017D U+0131 U+0142
       U+0153 U+0161 U+017E
     - `0x9F` → U+0000, `0xA0` → U+20AC (euro), `0xAD` → U+0000
     - `0xA1..=0xFF` (except 0xAD) → same code point as Latin-1.
     Note U+0000 results are *kept* in the output string in C++ (WideString
     may contain NULs). Return `Cow<str>` per SPEC; the PDFDoc path always
     allocates unless the bytes are pure ASCII.
- `encode_text` (C++ `PDF_EncodeText`, fpdf_parser_decode.cpp:580-626): if
  every char has a PDFDocEncoding byte (reverse lookup over the table, first
  match wins), emit those bytes; otherwise emit `FE FF` + UTF-16BE. Needed by
  doc/edit; lives here beside the table.
- Serialization helpers used by dumps and edit (`PDF_EncodeString`
  fpdf_parser_decode.cpp:628-649: wrap in `(...)`, escape `\n`→`\\n`,
  `\r`→`\\r`, and backslash-escape `( ) \`; everything else verbatim including
  other control bytes; `PDF_HexEncodeString` :651-663: `<` + uppercase-hex? —
  note `FXSYS_IntToTwoHexChars` uses uppercase `%02X` — + `>`): provide as
  free functions here.

### 1.5 Names

- Stored **decoded**: the lexer hands the raw token (after `/`) to
  `PDF_NameDecode` (fpdf_parser_utility.cpp:96-115) before construction:
  `#XY` with **both** following chars present (`i + 2 < len`) is replaced by
  the byte `hex(X)*16 + hex(Y)`, where a non-hex char counts as 0
  (`FXSYS_HexCharToInt`, fx_extension.h:88-94). A `#` in the last two
  positions stays literal. `Name` equality is on decoded bytes.
- Encoding for output (`PDF_NameEncode`, fpdf_parser_utility.cpp:117-149):
  bytes ≥ 0x80, whitespace-class bytes, `#`, and delimiter-class bytes each
  become `#` + two uppercase hex chars; all else verbatim. (Char classes:
  see the parser brief §1.0 — the classifier tables live in the parser crate;
  `PDF_NameEncode` needs them, so export the two predicates it uses from this
  crate's `name` module as private helpers duplicating the 8-char whitespace
  set and 10-char delimiter set. They are fixed by the PDF grammar, not
  parser state.)
- `Name::as_str()` — names are almost always ASCII; return `Option<&str>` via
  `str::from_utf8().ok()`. `GetUnicodeText` on a name runs `PDF_DecodeText`
  over the decoded bytes (`cpdf_name.cpp:42-44`) — provide `as_text()` too;
  `pdfium_test --show-structure` prints names that way.
- `names!` macro (STYLE §2b) generates `pub mod names` constants. Seed set =
  the constants the M1 crates need, matching `constants/*.h` spellings:
  - stream dict (`constants/stream_dict_common.h`): `Length`, `Filter`,
    `DecodeParms`, `F`, `DL`.
  - trailer/xref: `Root`, `Info`, `Size`, `Prev`, `XRefStm`, `Encrypt`, `ID`,
    `Index`, `W`, `Type`, `XRef`, `ObjStm`, `N`, `First`.
  - pages (`constants/catalog.h` has `kPages`): `Pages`, `Page`, `Kids`,
    `Count`, `Parent`.
  - encryption: `V`, `R`, `O`, `U`, `P`, `OE`, `UE`, `Perms`, `StmF`, `StrF`,
    `EFF`, `CF`, `CFM`, `AuthEvent`, `EncryptMetadata`, `Standard`,
    `Identity`.
  - linearization: `Linearized`, `L`, `H`, `O`, `E`, `T`, `P` (reuse), plus
    `Metadata`, `Subtype`, `XML`.
  The table grows crate-by-crate; one declaration site, this crate.

### 1.6 Dictionaries

C++ storage is `std::map<ByteString, RetainPtr<CPDF_Object>, std::less<>>`
(`cpdf_dictionary.h:31`) — **sorted** iteration, **last duplicate wins** on
insert (`map_[key] = value`, `cpdf_dictionary.cpp:287`). SPEC pins
`Dict(Vec<(Name, Object)>)` with linear `get`; behavioral requirements:

- `get` returns the **last** entry with the key (parser inserts in document
  order; C++'s overwrite-on-insert makes the last one the survivor).
- Iteration order divergence (sorted vs insertion) — see Divergences.
- Value invariants maintained by construction (C++ CHECKs,
  `cpdf_dictionary.cpp:284-285`): values are inline (not indirect), and
  **never a direct Stream** — streams reach dicts only through references.
  The parser enforces this (its dict grammar drops stream values); this crate
  documents it, and debug asserts it in constructors.
- Null values are never stored; C++ `SetFor(key, nullptr)` erases
  (`cpdf_dictionary.cpp:277-289`). Parsed `null` values in dict bodies are
  dropped by the syntax layer (value parse yields Null object which is stored…
  — actually C++ stores parsed `CPDF_Null` fine; only explicit
  `SetFor(nullptr)` erases. Keep parsed Nulls.)

Typed accessor resolution matrix — this is load-bearing damage-tolerance
behavior; each Rust accessor must match its C++ column
(`cpdf_dictionary.cpp:106-257`):

| C++ getter | Resolves ref? | Type filter | Fallback |
|---|---|---|---|
| `GetObjectFor` | no | any | None |
| `GetDirectObjectFor` | 1 level (see §1.8) | any | None |
| `GetByteStringFor` | via `GetString()` virtual (refs delegate 1 level) | any | `""`/default |
| `GetUnicodeTextFor` | explicitly 1 level | any | `""` |
| `GetNameFor` | **no** | Name only | `""` |
| `GetBooleanFor` | **no** | Boolean only | caller default |
| `GetIntegerFor` | via `GetInteger()` (refs delegate 1 level; ref-to-ref → 0) | any (coerces) | 0 / caller default |
| `GetDirectIntegerFor` | **no** | Number only | 0 |
| `GetFloatFor` | via `GetNumber()` (refs delegate) | any (coerces) | 0.0 |
| `GetDictFor` | yes | Dict **or Stream's dict** (`GetDictInternal`) | None |
| `GetArrayFor` | yes | Array only | None |
| `GetStreamFor` | yes | Stream only | None |
| `GetNumberFor` | **no** | Number only | None |
| `GetStringFor` (object) | **no** | String only | None |
| `GetRectFor` | via `GetArrayFor` | array len == 4 else zero rect | zero rect |
| `GetMatrixFor` | via `GetArrayFor` | array len == 6 else identity | identity |

The "no resolve" rows matter: `/Size`, `/Prev`, `/XRefStm` are read with
`GetDirectIntegerFor` in the parser (an *indirect* `/Prev` is ignored — real
recovery behavior), while `/Length` is read with `GetDirectObjectFor` (an
indirect `/Length` **is** chased). SPEC's `Dict::{int, number, name, ...}`
each take `&impl Resolve`; add the non-resolving variants the parser needs:
`direct_int(key)` (Number-typed, no resolution) and `raw(key)` (no
resolution, any type). SPEC's resolving `int()` implements the
`GetIntegerFor` row (resolve one level through the store, then coerce).

`GetDictFor` accepting a stream's dict: keep — `Array::GetDictAt` does the
same (`cpdf_array.cpp:161-175`), and pages-tree/xref code leans on it.

### 1.7 Arrays

- Accessors mirror the dict matrix (`cpdf_array.cpp:125-203`):
  `GetByteStringAt`/`GetIntegerAt`/`GetFloatAt`/`GetBooleanAt` do **not**
  resolve (but numeric coercion delegates through a Reference's virtual
  `GetInteger`/`GetNumber`, i.e. 1 level); `GetDictAt`/`GetArrayAt`/
  `GetStreamAt` resolve one level; `GetNumberAt`/`GetStringAt` do **not**
  resolve and are type-filtered (used by xref-stream `/Index` validation —
  an indirect number in `/Index` is *skipped*).
- `GetRect` (`cpdf_array.cpp:65-76`): exactly 4 elements or zero rect;
  elements via `GetFloatAt` (missing/mistyped → 0.0). `GetMatrix`
  (:78-85): exactly 6 elements or identity.
- Out-of-range index → None/default everywhere; never panics.
- Same value invariants as dicts: elements inline, no direct streams
  (`cpdf_array.cpp:244-282` CHECKs; the syntax layer drops stream elements
  citing ISO 32000-1 §7.3.8.1, `cpdf_syntax_parser.cpp:590-596`).
- `Find`/`Contains` compare by *resolved pointer identity* in C++
  (`cpdf_array.cpp:87-98`) — used by page-tree code. Our equivalent: compare
  `ObjRef`s / structural equality at the call site; do not port pointer
  identity into this crate.

### 1.8 References and resolution semantics

C++ resolution is exactly **one level** and never chases ref-to-ref:

- `CPDF_Reference::GetDirectInternal()` returns whatever the holder parsed for
  that object number — which may itself be a Reference (an indirect object
  whose body is `N 0 R`); it is returned un-chased
  (`cpdf_reference.cpp:79-82`).
- The value-coercion paths (`GetString/GetNumber/GetInteger/GetDictInternal`
  on a Reference) go through `FastGetDirect` (`cpdf_reference.cpp:65-72`),
  which additionally **refuses** a ref-to-ref target (returns null → default
  value).
- Unresolvable (free / missing / cycle-guarded) → null → typed getters produce
  their fallback.

Rust: `Object::resolve(&self, r)` per SPEC returns `Resolved::Direct(self)`
for non-refs and `Resolved::Indirect(store fetch)` for `Ref` — fetch errors
map to `Error`, and a fetched object that is itself `Ref` is returned as-is
(callers' typed matches then fail, matching C++). Typed accessors implement
the `FastGetDirect` rule: after one resolve, a `Ref` result counts as absent.

Cycle safety during `fetch` is the parser store's job (in-progress set,
SPEC §5); this crate only guarantees accessors never recurse unboundedly:
resolution is one level by construction.

### 1.9 Cloning

Ported for the edit/page layers (not used during load):

- `CloneNonCyclic` semantics (`cpdf_object.cpp:49-62`,
  `cpdf_dictionary.cpp:54-70`, `cpdf_array.cpp:49-63`,
  `cpdf_stream.cpp:100-115`, `cpdf_reference.cpp:52-63`): a visited set of
  ancestors guards recursion; **each child recurses with a copy of the set**
  (siblings may share substructure; only ancestor cycles are cut). A cyclic
  child is silently dropped (dict key omitted, array element omitted, stream
  dict → empty).
- `Clone` (shallow-with-refs) = deep copy keeping `Ref` variants.
  `CloneDirectObject` = deep copy flattening refs through the resolver;
  unresolvable/cyclic refs → dropped.
- In Rust, plain `#[derive(Clone)]` on `Object` is the `Clone` equivalent
  (values, no aliasing). Provide `clone_direct(&self, r: &impl Resolve) ->
  Option<Object>` implementing the flattening + ancestor-set rule.

### 1.10 Streams

- `Stream { dict, data: ByteSpan }` per SPEC. The C++ stream owns bytes
  (copied out of the file at parse time, `cpdf_syntax_parser.cpp:854-872`) or
  a file-backed range; we standardize on `ByteSpan` — a window into *some*
  `Arc<[u8]>`. Three backings occur in practice: the mmapped/whole file, a
  decrypted buffer (decryption replaces stream bytes,
  `cpdf_crypto_handler.cpp:288-315`), and a decoded object-stream payload.
  `ByteSpan::new(arc, range)` + `Deref<[u8]>`; range is validated at
  construction (`Result`, no panics).
- `/Length` in the dict is *not* trusted to equal `data.len()`: the parser
  repairs mismatches (parser brief §1.5) and the resulting `ByteSpan` length
  is authoritative. C++ keeps the original dict `/Length` untouched after a
  keyword-scan repair; we do the same (dict is the parsed dict; repair is
  recorded in `Diagnostics`).
- No decoded-data cache here (SPEC: page layer owns caches). Raw data
  accessor only; filter application is `pdfrum-filters` + caller.
- `CPDF_Stream::GetUnicodeText` decodes filters then `PDF_DecodeText`
  (`cpdf_stream.cpp:171-175`) — cannot live here (no filters dependency);
  the parser/doc layer composes it. Note it so nobody adds a filters dep.

### 1.11 Equality, hashing, misc

- Structural equality on `Object` (derive `PartialEq`); C++ has no generic
  object equality — tests compare field-by-field. `ObjRef`:
  `Copy + Eq + Hash` per SPEC.
- `KeyForCache` (`cpdf_object.cpp:28-35`) — not ported; Rust cache keys are
  `ObjRef` (indirect) or pointer-free structural keys chosen by the caching
  layer.
- All types `Send + Sync` (no interior mutability anywhere in this crate).
- `Object` is self-referential-free by construction (a value tree; refs are
  ids) — the C++ destructor cycle-breaking dances
  (`cpdf_dictionary.cpp:31-40`, `cpdf_array.cpp:27-35`,
  `cpdf_stream.cpp:70-75`) have no Rust equivalent and are *not* ported.

### 1.12 Hard limits touching this crate

| Constant | Value | C++ source | Where enforced |
|---|---|---|---|
| `kInvalidObjNum` | `0xFFFF_FFFF` | cpdf_object.h:52 | re-exported const; lexer refuses refs to it |
| max nesting depth | 64 | cpdf_syntax_parser.h:41 | parse time (parser crate); accessors may recurse freely on parsed values |
| integer overflow behavior | u32/i32 rules → 0 | fx_number.cpp:38-84 | number parse (lexer) + this crate's stored-range invariant |
| float fmt cap | 48 chars | cpdf_contentstream_write_utils.cpp:17 | `fmt_number` |

No C++ limit exists on string length, name length beyond the lexer's token
buffer (a *parser* behavior: names truncate at 255 bytes after `/` — see
parser brief §1.2), array length, or dict size.

---

## 2. Divergences

1. **Enum instead of class hierarchy** — settled in SPEC §2; only behavioral
   notes here. The C++ virtual-accessor defaults (`GetString()` on an Array →
   `""`, etc., `cpdf_object.cpp:64-78`) become exhaustive matches returning
   the same defaults.
2. **`Int(i64)` instead of the u32/i32 tri-state.** We store the mathematical
   parse result and reproduce both C++ observables via the accessor rules in
   §1.2 (wrapping `as_c_int` view for integer contexts, direct `as f32` for
   numeric contexts). Rationale: one variant, no unsigned/signed flag, and
   the reachable range (−2^31 ..= 2^32−1) is documented + debug-asserted.
3. **Dict iteration order = insertion (document) order**, not
   lexicographically sorted as in C++. `get` semantics (last dup wins) are
   identical, so parse-time behavior matches. Order is observable only in
   serialization and key-listing; SPEC §11 already pins serializer order to
   insertion order, and the conformance oracle for M7 is "oracle re-opens our
   files", not byte-diff. Any Tier-A dump that enumerates dict keys
   (`--show-metadata` prints a fixed key list, not an enumeration) must be
   checked during M1; if a dump does enumerate, the dump code sorts at the
   call site rather than changing storage.
4. **No string interning** (C++ `ByteStringPool` weak-pool threaded through
   constructors, e.g. `cpdf_string.cpp:20-34`). SPEC: revisit with benchmarks
   only.
5. **Unpaired surrogates in `as_text()`** become U+FFFD instead of surviving
   as lone surrogate code units (Rust `str` cannot hold them). Tier-A text
   comparisons transcode the oracle's UTF-32LE output, which can contain lone
   surrogates for garbage input; the conformance harness must apply the same
   U+FFFD normalization on the oracle side. Recorded here so the harness
   author sees it (also flagged to `pdfrum-text`).
6. **No mutation API in v1** (`SetString`, `SetFor`, `ConvertToIndirectObject…`
   etc.). Construction happens via literals/builders in the parser and edit
   layers; `Dict`/`Array` expose `push`-style constructors only. Mutable
   editing arrives with `pdfrum-edit` and operates on owned values.
7. **`Find`/`Contains` by pointer identity not ported** (§1.7).
8. **Cache keys / obj-num-on-object dropped** (§1.1, §1.11): identity lives in
   `ObjRef` keys held by the store.

---

## 3. Module plan

```
crates/pdfrum-object/src/
  lib.rs        // re-export block: Object, ObjRef, PdfString, Name, Array,
                // Dict, Stream, ByteSpan, Resolve, Resolved, Error, names,
                // fmt_number, decode_text, encode_text
  object.rs     // enum Object + type predicates + resolve() + clone_direct()
  number.rs     // parse-time invariant docs, as_c_int / as_c_float rules,
                // fmt_number (FloatToDecimal port over ryu digits)
  string.rs     // PdfString, PDFDoc table (const [u16;256] with the delta
                // spelled in §1.4), decode_text/encode_text,
                // encode_string_literal/encode_string_hex
  name.rs       // Name, name_decode/name_encode, names! macro + names mod
  dict.rs       // Dict (Vec assoc), full accessor matrix incl. direct_int/raw
  array.rs      // Array + typed accessors + rect/matrix
  stream.rs     // Stream, ByteSpan
  resolve.rs    // trait Resolve, enum Resolved<'a> (+Deref), one-level rules
  from_obj.rs   // private FromObj trait powering Dict::get_as::<T>() (STYLE §2b)
  error.rs      // Error enum (thiserror): e.g. UnresolvedRef(ObjRef),
                // RefLoop(ObjRef), SpanOutOfBounds { len, range }
```

Data flow: the parser crate constructs `Object` trees bottom-up from tokens
(no builder state in this crate), hands `Arc<Object>` roots to the store; all
higher layers read through `Dict`/`Array` accessors + `&impl Resolve`.
`Resolve` has two real implementations from day one: the parser's store and a
`HashMap`-backed test resolver in this crate's `#[cfg(test)]` (satisfies
STYLE's two-impl rule and lets the accessor matrix be tested without the
parser).

Types beyond SPEC (all small records/functions, no managers):
`fmt_number(f32) -> String` and `fmt_int(i64) -> String`;
`decode_text(&[u8]) -> String`; `encode_text(&str) -> Vec<u8>`;
`name_decode(&[u8]) -> Vec<u8>` / `name_encode(&[u8]) -> Vec<u8>`;
`const PDF_DOC_ENCODING: [u16; 256]`.

Kurbo interop: `Dict::rect`/`Array::as_rect` return `kurbo::Rect`
(re-exported via `pdfrum-common`), matching `CFX_FloatRect`
(left, bottom, right, top — note kurbo `Rect` is (x0,y0,x1,y1); document the
mapping left→x0, bottom→y0, right→x1, top→y1 and do **not** normalize:
C++ returns the values as-read, possibly inverted). Matrix →
`kurbo::Affine([a,b,c,d,e,f])`.

Lints per STYLE §3; `#![forbid(unsafe_code)]`; `indexing_slicing = warn` and
all untrusted slicing via `get()` (ByteSpan construction, table lookups are
fixed-size arrays indexed by `u8` → safe by type).

---

## 4. Test plan

Unit tests (port assertions, restated over our types):

- From `cpdf_object_unittest.cpp` (:210-469): `GetString`-per-type table
  (booleans "false"/"true", numbers "1245"/"9.00345", string "A simple test",
  name "space"; null/array/dict/stream → ""), `GetUnicodeText` (string with
  literal NULs → decoded; stream text via higher layer — skip here),
  `GetNumber` per type (non-numeric → 0), `GetInteger` per type (bool→0/1,
  float 5.2 → 5), `GetDict` per type (dict→itself, stream→its dict),
  array/dict getter fallbacks (`GetNameFor` on wrong types → "",
  `GetTypeAt`-style matrices from :509-761), reference-delegation semantics
  (`PDFObjectsTest.GetDirect`, direct/indirect matrices at :350-360).
- From `cpdf_object_unittest.cpp:471-507`: `GetMatrix`/`GetRect` count-based
  behavior (wrong length → identity/zero).
- Clone semantics: `PDFArrayTest.CloneDirectObject` (:845-862) — flattened
  refs; `PDFObjectTest.CloneCheckLoop` (:967-1013) — self-referential array /
  dict-stream loop yields clone with cycle edge dropped;
  `PDFDictionaryTest.CloneDirectObject` (:947-965).
- From `cpdf_array_unittest.cpp`: `GetBooleanAt` strictness (ints are not
  bools), `Find`/`Contains` (restated over structural eq), iterator behavior.
- From `cpdf_dictionary_unittest.cpp:13-37`: iteration + locking → restate as
  "iteration yields insertion order" (divergence #3 documented in the test).
- From `cpdf_number_unittest.cpp:37-131`: the exact `fmt_number` goldens of
  §1.2 — these are the dragonbox-parity anchors; plus hand-written
  `FX_Number` parse-rule tests: `"4294967295"` (int view −1, float view
  4294967296.0), `"2147483648"` (int view −2147483648), `"5000000000"` → 0,
  `"-2147483648"` exact, `"+5"` → 5, `"12abc"` → 12, `""` → 0, `"--37"` →
  0 (sign not at digit start), `"1.5e3"` → 1500.0 (contains `.` → float
  path, fast_float general format accepts exponents), `"4.5.6"` → float
  parse of the prefix per fast_float (4.5).
- From `fpdf_parser_decode_unittest.cpp:324-434`: `DecodeText` /
  `DecodeTextWithUnicodeEscapes` (0x1B regions) /
  `DecodeTextWithInvalidUnicodeEscapes` / `...UnpairedSurrogates` (adjusted
  for U+FFFD, divergence #5) / `EncodeText` / `RoundTripText`.
- From `fpdf_parser_utility_unittest.cpp:23-41`: `NameDecode`
  (`"A#42"→"AB"`, trailing-# literals, non-hex-after-# behavior) and
  `NameEncode` goldens.

Snapshot tests (insta): a `Debug`-formatted dump of one composite object tree
(dict containing every variant) — guards accidental representation changes.

Fuzz targets: `decode_text` (untrusted bytes in, must never panic; asserts
output len ≤ input len for PDFDoc path? no — UTF-8 expansion; just no-panic +
no-OOM), `name_decode`. The lexer/object-body fuzzers live in the parser
crate.

Conformance clusters: none directly (this crate has no file-level surface);
it is exercised through parser-crate M1 clusters (page counts, metadata
dumps) and `pdfrum-text` Tier-A later.

---

## 5. Open questions

1. **Hex-string flag on `PdfString`.** C++ round-trips `<...>` vs `(...)`
   spelling via `output_is_hex_`; SPEC pins `PdfString(pub Box<[u8]>)` with no
   flag. `pdfrum-edit` (M7 round-trip) and possibly `--annot` dumps need it.
   Proposal: `[spec]` change adding `pub hex: bool` (or
   `PdfString { bytes: Box<[u8]>, hex: bool }`) before implementation.
   Needs user/spec sign-off.
2. **Where does the wrapping "C int view" surface?** Proposal (§1.2):
   `Dict::int()`/`Array::int_at()` return the C-int view as `i64` so xref
   `/Size`, `/N`, `/First` etc. behave byte-for-byte like C++. Alternative —
   return the stored i64 and let callers wrap — risks silent drift at dozens
   of call sites. Resolve in review of this brief; default to the proposal.
3. **`Resolved` + fetch error signaling.** SPEC's `Dict::get -> Option`
   swallows the difference between "absent" and "fetch failed" (C++ treats
   both as absent everywhere). Proposal: keep `Option` (match C++), and rely
   on the store recording fetch failures in `Diagnostics`. No spec change.
4. **kurbo `Rect` non-normalization** (§3): confirm downstream crates expect
   possibly-inverted rects (C++ `CFX_FloatRect` normalizes only when asked);
   page crate brief must restate.
