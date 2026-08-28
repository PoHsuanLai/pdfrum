# Design Brief — `pdfrum-cmap`

**Behavior source (read-only oracle):** `/mnt/data2/pdfium/pdfium-c++`
- `core/fpdfapi/cmaps/fpdf_cmaps.h` / `.cpp` — the static-table binary format and
  the two lookup functions (`CIDFromCharCode`, `CharCodeFromCID`)
- `core/fpdfapi/cmaps/{CNS1,GB1,Japan1,Korea1}/` — 56 CMap data arrays, 4 index
  tables (`cmaps_*.inc`), 4 CID→Unicode tables (`Adobe-*-UCS2_*.inc`)
- `core/fpdfapi/font/cpdf_cmap.h` / `.cpp` — `CPDF_CMap`: the predefined-name
  table, coding schemes, `GetNextChar` state machine, `CIDFromCharCode`
- `core/fpdfapi/font/cpdf_cmapparser.h` / `.cpp` — the embedded-CMap word parser
- `core/fpdfapi/font/cpdf_cid2unicodemap.cpp` — CID→Unicode over the static tables
- `core/fpdfapi/font/cpdf_fontglobals.cpp` — registry wiring (a global we erase)
- `core/fpdfapi/parser/cpdf_simple_parser.cpp` — the PostScript-ish word lexer
  both the CMap parser and the ToUnicode parser run on
- Tests: `core/fpdfapi/font/cpdf_cmapparser_unittest.cpp`

**Contract:** SPEC.md §6. **Style:** STYLE.md. **Deps:** DEPS.md — this crate has
**no external dependencies** beyond `pdfrum-common` and `pdfrum-object`
(`thiserror` for `Error`). The static tables become a `build.rs`-generated blob.

This brief is written to be sufficient on its own: an implementing agent should
need SPEC.md + STYLE.md + this file, and never the C++.

---

## 1. Behavior inventory

A "CMap" in PDF is two things fused together, and PDFium fuses them too:

1. a **byte-decoder**: how a string of bytes splits into character codes
   (1-byte, 2-byte, or mixed-width), and
2. a **charcode → CID map**.

`CPDF_CMap` carries both. There are two construction paths that share almost no
code but must produce the same observable behavior surface:

- **predefined** — `CPDF_CMap(ByteStringView bsPredefinedName)`
  (`cpdf_cmap.cpp:274-301`): decoder comes from a hard-coded 33-entry table, the
  CID map from the static CJK tables in `core/fpdfapi/cmaps/`.
- **embedded** — `CPDF_CMap(span<const uint8_t>)` (`cpdf_cmap.cpp:303-315`):
  both come from parsing a PostScript CMap program out of a stream.

Everything in §1.1–§1.4 is the predefined path, §1.5–§1.9 the embedded path,
§1.10–§1.12 shared.

### 1.1 The predefined-CMap name table (33 entries, complete)

`kPredefinedCMaps`, `cpdf_cmap.cpp:36-173`. Each row is
`{ name, charset, coding, coding_scheme, leading_segs[2] }`. A `leading_segs`
entry is an **inclusive** `{first, last}` byte range; a `{0,0}` entry terminates
the list (`LoadLeadingSegments`, `:187-199` — note the consequence: a leading
range that legitimately starts and ends at byte 0x00 is unrepresentable, which
is fine because no real range does).

| # | name | charset | coding | scheme | leading segs |
|---|---|---|---|---|---|
| 0 | `GB-EUC` | GB1 | GB | MixedTwoBytes | a1–fe |
| 1 | `GBpc-EUC` | GB1 | GB | MixedTwoBytes | a1–fc |
| 2 | `GBK-EUC` | GB1 | GB | MixedTwoBytes | 81–fe |
| 3 | `GBKp-EUC` | GB1 | GB | MixedTwoBytes | 81–fe |
| 4 | `GBK2K-EUC` | GB1 | GB | MixedTwoBytes | 81–fe |
| 5 | `GBK2K` | GB1 | GB | MixedTwoBytes | 81–fe |
| 6 | `UniGB-UCS2` | GB1 | UCS2 | TwoBytes | — |
| 7 | `UniGB-UTF16` | GB1 | UTF16 | TwoBytes | — |
| 8 | `B5pc` | CNS1 | BIG5 | MixedTwoBytes | a1–fc |
| 9 | `HKscs-B5` | CNS1 | BIG5 | MixedTwoBytes | 88–fe |
| 10 | `ETen-B5` | CNS1 | BIG5 | MixedTwoBytes | a1–fe |
| 11 | `ETenms-B5` | CNS1 | BIG5 | MixedTwoBytes | a1–fe |
| 12 | `UniCNS-UCS2` | CNS1 | UCS2 | TwoBytes | — |
| 13 | `UniCNS-UTF16` | CNS1 | UTF16 | TwoBytes | — |
| 14 | `83pv-RKSJ` | Japan1 | JIS | MixedTwoBytes | 81–9f, e0–fc |
| 15 | `90ms-RKSJ` | Japan1 | JIS | MixedTwoBytes | 81–9f, e0–fc |
| 16 | `90msp-RKSJ` | Japan1 | JIS | MixedTwoBytes | 81–9f, e0–fc |
| 17 | `90pv-RKSJ` | Japan1 | JIS | MixedTwoBytes | 81–9f, e0–fc |
| 18 | `Add-RKSJ` | Japan1 | JIS | MixedTwoBytes | 81–9f, e0–fc |
| 19 | `EUC` | Japan1 | JIS | MixedTwoBytes | 8e–8e, a1–fe |
| 20 | `H` | Japan1 | JIS | **TwoBytes** | 21–7e *(ignored, see below)* |
| 21 | `V` | Japan1 | JIS | **TwoBytes** | 21–7e *(ignored)* |
| 22 | `Ext-RKSJ` | Japan1 | JIS | MixedTwoBytes | 81–9f, e0–fc |
| 23 | `UniJIS-UCS2` | Japan1 | UCS2 | TwoBytes | — |
| 24 | `UniJIS-UCS2-HW` | Japan1 | UCS2 | TwoBytes | — |
| 25 | `UniJIS-UTF16` | Japan1 | UTF16 | TwoBytes | — |
| 26 | `KSC-EUC` | Korea1 | KOREA | MixedTwoBytes | a1–fe |
| 27 | `KSCms-UHC` | Korea1 | KOREA | MixedTwoBytes | 81–fe |
| 28 | `KSCms-UHC-HW` | Korea1 | KOREA | MixedTwoBytes | 81–fe |
| 29 | `KSCpc-EUC` | Korea1 | KOREA | MixedTwoBytes | a1–fd |
| 30 | `UniKS-UCS2` | Korea1 | UCS2 | TwoBytes | — |
| 31 | `UniKS-UTF16` | Korea1 | UTF16 | TwoBytes | — |

**Quirk (rows 20, 21):** `H` and `V` declare `leading_segs = {0x21,0x7e}` but
their scheme is `TwoBytes`; `LoadLeadingSegments` is called **only** when the
scheme is `MixedTwoBytes` (`cpdf_cmap.cpp:290-292`), so those bytes are dead
data. Port the table verbatim including the dead entries — a future reader
comparing tables must find them identical — but the loader must reproduce the
same "only for MixedTwoBytes" gate.

**Name lookup — the `-H`/`-V` suffix strip** (`GetPredefinedCMap`, `:175-185`):

```
if name.len() > 2 { name = name[..name.len()-2] }   // unconditional!
linear scan of the 33 rows for an exact byte match
```

This is not "strip a `-H` or `-V` suffix". It **unconditionally removes the last
two bytes of any name longer than 2**, then compares. Consequences that must be
reproduced exactly:

- `"GB-EUC-H"` → `"GB-EUC"` ✓.
- `"UniJIS-UCS2-HW-H"` → `"UniJIS-UCS2-HW"` ✓ (row 24).
- `"UniJIS-UCS2-HW"` (no direction suffix) → `"UniJIS-UCS2"` → matches row 23.
  A caller asking for the HW variant *without* a suffix silently gets the
  non-HW row's decoder. Preserve this.
- `"H"` and `"V"` are length 1, so no strip happens and they match rows 20/21
  directly.
- `"GBK2K-EUC-H"` → `"GBK2K-EUC"` (row 4); `"GBK2K-H"` → `"GBK2K"` (row 5).
  Both exist as separate rows for exactly this reason.
- Any garbage name of length ≥ 3 has its last 2 bytes eaten before the compare,
  so e.g. `"GB-EUC-XY"` matches row 0. This is a real damage-tolerance behavior,
  not a bug to fix.

**Vertical flag** (`cpdf_cmap.cpp:275`): `vertical_ = name.back() == 'V'`,
computed on the **unstripped** name, before any table lookup, and **before** the
Identity check. So `"Identity-V"` is vertical, and so is a nonsense name ending
in `V`.

**Identity short-circuit** (`:276-280`): if the full name is exactly
`"Identity-H"` or `"Identity-V"`, set `coding = kCID`, `loaded = true`, and
return — no charset, no embed map, scheme stays at its default `TwoBytes`.

**Leading `/`** — `LoadPredefinedCMap` (`cpdf_fontglobals.cpp:26-31`) strips one
leading `/` before construction. This matters because `/Encoding` may arrive as
a `Name` whose spelling in some call paths still carries the slash.

**Failure mode** (`:282-300`): unknown name ⇒ everything default, `loaded_`
stays `false`, `charset_` stays `kUnknown`, `coding_` stays `kUNKNOWN`, scheme
stays `TwoBytes`. **The object is still returned and still used.** A font with
`/Encoding /Nonsense` therefore decodes its strings as fixed 2-byte codes and
maps every code to itself (see §1.10). This is the single most important
damage-tolerance path in this crate.

Also failure: a name that matches a row but whose charset has no entry with that
exact (unstripped) name in the static index table (`FindEmbeddedCMap`, `:262-270`)
leaves `embed_map_` null and `loaded_` false — but `charset_`, `coding_` and
`coding_scheme_` are **already set** at that point (`:287-289`). So a
half-configured CMap is a real, reachable state: correct decoder, no CID map.

### 1.2 `CIDSet` and `CIDCoding` enums

`CIDSet` (`cpdf_cidfont.h:22-30`), a `uint8_t`, and its ordinal values are
load-bearing (used to index `kCharsetCodePages` and the CID2Unicode array):
`kUnknown=0, kGB1=1, kCNS1=2, kJapan1=3, kKorea1=4, kUnicode=5, kNumSets=6`.

`CIDCoding` (`cpdf_cmap.h:25-34`), also `uint8_t`:
`kUNKNOWN=0, kGB=1, kBIG5=2, kJIS=3, kKOREA=4, kUCS2=5, kCID=6, kUTF16=7`.

`CodingScheme` (`cpdf_cmap.h:40-45`): `OneByte, TwoBytes, MixedTwoBytes,
MixedFourBytes`. **Default is `TwoBytes`** (`cpdf_cmap.h:96`) — this default is
what an unrecognized predefined name and a codespace-range-free embedded CMap
both fall back to.

`CharsetFromOrdering` (`cpdf_cmapparser.cpp:213-223`) maps the `/Ordering`
string to a `CIDSet`, using the table
`{nullptr, "GB1", "CNS1", "Japan1", "Korea1", "UCS"}` indexed by the enum
ordinal, scanning from index 1. Note `"UCS"` (not `"UCS2"`, not `"Identity"`)
is the spelling that yields `kUnicode`. Anything else ⇒ `kUnknown`.

### 1.3 The static-table binary layout — exact, for `build.rs`

This is the format `build.rs` must convert. Source of truth:
`fpdf_cmaps.h:14-33` for the structs, `fpdf_cmaps.cpp:21-77` for how they are
reinterpreted.

**The index record** — `fxcmap::CMap`, one per predefined CMap per registry:

```c
struct CMap {
  const char*        name_;            // NUL-terminated ASCII, e.g. "GB-EUC-H"
  const uint16_t*    word_map_;        // reinterpreted, see below
  const DWordCIDMap* dword_map_;       // may be null
  uint16_t           word_count_;      // records, NOT uint16s
  uint16_t           dword_count_;     // records
  enum Type : bool { kSingle, kRange } word_map_type_;
  int8_t             use_offset_;      // signed index delta, 0 = none
};
```

**`word_map_` is a type-punned array.** `word_count_` counts *records*, and the
record width depends on `word_map_type_` (`fpdf_cmaps.cpp:29-38`):

- `kSingle` ⇒ `struct SingleCmap { uint16_t code; uint16_t cid; }`
  — array length in `uint16_t` is `word_count_ * 2`.
- `kRange` ⇒ `struct RangeCmap { uint16_t low; uint16_t high; uint16_t cid; }`
  — array length in `uint16_t` is `word_count_ * 3`.

The declared C array dimensions confirm this exactly: e.g.
`kGB_EUC_H_0[90 * 3]` with `word_count_ = 90, kRange`, and
`kUniCNS_UTF16_H_0[14557 * 2]` with `word_count_ = 14557, kSingle`.

**`dword_map_`** — `struct DWordCIDMap { uint16_t hi_word_, lo_word_low_,
lo_word_high_, cid_; }`, 4 × `uint16_t` per record, `dword_count_` records.
Only 3 CMaps have one (`kGBK2K_H_5_DWord[1017]`, `kCNS_EUC_H_0_DWord[238]`,
`kCNS_EUC_V_0_DWord[261]`).

**Both arrays are sorted** and looked up with `std::lower_bound` (§1.4). The
generator must assert sortedness on the source data and preserve order; do not
re-sort silently, because a table that is *not* sorted in the C++ produces the
C++'s lower_bound result on unsorted input, which our port must match. (In
practice all 56 are sorted; assert and fail the build if one is not, and
escalate rather than "fixing" it.)

**`use_offset_`** is a **signed index delta within the same registry's index
array** (`FindNextCMap`, `fpdf_cmaps.cpp:47-54`): the next CMap in the chain is
`&cmap[use_offset_]`. `0` terminates. E.g. in `kGB1_cmaps`, `"GB-EUC-V"` has
`use_offset_ = -1`, so it falls back to `"GB-EUC-H"` at index 0; `"GBK2K-H"` has
`-4`, chaining to `"GBK-EUC-H"`. This is the static equivalent of `usecmap`. In
our blob it becomes an explicit `Option<u16>` index (or a `u16` sentinel) —
never a pointer offset.

**The 4 index tables (complete).** All names below are exactly as they appear in
the `name_` field; `wc`/`dc` are `word_count_`/`dword_count_`; `T` is
`R`(ange)/`S`(ingle); `uo` is `use_offset_`.

`kGB1_cmaps` (`GB1/cmaps_gb1.inc:7-25`), 14 entries:

| i | name | word array | dword array | wc | dc | T | uo |
|---|---|---|---|---|---|---|---|
| 0 | GB-EUC-H | kGB_EUC_H_0 | — | 90 | 0 | R | 0 |
| 1 | GB-EUC-V | kGB_EUC_V_0 | — | 20 | 0 | R | −1 |
| 2 | GBpc-EUC-H | kGBpc_EUC_H_0 | — | 91 | 0 | R | 0 |
| 3 | GBpc-EUC-V | kGBpc_EUC_V_0 | — | 20 | 0 | R | −1 |
| 4 | GBK-EUC-H | kGBK_EUC_H_2 | — | 4071 | 0 | R | 0 |
| 5 | GBK-EUC-V | kGBK_EUC_V_2 | — | 20 | 0 | R | −1 |
| 6 | GBKp-EUC-H | kGBKp_EUC_H_2 | — | 4070 | 0 | R | −2 |
| 7 | GBKp-EUC-V | kGBKp_EUC_V_2 | — | 20 | 0 | R | −1 |
| 8 | GBK2K-H | kGBK2K_H_5 | kGBK2K_H_5_DWord | 4071 | 1017 | R | −4 |
| 9 | GBK2K-V | kGBK2K_V_5 | — | 41 | 0 | R | −1 |
| 10 | UniGB-UCS2-H | kUniGB_UCS2_H_4 | — | 13825 | 0 | R | 0 |
| 11 | UniGB-UCS2-V | kUniGB_UCS2_V_4 | — | 24 | 0 | R | −1 |
| 12 | UniGB-UTF16-H | kUniGB_UCS2_H_4 | — | 13825 | 0 | R | 0 |
| 13 | UniGB-UTF16-V | kUniGB_UCS2_V_4 | — | 24 | 0 | R | −1 |

Note rows 12/13 **alias** rows 10/11's data arrays: UTF16 and UCS2 share the
table. The blob must dedupe by array identity, not by name.

`kCNS1_cmaps` (`CNS1/cmaps_cns1.inc:7-27`), 16 entries:

| i | name | word array | dword array | wc | dc | T | uo |
|---|---|---|---|---|---|---|---|
| 0 | B5pc-H | kB5pc_H_0 | — | 247 | 0 | R | 0 |
| 1 | B5pc-V | kB5pc_V_0 | — | 12 | 0 | R | −1 |
| 2 | HKscs-B5-H | kHKscs_B5_H_5 | — | 1210 | 0 | R | 0 |
| 3 | HKscs-B5-V | kHKscs_B5_V_5 | — | 13 | 0 | R | −1 |
| 4 | ETen-B5-H | kETen_B5_H_0 | — | 254 | 0 | R | 0 |
| 5 | ETen-B5-V | kETen_B5_V_0 | — | 13 | 0 | R | −1 |
| 6 | ETenms-B5-H | kETenms_B5_H_0 | — | **1** | 0 | R | −2 |
| 7 | ETenms-B5-V | kETenms_B5_V_0 | — | 18 | 0 | R | −1 |
| 8 | CNS-EUC-H | kCNS_EUC_H_0 | kCNS_EUC_H_0_DWord | 157 | 238 | R | 0 |
| 9 | CNS-EUC-V | kCNS_EUC_V_0 | kCNS_EUC_V_0_DWord | 180 | 261 | R | 0 |
| 10 | UniCNS-UCS2-H | kUniCNS_UCS2_H_3 | — | 16418 | 0 | R | 0 |
| 11 | UniCNS-UCS2-V | kUniCNS_UCS2_V_3 | — | 13 | 0 | R | −1 |
| 12 | UniCNS-UTF16-H | kUniCNS_UTF16_H_0 | — | 14557 | 0 | **S** | 0 |
| 13 | UniCNS-UTF16-V | kUniCNS_UCS2_V_3 | — | 13 | 0 | R | −1 |

(Note: `UniCNS-UTF16-V` at index 13 aliases index 11's array, and its
`use_offset_ = -1` points at index 12, the UTF16-H *Single* table — a chain
crossing between a Range table and a Single table. `CIDFromCharCode` switches on
each link's own `word_map_type_`, so this works; the blob must keep per-entry
type, not per-chain.)

Also note row 6, `ETenms-B5-H`, has `word_count_ = 1`: essentially all of its
lookups fall through `use_offset_ = -2` to `ETen-B5-H` at index 4.

`kJapan1_cmaps` (`Japan1/cmaps_japan1.inc:7-28`), 20 entries:

| i | name | word array | wc | T | uo |
|---|---|---|---|---|---|
| 0 | 83pv-RKSJ-H | k83pv_RKSJ_H_1 | 222 | R | 0 |
| 1 | 90ms-RKSJ-H | k90ms_RKSJ_H_2 | 171 | R | 0 |
| 2 | 90ms-RKSJ-V | k90ms_RKSJ_V_2 | 78 | R | −1 |
| 3 | 90msp-RKSJ-H | k90msp_RKSJ_H_2 | 170 | R | −2 |
| 4 | 90msp-RKSJ-V | k90msp_RKSJ_V_2 | 78 | R | −1 |
| 5 | 90pv-RKSJ-H | k90pv_RKSJ_H_1 | 263 | R | 0 |
| 6 | Add-RKSJ-H | kAdd_RKSJ_H_1 | 635 | R | 0 |
| 7 | Add-RKSJ-V | kAdd_RKSJ_V_1 | 57 | R | −1 |
| 8 | EUC-H | kEUC_H_1 | 120 | R | 0 |
| 9 | EUC-V | kEUC_V_1 | 27 | R | −1 |
| 10 | Ext-RKSJ-H | kExt_RKSJ_H_2 | 665 | R | −4 |
| 11 | Ext-RKSJ-V | kExt_RKSJ_V_2 | 39 | R | −1 |
| 12 | H | kH_1 | 118 | R | 0 |
| 13 | V | kV_1 | 27 | R | −1 |
| 14 | UniJIS-UCS2-H | kUniJIS_UCS2_H_4 | 9772 | **S** | 0 |
| 15 | UniJIS-UCS2-V | kUniJIS_UCS2_V_4 | 251 | **S** | −1 |
| 16 | UniJIS-UCS2-HW-H | kUniJIS_UCS2_HW_H_4 | **4** | R | −2 |
| 17 | UniJIS-UCS2-HW-V | kUniJIS_UCS2_HW_V_4 | 199 | R | −1 |
| 18 | UniJIS-UTF16-H | kUniJIS_UCS2_H_4 | 9772 | **S** | 0 |
| 19 | UniJIS-UTF16-V | kUniJIS_UCS2_V_4 | 251 | **S** | −1 |

No dword maps in Japan1. Row 16 has only 4 range records and chains −2 to
row 14 (`UniJIS-UCS2-H`, Single) — again a mixed-type chain.

`kKorea1_cmaps` (`Korea1/cmaps_korea1.inc:7-20`), 11 entries:

| i | name | word array | wc | T | uo |
|---|---|---|---|---|---|
| 0 | KSC-EUC-H | kKSC_EUC_H_0 | 467 | R | 0 |
| 1 | KSC-EUC-V | kKSC_EUC_V_0 | 16 | R | −1 |
| 2 | KSCms-UHC-H | kKSCms_UHC_H_1 | 675 | R | −2 |
| 3 | KSCms-UHC-V | kKSCms_UHC_V_1 | 16 | R | −1 |
| 4 | KSCms-UHC-HW-H | kKSCms_UHC_HW_H_1 | 675 | R | 0 |
| 5 | KSCms-UHC-HW-V | kKSCms_UHC_HW_V_1 | 16 | R | −1 |
| 6 | KSCpc-EUC-H | kKSCpc_EUC_H_0 | 509 | R | **−6** |
| 7 | UniKS-UCS2-H | kUniKS_UCS2_H_1 | 8394 | R | 0 |
| 8 | UniKS-UCS2-V | kUniKS_UCS2_V_1 | 18 | R | −1 |
| 9 | UniKS-UTF16-H | kUniKS_UTF16_H_0 | 158 | **S** | −2 |
| 10 | UniKS-UTF16-V | kUniKS_UCS2_V_1 | 18 | R | −1 |

Korea1 has **no `KSCpc-EUC-V`** (only 11 rows, breaking the H/V pairing). Row 6
chains −6 to row 0. Row 9 chains −2 to row 7 (Single → Range).

**Total data volume.** 56 word arrays + 3 dword arrays. Sum of `word_count_`
records: GB1 (dedup by array) = 90+20+91+20+4071+20+4070+20+4071+41+13825+24 =
26363; CNS1 = 247+12+1210+13+254+13+1+18+157+180+16418+13+14557 = 33093;
Japan1 = 222+171+78+170+78+263+635+57+120+27+665+39+118+27+9772+251+4+199 =
12896; Korea1 = 467+16+675+16+675+16+509+8394+18+158 = 10944. In `u16` units:
GB1 26363×3 = 79089 (all Range); CNS1 = (33093−14557)×3 + 14557×2 = 55608+29114
= 84722; Japan1 = (12896−9772−251)×3 + (9772+251)×2 = 8619+20046 = 28665;
Korea1 = (10944−158)×3 + 158×2 = 32358+316 = 32674. Plus dword: (1017+238+261)
×4 = 6064 `u16`. **Grand total ≈ 231 214 `u16` = ~452 KiB** for the CMap tables.

**The CID→Unicode tables** (`Adobe-*-UCS2_*.inc`), plain `uint16_t` arrays
indexed by CID, one per registry, `0xFFFD` at index 0:

| symbol | file | length (u16) |
|---|---|---|
| `kGB1CID2Unicode_5` | `GB1/Adobe-GB1-UCS2_5.inc` | 30 284 |
| `kCNS1CID2Unicode_5` | `CNS1/Adobe-CNS1-UCS2_5.inc` | 19 088 |
| `kJapan1CID2Unicode_4` | `Japan1/Adobe-Japan1-UCS2_4.inc` | 15 444 |
| `kKorea1CID2Unicode_2` | `Korea1/Adobe-Korea1-UCS2_2.inc` | 18 352 |

Total 83 168 `u16` = 162 KiB. **Grand total blob ≈ 615 KiB uncompressed.**

### 1.4 Static-table lookup — `CIDFromCharCode` and `CharCodeFromCID`

**`fxcmap::CIDFromCharCode(cmap, charcode) -> u16`** (`fpdf_cmaps.cpp:79-121`).

If `charcode >> 16` is nonzero, dispatch to the dword path. Otherwise, with
`loword = charcode as u16`, walk the `use_offset_` chain; at each link, switch on
that link's own `word_map_type_`:

- **kSingle**: `lower_bound` over the `(code, cid)` records comparing
  `element.code < loword`. Hit iff `found != end && found->code == loword`;
  return `found->cid`.
- **kRange**: `lower_bound` comparing `element.high < loword` (i.e. find the
  first record whose `high >= loword`). Hit iff
  `found != end && loword >= found->low && loword <= found->high`; return
  `found->cid + loword - found->low`.

Miss ⇒ follow `use_offset_`; chain exhausted ⇒ **return 0**. Note the C++
`CHECK(cmap->word_map_)` at `:87` — every link in every chain has a non-null
word map, which the blob generator must guarantee.

**Dword path** — `CIDFromCharCodeForDword` (`:56-77`). Walks the same
`use_offset_` chain, but skips links with a null `dword_map_`. At each link,
`lower_bound` over the `DWordCIDMap` records with a **two-key** comparator:

```
element < charcode  iff
    element.hi_word_ != (charcode >> 16) ? element.hi_word_ < (charcode >> 16)
                                         : element.lo_word_high_ < (charcode as u16)
```

Hit iff `found != end && loword >= found->lo_word_low_ && loword <=
found->lo_word_high_`, returning `found->cid_ + loword - found->lo_word_low_`.
Note `loword` here is `charcode as u16` — the comparator's high-word equality is
**not re-checked on the hit test**, so a record from a *different* hi_word whose
lo range happens to contain `loword` can match. `lower_bound` semantics make
this reachable only when the hi_word does not appear in the table at all and the
first record with a greater hi_word has a lo range covering `loword`. Reproduce
it; do not "fix" it.

Miss on all links ⇒ 0. **Important:** the dword path never falls back to the
word path, so a charcode ≥ 0x10000 in a CMap with no dword table is always 0.

**`fxcmap::CharCodeFromCID(cmap, cid) -> u32`** (`fpdf_cmaps.cpp:123-159`) — the
inverse, a **linear scan** (no binary search; the tables are sorted by code, not
by cid). Walks the `use_offset_` chain; per link, by type:

- kSingle: first record with `single.cid == cid` ⇒ return `single.code`.
- kRange: first record with `cid >= range.cid && cid <= range.cid + range.high -
  range.low` ⇒ return `range.low + cid - range.cid`.

Exhausted ⇒ 0. The C++ carries a standing `TODO(dsinclair)` noting it never
consults `dword_map_` — **a CID that only exists in the dword table is
unreachable by reverse lookup.** Preserve.

This function is used only on non-Windows builds, from
`EmbeddedCharcodeFromUnicode` (`cpdf_cidfont.cpp:179-197`), inside an O(n) scan
of the entire CID2Unicode table looking for a matching unicode. That whole path
is O(table_size × chain_length) per call. See §2 D4.

### 1.5 The embedded-CMap word lexer

Embedded CMaps are parsed by feeding `CPDF_SimpleParser::GetWord()` output to
`CPDF_CMapParser::ParseWord()` until a word comes back empty
(`cpdf_cmap.cpp:303-315`). `CPDF_SimpleParser` (`cpdf_simple_parser.cpp`) is a
minimal PostScript-ish tokenizer — **not** the PDF object parser — and its exact
tokenization is behavior:

- `SkipSpacesAndComments` (`:54-83`): skips PDF whitespace, then if the char is
  `%` skips to the next line ending and repeats. End of data ⇒ empty word.
- A non-delimiter start char ⇒ `HandleNonDelimiter` (`:135-146`): run until the
  next delimiter or whitespace. Token includes the start char.
- `/` ⇒ `HandleName` (`:85-97`): run until whitespace or delimiter. **If the
  data ends without hitting one, returns an empty ByteStringView** — i.e. a name
  at EOF with no trailing separator is silently dropped and terminates the
  parse loop.
- `<` ⇒ `HandleBeginAngleBracket` (`:99-116`): if the next char is `<`, return
  the 2-byte token `"<<"`. Otherwise run until a `>` is consumed or data ends;
  the token **includes** both `<` and `>`. On truncation the token is the
  unterminated remainder.
- `>` ⇒ `HandleEndAngleBracket` (`:118-124`): consumes a following `>` if
  present; token is `">"` or `">>"`.
- `(` ⇒ `HandleParentheses` (`:126-133`): balanced-paren scan with a nesting
  counter, **no escape handling** — a `\(` increments the level. Token includes
  both parens.
- Any other delimiter (`)`, `[`, `]`, `{`, `}`) ⇒ a single-char token.

The PDF delimiter set is `( ) < > [ ] { } / %` and whitespace is
`\0 \t \n \f \r space`.

`pdfrum-page`'s content lexer is a *different* lexer; do not share. This one
must live in `pdfrum-cmap` (and be re-exported for `pdfrum-font`'s ToUnicode
parser, which uses the identical tokenizer — see the font brief §1.6).

### 1.6 `CPDF_CMapParser::ParseWord` — the state machine

`cpdf_cmapparser.cpp:39-80`. A single `status_` enum plus a `code_seq_` counter
plus `last_word_`. Dispatch order is exactly:

```
word == "begincidchar"        -> status = ProcessingCidChar;      code_seq = 0
word == "begincidrange"       -> status = ProcessingCidRange;     code_seq = 0
word == "endcidrange" | "endcidchar" -> status = Start
word == "/WMode"              -> status = ProcessingWMode
word == "/Registry"           -> status = ProcessingRegistry
word == "/Ordering"           -> status = ProcessingOrdering
word == "/Supplement"         -> status = ProcessingSupplement
word == "begincodespacerange" -> status = ProcessingCodeSpaceRange; code_seq = 0
word == "usecmap"             -> (nothing at all)
else, by current status:
  ProcessingCidChar | ProcessingCidRange -> HandleCid(word)
  ProcessingRegistry                     -> status = Start   (value discarded)
  ProcessingOrdering  -> SetCharset(CharsetFromOrdering(CMap_GetString(word)));
                         status = Start
  ProcessingSupplement                   -> status = Start   (value discarded)
  ProcessingWMode     -> SetVertical(GetCode(word) != 0); status = Start
  ProcessingCodeSpaceRange -> HandleCodeSpaceRange(word)
last_word_ = word           // always, unconditionally
```

Things worth stating explicitly because they are load-bearing:

- **The keyword tests come first and are unconditional.** A literal token
  `"begincidchar"` appearing where a CID value was expected resets the state
  machine instead of being consumed as data.
- **`usecmap` is a complete no-op.** PDFium does **not** chase `usecmap` in
  embedded CMaps at all — no name resolution, no chaining, nothing. This is a
  significant fidelity fact: an embedded CMap that does `/GBK-EUC-H usecmap`
  and then only overrides a handful of codes gets **none** of the base map.
  Its `direct_charcode_to_cidtable_` has whatever it set explicitly and zeros
  everywhere else. SPEC §6 says the API offers "usecmap" — see §2 D1.
- `CMap_GetString` (`:23-28`) is `word.len() <= 2 ? "" : word[2..]`. For
  `/Ordering` the following word is expected to be a PostScript string
  `(Japan1)`, and the tokenizer hands back `"(Japan1)"` including both parens;
  `[2..]` yields `"apan1)"` — **which never matches any charset name.** Wait:
  that is for a *literal* string. In practice the `/Ordering` value in real
  CMaps is written `(Japan1)`, length 8, `[2..]` = `"apan1)"`. Compare against
  `"Japan1"` fails. **So `/Ordering` parsing in an embedded CMap essentially
  never sets the charset for PostScript-string spellings**, and only works when
  the value arrives as a token whose bytes at `[2..]` equal the name — e.g. the
  hex-string spelling or a `<<`-prefixed artifact. Port `CMap_GetString`
  verbatim as `word.get(2..).unwrap_or_default()`; do not "correct" it to strip
  parens. Tested by conformance, not by reasoning.
- `/WMode` reads the **next** word through `GetCode` and sets vertical iff
  nonzero. `GetCode("1")` = 1. `GetCode("<1>")` = 1 too (hex path).

### 1.7 `GetCode` — the number scanner

`cpdf_cmapparser.cpp:146-170`, `static`, and directly unit-tested.

```
empty        -> 0
starts '<'   -> scan bytes [1..] while hex digit, accumulate num = num*16 + d
                on u32 overflow -> return 0 (whole result, not partial)
otherwise    -> scan bytes [0..] while decimal digit, num = num*10 + d
                on overflow -> 0
```

Both loops **stop at the first non-matching byte** and return what was
accumulated — they do not fail. Pinned assertions
(`cpdf_cmapparser_unittest.cpp:21-37`):

| input | result |
|---|---|
| `""` | 0 |
| `"<"` | 0 (loop body never runs) |
| `"<c2"` | 194 |
| `"<A2"` | 162 |
| `"<Af2"` | 2802 |
| `"<A2z"` | 162 (stops at `z`) |
| `"12"` | 12 |
| `"12d"` | 12 |
| `"128"` | 128 |
| `"<FFFFFFFF"` | 4294967295 |
| `"<100000000"` | **0** (u32 overflow ⇒ whole result 0) |

Note the tokenizer hands `HandleCid` tokens *with* their trailing `>`, e.g.
`"<A2>"` — the hex loop stops at `>` and yields 162. The unittest calls
`GetCode` directly with unterminated forms, which is why they appear without
`>`.

### 1.8 `HandleCid` — cidchar and cidrange

`cpdf_cmapparser.cpp:82-110`. Accumulates code points into a fixed
`[u32; 4]` buffer indexed by `code_seq_`:

```
code_points_[code_seq_] = GetCode(word);  code_seq_ += 1
required = (status == ProcessingCidChar) ? 2 : 3
if code_seq_ < required { return }
if cidchar:  start = cp[0];  end = start;   cid = cp[1] as u16
if cidrange: start = cp[0];  end = cp[1];   cid = cp[2] as u16
if end < 65536 { SetDirectCharcodeToCIDTableRange(start, end, cid) }
else           { additional.push(CIDRange{start, end, cid}) }
code_seq_ = 0
```

Damage-tolerance facts:

- `code_seq_` can reach 3 for cidchar? No — it resets at 2. But the buffer is
  `[u32;4]` and `code_seq_` is a plain `int` that is only reset by `begin*` and
  by reaching `required`. A stream that enters `ProcessingCidRange` and supplies
  exactly 3 codes always resets. **However**, `begincodespacerange` sets
  `code_seq_ = 0` and `HandleCodeSpaceRange` increments it without bound
  (`:125`) — but that path never indexes `code_points_`. And `begincidchar`
  resets to 0. So the index is bounded by 3 in the CID paths. Our port keeps a
  `SmallVec`/array with an explicit bound anyway and drops on overflow with a
  diagnostic — see §2 D3.
- `cid` is a **truncating** `u32 → u16` cast. `begincidchar <00> <10000>`
  yields CID 0.
- `SetDirectCharcodeToCIDTableRange` (`cpdf_cmap.cpp:514-521`) writes
  `table[code] = (start_cid + code - start_code) as u16` for
  `code in start..=end`, **with no bounds check on `start_code`**. The C++ is
  safe only because `HandleCid` gated on `end < 65536`, and the table is exactly
  65536 entries. But `start_code > end_code` is possible (`begincidrange <10>
  <05> <0041>`): the C++ `for (code = start; code <= end; ++code)` simply does
  not execute. And `start_code` can be ≥ 65536 while `end_code < 65536` only if
  `start > end`, so the loop is empty. Our port asserts `start <= end` and skips
  otherwise, matching.
- The write is **last-wins**: a later range overwriting an earlier one keeps the
  later CID. There is no "first wins" or "min" here (unlike ToUnicode, font
  brief §1.6).
- `kDirectMapTableSize = 65536` (`cpdf_cmap.h:38`), allocated **zeroed** in the
  embedded constructor (`cpdf_cmap.cpp:304-305`). CID 0 is therefore the
  "unmapped" value, indistinguishable from an explicit mapping to CID 0.

### 1.9 `HandleCodeSpaceRange` — codespace ranges and the scheme decision

`cpdf_cmapparser.cpp:112-143`.

While the word is not `"endcodespacerange"`:
- if the word is empty or does not start with `<`, **return without incrementing
  `code_seq_`** — a stray non-hex token is skipped and does not break pairing.
- if `code_seq_` is **odd**, call `GetCodeRange(last_word_, word)` and push any
  `Some` result to `pending_ranges_`.
- `code_seq_ += 1`.

So ranges are formed from `(previous_word, current_word)` at odd positions,
i.e. the pairs (word0,word1), (word2,word3), …. `last_word_` is set by
`ParseWord` after every dispatch, so it really is the immediately preceding
token — **including tokens that were skipped for not starting with `<`**, since
`last_word_ = word` runs unconditionally at `:79`. That means
`<00> garbage <ff>` produces the pair `("garbage", "<ff>")` at `code_seq_`=1,
which `GetCodeRange` rejects (first doesn't start with `<`), and then `<ff>`
becomes `last_word_` for the next odd slot. Reproduce.

On `"endcodespacerange"` (`:129-142`):

```
nSegs = ranges_.len() + pending_ranges_.len()
if nSegs == 1 {
    first = ranges_.get(0).or(pending_ranges_.get(0))
    SetCodingScheme(first.char_size == 2 ? TwoBytes : OneByte)
}                                       // pending_ranges_ is NOT drained here
else if nSegs > 1 {
    SetCodingScheme(MixedFourBytes)
    ranges_.extend(pending_ranges_.drain(..))
}
status = Start
```

Three quirks that matter:

1. **When `nSegs == 1`, `pending_ranges_` is not moved into `ranges_`.** So a
   CMap with exactly one codespace range ends with `ranges_` empty. In the
   destructor (`:34-37`) `SetMixedFourByteLeadingRanges(ranges_)` is called with
   the empty vector, and `SetAdditionalMappings` (`cpdf_cmap.cpp:496-508`)
   returns early because the scheme is not `MixedFourBytes`. The single range's
   *bounds* are therefore discarded — only its `char_size` survives, as
   OneByte/TwoBytes. A `<00> <7F>` 1-byte codespace decodes byte 0xFF just as
   happily as 0x00.
2. `char_size == 2 → TwoBytes`, **anything else → OneByte**. So a lone 3-byte or
   4-byte codespace range yields a *one-byte* decoder.
3. `nSegs > 1` always yields `MixedFourBytes`, even if every range is 1 byte
   wide. Two `<00><1F>`/`<20><7F>` ranges give a four-byte-capable decoder that
   in practice reads one byte at a time via the `CheckFourByteCodeRange` walk.
4. **Repeated `begincodespacerange`/`endcodespacerange` blocks accumulate**:
   `ranges_` persists across blocks, `pending_ranges_` is drained only in the
   `nSegs > 1` branch. A second block therefore sees `nSegs` including the
   first block's surviving `ranges_`, and the scheme is re-decided each time,
   last-block-wins.

**`GetCodeRange(first, second)`** (`cpdf_cmapparser.cpp:172-210`), `static`,
unit-tested:

```
if first is empty or first[0] != '<' -> None
scan i from 1 until first[i] == '>' or end of string
char_size = (i - 1) / 2
if char_size > 4 -> None
lower[j] = hex(first[2j+1])*16 + hex(first[2j+2])   for j in 0..char_size
upper[j] = hex(second[2j+1])*16 + hex(second[2j+2]) for j in 0..char_size,
           substituting '0' for any index past second's end
```

Notes:
- The `>` scan does **not** require finding one: an unterminated `<a1` gives
  `i = len`, `char_size = (len-1)/2`.
- The lower-bound digit reads index into `first` **without bounds checks in
  C++** (`first[i*2+1]`, `first[i*2+2]`); they are safe only because
  `char_size` was derived from `i`. But `i` may equal `first.len()` when no `>`
  was found, making `char_size = (len-1)/2` and the last read `first[len-1]` or
  `first[len]` — the latter reads the NUL terminator of the underlying
  `ByteStringView`'s buffer in practice. **Our port must clamp and substitute
  `0`**, and record a `Diagnostic`. See §2 D2.
- `FXSYS_HexCharToInt` on a non-hex byte returns 0 in PDFium
  (`fx_extension.h`), so `<zz>` yields lower = 0, not a rejection.
- The upper bound pads with `'0'` (the *character*, hex value 0) past the end of
  `second`, hence the pinned `GetCodeRange("<a1>", "")` ⇒ `upper[0] == 0`.

Pinned assertions (`cpdf_cmapparser_unittest.cpp:39-75`):

| first | second | result |
|---|---|---|
| `""` | `""` | None |
| `"A"` | `""` | None |
| `"<aaaaaaaaaa>"` | `""` | None (char_size 5 > 4) |
| `"<12345678>"` | `"<87654321>"` | char_size 4, lower `[18,52,86,120]`, upper `[135,101,67,33]` |
| `"<a1>"` | `"<F3>"` | char_size 1, lower `[161]`, upper `[243]` |
| `"<a1>"` | `""` | char_size 1, lower `[161]`, upper `[0]` |

### 1.10 `CIDFromCharCode` — the runtime CID lookup, all four cases

`cpdf_cmap.cpp:319-344`, in order:

1. `coding_ == kCID` ⇒ `charcode as u16`. (Identity-H/V, and any embedded CMap
   whose `/Ordering` never set a charset — no: `coding_` is only ever `kCID` via
   the Identity short-circuit. Embedded CMaps leave `coding_` at `kUNKNOWN`.)
2. `embed_map_` present ⇒ `fxcmap::CIDFromCharCode` (§1.4).
3. **`direct_charcode_to_cidtable_` empty ⇒ `charcode as u16`.** This is the
   fallback for an *unrecognized predefined name* (which has no table and no
   embed map) — it behaves as Identity. Also for a predefined name that matched
   a row but whose embed map lookup failed (§1.1).
4. Otherwise (embedded CMap): `charcode < 65536` ⇒ `table[charcode]`. Else
   `lower_bound` over `additional_charcode_to_cidmappings_` on `end_code_`; miss
   or `it->start_code_ > charcode` ⇒ **0**; else
   `it->start_cid_ + charcode - it->start_code_`.

`SetAdditionalMappings` (`:496-508`) sorts by `end_code_` **and returns without
storing anything unless `coding_scheme_ == MixedFourBytes`**. So an embedded
CMap declaring a 2-byte codespace but containing a `begincidrange` with codes
≥ 65536 loses those mappings entirely. Preserve.

### 1.11 `GetNextChar` — the byte-decoder state machine

`cpdf_cmap.cpp:346-391`. Takes a byte string and a mutable offset; returns the
next charcode and advances. **Out-of-range reads yield 0, not termination** —
this is the key damage-tolerance property.

- **OneByte**: `offset < len ? bytes[offset++] : 0`.
- **TwoBytes**: read two bytes with the same guard each,
  `256*b1 + b2`. At end of string with one byte left, `b2 = 0` and the offset is
  **not** advanced past the end (the guard fails), so the loop caller sees the
  offset unchanged for the second byte — but the first read did advance. Net:
  the odd trailing byte produces a code `b1 * 256`, then the caller's
  `offset < len` test fails and iteration stops. Matches `CountChar`'s
  `(len + 1) / 2`.
- **MixedTwoBytes**: read `b1`; if `!leading_bytes[b1]` return `b1` (1-byte
  code); else read `b2` and return `256*b1 + b2`. **`leading_bytes` is indexed
  by the raw byte with no bounds concern (always 256 entries)** — but note that
  for an *embedded* CMap the scheme can never be `MixedTwoBytes` (the parser
  only ever sets OneByte/TwoBytes/MixedFourBytes), so the vector is always the
  one built by `LoadLeadingSegments`. For a **predefined name that failed
  lookup**, the scheme is the default `TwoBytes`, so this arm is unreachable
  with an empty vector. Our port encodes this as a type invariant (§3).
- **MixedFourBytes** (`:366-388`): the interesting one.

```
codes[0] = next byte (or 0 at EOF); char_size = 1
loop {
    ret = CheckFourByteCodeRange(&codes[..char_size], ranges)
    if ret == 0 { return 0 }                       // no range matches at all
    if ret == 2 { return big-endian(codes[..char_size]) }
    if char_size == 4 || offset == len { return 0 }
    codes[char_size] = bytes[offset]; offset += 1; char_size += 1
}
```

**`CheckFourByteCodeRange(codes, ranges)`** (`cpdf_cmap.cpp:201-224`) walks the
range list **backwards** (last declared first) and returns 0/1/2:

```
for range in ranges.iter().rev() {
    if range.char_size < codes.len() { continue }
    iChar = count of leading positions j where lower[j] <= codes[j] <= upper[j]
    if iChar == range.char_size { return 2 }                      // full match
    if iChar != 0 { return codes.len() == range.char_size ? 2 : 1 }
}
return 0
```

Digest of the semantics: **2 = "this is a complete code", 1 = "prefix matches,
need more bytes", 0 = "nothing matches, give up"**. Note the second `return 2`
arm: a *partial* prefix match (`iChar < range.char_size`) where the input length
already equals `range.char_size` is also reported as a complete code. That
happens when e.g. `codes = [0x81, 0xFF]` against range `char_size=2,
lower=[0x81,0x40], upper=[0x9F,0xFC]`: `iChar` stops at 1 (0xFF > 0xFC), and
since `codes.len() == 2 == range.char_size`, it returns 2 — the out-of-range
second byte is accepted. This is a deliberate tolerance and must be ported.

Also note the **reverse iteration**: later-declared codespace ranges shadow
earlier ones. And `char_size` here is the *matching range's* size, compared
against the accumulated input length.

- **The `offset == len` guard returns 0 without consuming**, so a truncated
  multi-byte code at end of string yields charcode 0 and leaves the offset at
  `len`, terminating iteration.
- If `ranges` is empty, `CheckFourByteCodeRange` returns 0 immediately ⇒ every
  charcode is 0 and **the offset still advanced by 1**, so iteration terminates
  after consuming the whole string one byte at a time producing all zeros. This
  is reachable (embedded CMap with `nSegs > 1` at one `endcodespacerange` but
  where `ranges_` was populated... actually `nSegs > 1` always fills `ranges_`;
  the empty case needs a `MixedFourBytes` scheme with no ranges, which requires
  a second `endcodespacerange` block with `nSegs == 1` after a first with
  `nSegs > 1` — the scheme flips to OneByte then. So empty-ranges-with-
  MixedFourBytes is unreachable through the parser but *is* reachable through
  the public setters. Guard it.)

### 1.12 `GetCharSize`, `CountChar`, `AppendChar`

**`GetCharSize(charcode)`** (`cpdf_cmap.cpp:393-417`) — width from the *value*,
not from the byte stream:

| scheme | rule |
|---|---|
| OneByte | 1 |
| TwoBytes | 2 |
| MixedTwoBytes | `charcode < 0x100 ? 1 : 2` |
| MixedFourBytes | `<0x100 ? 1 : <0x10000 ? 2 : <0x1000000 ? 3 : 4` |

**`CountChar(s)`** (`:419-446`):

| scheme | rule |
|---|---|
| OneByte | `s.len()` |
| TwoBytes | `(s.len() + 1) / 2` |
| MixedTwoBytes | linear scan: `count += 1`, and `i += 1` extra if `leading[s[i]]` |
| MixedFourBytes | run `GetNextChar` to exhaustion, counting iterations |

Note the MixedTwoBytes scan can skip past the end (`i++` inside the loop with
`i < len` re-checked), so a trailing leading byte counts as one char. Consistent
with `GetNextChar`.

**`AppendChar(str, charcode)`** (`:448-494`) — the inverse encoder, used by the
text/search paths to rebuild a byte string:

| scheme | rule |
|---|---|
| OneByte | one byte `charcode as u8` (truncating) |
| TwoBytes | `charcode / 256`, `charcode % 256` (both truncating u8 casts) |
| MixedTwoBytes | if `charcode < 0x100 && !leading[charcode]` ⇒ one byte; else `charcode>>8`, `charcode` |
| MixedFourBytes | `<0x100`: pad with `GetFourByteCharSizeImpl(charcode, ranges) - 1` zero bytes then the byte; `<0x10000`: 2 BE bytes; `<0x1000000`: 3; else 4 |

**`GetFourByteCharSizeImpl`** (`:226-260`) is a separate, subtly different range
walk used only by the `AppendChar` `< 0x100` padding case:

```
if ranges.is_empty() { return 1 }
codes = [0, 0, (charcode >> 8) as u8, charcode as u8]
for offset in 0..4 {
    size = 4 - offset
    for iSeg in (0..ranges.len()).rev() {
        if ranges[iSeg].char_size < size { continue }
        iChar = count of leading j<size with lower[j] <= codes[offset+j] <= upper[j]
        if iChar == ranges[iSeg].char_size { return size }
    }
}
return 1
```

Note it only accepts the **full** match (no `iChar != 0` partial arm), and it
starts from the widest size. For `charcode < 0x100` the first two `codes` bytes
are always 0 and the third is 0 too, so this asks "is there a 4/3/2/1-byte
codespace range whose lower/upper contain `[0,0,0,c]` truncated to that size".

### 1.13 CID → Unicode

`CPDF_CID2UnicodeMap` (`cpdf_cid2unicodemap.cpp`) is trivially thin:

- `IsLoaded()` = the registry's static table is non-empty. Only the four CJK
  registries have one; `kUnknown` and `kUnicode` do not.
- `UnicodeFromCID(cid)`: if `charset == kUnicode` ⇒ **return `cid` as-is**
  (identity). Else `cid < table.len() ? table[cid] : 0`.

Index 0 of every table is `0xFFFD` (REPLACEMENT CHARACTER), so CID 0 maps to
U+FFFD, not to 0. This shows up in text extraction output and is Tier-A
observable.

### 1.14 The global registry (an anti-pattern we erase)

`CPDF_FontGlobals` (`cpdf_fontglobals.cpp`) is a process-wide singleton holding:
the four static charset arrays, the four CID2Unicode arrays, a
`ByteString -> RetainPtr<CPDF_CMap>` memo of predefined CMaps, and a per-document
stock-font map. `LoadEmbeddedMaps()` wires the four registries.

The only *behavioral* content here is the **memoization**: `GetPredefinedCMap`
(`:114-127`) caches by exact name string, and caches even a failed load (the
`if (!name.IsEmpty())` guard means only the empty name is not cached). Since our
predefined CMaps are pure functions of a name over a static blob, there is
nothing to memoize expensively; a small per-`Document` cache is optional and
non-observable. Everything else (`Create`/`Destroy`/`GetInstance`) is C++
lifetime scaffolding, deleted per STYLE.md §1.

---

## 2. Divergences

**D1 — `usecmap` for embedded CMaps: we implement it, PDFium does not.**
SPEC §6 promises "an embedded-CMap parser (CID ranges, usecmap)". PDFium's
parser treats `usecmap` as a no-op (§1.6). Implementing it would change
observable CID output on any file that relies on it, which is a Tier-A
(text-extraction) regression risk against the oracle.

*Decision:* **match PDFium — `usecmap` in an embedded CMap is parsed as a token
and ignored**, and the parser records a `Diagnostic` (`DiagKind::CMapUsecmapIgnored`)
naming the operand so the behavior is visible rather than silent. The
`predefined()` path's `use_offset_` chaining (§1.4) is the *static* usecmap and
**is** implemented — that is what SPEC §6's "usecmap" refers to in practice.
This is a `[spec]`-adjacent reading of the contract, not a contract change; the
brief records it here and SPEC §6's wording is satisfied by the static chain. If
the orchestrator disagrees, the alternative is a feature-flagged second
resolution mode, defaulted off. **Escalated as OQ-1.**

**D2 — bounds on `GetCodeRange`'s digit reads.** The C++ reads
`first[i*2+1]` / `first[i*2+2]` past the token's logical end when no `>` was
found (§1.9). We clamp: any index ≥ `first.len()` contributes hex value 0, and
we emit `DiagKind::CMapTruncatedCodespace`. In C++ the read lands on the
`ByteString`'s NUL terminator (also hex value 0 via `FXSYS_HexCharToInt`), so
**the values agree** — this is a safety divergence with no behavioral one.
`#![forbid(unsafe_code)]` and `clippy::indexing_slicing` make it mandatory.

**D3 — `code_points_` overflow.** `HandleCid`'s `[u32;4]` buffer is provably
bounded in the C++ only by the interaction of `required` and the reset. We use a
`[u32; 4]` with an explicit `if seq >= 4 { seq = 0; diag; return }` guard. Not
reachable through the C++'s own control flow, so no behavioral divergence; it
exists so a fuzzer cannot find one.

**D4 — reverse lookup (`CharCodeFromCID` / charcode-from-unicode) is
non-Windows-only in the C++.** `CPDF_CIDFont::CharCodeFromUnicode`'s tail is
`#if BUILDFLAG(IS_WIN)` / `#else` (`cpdf_cidfont.cpp:397-414`). The oracle is
built on Linux, so **the non-Windows branch is the normative one**: we implement
`EmbeddedCharcodeFromUnicode` (the O(table) scan calling `CharCodeFromCID`) and
never the `FX_WideCharToMultiByte` codepage path. This crate exports
`CMap::charcode_from_cid` and `registry_charcode_from_unicode`; `pdfrum-font`
composes them. Same for `EmbeddedUnicodeFromCharcode`. The Windows codepage
conversion tables are **out of scope permanently** — recorded here so nobody
"completes" the port later.

**D5 — no global registry.** The four static registries live in the generated
blob, addressed by `CidSet`. `predefined(name)` is a free function over that
blob. No `FontGlobals`, no `Create`/`Destroy`, no process state (STYLE.md §1).
Non-observable.

**D6 — `CIDRange`'s `end_code` vs the direct table.** PDFium keeps a 128 KiB
`Vec<u16>` per embedded CMap regardless of how sparse it is. For a CMap with a
single `begincidchar` entry that is 128 KiB of zeros. We keep the same dense
table (it is what makes `CIDFromCharCode` O(1) and the memory is per-font, not
per-page), but allocate it **lazily on first `SetDirectCharcodeToCIDTableRange`**
so an embedded CMap that only ever populates `additional` (all codes ≥ 65536, or
none at all) costs nothing. `IsDirectCharcodeToCIDTableIsEmpty()`'s semantics
(consulted by `CPDF_CIDFont::GlyphFromCharCode`, font brief §1.8) are preserved
exactly: "no direct table was ever allocated" ≡ "the C++'s `FixedSizeDataVector`
was default-constructed", which happens only for the **predefined** constructor.
So: predefined ⇒ always empty; embedded ⇒ always non-empty in the C++ (the
constructor zeroes it unconditionally). **Our lazy allocation would change that
predicate for an embedded CMap with no cidchar/cidrange at all.** To avoid it we
allocate on entry to the embedded parse (matching the C++), and D6 reduces to:
no divergence, dense table always. *Rejected — keep it simple and identical.*

**D7 — blob format.** The generated blob is our own layout (§3.3), not a
byte-image of the C++ structs. Non-observable; the *values* are byte-identical
and `build.rs` asserts the record counts against the numbers in §1.3's tables.

**D8 — `Diagnostics` everywhere.** Every recovery the C++ performs silently
(unknown predefined name, failed embed-map lookup, dropped codespace range,
truncated multi-byte code, discarded `additional` mappings, ignored `usecmap`)
emits a `Diagnostic`. Per STYLE.md §3 this is a first-class channel, not an
error. No behavior change.

---

## 3. Module plan

```
crates/pdfrum-cmap/
  build.rs              # C++ table -> blob converter (§3.3)
  tables/               # committed generated blob + manifest (§3.3)
    cmaps.bin
    cmaps.manifest.json
  src/
    lib.rs              # public surface (§3.1)
    error.rs            # Error enum
    ids.rs              # CidSet, CidCoding, CodingScheme, Cid, CharCode newtypes
    predefined.rs       # kPredefinedCMaps table + name lookup + blob accessors
    blob.rs             # zero-copy reader over include_bytes!(cmaps.bin)
    static_lookup.rs    # cid_from_charcode / charcode_from_cid over the blob
    decode.rs           # GetNextChar / CountChar / GetCharSize / AppendChar
    embedded/
      mod.rs            # CMap construction from a stream
      lexer.rs          # CPDF_SimpleParser port (shared with pdfrum-font)
      parser.rs         # ParseWord state machine, GetCode, GetCodeRange
    cid2unicode.rs      # registry CID -> char
```

### 3.1 `lib.rs` — public surface

```rust
#![forbid(unsafe_code)]

pub use error::Error;
pub use ids::{CharCode, Cid, CidCoding, CidSet, CodingScheme};

/// A CMap: a byte-decoder plus a charcode→CID map. ISO 32000-1 §9.7.5.
#[derive(Debug, Clone)]
pub struct CMap { /* private; see §3.2 */ }

/// Look up one of the 33 predefined CJK CMap names (plus Identity-H/V).
/// Returns `None` only for a name that matches no row *and* is not Identity —
/// note the caller is still expected to build a fallback CMap in that case
/// (`CMap::unrecognized`), because PDFium does (§1.1).
#[must_use]
pub fn predefined(name: &Name) -> Option<CMap>;

/// The total behavior of a `/Encoding` name, failures included: this is what
/// `pdfrum-font` calls. Never fails; records a diagnostic on an unknown name.
#[must_use]
pub fn from_encoding_name(name: &Name, diags: &mut Diagnostics) -> CMap;

/// Parse an embedded CMap program (a `/Encoding` stream's decoded bytes).
/// Infallible by design (PDFium's parser cannot fail); damage goes to `diags`.
#[must_use]
pub fn parse_embedded(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> CMap;

impl CMap {
    /// SPEC §6's decode entry point. Yields `(charcode, cid)` pairs.
    pub fn decode<'a>(&'a self, bytes: &'a [u8]) -> impl Iterator<Item = (CharCode, Cid)> + 'a;

    /// One step of the decoder; `offset` advances. Returns charcode 0 on damage
    /// without failing (§1.11).
    pub fn next_char(&self, bytes: &[u8], offset: &mut usize) -> CharCode;

    #[must_use] pub fn cid(&self, code: CharCode) -> Cid;             // §1.10
    #[must_use] pub fn charcode_from_cid(&self, cid: Cid) -> CharCode; // §1.4, static only
    #[must_use] pub fn char_size(&self, code: CharCode) -> u8;        // §1.12
    #[must_use] pub fn count_chars(&self, bytes: &[u8]) -> usize;     // §1.12
    pub fn append_char(&self, out: &mut Vec<u8>, code: CharCode);     // §1.12

    #[must_use] pub fn is_vertical(&self) -> bool;
    #[must_use] pub fn is_loaded(&self) -> bool;
    #[must_use] pub fn coding(&self) -> CidCoding;
    #[must_use] pub fn charset(&self) -> CidSet;
    #[must_use] pub fn coding_scheme(&self) -> CodingScheme;
    /// True iff this CMap has no dense charcode→CID table — i.e. it came from
    /// the predefined path. Consumed by `pdfrum-font`'s CID glyph ladder.
    #[must_use] pub fn has_no_direct_table(&self) -> bool;
    /// Present only for a predefined CMap that resolved to a static table.
    #[must_use] pub fn has_static_map(&self) -> bool;
}

/// Registry CID → Unicode over the four static tables. `CidSet::Unicode` is the
/// identity; `Unknown` always yields `None` (§1.13).
#[must_use]
pub fn unicode_from_cid(set: CidSet, cid: Cid) -> Option<char>;
/// True iff the registry has a static CID→Unicode table (§1.13 `IsLoaded`).
#[must_use]
pub fn has_cid2unicode(set: CidSet) -> bool;
/// The O(table) reverse scan (§1.4 / D4). Used only by CID fonts.
#[must_use]
pub fn charcode_from_unicode(cmap: &CMap, unicode: char) -> CharCode;

/// The PostScript-ish word lexer shared with `pdfrum-font`'s ToUnicode parser.
/// Yields byte-slice tokens; ends on the first empty token (§1.5).
pub mod lexer {
    pub struct Words<'a> { /* … */ }
    impl<'a> Words<'a> { pub fn new(bytes: &'a [u8]) -> Self; }
    impl<'a> Iterator for Words<'a> { type Item = &'a [u8]; }
}

/// Charset from a `/CIDSystemInfo /Ordering` value (§1.2).
#[must_use]
pub fn charset_from_ordering(ordering: &[u8]) -> CidSet;
```

`CMap` is `Send + Sync + Clone` (it is either a blob index or owned `Vec`s).
Cloning a predefined CMap is trivial (an index); cloning an embedded one copies
the 128 KiB table — callers hold it in an `Arc` inside `Font`, so this is rare.

### 3.2 `CMap`'s internals — data, not an object

Per STYLE.md §1 the decoder and the map are two records, and `CMap` is a small
record combining them:

```rust
struct CMap {
    decoder: Decoder,
    map: CidMap,
    vertical: bool,
    loaded: bool,
    charset: CidSet,
    coding: CidCoding,
}

/// The byte-splitting rule. Each variant carries exactly what it needs, so the
/// impossible states of the C++ (MixedTwoBytes with an empty leading vector) do
/// not exist.
enum Decoder {
    OneByte,
    TwoBytes,
    MixedTwoBytes { leading: Box<[bool; 256]> },
    MixedFourBytes { ranges: Vec<CodeRange> },   // may be empty; guarded
}

/// Where CIDs come from.
enum CidMap {
    /// `charcode as u16` — Identity, and the unrecognized-name fallback (§1.10).
    Identity,
    /// A chain root in the static blob.
    Static { registry: CidSet, index: u16 },
    /// An embedded CMap's tables.
    Embedded { direct: Box<[u16; 65536]>, additional: Vec<CidRange> },
}

struct CodeRange { char_size: u8, lower: [u8; 4], upper: [u8; 4] }
struct CidRange  { start_code: u32, end_code: u32, start_cid: u16 }
```

`CidMap::Identity` collapses C++ cases 1 and 3 of §1.10, which are behaviorally
identical. `has_no_direct_table()` is `!matches!(self.map, CidMap::Embedded{..})`.

The parser is a separate record that produces a `CMap`, not a mutator of one
(the C++'s `CPDF_CMapParser` holding an `UnownedPtr<CPDF_CMap>` and finishing its
work **in its destructor** is exactly the anti-pattern STYLE.md §1 forbids):

```rust
struct CMapBuilder {
    status: Status,
    code_seq: u32,
    code_points: [u32; 4],
    ranges: Vec<CodeRange>,
    pending_ranges: Vec<CodeRange>,
    additional: Vec<CidRange>,
    direct: Box<[u16; 65536]>,
    last_word: Vec<u8>,        // owned; the C++'s ByteString last_word_
    scheme: CodingScheme,
    charset: CidSet,
    vertical: bool,
}
enum Status { Start, CidChar, CidRange, Registry, Ordering, Supplement, WMode, CodeSpaceRange }
```

with `fn parse_embedded(bytes) -> CMap` doing `builder.feed(word)` per token and
`builder.finish()` applying the destructor's two calls in order:
`set_additional_mappings` (sort by `end_code`, drop unless `MixedFourBytes`)
then `set_mixed_four_byte_ranges`.

`last_word` must be **owned**, not a borrow: the C++ stores a `ByteString` copy
(`cpdf_cmapparser.h:57`), and `GetCodeRange(last_word_, word)` therefore sees a
stable value. A borrowed `&[u8]` into the input would work too since the input
outlives the parse — use `&'a [u8]` and keep the lifetime; simpler and matches.

### 3.3 `build.rs` and the blob

**Inputs.** The path to the oracle checkout, supplied by the environment
variable `PDFRUM_ORACLE` (default `/mnt/data2/pdfium/pdfium-c++`). The generated
blob and manifest are **committed** under `tables/`, and `build.rs` regenerates
only when `PDFRUM_REGEN_CMAP_TABLES=1` is set. This keeps `cargo build` hermetic
and offline for everyone who does not have the oracle checked out, while making
regeneration a one-command, reviewable diff. (An alternative — parse at build
time always — makes the crate unbuildable without the C++ tree, which is
unacceptable for a crate we intend to publish.)

**The converter.** A small hand-written scanner in `build.rs`, ~300 lines, no
dependencies (the dependency set is closed, DEPS.md):

1. For each registry `R in {CNS1, GB1, Japan1, Korea1}`:
   a. Parse `R/cmaps_<r>.inc` with a regex-free bracket scanner into the index
      rows: `(name, word_symbol, dword_symbol|None, word_count, dword_count,
      type, use_offset)`. The literal spellings to accept are exactly
      `CMap::Type::kSingle` / `CMap::Type::kRange` and `nullptr`.
   b. For each referenced symbol, find the `.cpp` declaring
      `const uint16_t <sym>[N * K] = { ... };` or
      `const DWordCIDMap <sym>[N] = { ... };`, and parse the `0x....`
      comma-separated literals.
   c. **Assert** `values.len() == word_count * (2 if kSingle else 3)` and
      `dword_values.len() == dword_count * 4`, cross-checked against the
      declared `[N * K]` dimension. Any mismatch fails the build loudly with the
      symbol name.
   d. **Assert sortedness**: kSingle by `code`, kRange by `high`, dword by
      `(hi_word, lo_word_high)` — the keys `lower_bound` uses. A violation fails
      the build and is an escalation, not an auto-fix (§1.3).
   e. **Assert** every chain link reachable via `use_offset` is in-bounds and
      terminates (no cycles), and that every link has a non-null word array
      (the C++'s `CHECK`).
2. Parse `R/Adobe-*-UCS2_*.inc` for the single `inline constexpr uint16_t
   <sym>[] = {...}` array. Assert the lengths in §1.3's table.
3. Deduplicate word/dword arrays **by symbol name** (not by content) so the
   UTF16/UCS2 aliases share one copy, as they do in C++.
4. Emit `tables/cmaps.bin`, little-endian, and a JSON manifest recording every
   symbol, its record count, type, and offset — the manifest is the human-
   reviewable artifact in code review and the input to the runtime blob reader's
   consistency check.

**Blob layout** (all integers little-endian, all offsets in bytes from the start
of the blob):

```
Header (32 bytes)
  u32  magic         = 0x504D4331   ("PMC1")
  u16  version       = 1
  u16  registry_count= 4
  u32  index_off     # -> IndexTable
  u32  words_off     # -> Words section
  u32  dwords_off    # -> DWords section
  u32  cid2uni_off   # -> Cid2Unicode section
  u32  total_len
  u32  reserved      = 0

IndexTable
  [4] RegistryHeader { u32 entry_off; u16 entry_count; u16 pad; }
      # indexed by CidSet ordinal - 1 (GB1=1 .. Korea1=4)
  then, per registry, `entry_count` × Entry (16 bytes):
      u32 name_off        # -> Names section, u8 length prefix + bytes
      u32 word_off        # byte offset into Words section
      u32 dword_off       # 0xFFFF_FFFF when absent
      u16 word_count      # records
      u16 dword_count     # records
      u8  word_type       # 0 = Single, 1 = Range
      i8  use_offset      # signed entry-index delta, 0 = end of chain
      u16 pad

Words     : packed u16 records (2 or 3 u16 each, per the entry's word_type)
DWords    : packed u16 records, 4 u16 each
Cid2Unicode:
  [4] { u32 off; u32 len; }    # indexed as above
  then the four u16 arrays
Names     : u8 length + ASCII bytes, no NUL
```

The runtime reader (`blob.rs`) does `include_bytes!("../tables/cmaps.bin")` and
exposes `&[u16]` slices via `chunks_exact` + `u16::from_le_bytes` — **no
transmute, no unsafe**. The blob is 8-byte aligned by construction and every
accessor is a checked `get()`; a malformed blob is a build-time impossibility
but the reader still returns `Option` and a `debug_assert!`-free error path so
`clippy::indexing_slicing` is satisfied.

**Size check.** ~615 KiB of table data (§1.3) plus ~2 KiB of index. That is
acceptable for a crate that exists to hold exactly these tables; the alternative
(fetching them at runtime) is not on the table. If the blob ever needs to
shrink, the two candidates are (a) delta-encoding the Range tables' `low`/`cid`
columns and (b) dropping the four aliased-name duplicate index entries. Neither
is worth doing at M2; recorded here so the question does not get re-litigated.

### 3.4 Data flow

```
/Encoding name  ──► from_encoding_name ──► predefined() ──► blob index
                                       └─► CMap::unrecognized (Identity+TwoBytes)

/Encoding stream ─► pdfrum-filters ─► bytes ─► lexer::Words ─► CMapBuilder
                                                            ─► CMap{Embedded}

CMap + bytes ─► CMap::decode ─► (CharCode, Cid)* ─► pdfrum-font::Font::decode
Cid + CidSet ─► unicode_from_cid ─► char        ─► pdfrum-text
```

Nothing in this crate touches `Resolve`, streams, or fonts. `parse_embedded`
takes already-decoded bytes; the `/Encoding`-stream fetch and filter chain
belong to `pdfrum-font` (which owns the `Resolve`).

---

## 4. Test plan

### 4.1 Ported directly from `cpdf_cmapparser_unittest.cpp`

`GetCode` — all 11 assertions of §1.7's table, as one table-driven test over
`parser::get_code`.

`GetCodeRange` — all 6 assertions of §1.9's table, including the two `None`
cases and the zero-padded upper bound.

### 4.2 Static-table tests (ours to write, oracle-derived)

- **Blob integrity** (a build-time assertion re-checked at test time): for every
  registry, entry count matches §1.3's tables; every `word_count`/`dword_count`
  matches; every `use_offset` chain terminates within ≤ 8 links; every name is
  unique within its registry.
- **Alias identity**: `UniGB-UTF16-H`'s word slice is byte-identical to
  `UniGB-UCS2-H`'s; likewise the three other aliased pairs (`UniGB-UTF16-V`,
  `UniCNS-UTF16-V`, `UniJIS-UTF16-H/V`, `UniKS-UTF16-V`).
- **Known CID lookups**, taken from the head of `GB-EUC-H_0.cpp`
  (`kGB_EUC_H_0[0..9] = {0x0020,0x0020,0x1E24, 0x0021,0x007E,0x032E,
  0xA1A1,0xA1FE,0x0060}`):
  `cid(0x0020) == 0x1E24`; `cid(0x0021) == 0x032E`; `cid(0x007E) == 0x032E +
  0x7E - 0x21 == 0x038B`; `cid(0xA1A1) == 0x0060`; `cid(0xA1FE) == 0x0060 +
  0x5D == 0x00BD`; `cid(0x0000) == 0` (below the first range's `high`, so
  `lower_bound` finds record 0 and the `>= low` test fails).
- **Chain traversal**: `GB-EUC-V` (`use_offset = -1`, 20 records) returns a
  `GB-EUC-H` CID for a code its own table does not cover, and its *own* CID for
  a code it does. Same for `KSCpc-EUC-H`'s −6 hop.
- **Mixed-type chain**: `UniCNS-UTF16-V` (Range, `-1`) → `UniCNS-UTF16-H`
  (Single). Assert a Single-table hit is reached through a Range-table link.
- **Dword path**: `GBK2K-H` with a charcode ≥ 0x10000 from `kGBK2K_H_5_DWord`'s
  first record; assert a charcode ≥ 0x10000 on `GB-EUC-H` (no dword table)
  returns 0 without falling back to the word table.
- **Reverse lookup**: `charcode_from_cid` round-trips the four spot CIDs above;
  a CID present only in the dword table returns 0 (the documented `TODO`).

### 4.3 Predefined-name resolution (damage tolerance)

Table-driven over `from_encoding_name`:

| input | scheme | coding | charset | vertical | loaded | map |
|---|---|---|---|---|---|---|
| `Identity-H` | TwoBytes | CID | Unknown | false | true | Identity |
| `Identity-V` | TwoBytes | CID | Unknown | **true** | true | Identity |
| `/Identity-H` | as above (leading slash stripped) |
| `GB-EUC-H` | MixedTwoBytes | GB | GB1 | false | true | Static |
| `GB-EUC-V` | MixedTwoBytes | GB | GB1 | **true** | true | Static |
| `UniJIS-UCS2-HW-H` | TwoBytes | UCS2 | Japan1 | false | true | Static |
| `UniJIS-UCS2-HW` | **TwoBytes/UCS2/Japan1** matching `UniJIS-UCS2` (row 23) — the suffix-strip quirk |
| `H` | TwoBytes | JIS | Japan1 | false | true | Static |
| `V` | TwoBytes | JIS | Japan1 | **true** | true | Static |
| `GB-EUC-XY` | MixedTwoBytes | GB | GB1 | false | **false** | Identity (row matched, embed lookup by full name failed) |
| `Nonsense` | **TwoBytes** | Unknown | Unknown | false | false | Identity |
| `NonsenseV` | TwoBytes | Unknown | Unknown | **true** | false | Identity |
| `""` | TwoBytes | Unknown | Unknown | false | false | Identity |
| `"X"` | TwoBytes | Unknown | Unknown | false | false | Identity |

Each row that is not `loaded` must also have produced a `Diagnostic`.

### 4.4 Decoder tests

One table per scheme, asserting `(charcodes, final_offset)` for a byte string:

- OneByte `[41 42 43]` ⇒ `[0x41,0x42,0x43]`.
- TwoBytes `[41 42 43]` ⇒ `[0x4142, 0x4300]`, `count_chars == 2`.
- TwoBytes `[]` ⇒ `[]`.
- MixedTwoBytes with leading `a1..fe`: `[41 A1 A2 FF]` ⇒
  `[0x41, 0xA1A2, 0xFF00]` — note `0xFF` **is** a leading byte (`a1..fe` does
  not include `ff`, so `0xFF` yields the 1-byte code `0xFF`). Re-derive against
  the actual table rather than trusting this line; assert `count_chars`
  agreement with the iteration count in every case.
- MixedTwoBytes truncated: `[A1]` ⇒ `[0xA100]`, offset advanced by 1 only.
- MixedFourBytes with ranges `[<00><80>], [<8140><9FFC>]`: assert `0x41` is a
  1-byte code, `[81 40]` is `0x8140`, `[81 FF]` is `0x81FF` (the `iChar != 0 &&
  len == char_size` tolerance arm, §1.11), and a truncated `[81]` yields 0.
- MixedFourBytes with an *empty* range list ⇒ every byte yields charcode 0 and
  iteration terminates (guard test, §1.11).
- `append_char` round-trips `next_char` for every scheme over the charcodes
  produced above (property test, `proptest`-free — a hand-rolled loop over
  0..=0xFFFF for the two-byte schemes and a sampled set for four-byte).
- `char_size` matches §1.12's table for boundary values 0xFF/0x100,
  0xFFFF/0x10000, 0xFFFFFF/0x1000000.

### 4.5 Embedded-parser tests

Written as small CMap programs, mirroring the shape of real `/Encoding` streams:

- **codespacerange arity.** One range `<00><FF>` ⇒ OneByte; one `<0000><FFFF>`
  ⇒ TwoBytes; one `<000000><FFFFFF>` ⇒ **OneByte** (§1.9 quirk 2); two ranges ⇒
  MixedFourBytes with both stored; one range with `ranges_` left empty (§1.9
  quirk 1) — assert `CodeRange` bounds are *not* consulted by the decoder.
- **Two codespace blocks**: first with 2 ranges (⇒ MixedFourBytes, `ranges_`
  has 2), then one with 1 range ⇒ `nSegs == 3 > 1` ⇒ still MixedFourBytes and
  `ranges_` grows to 3. Pin it.
- **cidchar / cidrange.** `1 begincidchar <20> <100> endcidchar` ⇒
  `cid(0x20) == 0x100`. `begincidrange <20> <7E> <100>` ⇒
  `cid(0x7E) == 0x100 + 0x5E`.
- **Last-wins overwrite**: two overlapping `begincidrange` entries; the later
  one's CID is observed.
- **CID truncation**: `begincidchar <01> <10000>` ⇒ `cid(1) == 0`.
- **Reversed range** `begincidrange <10> <05> <41>` ⇒ nothing written; `cid(5)`
  and `cid(0x10)` both 0.
- **`additional` gating**: a `begincidrange <10000> <10010> <200>` inside a
  **TwoBytes** CMap ⇒ mappings **dropped** (`cid(0x10005) == 0`); inside a
  **MixedFourBytes** CMap ⇒ `cid(0x10005) == 0x205`.
- **`usecmap` is ignored**: `/GBK-EUC-H usecmap` followed by a single cidchar ⇒
  only that one code maps; a GBK code that the base map covers yields 0.
  Diagnostic recorded.
- **Keyword-resets-state**: `begincidchar <20> begincidrange <21><22><1>
  endcidrange` — assert the stray `begincidrange` reset the machine and no
  cidchar entry was written.
- **`/WMode 1`** sets vertical; `/WMode 0` does not; `/WMode` with a missing
  operand followed by `endcmap` sets vertical from `GetCode("endcmap") == 0`
  ⇒ false.
- **`/Ordering (Japan1)`** does **not** set the charset (§1.6's `CMap_GetString`
  quirk) — pin the actual behavior, whatever the conformance run shows; this
  test is written after a one-off oracle probe and its expected value is
  recorded with a comment naming the probe.
- **Lexer edge cases** (own module tests): `%` comment skipping; `<<` returned
  as a 2-byte token; `(a(b)c)` returned whole; an unterminated `(` consuming to
  EOF; a `/Name` at EOF with no separator returning **empty** and thus
  terminating the parse (§1.5).

### 4.6 Snapshot, fuzz and conformance

- **Snapshot (`insta`)**: for each of the 33 predefined names, a compact dump of
  `(scheme, coding, charset, vertical, loaded, first 8 CID lookups over a fixed
  charcode probe set)`. This is the single highest-value regression net for the
  blob generator — a table-conversion bug shows up as a snapshot diff.
- **Fuzz target `cmap_embedded`**: `parse_embedded(data, &Limits::default(),
  &mut diags)` then, on the result, `decode` a second slice of the fuzz input
  and drive `append_char`/`char_size`/`count_chars` over every produced
  charcode. Seeded from `pdfium-c++/testing/fuzzers/` (`pdf_cmap_fuzzer` if
  present) plus every `/Encoding` stream extracted from the corpus. Must run 24 h
  clean before M2 exit (PLAN.md §5).
- **Fuzz target `cmap_lexer`**: the word iterator alone, asserting it always
  terminates and never produces a token extending past the input.
- **Conformance clusters**: `cjk` (every corpus file with a CJK registry),
  `identity-cid`, `embedded-cmap`. Tier-A `--txt` byte-exactness is the metric;
  a mismatch localized to this crate shows up as a wrong CID → wrong Unicode.

---

## 5. Open questions

**OQ-1 (escalated). `usecmap` in embedded CMaps.** SPEC §6 lists "usecmap" as a
feature of the embedded-CMap parser; PDFium implements it as a no-op (§1.6). The
brief proposes matching PDFium (D1) since Tier-A text extraction is measured
against PDFium. Confirm, or direct us to implement real `usecmap` chaining
behind a default-off flag. **Blocking only for the parser's `usecmap` arm; the
rest of the crate is unaffected.**

**OQ-2. Blob regeneration policy.** §3.3 proposes committing `tables/cmaps.bin`
(~615 KiB) plus a JSON manifest, regenerating only under an env var. The
alternative is regenerating on every build from `PDFRUM_ORACLE`. Committing is
recommended (publishable crate, hermetic builds, reviewable diffs) but the
615 KiB binary in git is a real cost. Confirm.

**OQ-3. `/Ordering` parsing.** §1.6 shows `CMap_GetString`'s `[2..]` slice
cannot match a parenthesized PostScript string, which would mean embedded CMaps
effectively never set their own charset and always fall back to
`/CIDSystemInfo /Ordering` at the font level (`cpdf_cidfont.cpp:478-486`). This
is derived by reading, not by running. **Action, not a question:** the
implementing agent must confirm with a one-off oracle probe (a minimal PDF with
an embedded CMap declaring `/Ordering (Japan1)` and no `/CIDSystemInfo` on the
descendant font, checking whether `--txt` produces Japan1 CID→Unicode output)
and record the answer in a code comment plus the §4.5 test. No design depends on
the answer; only one test's expected value does.

**OQ-4. `Limits` for this crate.** The embedded parser has exactly one unbounded
accumulator, `additional_charcode_to_cidmappings_` (and `ranges_` /
`pending_ranges_`, both bounded in practice by input size). PDFium has **no cap**
on any of them. Proposal: add `Limits.max_cmap_ranges` defaulting to 65 536
(one per possible codespace start byte pair is already absurd), exceeding it
records a diagnostic and drops further ranges rather than erroring — a deliberate
divergence in the spirit of the accepted `max_decoded_stream_len` decision
(SPEC §4). Confirm the constant, or confirm "no cap, match the C++".
