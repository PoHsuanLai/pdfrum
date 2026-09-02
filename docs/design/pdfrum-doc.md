# Design brief — `pdfrum-doc`

Behavior source: `pdfium-c++/core/fpdfdoc/` — navigation (`cpdf_bookmark.*`,
`cpdf_bookmarktree.*`, `cpdf_nametree.*`, `cpdf_numbertree.*`, `cpdf_dest.*`,
`cpdf_link.*`, `cpdf_linklist.*`, `cpdf_action.*`, `cpdf_aaction.*`,
`cpdf_filespec.*`), annotations (`cpdf_annot.*`, `cpdf_annotlist.*`,
`cpdf_generateap.*`, `cpvt_*`, `cpdf_defaultappearance.*`, `cpdf_color_utils.*`,
`cpdf_apsettings.*`, `cpdf_iconfit.*`, `cpdf_icon.*`), forms
(`cpdf_interactiveform.*`, `cpdf_formfield.*`, `cpdf_formcontrol.*`), and
structure/metadata (`cpdf_structtree.*`, `cpdf_structelement.*`,
`cpdf_pagelabel.*`, `cpdf_viewerpreferences.*`, `cpdf_metadata.*`).
Plus the dump-format sources the Tier-A targets are defined by:
`testing/pdfium_test/write.cc` (`--annot`), `testing/pdfium_test/dump.cc`
(`--show-structure`), and the thin API layers those go through
(`fpdfsdk/fpdf_annot.cpp`, `fpdfsdk/fpdf_structtree.cpp`,
`fpdfsdk/fpdf_doc.cpp`). Shape contract: SPEC.md §10 (binding). All C++ paths
relative to `/mnt/data2/pdfium/pdfium-c++/`.

This crate is **Tier-A byte-exact** for `--annot` and `--show-structure`, and
**pixel-load-bearing** for the form/annotation clusters, because appearance
streams this crate generates are what `pdfrum-page` then interprets and
`pdfrum-render` rasterizes. Two consequences drive the whole design:

1. **AP generation mutates the annotation dictionary**, and those mutations are
   visible in the `--annot` golden (a `/Text` annot's `/Rect` becomes a 20×20
   note box; an `/Ink` annot's `/Rect` is inflated by half the border width).
   Section §1.9 pins the ordering; Divergence D1 explains how we get the same
   observables without mutating parsed objects.
2. **The generated content-stream bytes must match byte-for-byte** where the
   oracle's own renderer consumes them. Two *different* float formatters are in
   play (`WriteFloat`/dragonbox vs. C++ `operator<<`/`%g`-with-6-significant-
   digits) and the choice is per-call-site, not per-function. §1.10.1 tabulates
   every site.

"Port the behavior, never the shape" — the C++ classes named here decompose per
§3 of this brief.

---

## 1. Behavior inventory

### 1.0 Scope boundaries (what this crate is and is not)

**Consumed from other crates** (nothing here re-implements them):

| Need | Provider | Note |
|---|---|---|
| `Object`/`Dict`/`Array` accessor matrix | `pdfrum-object` §1.6–1.8 | The resolve-vs-not columns are load-bearing here too |
| `decode_text` (PDFDocEncoding/BOM/`StripLanguageCodes`) | `pdfrum-object` | Every `GetUnicodeTextFor` in fpdfdoc is this function |
| `encode_text` (reverse PDFDocEncoding) | `pdfrum-object` | `PDF_EncodeText` in `CheckControl`, `ExportToFDF` |
| `encode_string_literal` | `pdfrum-object` | `PDF_EncodeString` for `Tj` operands (§1.10.6) |
| `name_decode` | `pdfrum-object` | `PDF_NameDecode` on the `/DA` `Tf` operand |
| `Resolve` / `Document::page_index(ObjRef)` / `page_count` | `pdfrum-parser` §1.17 | Dest page resolution (§1.3) |
| `Document::root()` (catalog dict) | `pdfrum-parser` | Every entry point starts here |
| `decode_chain` (stream bytes) | `pdfrum-filters` | `/Metadata` XMP, embedded file streams |
| Page `/Annots`, `/Resources`, `MediaBox`-derived width/height, rotation | `pdfrum-page` §1.7 | `/Annots` is **not** inherited — plain `Dict::array(b"Annots")` on the page dict (`cpdf_page.cpp:242`) |
| `Font::load`, stock fonts, ascent/descent, char widths, charcode↔unicode | `pdfrum-font` | Interface gap — see §1.11.1 and OQ-3 |
| `parse_content` + `build_page` (to *count* AP objects for `--annot`) | `pdfrum-page` | `FPDFAnnot_GetObjectCount` parses the AP form (§1.12.2) |
| `kurbo::Rect`/`Affine` | `pdfrum-common` | With the C++ rect semantics of §1.0.1 |

**Deliberately out of scope** (PLAN.md §1: `fpdfsdk` is not ported):

- `cpdf_bafontmap.*` (469 LOC) — only `fpdfsdk/cpdfsdk_{widget,appstream}.cpp`
  and `fpdfsdk/formfiller/` use it; it exists to add *native OS* fonts to a
  widget's `/DR`, which is a Windows-GDI path (`GetNativeFontName` is
  `#if BUILDFLAG(IS_WIN)`-only, `cpdf_interactiveform.cpp:98-137`). On Linux
  — the oracle's platform — `GetNativeFontName` returns the empty string, so
  `CPVT_FontMap::SetupAnnotSysPDFFont` yields no font and font index 1 is
  always null. **We implement font index 1 as permanently absent** (D6).
- `cpdf_icon.*` / the `/MK` icon-stream path (`CPDF_IconFit`) is *parsed* into
  the data model (it is cheap and the AcroForm reader should be complete) but
  no icon is rendered, because only `cpdfsdk_appstream` draws icons.
- The `NotifierIface` observer web (`cpdf_interactiveform.h:47-70`,
  `NotifyBeforeValueChange`/`AfterValueChange`/…): pure JS-callback plumbing
  with no core consumer. STYLE §1 calls this an anti-pattern; `set_value`
  returns what changed instead (§3, `FieldChange`).
- `ExportToFDF` (`cpdf_interactiveform.cpp:1037-1119`) needs `CFDF_Document`
  from `pdfrum-edit`; deferred to M7 with a note in that crate's brief.
- `cpdfsdk_appstream.cpp` — see §1.13 and **Escalation E1**; the default is
  **not ported in v1**.

### 1.0.1 Rect and float primitives the whole crate depends on

`CFX_FloatRect` is `{left, bottom, right, top}` (y-up). `kurbo::Rect` is
`{x0, y0, x1, y1}` with no orientation guarantee, so the mapping is
`(x0,y0,x1,y1) = (left, bottom, right, top)` and every C++ method below becomes
a free function in `geom.rs` — **do not** use kurbo's own `inflate`/`union`,
whose normalization differs.

| C++ | `fx_coordinates.cpp` | Behavior to port |
|---|---|---|
| `Normalize()` | :158-165 | swap if `left > right`; swap if `bottom > top` |
| `IsEmpty()` | `fx_coordinates.h:200` | `left >= right \|\| bottom >= top` (**not** normalized first) |
| `Inflate(x, y)` | :252-265 | **Normalizes first**, then `left -= x; bottom -= y; right += x; top += y` |
| `Deflate(x, y)` | :271-280 | `Inflate(-x, -y)` — also normalizes first |
| `Union(other)` | :180-188 | normalize both, then min/min/max/max |
| `Intersect(other)` | :167-174 | normalize both, then max/max/min/min |
| `Contains(point)` | :229-234 | normalize a copy; **inclusive** on all four edges |
| `Translate(e, f)` | :297-302 | no normalization |
| `GetCenterSquare()` | :218-227 | half-extent = `min(w, h) / 2` about the center |
| `ScaleFromCenterPoint(s)` | :311-322 | scale half-extents about the center |
| `Width()`/`Height()` | header | `right - left` / `top - bottom` — **can be negative** |

Float comparisons in the layout engine are epsilon-based
(`fx_system.h:36-41`) and the epsilon is `1e-4` **as a `double` literal**, so
the comparison is done in `double`:

```
is_float_zero(f)      = (f as f64) < 1e-4 && (f as f64) > -1e-4
is_float_bigger(a, b) = a > b && !is_float_zero(a - b)
is_float_smaller(a,b) = a < b && !is_float_zero(a - b)
```

`CFX_Matrix::MatchRect(dest, src)` (:430-441) — used to fit an AP form's BBox
into the annot rect (§1.9.4):

```
a = if |src.left - src.right| < 0.001 { 1 } else { (dest.left-dest.right)/(src.left-src.right) }
d = if |src.bottom - src.top| < 0.001 { 1 } else { (dest.bottom-dest.top)/(src.bottom-src.top) }
e = dest.left - src.left * a;   f = dest.bottom - src.bottom * d;   b = c = 0
```

`CFX_Color` (`core/fxge/cfx_color.h:12-64`) becomes:

```rust
pub enum Color { Transparent, Gray(f32), Rgb(f32,f32,f32), Cmyk(f32,f32,f32,f32) }
```

`Default` is `Transparent`. Equality is `|a-b| < 1e-4` per component
(`cfx_color.h:67-89`) — implement as an inherent `nearly_eq`, not `PartialEq`,
so `#[derive(PartialEq)]` stays honest (STYLE §1).

`CFXColorFromArray` (`cpdf_color_utils.cpp:15-31`) dispatches **purely on array
length**, and any other length (0, 2, 5, …) yields `Transparent`:

| len | result | components via |
|---|---|---|
| 1 | `Gray(a[0])` | `Array::float_at` (non-resolving; missing/mistyped → 0.0) |
| 3 | `Rgb(a[0],a[1],a[2])` | same |
| 4 | `Cmyk(a[0],a[1],a[2],a[3])` | same |
| else | `Transparent` | — |

### 1.1 Outline (bookmarks) — `cpdf_bookmark.*`, `cpdf_bookmarktree.*`

The C++ "tree" is two free functions over dicts; there is no tree object.

- **First child** (`cpdf_bookmarktree.cpp:20-36`): given a parent bookmark dict,
  `parent[/First]` as a **dict**; given *no* parent (the root query),
  `catalog[/Outlines][/First]` — with `catalog::kOutlines == "Outlines"`.
  Missing catalog or missing `/Outlines` → none. Note `GetDictFor` accepts a
  *stream's* dict too (object brief §1.6), so a `/First` that is a stream still
  yields its dict; keep that.
- **Next sibling** (:38-47): `dict[/Next]` as a dict, **except** that a `/Next`
  pointing at the node itself returns none. In C++ this is pointer identity
  (`next != dict`); ours is `ObjRef` equality when both are indirect, and
  structural identity is *not* used — an inline `/Next` duplicate is a
  different node in C++ and must stay one here.
- **Title** (`cpdf_bookmark.cpp:28-44`): `/Title` via `GetDirectObjectFor` then
  a **`ToString` type filter** (a `/Title` that is a Name yields the empty
  string), `decode_text`, then **every code unit is clamped up to 0x20**
  (`result += std::max(wc, 0x20)`). Control characters become spaces; the
  string length is unchanged. Port the clamp — it is observable in
  `FPDFBookmark_Find`'s case-insensitive compare.
- **Count** (:75-77): `/Count` via `GetIntegerFor`. **Sign is preserved and the
  value is not masked**: positive = open with N descendants, negative = closed,
  0 = leaf. Pinned: `bookmarks.pdf` → `0`, `1`, `-2`, `0`
  (`fpdf_doc_embeddertest.cpp:539`).
- **Style** (:71-73): `/F` via `GetIntegerFor`, **unmasked** — `Style Is 15` →
  `15`, not `3` (`fpdf_doc_embeddertest.cpp:662`). A non-integer `/F` → 0.
- **Color** (:57-69): `/C` must be an array of **exactly 3**; components via
  `GetNumberAt(i)->GetNumber()`. The C++ here dereferences
  `GetNumberAt(i)` **without a null check** — a 3-element array whose members
  are not numbers is an upstream null-deref (latent crash). We return
  `None` in that case (D2, pixel-invisible). The API layer additionally
  rejects any component outside `[0, 1]` (`fpdf_doc.cpp:190-199`) — that
  clamp lives in our accessor, not the caller, because there is no other
  consumer. Pinned by `bookmarks_color.pdf`: five rejection shapes, one
  acceptance `(0.1, 0.2, 0.3)`.
- **Dest / Action** (:46-55): `/Dest` through `Dest::create` (§1.3), `/A` as a
  dict through `Action` (§1.4). The *caller-level* fallback — try `/Dest`
  first, and only if that yields no array try `action.dest()` — is in
  `fpdf_doc.cpp:157-177` and is the behavior users see; put it in
  `Bookmark::dest()` here (there is no second caller).

**Cycle guard.** `core/` has none; the guard lives in `FindBookmark`
(`fpdf_doc.cpp:41-67`) and is a **visited set of dict identities** with two
distinct checks: on entry (return empty if already visited) and in the sibling
loop (`while (child && !visited.contains(child))`). Insertion happens *before*
the title compare, so a self-referential root is visited once. Pinned by
`bookmarks_circular.pdf` (`fpdf_doc_embeddertest.cpp:695`): searching must
terminate and return none. Our iterator (§3, `outline::walk`) carries the same
`HashSet<ObjRef>` and additionally guards the `/First` descent (C++ relies on
the sibling check catching it one step later — same outcome, one fewer
allocation for us).

Search is **case-insensitive** (`WideString::CompareNoCase`, `fpdf_doc.cpp:50`)
and pre-order: check self, then for each child recurse fully before advancing
to the sibling.

### 1.2 Name trees and number trees — `cpdf_nametree.*`, `cpdf_numbertree.*`

Both are `/Kids` + `/Limits` + leaf-array structures, but the two files share
no code and differ in three places that matter, so the brief keeps them apart.

#### 1.2.1 Name tree — construction and the four entry points

`Create(doc, category)` (`cpdf_nametree.cpp:482-502`) walks
`catalog[/Names][<category>]`; **any missing link yields no tree at all**
(not an empty one). Categories used in `core/`: `"Dests"`; the API layer adds
`"EmbeddedFiles"`, `"JavaScript"`, `"AP"`, `"Pages"`, `"Templates"`, `"IDS"`,
`"URLS"`, `"Renditions"`, `"AlternatePresentations"`.

`kNameTreeMaxRecursion = 32` (:26) caps every recursive walk. Exceeding it
returns "not found" / count 0 — never an error.

**Cycle protection is per-call and objnum-based** (:203-227). Three predicates:

- `IsTraversedObject(obj, seen)` — `obj.objnum == 0` (i.e. a *direct* object)
  is **never** considered traversed and is never inserted; otherwise insert and
  return whether it was already present.
- `IsArrayWithTraversedObject(array, seen)` — true if the array itself is
  traversed **or any element** is. Note this inserts every element's objnum
  into `seen` as a side effect, so a later node sharing any element is
  poisoned. Port the side effect.
- The set is `absl::flat_hash_set<uint32_t>` created fresh per
  `SearchNameNodeByName` call (:335) — **not** shared across lookups.

**`GetNodeLimits(limits)`** (:34-52) — the repair that everything else builds on:

1. While `limits.len() < 2`, **append an empty string** (mutation!).
2. `left = decode_text(limits[0])`, `right = decode_text(limits[1])` — via
   `GetUnicodeText`, so a non-string limit yields the empty string.
3. **If `left > right`, swap them — and swap the array elements too.**

`TrimNodeLimits(limits)` (:54-58) truncates a `/Limits` array to 2 elements,
also by mutation.

**`SearchNameNodeByNameInternal`** (:234-326) — the actual lookup:

```
if level > 32 { return None }
limits = node[/Limits] as array;  names = node[/Names] as array
if names is Some && IsArrayWithTraversedObject(names)  { names = None }
if limits is Some && IsArrayWithTraversedObject(limits) { limits = None }

if let Some(limits) = limits {
    if inserting { TrimNodeLimits(limits) }        // lookup does NOT trim
    let (left, right) = GetNodeLimits(limits);     // may swap + pad
    if name < left  { return None }                // prune below
    if name > right {
        if !inserting { return None }              // prune above
        if let Some(names) = names {               // insertion: remember slot
            insert_slot = (names, names.len()/2 - 1);
            return None;
        }
    }
}

if let Some(names) = names {                       // LEAF
    for i in 0 .. names.len()/2 {
        let key = decode_text(names[2*i]);
        match key.cmp(name) {
            Greater => break,
            _ => {
                if inserting { insert_slot = (names, i) }
                if key < name { continue }
                *index += i;  return names.direct_at(2*i + 1);   // FOUND
            }
        }
    }
    *index += names.len()/2;
    return None;
}

kids = node[/Kids] as array;  if none || IsTraversedObject(kids) { return None }
for kid in kids {
    let kid = kid as dict;  if none || IsTraversedObject(kid) { continue }
    if let Some(found) = recurse(kid, level+1) { return found }
}
None
```

Behaviors a reimplementation gets wrong:

- The leaf scan is **linear, not binary**, and it relies on the names array
  being sorted only for the early `break`. An unsorted array finds nothing past
  the first out-of-order key. Keep the linear scan.
- Comparison is on the **decoded text** (UTF-16 code-unit order after
  `decode_text`), not the raw bytes. `WideString::Compare` is a `wmemcmp` over
  `wchar_t` — on Linux `wchar_t` is 32-bit and `WideString` holds UTF-32-ish
  code points, so a supplementary-plane key compares by scalar value.
  In Rust, comparing `Vec<char>`/`str` by `char` gives the same order (D3).
- A node that has **both** `/Names` and `/Kids` is treated as a leaf; `/Kids` is
  never reached.
- `*index` accumulates leaf sizes across the whole traversal, so on a *miss* it
  ends up as the total pair count of every leaf visited — used only by the
  insertion path.
- **Lookup does not trim `/Limits`; insertion does.** Pinned exactly by
  `CPDFNameTreeTest.GetFromTreeWithLimitsArrayWith4Items`
  (`cpdf_nametree_unittest.cpp:131-179`): a 4-element `/Limits` stays 4 after
  two `LookupValue` calls and becomes 2 after one `AddValueAndName`.

**`SearchNameNodeByIndexInternal`** (:353-411) — the lookup-by-ordinal:

```
if level > 32 { return None }
if let Some(names) = node[/Names] {                 // LEAF — note: no /Limits check
    let count = names.len()/2;
    if target >= *cur + count { *cur += count; return None }
    let idx = 2*(target - *cur);
    let value = names.direct_at(idx + 1)?;          // a null value aborts the WHOLE search
    return Some((decode_text(names[idx]), value, names, idx));
}
for kid in node[/Kids]? { if let Some(r) = recurse(kid, level+1) { return Some(r) } }
None
```

Two divergences from the by-name path: **no cycle guard at all** (a `/Kids`
cycle recurses to depth 32 and stops), and **no `/Limits` pruning** — the walk
is a pure in-order enumeration. A missing/unresolvable *value* at the target
index returns `None` from the whole search rather than skipping.

**`CountNamesInternal`** (:414-446): depth-capped at 32, with a *dict-pointer*
visited set (`flat_hash_set<const CPDF_Dictionary*>`, so direct dicts count
too — different from the objnum set above). `/Names` present → `len()/2`,
**without** descending into `/Kids`. Otherwise sum over kid dicts, skipping
non-dict kids.

**Named-destination lookup** (`LookupNamedDest`, :542-554) is a two-rung ladder:

1. `Create(doc, "Dests")` → `LookupValue(decode_text(name_bytes))`, then
   `GetNamedDestFromObject`.
2. If that yields nothing: `LookupOldStyleNamedDest` (:461-470) —
   `catalog[/Dests]` as a dict, `GetDirectObjectFor(name)` **using the raw byte
   string as the key**, then `GetNamedDestFromObject`.

`GetNamedDestFromObject(obj)` (:448-459): an Array is the dest; a Dict yields
`dict[/D]` as an array; anything else → none.

Note the asymmetry: the new-style key is `decode_text(bytes)` (so a UTF-16BE
name string in the tree matches), while the old-style key is the **byte string
verbatim** as a dict key. Both rungs are exercised by `named_dests.pdf`
(`fpdf_view_embeddertest.cpp:853`): `"First"` resolves via the tree, and
`"FirstAlternate"` only via the old-style dict.

**Mutation APIs.** `AddValueAndName` (:561-621) and `DeleteValueAndName`
(:623-640) plus their helpers `GetNodeAncestorsLimits` (:63-103) and
`UpdateNodesAndLimitsUponDeletion` (:108-201) are a complete
limits-maintenance algorithm. They have **no core consumer** — only
`fpdfsdk/fpdf_attachment.cpp` (add/delete embedded attachments) — so they are
deferred to `pdfrum-edit`'s brief and *not* implemented in v1 (D5). Their
unittest assertions (`cpdf_nametree_unittest.cpp:181-415`) are recorded in §4
so the port has its oracle when M7 needs it.

#### 1.2.2 Number tree — `cpdf_numbertree.cpp`

Simpler and unguarded. `FindNumberNode(node, num)` (:17-55):

```
if let Some(limits) = node[/Limits] {
    if num < limits.int_at(0) || num > limits.int_at(1) { return None }
}
if let Some(nums) = node[/Nums] {
    for i in 0 .. nums.len()/2 {
        let key = nums.int_at(2*i);
        if key == num { return nums.direct_at(2*i + 1) }
        if key > num  { break }
    }
    return None;                            // leaf: never falls through to /Kids
}
for kid in node[/Kids]? { if let Some(r) = FindNumberNode(kid, num) { return r } }
None
```

**No depth cap and no cycle guard.** A `/Kids` cycle is unbounded recursion —
an upstream stack-overflow bug. We add `Limits.max_name_tree_depth` (default
32, matching the name tree) and record a diagnostic on exceeding it (D4).

`FindLowerBound(node, num)` (:57-104) — the page-label predecessor search, and
the only *interesting* function in the file:

```
if let Some(limits) = node[/Limits] {
    if num < limits.int_at(0) { return None }
    let max = limits.int_at(1);
    if num >= max { return Some((max, FindNumberNode(node, max))) }   // NOTE: re-descends
}
if let Some(nums) = node[/Nums] {
    for i in (0 .. nums.len()/2).rev() {          // BACKWARD scan
        let key = nums.int_at(2*i);
        if num >= key { return Some((key, nums.direct_at(2*i + 1))) }
    }
    return None;
}
for kid in node[/Kids]?.iter().rev() {            // BACKWARD over kids
    if let Some(r) = FindLowerBound(kid, num) { return Some(r) }
}
None
```

Three things to preserve exactly:

- The `num >= max` short-circuit returns the key `max` **paired with a fresh
  `FindNumberNode(node, max)` lookup** — which can return `None` while the key
  is still `max`. `CPDF_PageLabel` then treats a `None` value as "no label
  dict" but still uses the key for the offset arithmetic (§1.8).
- Both the `/Nums` scan and the `/Kids` walk are **reverse order**, which is
  what makes "greatest key ≤ num" work without sorting assumptions across kids.
- Missing/mistyped `/Limits` entries read as `0` via `Array::int_at`, so a
  malformed limits array prunes as if the range were `[0, 0]`.

`kids_array.dict_at(i)` returning `None` (a non-dict kid) is `continue`d in
both functions.

### 1.3 Destinations — `cpdf_dest.cpp`

A `CPDF_Dest` wraps **one array** (possibly null). Construction
(`Create`, :41-52) accepts any object:

| `/Dest` (or `/D`) object type | result |
|---|---|
| absent / null | empty dest |
| **String** or **Name** | `NameTree::lookup_named_dest(doc, obj.GetString())` — the §1.2.1 ladder |
| Array | that array |
| anything else (`ToArray` fails) | empty dest |

`obj->GetString()` on a Name yields the name bytes; on a String the raw bytes.
So `/Dest /Chapter1` and `/Dest (Chapter1)` are the same lookup — this is why
the ladder takes a byte string, not text.

**The eight dest syntaxes** are one array shape with a name at index 1
(`kZoomModes`, :24-26). The mode enum is the `PDFDEST_VIEW_*` numbering, and
index 0 is the reserved "unknown":

| index | name at `[1]` | max params (`kZoomModeMaxParamCount`, :28-29) | ISO 32000-1 §12.3.2.2 params |
|---|---|---|---|
| 0 | `Unknown` (unmatched) | 0 | — |
| 1 | `XYZ` | 3 | left, top, zoom |
| 2 | `Fit` | 0 | — |
| 3 | `FitH` | 1 | top |
| 4 | `FitV` | 1 | left |
| 5 | `FitR` | 4 | left, bottom, right, top |
| 6 | `FitB` | 0 | — |
| 7 | `FitBH` | 1 | top |
| 8 | `FitBV` | 1 | left |

**`zoom_mode()`** (:87-102): `array.direct_at(1)`, then `GetString()` —
which coerces **any** type (a String `(XYZ)` matches, and so does a Number
whose `GetString()` is `"0"`, matching nothing). Linear scan over indices
**1..=8** (index 0 is skipped so a literal `/Unknown` is not matched); no match
→ 0. Missing element → 0.

**`num_params()`** (:156-164): `min(kZoomModeMaxParamCount[mode], len - 2)`,
and 0 if the array has fewer than 2 elements. **`param(i)`** (:166-168) is
`array.float_at(2 + i)` — non-resolving, missing/mistyped → 0.0, and
**unbounded**: `param(99)` on a short array is 0.0, not a panic.

**`scroll_position()`** (:75-85) is a different accessor over the same data:
every element from index 2 to the end via `float_at`, ignoring the mode
entirely. It is used by `FPDF_GetPageAAction` consumers; keep it as
`params_all()`.

**`xyz()`** (:104-154) is the strictest reader and the one with the null
semantics:

```
has_x = has_y = has_zoom = false
if array is none || array.len() < 5 { return false }
let name = array.direct_at(1) as Name;                 // TYPE-FILTERED: must be a Name
if name != b"XYZ" { return false }
let nx = array.direct_at(2) as Number;                 // a Null yields None
let ny = array.direct_at(3) as Number;
let nz = array.direct_at(4) as Number;
has_x = nx.is_some();  has_y = ny.is_some();  has_zoom = nz.is_some();
if let Some(n) = nx { x = n.as_f32() }
if let Some(n) = ny { y = n.as_f32() }
if let Some(n) = nz { if n.as_f32() == 0.0 { has_zoom = false } else { zoom = n.as_f32() } }
true
```

Four traps: the `< 5` length gate (a 4-element `[0 /XYZ 4 5]` returns false —
`cpdf_dest_unittest.cpp:33-36`); the `ToName` filter at index 1 (unlike
`zoom_mode`, a String `(XYZ)` here returns false); a `Null` at 2/3/4 sets the
corresponding `has_*` to false while the function still returns **true**
(:56-66 of the unittest); and **zoom == 0.0 is treated as null** but `x`/`y`
of 0.0 are not (:49-55). A *negative* zoom is kept
(`bug_821454.pdf` → `-200`, `fpdf_doc_embeddertest.cpp:125`).

Note `xyz()` writes `x`/`y`/`zoom` only when the corresponding value is
present, so the caller's variables keep their prior contents — which the
embedder test relies on. In Rust this becomes
`fn xyz(&self) -> Option<Xyz>` with `Xyz { x: Option<f32>, y: Option<f32>,
zoom: Option<f32> }`; "returns false" is `None`, and the untouched-output
semantics disappear (D8, unobservable).

**Page resolution** (`GetDestPageIndex`, :54-73):

```
let page = array?.direct_at(0)?;         // resolves one level
if page.is_number() { return page.as_c_int() }        // RAW INDEX, not validated
if !page.is_dictionary() { return -1 }
doc.page_index(page.objnum())                          // -1 if not found
```

Consequences pinned by `named_dests.pdf` (`fpdf_doc_embeddertest.cpp:97`):

- A number at index 0 is returned **verbatim**, with no bounds check against
  the page count — `"FirstAlternate"` yields `11` in a document with fewer
  pages. Do not clamp.
- A **dict** at index 0 needs `GetObjNum()`, i.e. the object number of the
  *resolved* page dict. `Object::resolve` gives us the value but not the
  objnum, so this path needs the `ObjRef` itself: read index 0 with
  `Array::raw_at(0)` and, if it is a `Ref`, use that ref's number; if it is an
  *inline* dict its objnum is 0 and `page_index(0)` must yield `-1`
  (`cpdf_document.cpp:424-455` returns −1 for out-of-range).
- A ref to a non-existent object resolves to null → `-1` (`"LastAlternate"`).

The page index lookup itself is `pdfrum-parser`'s `Document::page_index`
(parser brief §1.17): linear scan of the page cache, else a counted tree
search, and it only caches a hit when the object is a *valid page dict*
(`/Type` Name == `Page`). Crucially it must **not** depend on which pages have
been parsed — pinned by `bug_1506.pdf` returning `3` under three different
page-load orders (`fpdf_doc_embeddertest.cpp:212-243`).

**Remote dests.** There is no "by-remote-name" resolution in `core/`: for a
`/GoToR` or `/GoToE` action, `Action::dest()` still calls `Dest::create` on the
action's `/D`, which for a *string* `/D` runs the **local** name-tree ladder
against the *current* document (`cpdf_action.cpp:75-81`). The remote file named
by `/F` is never opened. So a remote-by-name dest resolves against the wrong
document, usually to nothing — that is the behavior, and `gotoe_action.pdf`
pins it: the dest resolves to `PDFDEST_VIEW_FIT` with 0 params from the local
tree while `GetFilePath` separately returns `"ExampleFile.pdf"`
(`fpdf_doc_embeddertest.cpp:463`).

### 1.4 Actions — `cpdf_action.cpp`, `cpdf_aaction.cpp`

**Type** (:53-73): `ValidateDictOptionalType(dict, "Action")` first —
`fpdf_parser_utility.cpp:188-192`: the dict must exist and either **lack**
`/Type` or have `/Type` == the **Name** `Action`. A `/Type /Lights` → Unknown.
Then `/S` via `GetNameFor` (**Name-typed, non-resolving**) — a String `/S`
yields `""` → Unknown. Then a linear scan of the 18-entry table (:23-42) in
declaration order, returning `index + 1`:

```
GoTo GoToR GoToE Launch Thread URI Sound Movie Hide
Named SubmitForm ResetForm ImportData JavaScript SetOCGState Rendition Trans GoTo3DView
```

Matching is **exact and case-sensitive**: `"Javascript"` → Unknown
(`cpdf_action_unittest.cpp:116-126`). In Rust this is
`enum ActionKind` with 18 variants plus `Unknown`, and a `from_name` built by
the `names!` macro so the spelling table has one source (STYLE §2b).

**Per-type accessors:**

- **`dest(doc)`** (:75-81): only for `GoTo | GoToR | GoToE`; else empty.
  `Dest::create(doc, dict[/D] via GetDirectObjectFor)`.
- **`file_path()`** (:83-107): only for `GoToR | GoToE | Launch | SubmitForm |
  ImportData`. `dict[/F]` via `GetDirectObjectFor` → `FileSpec::file_name()`
  (§1.5). If `/F` is absent **and** the type is exactly `Launch`, fall back to
  `dict[/Win][/F]` as a **byte string** decoded with `FromDefANSI` (Latin-1,
  *not* PDFDocEncoding) — this is the only `FromDefANSI` in the navigation
  path and it matters for high bytes. All other types with no `/F` → empty.
- **`uri(doc)`** (:109-126): only for `URI`. `dict[/URI]` via
  `GetByteStringFor` — **raw bytes, never re-encoded or percent-decoded**
  (`uri_action_nonascii.pdf` round-trips `\xA5` and `\xC7` verbatim,
  `fpdf_doc_embeddertest.cpp:390`). Then the base-URI join: if
  `catalog[/URI]` is a dict and the URI has **no `:` at index > 0** (i.e.
  `find(":")` is `None` **or** `== 0`), and `catalog[/URI][/Base]` is a
  **String or Stream**, prepend `base.GetString()`. Note the `== 0` case: a
  URI starting with `:` is treated as relative. `GetString()` on a Stream
  returns `""` in the C++ object model, so a `/Base` stream contributes
  nothing while still satisfying the type test — keep the shape, the effect is
  a no-op prepend.
- **`hide_status()`** (:128-130): `dict.bool(b"H").unwrap_or(true)` —
  `GetBooleanFor` is **Boolean-typed and non-resolving**, so `/H 0` (an Int)
  yields the default `true`.
- **`named_action()`** (:132-134): `/N` via `GetByteStringFor`.
- **`flags()`** (:136-138): `/Flags` via `GetIntegerFor`.
- **`fields()`** (:140-175): `/S == "Hide"` (read with `GetByteStringFor`, so
  a String `/S` works here even though `type()` would have said Unknown!) →
  `dict[/T]` via `GetDirectObjectFor`; otherwise `dict[/Fields]` as an array.
  Then: a **Dict or String** result becomes a one-element list; an Array
  becomes its resolved direct elements with nulls dropped; anything else is
  empty.
- **`javascript()`** (:177-235): `/JS` via `GetDirectObjectFor`, kept only if
  it `IsString() || IsStream()`; then `GetUnicodeText()`. The distinction
  between "absent" and "present but empty" is preserved by returning
  `Option<String>` (C++ has both `MaybeGetJavaScript` and `GetJavaScript`).
  A Stream's `GetUnicodeText` decodes its **raw** (undecoded) bytes in the
  C++ object model — note this, it is not the filtered stream.

**Chained `/Next`** (:190-226). Two accessors that must agree:

```
count():  if !dict.contains(/Next) { 0 }
          else match dict.direct(/Next) { None => 0, Dict => 1, Array(a) => a.len(), _ => 0 }
sub(i):   if !dict.contains(/Next) { none }
          else match dict.direct(/Next) { Array(a) => Action(a.dict_at(i)),
                                          Dict(d) if i == 0 => Action(d),
                                          _ => none }
```

The `KeyExist` pre-check matters: a `/Next` whose value is an unresolvable
reference has the key but no direct object → count 0. An array element that is
not a dict yields an empty action at that index while the count still includes
it. There is **no cycle guard** on `/Next` in `core/` — chasing
`a[/Next] = a` loops forever. No `core/` caller walks the chain (only
`fpdfsdk/cpdfsdk_actionhandler.cpp`, not ported), but our iterator adds a
visited set (D4) since we expose one.

**Additional actions** (`cpdf_aaction.cpp:19-42`) — a fixed 21-entry key table
indexed by an enum, and the table has a **genuine duplicate**: `"C"` appears
at both `kClosePage` and `kCalculate`. Port the table verbatim including the
collision; the enum discriminant is the API, the string is not unique.

```
E X D U Fo Bl PO PC PV PI O C K F V C WC WS DS WP DP
```

`kDocumentOpen` is an artificial 22nd enum value with **no** table entry
(`ActionExist(kDocumentOpen)` would read out of bounds in C++ — the callers
never do). Our enum has 21 variants and the "document open" case is handled by
the caller reading `catalog[/OpenAction]` directly. `IsUserInput` (:62-71) is
`{kButtonUp, kButtonDown, kKeyStroke}`.

Pinned: `get_page_aaction.pdf` page 0 `/AA /O` is an `EMBEDDEDGOTO` whose
filespec path is `\\127.0.0.1\c$\Program Files\test.exe`
(`fpdf_doc_embeddertest.cpp:978`); page 1 has no `/AA`.

### 1.5 File specifications — `cpdf_filespec.cpp`

**`file_name()`** (:100-133) over an object that may be a String or a Dict:

```
match obj {
  Dict(d) => {
    // 1. /UF, TYPE-FILTERED to String, decoded as PDF text (BOM-aware)
    let mut name = d.direct(/UF).as_string().map(decode_text).unwrap_or_default();
    // 2. fallback /F, TYPE-FILTERED to String, decoded as Latin-1 (FromDefANSI)
    if name.is_empty() {
        name = d.direct(/F).as_string().map(latin1).unwrap_or_default();
    }
    // 3. FS == URL short-circuits BEFORE platform decoding
    if d.byte_string(/FS) == b"URL" { return name }
    // 4. fallback DOS, then Mac, then Unix — first PRESENT one wins (even if empty)
    if name.is_empty() {
        for key in [b"DOS", b"Mac", b"Unix"] {
            if let Some(s) = d.direct(key).as_string() { name = latin1(s); break }
        }
    }
  }
  Str(s) => name = latin1(s),
  _ => return empty,          // a Name object yields nothing
}
decode_file_name(name)
```

Five traps, all pinned by `cpdf_filespec_unittest.cpp:61-134`:

- The `ToString` filters are why `/UF /http://evil.org` (a **Name**) yields the
  empty string rather than the name text — the crbug.com/959183 fix.
- `/UF` uses `decode_text` (UTF-16BE/LE BOM, UTF-8 BOM, else PDFDocEncoding)
  while `/F`, `/DOS`, `/Mac`, `/Unix` and the bare-String case use
  **`FromDefANSI` = Latin-1** (byte → same code point). These are different
  functions; do not unify them.
- Precedence is `UF > F > DOS > Mac > Unix`, and the unittest sets them in
  *reverse* order asserting the answer changes each time.
- `/FS == "URL"` returns **before** `decode_file_name`, so a URL is never
  slash-translated. It does not prevent the `UF`/`F` reads.
- The `DOS/Mac/Unix` loop `break`s on the first key whose value is a String —
  even an empty one — so a present-but-empty `/DOS` shadows `/Mac`.

**`decode_file_name`** (:66-98) is the platform path translation. On the
oracle's platform (Linux) it is the **identity function** — the whole
`ChangeSlashToPlatform` machinery is `#if IS_APPLE || IS_WIN`. We implement
Linux behavior only (D7); the Windows/Apple rules are recorded here so a future
port has them, and because `EncodeFileName` (the inverse) is needed by
`pdfrum-edit`:

- *Apple decode*: if the path starts with `/Mac`, drop the leading `/`; then
  every `/` → `:`.
- *Windows decode*: `view[0] != '/'` → slashes to `\`. `view[1] == '/'` → drop
  one leading `/`, slashes to `\`. `view[2] == '/'` → `view[1] + ":" +
  translate(view[2..])`. Else `"\\" + translate(view)`. **Note the unchecked
  indexing at `view[1]`/`view[2]` on a 1- or 2-char path — an upstream OOB read
  we simply do not reproduce.**
- *Windows encode* (:186-203): `view[1] == ':'` → `"/" + view[0] +
  (if view[2] != '\\' { "/" }) + translate(view[2..])`; `\\`-prefixed → drop
  one; `\`-prefixed → prepend `/`; else translate. Translation is
  `'\\' | ':' → '/'`.
- *Apple encode* (:204-209): a path starting with `Mac` gets a leading `/`;
  then `'\\' | ':' → '/'`.

**`file_stream()`** (:135-162): needs `obj` to be a Dict with an `/EF` dict.
Then, over the key list `["UF", "F", "DOS", "Mac", "Unix"]` truncated to the
**first 2** when `/FS == "URL"`: if `dict.unicode_text(key)` is non-empty, try
`ef[key]` as a **Stream**; the first stream found wins. Note the presence test
is on the *outer* dict (the file name) but the stream comes from `/EF` — a name
without a matching `/EF` entry falls through to the next key.
`params_dict()` (:164-167) is `file_stream()?.dict()[/Params]`.

### 1.6 Links — `cpdf_link.cpp`, `cpdf_linklist.cpp`

`CPDF_Link` is three accessors over an annotation dict: `/Rect` via
`GetRectFor` (`cpdf_link.cpp:22-24`, **not normalized**), `/Dest` through
`Dest::create` (:26-28), `/A` as a dict through `Action` (:30-32). The
`/Dest`-then-`/A` fallback is again at the API layer
(`fpdf_doc.cpp:399-416`) and belongs in `Link::dest()` here.

**`LinkList::for_page`** (`cpdf_linklist.cpp:49-75`) builds a per-page vector
from `/Annots`, pushing the dict for entries whose `/Subtype` (via
`GetByteStringFor`, so a *String* subtype also matches) is `"Link"` and
pushing a **null placeholder for every other entry** — "Add non-links as
nullptrs to preserve z-order" (:71). A page whose dict has objnum 0 (inline)
gets no list at all (:51-54). The cache is keyed by page objnum.

**`link_at_point`** (:20-47): iterate the vector **backwards** (topmost first),
skip nulls, and return the first whose `GetRect().Contains(point)` — where
`Contains` normalizes and is inclusive on all edges (§1.0.1). The z-order
reported is the **index in the full `/Annots` array**, which is exactly why the
nulls are kept. Pinned: `bug_821454.pdf` gives z-orders `0` and `1` for the two
links (`fpdf_doc_embeddertest.cpp:255`); `annots.pdf` maps link hit-points to
annot indices 0 and 1 (`fpdf_doc_embeddertest.cpp:419`).

`FPDFLink_Enumerate` (`fpdf_doc.cpp:437-462`) is a separate, simpler walk:
scan `/Annots` from `start_pos`, resolve each element to a dict, skip
non-dicts, and return the first with `/Subtype == "Link"` plus the *next*
index. It does not use the cache and does not preserve nulls.

### 1.7 Structure tree — `cpdf_structtree.cpp`, `cpdf_structelement.cpp`

This is the `--show-structure` Tier-A target, and its shape is unusual: the
tree is **built per page, bottom-up, from the parent tree**, not by walking
`/StructTreeRoot /K` downward.

#### 1.7.1 Load

`LoadPage(doc, page_dict)` (:30-40) returns nothing unless `IsTagged`
(:21-25): `catalog[/MarkInfo][/Marked]` via `GetIntegerFor` must be non-zero.
A `/Marked true` (Boolean) yields 0 through `GetIntegerFor` — **a Boolean
`/Marked` disables the whole tree**. Untagged → the tool prints only a stderr
line and the golden is a **0-byte file** (verified across the golden store).

Construction (:42-44): `tree_root = catalog[/StructTreeRoot]` as a dict;
`role_map = tree_root[/RoleMap]` as a dict. Both may be absent.

`LoadPageTree(page_dict)` (:58-106):

```
page = page_dict
tree_root?                                          // else empty tree
let kids_obj = tree_root.direct(/K)?;               // else empty
kids_count = match kids_obj { Dict => 1, Array(a) => a.len(), _ => return }
kids = vec![None; kids_count]                       // PRE-SIZED, sparse
let parent_tree = tree_root[/ParentTree] as dict?;  // else empty (kids stays sparse!)
let parents_id = page.int(/StructParents).unwrap_or(-1);
if parents_id < 0 { return }
let parent_array = NumberTree(parent_tree).lookup(parents_id) as array?;
let mut element_map = BTreeMap::new();
for p in parent_array { if let Some(d) = p.dict() { add_page_node(d, &mut element_map, 0) } }
```

`kids_` is sized from `/K` but **populated only via `AddTopLevelNode`**, so
`CountTopElements()` can exceed the number of populated slots and
`GetTopElement(i)` can return null. `FPDF_StructTree_GetChildAtIndex` then
returns null and `dump.cc:249-252` prints a **stderr** line, contributing
nothing to the golden. Pinned by `bug_1768.pdf` (`CountChildren == 1`,
`GetChildAtIndex(tree,0)` false) and by `tagged_mcr_objr.pdf`, where several
`CountChildren` values exceed the fetchable children
(`fpdf_structtree_embeddertest.cpp:982`, `:898`).

`AddPageNode(dict, map, level)` (:108-146) — the bottom-up walk:

```
if level > 32 { return None }                       // kStructTreeMaxRecursion
if let Some(e) = map.get(dict) { return Some(e) }   // memo on dict identity
let element = StructElement::new(tree, dict);       // constructor LOADS KIDS (below)
map.insert(dict, element);
let parent = dict[/P] as dict;
if parent.is_none() || parent.name(/Type) == b"StructTreeRoot" {
    if !add_top_level_node(dict, element) { map.remove(dict) }
    return Some(element)
}
let parent_el = add_page_node(parent, map, level + 1)?;   // None => return element, unlinked
if !parent_el.update_kid_if_element(dict, element) { map.remove(dict); return Some(element) }
element.set_parent(parent_el);
Some(element)
```

`AddTopLevelNode(dict, element)` (:148-178):

```
let k = tree_root.direct(/K)?;                       // else false
if k.is_dictionary() {
    if k.objnum() != dict.objnum() { return false }
    kids[0] = element;                               // then FALLS THROUGH
}
let top = k.as_array();  if top.is_none() { return true }    // note: true, not false
let mut saved = false;
for (i, item) in top.iter().enumerate() {
    if let Some(r) = item.as_ref() {                 // RAW element: must be a Reference
        if r.objnum() == dict.objnum() { kids[i] = element; saved = true }
    }
}
saved
```

Two quirks: after the dict branch sets `kids[0]` it does **not** return, it
falls into the array branch which then returns `true` because `AsArray()` is
null. And the array branch matches only **direct references** (`ToReference`
on the raw element), so an *inline* dict kid of `/K` is never linked — the
documented `tagged_mcr_objr.pdf` gap.

`update_kid_if_element(dict, element)` (`cpdf_structelement.cpp:103-113`) scans
the parent's already-loaded kid list for `Kid::kElement` entries whose stored
`dict_` **pointer** equals `dict`, replaces `element_`, and returns whether any
matched. Ours compares `ObjRef` when both are indirect; an inline kid dict
cannot be matched this way in either implementation (it is a distinct
allocation in C++ too, since `GetDirectObjectAt` on an inline dict returns the
same pointer — so inline kids *do* match in C++ but not by objnum). **D9**: we
key kids by `(ObjRef | slot-path)` so inline kids match by their position in
the parent's `/K`, which is the same set of nodes.

`GetRoleMapNameFor(type)` (:48-56): `role_map[type]` via `GetNameFor`
(Name-typed, non-resolving); non-empty → mapped, else the original. Applied
**once, in the element constructor** (:31), so `GetType()` is the mapped name
and the raw `/S` is not retained.

#### 1.7.2 Element and the five `/K` shapes

`LoadKids()` (`cpdf_structelement.cpp:115-135`):

```
let page_obj_num = dict.raw(/Pg).as_ref().map(|r| r.objnum()).unwrap_or(0);
let kids_obj = dict.direct(/K)?;                     // absent => zero kids
if let Array(a) = kids_obj { kids = a.iter().map(|e| load_kid(page_obj_num, e.direct())).collect() }
else { kids = vec![load_kid(page_obj_num, kids_obj)] }
```

`LoadKid(page_obj_num, obj, kid)` (:137-191) — **the five shapes**:

| `/K` element | condition | resulting kid |
|---|---|---|
| absent/null | — | `Invalid` (a slot is still reserved) |
| **Number** | `tree.page_objnum() == page_obj_num` | `PageContent { content_id: int, page }` |
| **Number** | page mismatch | `Invalid` |
| **Dict** `/Type /MCR` | page match (after `/Pg` override) | `StreamContent { ref_obj_num: /Stm ref or 0, content_id: /MCID int, page }` |
| **Dict** `/Type /OBJR` | page match | `Object { ref_obj_num: /Obj ref or 0, page }` |
| **Dict**, MCR/OBJR but page **mismatch** | — | `Invalid` |
| **Dict**, any other `/Type` (incl. absent) | — | `Element { dict }` (resolved later) |
| anything else | — | `Invalid` |

The per-kid `/Pg` override happens **before** the type test (:160-163) and only
for reference-valued `/Pg`. So a kid dict carrying its own `/Pg` is judged
against that page, and one without inherits the parent element's `/Pg`. The
element-typed branch does **no** page test at all — element kids cross pages
freely, which is why `tagged_mcr_multipage.pdf` gives the same element tree
from both pages.

`GetKidContentId(i)` (:96-101) returns `content_id_` for `PageContent |
StreamContent`, else `-1` — this is the **page-filtered** MCID accessor and the
one a port should use.

Element accessors (:43-85), all straightforward but each with a distinct
type filter:

| accessor | key | reader | absent |
|---|---|---|---|
| `obj_type()` | `/Type` | `GetByteStringFor` (coerces) | `""` |
| `alt_text()` | `/Alt` | `GetUnicodeTextFor` | `""` |
| `actual_text()` | `/ActualText` | `GetUnicodeTextFor` | `""` |
| `expansion()` | `/E` | `GetUnicodeTextFor` | `""` |
| `title()` | `/T` | `GetUnicodeTextFor` | `""` |
| `id()` | `/ID` | `GetObjectFor` (**non-resolving**) + `IsString()` filter | `None` |
| `lang()` | `/Lang` | same as `/ID` | `None` |
| `a()` | `/A` | `GetObjectFor` (**non-resolving**) | `None` |
| `k()` | `/K` | `GetObjectFor` (**non-resolving**) | `None` |

`/ID` and `/Lang` return `Option` precisely so "present but empty" (`2U` bytes
of UTF-16 = just the terminator) differs from "absent" (`0U`) — pinned at
`fpdf_structtree_embeddertest.cpp:258` and `:301`. **`/Lang` is not
inherited**: a `TR` under a `Table` with `/Lang /hu` reports `0U`.

#### 1.7.3 The `--show-structure` output contract

`dump.cc:239-256` + `:141-229`. Per page:

```
Structure Tree for Page %d\n          // page_idx, only if the tree loaded
  <for each top element i: DumpChildStructure(child, indent=0)>
\n\n                                   // two blank lines, always
```

`DumpChildStructure(child, indent)` emits, **in this fixed order**, using
`printf("%*s ...", indent*2, "")` — i.e. `indent*2` spaces from `%*s` **plus
one literal space** from the format, so depth *d* is indented `2d + 1` spaces:

1. `S: <type>` — only if the UTF-16 length is > 0.
2. For each attribute index `i` in `0..GetAttributeCount()`: `A[<i>]:` then the
   attribute dump at `indent*2 + 2`.
3. `ActualText: <s>` if non-empty.
4. `AltText: <s>` if non-empty.
5. `ID: <s>` if non-empty. (Note: `GetID` returns 0 for absent **and** the
   `len > 0` test then also suppresses the present-but-empty case, so an empty
   `/ID` prints nothing here even though the API distinguishes them.)
6. `Lang: <s>` if non-empty.
7. For each `i` in `0..GetMarkedContentIdCount()`: `MCID<i>: <id>` when the id
   is `!= -1`.
8. `Parent ID: <s>` — only if `GetParent()` is non-null **and** the parent's
   `/ID` is non-empty.
9. `Title: <s>` if non-empty.
10. `Type: <s>` if non-empty (this is `/Type`, i.e. usually `StructElem`).
11. Recurse over `0..CountChildren()`, skipping indices whose
    `GetChildAtIndex` is null, at `indent + 1`.

`GetMarkedContentIdCount` (`fpdf_structtree.cpp:506-521`) is the **unfiltered**
one: `/K` absent → `-1`; Number or Dict → `1`; Array → its length; else `-1`.
`GetMarkedContentIdAtIndex` (:524-559): Number → the int at index 0 else `-1`;
Dict → `GetMcidFromDict` regardless of index; Array → the element at index,
Number → its int, Dict → `GetMcidFromDict`, else `-1`. `GetMcidFromDict`
(:34-41) requires `/Type == "MCR"` (Name) and a **Number** `/MCID`.
This ignores the page, which is the documented deviation
(`fpdf_structtree_embeddertest.cpp:443`); `--show-structure` uses it, so we
must reproduce it exactly, not the page-filtered `GetKidContentId`.

Attribute dumping (`dump.cc:122-137` + `:50-120`):

- `GetAttributeCount` (`fpdf_structtree.cpp:161-179`): `/A` **resolved**
  (`GetDirect`); Array → its length; Dict → `1`; else `-1`. A `-1` makes the
  `for i in 0..count` loop body never run.
- `GetAttributeAtIndex` (:181-219): Dict → index 0 only; Array → the
  **`GetDictAt(index)`**, which resolves and type-filters.
- `Attr_GetCount` = the dict's key count. `Attr_GetName(attr, i, ...)` walks
  the dict's iteration order and returns the *i*-th key. **This is where the
  sorted-vs-insertion-order divergence bites** — see **Escalation E2**.
- `Attr_GetValue(attr, name)` = `dict.GetDirectObjectFor(name)` (resolves).
- Per value type, `dump.cc:61-119` prints:

  | type | format | note |
  |---|---|---|
  | Boolean | `%*s %s: %d` | `0`/`1` |
  | Number | `%*s %s: %f` | **`%f` = 6 decimals**, e.g. `2.000000` |
  | String / Name | `%*s %s: %ls` | UTF-16 → wide |
  | Array | `%*s %s:` then each child at `indent+2` **with the same name** | hence `BBox:` / `  BBox: 44.000000` ×4 |
  | Unknown | `%*s %s: FPDF_OBJECT_UNKNOWN` | |
  | other | `%*s %s: NOT_YET_IMPLEMENTED: %d` | |

  `Attr_GetChildAtIndex` (:491-504) uses `GetObjectAt` (**non-resolving**), so
  an array of references dumps as `FPDF_OBJECT_UNKNOWN`? No — `Attr_GetType`
  on a Reference returns `FPDF_OBJECT_REFERENCE` (8), which falls into the
  `default` arm → `NOT_YET_IMPLEMENTED: 8`. Preserve that.

The whole per-string path goes through `Utf16EncodeMaybeCopyAndReturnLength` →
`ConvertToWString` → `%ls`, which on Linux widens each UTF-16 code unit to a
32-bit `wchar_t` and writes it in the C locale. **Lone surrogates and
non-representable code points make `printf` fail mid-string**, truncating the
line. We match by encoding UTF-16 code units to UTF-8 and stopping at the first
unit that has no scalar value (D10) — the same truncation point.

### 1.8 Page labels — `cpdf_pagelabel.cpp`

`GetLabel(page_index)` (:96-140):

```
if page_index < 0 || page_index >= doc.page_count() { return None }
let labels = catalog[/PageLabels] as dict?;             // else None
let (key, value) = NumberTree(labels).lower_bound(page_index)?;    // §1.2.2
let label_dict = value.and_then(|v| v.direct().as_dict());
if label_dict.is_none() { return Some(format!("{}", page_index + 1)) }   // 1-BASED fallback
let mut label = if label_dict.contains(/P) { label_dict.unicode_text(/P) } else { String::new() };
let style = label_dict.byte_string(/S).unwrap_or_default();             // GetByteStringFor
let number = page_index - key + label_dict.int(/St).unwrap_or(1);
label += &label_num_portion(number, style);
Some(label)
```

Note the two-tier "no label" answer: **out of range → `None`** (the tool prints
nothing, byte length 0), but **in range with no matching tree entry →
`Some(decimal(index+1))`**. And when `lower_bound` succeeds but its *value* is
absent or not a dict, the same decimal fallback applies while the key is
discarded. A `/S` that is not a Name still reads through `GetByteStringFor`, so
a String `(R)` selects roman.

**The five styles** (`GetLabelNumPortion`, :64-88) — exact-match, case
sensitive; anything else (including an absent `/S`) contributes the empty
string, so the label is the prefix alone:

| `/S` | function |
|---|---|
| `D` | decimal, `WideString::FormatInteger` = `%d` on a C `int` |
| `R` | `MakeRoman(n)` uppercased |
| `r` | `MakeRoman(n)` |
| `A` | `MakeLetters(n)` uppercased |
| `a` | `MakeLetters(n)` |

`MakeRoman(num)` (:21-41): `num %= 1_000_000`; greedy subtraction over

```
value:  1000 900 500 400 100  90  50  40  10   9   5   4   1
glyph:  m    cm  d   cd  c    xc  l   xl  x    ix  v   iv  i
```

The loop is `while num > 0 { while num >= kArabic[i] { … } ++i }`. **A negative
`num` produces the empty string** (outer loop never entered); the modulo is
applied to the signed value so `-1_000_001` stays negative. `MakeUpper` is
ASCII-only here.

`MakeLetters(num)` (:43-62): `0` → empty; else `--num`;
`count = (num / 26 + 1) % 1000`; `ch = 'a' + num % 26`; the result is `ch`
repeated `count` times. So 1→`a`, 26→`z`, 27→`aa`, 52→`zz`, 53→`aaa`. The
`% 1000` means `num = 26*1000` yields **count 0 = the empty string** — a real
cliff. And a negative `num` gives a negative `count`, which
`WideString::GetBuffer(negative)` handles as 0 in practice; we return empty.

Pinned exhaustively by `cpdf_pagelabel_unittest.cpp:169-204` over a
three-level tree — including `GetLabel(37) == "XXXVIII"` (start 1 at key 0),
`GetLabel(100) == "abcE"` (prefix `abc`, style `A`, `/St 5`),
`GetLabel(525) == "abc" + 17×"N"`, `GetLabel(900) == "999"` (`/St 999`),
`GetLabel(1234) == "1333"`, `GetLabel(2999) == "3098"` (the run continues past
the next key's *limits* because 2999 < 3000), `GetLabel(4999) == "mm"`,
`GetLabel(7654) == 103×"c"`, and `GetLabel(8000..=10000) == "x"` (a prefix-only
entry with no `/S`), `GetLabel(10001) == None`.

Also pinned by `page_labels.pdf` (`fpdf_doc_embeddertest.cpp:1014`):
`i, ii, 1, 2, zzA, zzB, ""` — index 6 is a **present but empty** label
(`2u` UTF-16 bytes) distinct from out-of-range (`0u`).

### 1.9 Viewer preferences and metadata

**`cpdf_viewerpreferences.cpp`** — six accessors over
`catalog[/ViewerPreferences]`, each with its own default when the dict is
absent:

| accessor | key | reader | no-dict default |
|---|---|---|---|
| `is_direction_r2l` | `/Direction` | `GetByteStringFor == "R2L"` | `false` |
| `print_scaling` | `/PrintScaling` | `!= "None"` | **`true`** |
| `num_copies` | `/NumCopies` | `GetIntegerFor` | **`1`** (but `0` if the dict exists without the key) |
| `print_page_range` | `/PrintPageRange` | `GetArrayFor` | `None` |
| `duplex` | `/Duplex` | `GetByteStringFor` | **`"None"`** |
| `generic_name(key)` | any | `GetObjectFor` + **Name filter** | `None` |

`generic_name` is the only type-filtered one: `"HideToolbar"` (a Boolean) and
`"NumCopies"` (an Integer) return `None` even when present, and keys are
case-sensitive (`"foo"` misses, `"Foo"` hits). Pinned by `viewer_ref.pdf`
(`fpdf_view_embeddertest.cpp:703`): `NumCopies == 5`, `Direction == "R2L"`,
`ViewArea == "CropBox"`, print page range `[0, 2, 4, 4]` (duplicates kept, not
deduplicated), and out-of-range element → `-1`.

**`cpdf_metadata.cpp`** is *not* a metadata reader. Its single method
`CheckForSharedForm` (:84-98) decodes the `/Metadata` stream, parses it as XML,
and walks looking for an element carrying
`xmlns:adhocwf == "http://ns.adobe.com/AcrobatAdhocWorkflow/1.0/"` with a
child element named `adhocwf:workflowType`, mapping its integer text to one of
three `UnsupportedFeature` values (0 → Email, 1 → Acrobat, 2 → Filesystem),
**taking only the first** child match. Depth cap `kMaxMetaDataDepth = 128`
(:23); exceeding it aborts the *entire* walk (the recursive call returns false
and the parent `return false`s immediately — so a single deep subtree
suppresses later siblings). Only `fpdfsdk/fpdf_ext.cpp` consumes it, to fire
an "unsupported feature" callback that `pdfium_test` prints to **stderr**
(`pdfium_test.cc:473-475`), so it contributes nothing to any golden.

Consequence: **we do not ship an XML parser.** `Metadata` in v1 exposes only
the raw decoded `/Metadata` bytes (`Document::metadata_xmp() -> Option<Vec<u8>>`)
and the shared-form scan is a documented non-feature (D11). The `--show-metadata`
dump the harness actually diffs reads the **`/Info` dictionary**
(`dump.cc:258-270`), which is eight `GetUnicodeTextFor` calls on
`Title Author Subject Keywords Creator Producer CreationDate ModDate` — plain
`pdfrum-parser` trailer access, not this crate.

Pinned `/Info` behavior worth a test here anyway, because it exercises
`decode_text` and the parser's last-wins duplicate rule: `bug_601362.pdf`
gives `Creator == "Microsoft Word"` and `CreationDate == ModDate ==
"D:20160411190039+00'00'"` with absent tags returning an **empty (not
missing)** string; `annotation_highlight_square_with_ap.pdf` has two objects
numbered `1 0` both `/Info` and the **last** one wins
(`fpdf_doc_embeddertest.cpp:901`, `:952`).

### 1.10 Annotations — `cpdf_annot.cpp`

#### 1.10.1 Subtype

`StringToAnnotSubtype` (:304-391) is a 28-arm exact string chain over
`/Subtype` read with `GetByteStringFor` (coercing, non-type-filtered), and
`AnnotSubtypeToString` (:394-456) is its inverse with `Unknown → ""`. The
enum order is fixed by `cpdf_annot.h:35-65` and is the `FPDF_ANNOT_*`
numbering, so it is API:

```
Unknown=0 Text Link FreeText Line Square Circle Polygon PolyLine
Highlight Underline Squiggly StrikeOut Stamp Caret Ink Popup
FileAttachment Sound Movie Widget Screen PrinterMark TrapNet
Watermark ThreeD RichMedia XfaWidget Redact
```

Spellings differ from the variant names in three places: `ThreeD` ↔ `"3D"`,
`XfaWidget` ↔ `"XFAWidget"`, `PolyLine` ↔ `"PolyLine"` (capital L). One
`names!`-generated table, used by both directions and by the `--annot`
dump (`write.cc:53-131` has an *identical* third copy — do not add a third).

`is_text_markup` (:39-44) = `{Highlight, Squiggly, StrikeOut, Underline}`. Note
this is **not** the same as `has_attachment_points`
(`fpdf_annot.cpp:828-838`), which additionally includes `Link` — the `--annot`
dump uses the latter to decide whether to print quadpoints at all.

#### 1.10.2 Flags and rect

`flags()` (:199-201) is `/F` via `GetIntegerFor` (coercing, so a Real `/F 4.9`
→ 4). Bits (`constants/annotation_flags.h`):

| bit | mask | name | `--annot` string |
|---|---|---|---|
| 0 | 1 | Invisible | `Invisible` |
| 1 | 2 | Hidden | `Hidden` |
| 2 | 4 | Print | `Print` |
| 3 | 8 | NoZoom | `NoZoom` |
| 4 | 16 | NoRotate | `NoRotate` |
| 5 | 32 | NoView | `NoView` |
| 6 | 64 | ReadOnly | `ReadOnly` |
| 7 | 128 | Locked | `Locked` |
| 8 | 256 | ToggleNoView | `ToggleNoView` |
| 9 | 512 | LockedContents | *(not printed)* |

`is_hidden()` (:203-205) = `flags & Hidden`.

`RectForDrawing()` (:184-191) is the pivot:

```
if is_text_markup && has_generated_ap { bounding_rect_from_quad_points(dict) }
else { dict.rect(/Rect) }
```

`GetRect()` (:193-197) = `RectForDrawing().normalize()`. But **`--annot` does
not call either**: `FPDFAnnot_GetRect` (`fpdf_annot.cpp:926-936`) reads
`dict.rect(/Rect)` directly, **unnormalized**. So the golden shows raw
`/Rect` ordering — including inverted rects — and the only reason the values
change is that AP generation *rewrote* `/Rect` (§1.12.1).

**Quadpoints** (:252-301):

```
quad_point_count(array) = array.len() / 8                      // :459-461
rect_from_quad_points_array(array, i) = FloatRect(             // :253-272
     left   = array.float_at(4 + i*8),
     bottom = array.float_at(5 + i*8),
     right  = array.float_at(2 + i*8),
     top    = array.float_at(3 + i*8))
```

Read that carefully: the quad is `[x1 y1 x2 y2 x3 y3 x4 y4]` = TL, TR, BL, BR,
and the rect is built from **(x3,y3) as left/bottom and (x2,y2) as
right/top** — i.e. `left = BL.x`, `bottom = BL.y`, `right = TR.x`,
`top = TR.y`. With the canonical PDF ordering this is a normalized rect; with
the *unittest's* ordering `{0,1,2,3,4,5,6,7}` it yields
`left=4, bottom=5, right=2, top=3` — **an inverted rect that is deliberately
not normalized** (`cpdf_annot_unittest.cpp:27-41`). Port the index arithmetic,
not an interpretation of it.

`bounding_rect_from_quad_points(dict)` (:275-290): zero quads → the zero rect;
else `rect_from_quad_points_array(0)` unioned with each subsequent one — and
`Union` **normalizes both operands** (§1.0.1), so the bounding rect of a
single inverted quad is *not* normalized (union is never called) while two
quads produce a normalized result. Pinned exactly by
`cpdf_annot_unittest.cpp:43-74`: one quad `{0..7}` → `(4,5,2,3)`;
three quads → `(2,3,6,7)`.

`rect_from_quad_points(dict, i)` (:293-301) returns the **zero rect** for
`i >= count` rather than clamping.

#### 1.10.3 The appearance-stream lookup ladder

`GetAnnotAPInternal(dict, mode, fallback_to_normal)` (:79-125) — the single
most-used function in the crate:

```
let ap = dict[/AP] as dict?;                          // else None
let mut entry = match mode { Normal => "N", Rollover => "R", Down => "D" };
if fallback_to_normal && !ap.contains(entry) { entry = "N" }
let sub = ap.direct(entry)?;                          // else None
if let Stream(s) = sub { return Some(s) }             // direct stream: done
let sub_dict = sub.as_dict()?;                        // else None
let mut as_name = dict.byte_string(/AS);
if as_name.is_empty() {
    let mut value = dict.byte_string(/V);
    if value.is_empty() { value = dict[/Parent].map(|p| p.byte_string(/V)).unwrap_or_default() }
    as_name = if !value.is_empty() && sub_dict.contains(&value) { value } else { b"Off" };
}
sub_dict.stream(&as_name)                             // Stream-typed; else None
```

Everything here is a trap:

- The mode fallback tests `KeyExist`, not "is a usable stream" — an `/AP /R`
  present but null suppresses the fallback and then yields nothing.
- `GetAnnotAP` (:207-211) passes `fallback = true`; `GetAnnotAPNoFallback`
  (:213-217) passes `false`. Only `fpdfsdk/cpdfsdk_baannot.cpp` uses the
  no-fallback form; `--annot` and rendering use the fallback form.
- The `/AS` state selection reads `/V` **from the annotation dict itself** and
  then from `/Parent` — one level only, not the full field-attribute walk of
  §1.14.1. A grandparent's `/V` is invisible here.
- `/V` and `/AS` are read with `GetByteStringFor`, which coerces: a Name `/V
  /Yes` and a String `/V (Yes)` both give `Yes`.
- The `sub_dict.contains(value)` guard means a `/V` naming a state the
  sub-dictionary lacks falls back to `"Off"` — **not** to the sole non-Off
  state. (`CPDF_FormControl::GetOnStateName` does that, §1.14.3, but it is a
  different function used for a different purpose.)
- The final `GetMutableStreamFor` is Stream-typed, so an `/AP /N << /Yes 5 >>`
  yields nothing.

#### 1.10.4 AP form instantiation and the annot matrix

`GetAPForm(page, mode)` (:219-237) memoizes one parsed `CPDF_Form` per
`(annot, stream)`. The form is constructed with **the page's resources** as the
parent resources and parsed immediately. In our terms:
`build_page(parse_content(stream_bytes), Resources{ chosen: stream.dict[/Resources], page: page_resources }, …)`.

`AnnotGetMatrix(page, annot, mode, user2device, out)` (:46-77):

```
let form = annot.ap_form(page, mode)?;
let form_matrix = form.dict.matrix(/Matrix);                     // identity if absent/short
let form_bbox = form_matrix.transform_rect(form.dict.rect(/BBox));
let mut m = Affine::match_rect(annot.rect(), form_bbox);          // §1.0.1; annot.rect() IS normalized
if annot.flags() & NoRotate != 0 && page.rotation() != 0 {
    let (ox, oy) = (annot.rect().left, annot.rect().top);         // TOP-LEFT anchor
    m = m * translate(-ox, -oy) * rotate(PI/2 * page.rotation()) * translate(ox, oy);
}
m * user2device
```

`CFX_Matrix::Concat(other)` is `*this = *this * other` in PDF order, so the
Rust composition above reads left-to-right in the same order as the C++
`Concat` calls. `TransformRect` on a `CFX_FloatRect` maps all four corners and
takes the bounding box. `page.rotation()` is the 0..3 quarter-turn count
(page brief §1.7), so the angle is `rotation * π/2` — the comment
"fractions of pi/2" is correct.

This function is **render-path only**; `--annot` never calls it. It is
specified here because `pdfrum-render` needs it and there is no better home.

### 1.11 The annotation list — `cpdf_annotlist.cpp`

Construction (:182-224) is where AP generation is triggered, so the ordering is
load-bearing:

```
let annots = page.dict[/Annots] as array?;                    // else empty list
let regenerate = catalog[/AcroForm].map(|f| f.bool(/NeedAppearances, false)).unwrap_or(false);
for i in 0..annots.len() {
    let dict = annots.direct_at(i).as_dict();  if none { continue }
    let subtype = dict.byte_string(/Subtype);
    if subtype == b"Popup" { continue }                       // (1) file popups are DROPPED
    annots.convert_to_indirect_at(i);                          // (2) mutation, see D1
    list.push(Annot::new(dict));                               // (3) RUNS GenerateAPIfNeeded
    if regenerate && subtype == b"Widget"
       && CPDF_InteractiveForm::IsUpdateAPEnabled()            // (4) global flag
       && !dict.contains(/AP) {
        generate_widget_ap(doc, dict);                         // (5) §1.11.1
    }
}
annot_count = list.len();                                      // the "real" annots
for i in 0..annot_count { if let Some(p) = create_popup_annot(list[i]) { list.push(p) } }
```

- **(1)** Popup annotations *present in the file* are excluded from the list —
  "PDFium provides its own". They are still in `/Annots`, so `--annot` (which
  walks `/Annots` directly, §1.12) **does** dump them. Two different views.
- **(3)** `CPDF_Annot`'s constructor calls `GenerateAPIfNeeded` (:129-138).
  This is the mutation point for `--annot` (§1.12.1).
- **(4)** `IsUpdateAPEnabled` is a **process-global static** initialised to
  `true` (`cpdf_interactiveform.cpp:613`) that `CPDFSDK_PageView::LoadFXAnnots`
  sets to `false` around this very construction
  (`fpdfsdk/cpdfsdk_pageview.cpp:595-600`). Since `pdfium_test` always builds
  its list through the form-fill environment, **step (5) never runs in the
  oracle**. STYLE §1 bans global state; we model it as a
  `GenerateWidgetAp` field on the options record, defaulting to **off** to
  match the oracle, with the on-path implemented because it is the documented
  `/NeedAppearances` behavior (D12).
- The `bRegenerateAP` read uses `GetBooleanFor("NeedAppearances", false)` —
  Boolean-typed, so `/NeedAppearances 1` (an Int) is false.

**Popup synthesis** (`CreatePopupAnnot`, :74-127):

```
if !popup_appears_for(parent.subtype) { return None }
let contents = parent.dict.byte_string(/Contents);
if decode_text(&contents).is_empty() { return None }           // decoded, not raw!
let popup = Dict{ /Type /Annot, /Subtype /Popup,
                  /T <parent's /T byte string>, /Contents <parent's raw /Contents bytes>,
                  /Rect <see below>, /F 0 };
```

`popup_appears_for` (:38-72) is an exhaustive match — **true** for
`{Text, Line, Square, Circle, Polygon, PolyLine, Highlight, Underline,
Squiggly, StrikeOut, Stamp, Caret, Ink, FileAttachment, Redact}`, false for
everything else including `FreeText` and `Widget`.

The emptiness test is on the **decoded** text, which is why
`{0xFE,0xFF}` (a bare BOM) and `{0xFE,0xFF,0,0x1B,'j','a',0,0x1B}` (a
BOM plus a language-code region that `StripLanguageCodes` removes entirely)
both suppress the popup while `{'A','a',0xE4,0xA0}` does not. Pinned by
`cpdf_annotlist_unittest.cpp:76-126`.

Rect placement (:102-120):

```
let r = parent.dict.rect(/Rect).normalize();
let mut popup = FloatRect(0, 0, 200, 200);                     // 200×200 fixed
if r.left + 200 > page.width() && r.bottom - 200 < 0 {
    popup.translate(r.right - 200, r.top);                     // bottom-right corner case
} else {
    popup.translate(min(r.left, page.width() - 200), max(r.bottom - 200, 0.0));
}
```

`page.width()` is the CropBox-derived width (page brief §1.7). Pinned by the
golden store: parent rect top 360.046 → popup `(490.511, 240.046, 670.511,
360.046)` on a 690.511-wide page, i.e. `min(left, 690.511-200) = 490.511`.

The synthesized popup dict is then wrapped in a `CPDF_Annot`, whose
constructor runs `GenerateAPIfNeeded` → `GeneratePopupAP` (§1.13.9), and the
parent gets a back-pointer (`SetPopupAnnot`, :125). The back-pointer exists
only so `SetPopupAnnotOpenState` / `GetPopupAnnotRect` work
(`cpdf_annot.cpp:239-250`); we return the popup by index instead of holding a
pointer (STYLE §2).

**Display filtering** (`DisplayPass`, :244-271) — the render-path rules:

```
for annot in list {
    let is_widget = annot.subtype == Widget;
    if widget_pass != is_widget { continue }        // two passes: non-widgets, then widgets
    if flags & Hidden != 0 { continue }
    if printing && flags & Print == 0 { continue }
    if !printing && flags & NoView != 0 { continue }
    annot.draw_in_context(...)
}
```

`DisplayAnnots` (:273-282) runs the non-widget pass then, if requested, the
widget pass — so **widgets always paint on top of every other annotation**
regardless of `/Annots` order. `ShouldDrawAnnotation` (`cpdf_annot.cpp:169-174`)
adds one more gate inside `DrawInContext`: hidden → no, and a **Popup draws
only when its `open_state_` is set**, which nothing in `core/` ever sets — so
synthesized popups are created, get APs generated, mutate nothing, and are
never drawn.

#### 1.11.1 Widget AP dispatch (the `/NeedAppearances` path)

`GenerateAP(doc, dict)` (:129-178), reached only from step (5):

```
if dict.byte_string(/Subtype) != b"Widget" { return }
let ft = field_attr(dict, b"FT")?;                             // §1.14.1 inherited walk
match ft.as_byte_string() {
    b"Tx" => generate_form_ap(doc, dict, TextField),
    b"Ch" => { let ff = field_attr(dict, b"Ff").map(int).unwrap_or(0);
               generate_form_ap(doc, dict,
                   if ff & ChoiceCombo != 0 { ComboBox } else { ListBox }) }
    b"Btn" => {
        let ff = field_attr(dict, b"Ff").map(int).unwrap_or(0);
        if ff & ButtonPushbutton != 0 { return }
        if dict.contains(/AS) { return }
        let parent = dict[/Parent] as dict?;  if !parent.contains(/AS) { return }
        dict.set(/AS, String(parent.byte_string(/AS)));         // NOTE: a STRING, not a Name
    }
    _ => {}
}
```

The button branch generates no appearance at all — it only **copies the
parent's `/AS` down**, and it writes it as a `CPDF_String` even though `/AS`
is spec'd as a name. Since `GetByteStringFor` coerces, the ladder in §1.10.3
still finds it. Port the type (it is observable through
`FPDFAnnot_GetValueType`).

### 1.12 The `--annot` Tier-A contract

`WriteAnnot` (`testing/pdfium_test/write.cc:373-481`) walks the page's
`/Annots` array **directly** — not `CPDF_AnnotList`. Every value is read
through the thin `FPDFAnnot_*` layer over the raw dict.

#### 1.12.1 What has already happened to the dicts

`pdfium_test` loads each page through `GetPageForIndex`
(`pdfium_test.cc:855-874`), which calls **`FORM_OnAfterLoadPage`** →
`CPDFSDK_PageView::LoadFXAnnots` → `CPDF_AnnotList(page)`. So by the time
`WriteAnnot` runs:

- Every non-Popup annotation has had `GenerateAPIfNeeded` run on it.
- `GenerateTextAP` has **replaced** each `/Text` annot's `/Rect` with a 20×20
  box anchored at the original bottom-left (`cpdf_generateap.cpp:1237-1241`).
  Confirmed in the golden store: `(234.372, 340.046, 254.372, 360.046)`.
- `GenerateInkAP` has **inflated** each `/Ink` annot's `/Rect` by half the
  border width (:1205-1207).
- Every generated annot has gained `/AP /N` (a reference to a new stream) and
  the marker key `PDFIUM_HasGeneratedAP` (a Boolean `true`,
  `cpdf_annot.cpp:37`, `:153`).
- Popup annots that were *in the file* were skipped by the list, so they are
  untouched — but they **are** dumped.
- Synthesized popups were appended to the list, not to `/Annots`, so they are
  **not** dumped.

Anything the tool then reports about `/Rect` or `/AP` reflects that state.
This is why the brief insists (D1) that our AP generation return an
**overlay** the whole crate reads through, rather than a mutation.

`ShouldGenerateAP` (`cpdf_annot.cpp:157-167`) is the gate:

```
if let Some(ap) = dict[/AP] as dict { if ap[/N] as dict is Some { return false } }
!is_hidden()
```

Note `ap[/N]` must be a **Dict** to suppress generation — an `/AP /N` that is
a *stream* (the overwhelmingly common case!) does **not** suppress it. Read
again: `GetDictFor` accepts a stream's dict (object brief §1.6), so a stream
`/N` *does* return a dict here and generation is suppressed. A `/N` that is a
plain dict of states (checkbox `<< /Yes 5 0 R /Off 6 0 R >>`) also suppresses.
Only a missing/scalar `/N`, or a missing `/AP`, allows generation. Plus:
hidden annots never generate.

`GenerateAPIfNeeded` also sets `has_generated_ap_`, which feeds
`RectForDrawing`'s text-markup branch (§1.10.2) — so a Highlight that had no
AP now draws at its quadpoint bounding box instead of its `/Rect`.

#### 1.12.2 The exact output format

```
Number of annotations: %d\n
\n
<for i in 0..count:>
Annotation #%d:\n                        // i + 1
  <if annot is null:>  Failed to retrieve annotation!\n\n   and continue
Subtype: %s\n                            // the AnnotSubtypeToCString table
Flags set: %s\n                          // comma-space joined, in bit order; empty allowed
Number of objects: %d\n
  <if > 0:>  Object types: %s  %s  ...\n // each followed by TWO spaces
Color in RGBA: %d %d %d %d\n             // or "Failed to retrieve color.\n"
Interior color in RGBA: %d %d %d %d\n    // or "Failed to retrieve interior color.\n"
Content: %ls\n
Author: %ls\n
  <if has_attachment_points:>
Number of quadpoints sets: %zu\n
    <per set:> Quadpoints set #%zu: (%.3f, %.3f), (%.3f, %.3f), (%.3f, %.3f), (%.3f, %.3f)\n
Rectangle: l - %.3f, b - %.3f, r - %.3f, t - %.3f\n\n
  <or on failure:> Failed to retrieve annotation rectangle.\n     // ONE newline
```

Per-line semantics:

- **count** = `/Annots` array length, or 0 if absent
  (`fpdf_annot.cpp:427-435`). A malformed entry still counts.
- **annot null** when `/Annots[i]` does not resolve to a dict (`:437-459`) —
  `bad_annots_entry.pdf` gives count 1 with a null annot.
- **Subtype** goes through `AnnotSubtypeToCString` (`write.cc:53-131`), which
  **`NOTREACHED()`s on `FPDF_ANNOT_UNKNOWN` and on `Redact`** — the table has
  no arm for either. In practice an unknown subtype crashes the oracle; we
  emit `Unknown` and record a diagnostic (D13, and see **Escalation E3**:
  the golden store may contain no such file, in which case this is moot).
- **Flags** joined by `", "` in the bit order of the table in §1.10.2, first
  nine bits only.
- **Number of objects** = `FPDFAnnot_GetObjectCount` (`:653-669`): resolve the
  **Normal** AP through the full §1.10.3 ladder, and if there is a stream,
  parse it as a form and count its page objects. No stream → 0. This is the
  one place `--annot` needs `pdfrum-page`. **Object types** are
  `Text|Path|Image|Shading|Form` (`write.cc:167-183`), from
  `PageObject`'s discriminant.
- **Color** (`:762-828`): returns **false** — printing "Failed to retrieve
  color." — whenever `HasAPStream(dict)` (i.e. the Normal AP resolves to a
  stream). Otherwise `/C` (or `/IC` for the interior query) via
  `GetArrayFor`; alpha is `(dict.contains(/CA) ? dict.float(/CA) : 1) * 255`
  truncated to `unsigned int`. With **no** color array, the default is
  `(255,255,0)` for `Highlight` (matching `GenerateHighlightAP`) and
  `(0,0,0)` otherwise, and the call still returns **true**. With an array,
  `CFXColorFromArray` then:
  - `Rgb`: `R = c1*255`, etc. (truncating float→uint conversion)
  - `Gray`: all three = `255*c1`
  - `Cmyk`: `R = 255*(1-c)*(1-k)`, `G = 255*(1-m)*(1-k)`, `B = 255*(1-y)*(1-k)`
  - `Transparent` (a 0-, 2-, or 5-element array): `(0,0,0)`
  Note the C→uint conversion of a **negative** float is UB in C++ and in
  practice wraps; we saturate at 0 (D14, and no corpus file exercises it).
- **Content / Author** = `FPDFAnnot_GetStringValue(dict, "Contents" | "T")`
  = `dict.unicode_text(key)`, i.e. `decode_text` of the raw bytes with
  one level of resolution. Printed with `%ls` after the UTF-16 round-trip, so
  the same truncation caveat as §1.7.3 applies.
- **has_attachment_points** (`:828-838`) = subtype ∈
  `{Link, Highlight, Underline, Squiggly, StrikeOut}` — **includes Link**,
  which is why every Link in the golden store prints
  `Number of quadpoints sets: 0`.
- **quadpoint count** = `/QuadPoints` array length / 8 (`:880-890`), where the
  array is found by `GetQuadPointsArrayFromDictionary` = plain
  `dict[/QuadPoints]` as an array.
- **`Quadpoints set #N`** prints the raw eight floats in file order
  (`:893-906` → `GetQuadPointsAtIndex`), **not** the derived rect.
- **Rectangle** is `dict.rect(/Rect)` **unnormalized** (`:926-936`), and
  `FPDFAnnot_GetRect` returns false only when the dict is null — which cannot
  happen here since a null annot was already handled. So the failure branch is
  dead in practice; keep it for shape.
- `%.3f` on a `float` promoted to `double`: three decimals, round-half-to-even
  in glibc. Our formatter must match; `format!("{:.3}", x as f64)` in Rust
  rounds half away from zero on ties. Ties at the third decimal of a `float`
  are essentially unreachable (a `float` rarely lands exactly on a half-milli
  boundary), but §4 pins a snapshot test over the whole golden corpus rather
  than trusting that argument (**Escalation E4** if it fails).
- The trailing `\n\n` after `Rectangle:` means each annotation block ends with
  a blank line, and the file therefore ends with `\n\n`. The header is
  `...\n\n` too. A file with zero annotations is exactly
  `"Number of annotations: 0\n\n"`.

### 1.13 Appearance-stream generation — `cpdf_generateap.cpp`

1664 lines producing content-stream bytes. Ten per-subtype generators plus the
three form-field generators, over a shared set of emitters.

#### 1.13.1 The two float formatters — READ THIS FIRST

`cpdf_generateap.cpp` emits floats **two different ways**, and which one a
given call site uses is not derivable from context:

**(A) `WriteFloat` / `WritePoint` / `WriteRect`**
(`core/fpdfapi/edit/cpdf_contentstream_write_utils.cpp:29-140`) — dragonbox
shortest round-trip, **never scientific notation**, with these rules:

- `NaN` → `"0"`; `±0.0` → `"0"`.
- `+INFINITY` → `FLT_MAX`; `-INFINITY` → `-FLT_MAX` (then formatted normally).
- Negative → a `-` then the magnitude.
- `to_decimal(v, sign::ignore, trailing_zero::remove)` gives
  `(significand, exponent)`; digits are emitted most-significant-first.
- `exponent >= 0` → all digits then `exponent` zeros.
- `exponent < 0` and `places_before = ndigits + exponent > 0` →
  `places_before` digits, `.`, the rest.
- `exponent < 0` and `places_before <= 0` → `.`, `-places_before` zeros, then
  the digits — **so there is no leading `0`**: `0.5` prints as `.5`,
  `0.25` as `.25`. This is the single most visible difference from Rust's
  `{}`.
- Buffer cap 49 bytes; the leading-zeros branch breaks out when
  `out_idx + 1 >= 49`, truncating the tail of a denormal.

`WriteRect(stream, r)` emits `left ' ' bottom ' ' Width() ' ' Height()` —
**x, y, w, h**, not x0 y0 x1 y1. Every `re` in this file is therefore already
in operator order; a `WriteRect` followed by anything else is still w/h.

**(B) bare `stream << float`** — C++ `std::ostream` default: `%g`-like with
**6 significant digits**, scientific notation for exponents `< -5` or `>= 6`,
and trailing zeros removed. `0.5` → `0.5`; `1.0` → `1`;
`123456789.0` → `1.23457e+08`; `0.0000001` → `1e-07`.

In Rust, (A) is `ryu`-shortest post-processed to strip a leading `0` before
`.` and to force fixed notation (DEPS.md already has `ryu`; the C++ uses
dragonbox and SPEC §11 already commits to "ryu shortest, matching dragonbox
behavior"). (B) needs a **hand-written 6-significant-digit `%g`** — about 40
lines, and STYLE §5 says write the 40 lines rather than add a crate.

**Which sites use which** (this table is the implementation checklist):

| emitter / site | line | formatter |
|---|---|---|
| `StringFromFontNameAndSize` — the `Tf` size | :102 | **A** |
| `GenerateColorAP` — all components | :416-433 | **A** |
| `GenerateBorderAP` — the rects, points, and `w` width | :461-533 | **A** |
| `GenerateBorderAP` — the dash `[d g] p` integers | :474-475 | integers, `<<` on `int32_t` |
| `GetDashPatternString` — each dash element | :593 | **A** |
| `GenerateEditAP` — every `Td` point | :352-388 | **A** |
| `GenerateComboBoxAP` — the clip `re`, the button `re` | :863, :873 | **A** |
| `GenerateComboBoxAP` — the triangle points | :888-891 | **A** |
| `GenerateListBoxAP` — the selection `re` | :957 | **A** |
| `GenerateFreeTextAP` — background/body rects | :1101, :1132 | **A** |
| `GenerateFormAP` — background/body rects | :1495, :1545, :1564 | **A** |
| `GenerateTextSymbolAP` — all points | :700-719 | **A** |
| `GenerateTextSymbolAP` — the `1 w` width | :684 | integer |
| **`GenerateCircleAP`** — border width, all Bézier coords | :994-1035 | **B** ← |
| **`GenerateSquareAP`** — border width, the `re` operands | :1339-1355 | **B** ← |
| **`GenerateHighlightAP`** — every quad coordinate | :1168-1170 | **B** ← |
| **`GenerateUnderlineAP`** — every coordinate | :1270-1271 | **B** ← |
| **`GenerateSquigglyAP`** — every coordinate | :1387-1401 | **B** ← |
| **`GenerateStrikeOutAP`** — every coordinate | :1434-1435 | **B** ← |
| **`GenerateInkAP`** — border width and every coordinate | :1200-1220 | **B** ← |
| **`GeneratePopupAP`** — border width and the `re` operands | :1292-1299 | **B** ← |

The pattern: the **newer** code (form fields, free text, borders, colors, the
text symbol) was converted to `WriteFloat`; the **older** per-subtype markup
generators were not. Six of the ten `GenerateAnnotAP` cases are formatter **B**.
Note `GenerateSquareAP` and `GeneratePopupAP` also emit `re` as
`left bottom Width() Height()` by hand — same operand order as `WriteRect`,
different formatter.

#### 1.13.2 Colors

`GenerateColorAP(color, op)` (:412-439):

| color | fill output | stroke output |
|---|---|---|
| `Rgb(r,g,b)` | `<r> <g> <b> rg\n` | `<r> <g> <b> RG\n` |
| `Gray(g)` | `<g> g\n` | `<g> G\n` |
| `Cmyk(c,m,y,k)` | `<c> <m> <y> <k> k\n` | `<c> <m> <y> <k> K\n` |
| `Transparent` | `""` (empty) | `""` |

Each component is followed by a **single space**, then the operator, then `\n`.
Formatter **A**. The empty-string return for `Transparent` is load-bearing:
every caller tests `len > 0` to decide whether to emit the surrounding `q`/`Q`
or the path at all.

`GetColorStringWithDefault(array, default, op)` (:541-550): an array present
(even a 0- or 2-element one, which yields `Transparent`) uses
`CFXColorFromArray`; absent uses the caller's default. So `/C []` produces
**no** color operator while `/C` absent produces the subtype's default.

#### 1.13.3 Border style

`GetBorderStyleInfo(bs_dict)` (:133-173) → `{ width: f32 = 1.0, style =
Solid, dash = (3, 0, 0) }`:

```
if bs.contains(/W) { width = bs.float(/W) }              // GetFloatFor: coerces, 0.0 default
match bs.byte_string(/S).first() {                       // FIRST BYTE ONLY, empty-safe
    b'S' => Solid,
    b'D' => Dash,
    b'B' => { Beveled;   width *= 2 }
    b'I' => { Inset;     width *= 2 }
    b'U' => Underline,
    _    => {}                                           // keeps Solid
}
if let Some(d) = bs[/D] as array { dash = (d.int_at(0), d.int_at(1), d.int_at(2)) }
```

Only the **first byte** of `/S` is examined, so `/S /Dotted` is `Dash` and
`/S /Beveled` doubles the width. Missing dash elements read as 0 via
`Array::int_at`. The `*= 2` for Beveled/Inset happens **before** any caller
sees the width, and `GenerateFormAP` then deflates the body rect by the
doubled width (:1508).

`GenerateBorderAP(rect, style_info, color)` (:441-539) — `width <= 0` returns
empty. `half = width / 2`. Per style, with `l/b/r/t` the raw rect fields:

**Solid**: only if the fill color string is non-empty —
```
<fill color>
<WriteRect(rect)> re\n
<WriteRect(rect.deflate(width, width))> re f*\n
```
i.e. an even-odd donut. `Deflate` normalizes first (§1.0.1).

**Dash**: only if the stroke color string is non-empty —
```
<stroke color>
<A:width> w [<dash> <gap>] <phase> d\n
<A:(l+half, b+half)> m\n
<A:(l+half, t-half)> l\n
<A:(r-half, t-half)> l\n
<A:(r-half, b+half)> l\n
<A:(l+half, b+half)> l S\n
```
Note `[` immediately after the space that `WriteFloat(width) << " w ["` emits,
the dash/gap/phase as bare integers with single spaces, and the final segment
using ` l S\n` (space-l-space-S).

**Beveled / Inset** — three parts, always emitted (no color gate on the first
two):
```
<fill GenerateColorAP(Gray(beveled ? 1.0 : 0.5))>
<A:(l+half, b+half)> m\n  <A:(l+half, t-half)> l\n  <A:(r-half, t-half)> l\n
<A:(r-width, t-width)> l\n  <A:(l+width, t-width)> l\n  <A:(l+width, b+width)> l f\n
<fill GenerateColorAP(Gray(beveled ? 0.5 : 0.75))>
<A:(r-half, t-half)> m\n  <A:(r-half, b+half)> l\n  <A:(l+half, b+half)> l\n
<A:(l+width, b+width)> l\n  <A:(r-width, b+width)> l\n  <A:(r-width, t-width)> l f\n
<if fill color non-empty:>
  <fill color>  <WriteRect(rect)> re\n  <WriteRect(rect.deflate(half, half))> re f*\n
```
The two grey values: Beveled = 1.0 top-left / 0.5 bottom-right; Inset =
0.5 / 0.75. **The final donut deflates by `half`, not `width`** — unlike
Solid.

**Underline**: only if the stroke color is non-empty —
```
<stroke color>  <A:width> w\n  <A:(l, b+half)> m\n  <A:(r, b+half)> l S\n
```

`GetBorderWidth(dict)` (:552-564) — the *annotation-level* width, distinct
from `GetBorderStyleInfo`: `/BS /W` if `/BS` exists **and has the key**, else
`/Border[2]` if `/Border` has **more than 2** elements, else `1`.

`GetDashArray(dict)` (:566-579): `/BS` with `/S == "D"` → `/BS /D`; else
`/Border` with **exactly 4** elements → `/Border[3]` as an array; else none.

`GetDashPatternString(dict)` (:581-598): none/empty → `""`; else
`"[" + each of the first min(len, 10) elements as `<A:f>` followed by a space
+ "] 0 d\n"` — note the trailing space before `]` and the hard-coded phase `0`.

#### 1.13.4 The `/DA` default-appearance string

`cpdf_defaultappearance.cpp`. The DA string is obtained by
`GetDefaultAppearanceString(annot_dict, acroform_dict)` (:22-34):
`field_attr(annot_dict, "DA")` (the §1.14.1 inherited walk) as a string; if
empty **and** an AcroForm dict was supplied, `acroform[/DA]` via
`GetByteStringFor`.

Parsing is by `CPDF_SimpleParser` + `FindTagParamFromStart(parser, token, n)`
(:38-73) — a ring buffer of `n+1` positions:

```
n += 1;  buf = vec![0u32; n];  buf_index = 0;  buf_count = 0;
parser.set_position(0);
loop {
    buf[buf_index] = parser.position();  buf_index = (buf_index + 1) % n;
    buf_count = min(buf_count + 1, n);
    let word = parser.next_word();
    if word.is_empty() { return false }                       // EOF
    if word == token {
        if buf_count < n { continue }                          // not enough preceding words
        parser.set_position(buf[buf_index]);  return true      // rewind to n words back
    }
}
```

`CPDF_SimpleParser::GetWord` (`cpdf_simple_parser.cpp:21-46`) is a *different*
tokenizer from the main lexer: it skips whitespace and `%`-comments
(:55-84), then dispatches on the first char — `/` scans to the next
whitespace-or-delimiter (:86-98); `<` returns `<<` or scans to `>` (:100-118);
`>` returns `>>` or `>` (:120-127); `(` scans with **paren nesting but no
backslash escapes** (:129-141); any other delimiter is a 1-char token; else
scan to the next delimiter/whitespace (:143-155). Returns an empty view at
EOF, and `HandleName` returns **empty** if it runs to EOF without a
terminator. No 256-byte cap (unlike the main lexer).

`GetFont()` (:88-105): `FindTagParamFromStart(&p, "Tf", 2)`; **on failure
returns `Some(FontNameAndSize::default())`** — an empty name and size 0, *not*
`None`. `None` is returned only when the DA string itself is empty. Then two
sequential `GetWord()` calls with a comment insisting on evaluation order:
`name = PDF_NameDecode(word.substr(1))` (dropping the leading `/`; a word that
does not start with `/` loses its first character!) and
`size = StringToFloat(word)`.

`GetColor()` (:115-139) tries three tokens **in this order**, first match wins:
`"g"` with 1 param → `Gray`; `"rg"` with 3 → `Rgb`; `"k"` with 4 → `Cmyk`.
None match → `None`. Because `FindTagParamFromStart` restarts at position 0
each time, a DA of `"0 g 1 0 0 rg"` yields **Gray(0)**, not Rgb.
`GetDefaultAppearanceInfo` (:261-274) then uses
`appearance.GetColor().value_or(CFX_Color())` — i.e. **`Transparent`** when
there is no colour operator, which `GenerateColorAP` renders as the empty
string, so the text gets whatever colour the enclosing stream left. Pinned:
`freetext_annotation_without_da.pdf` reports font colour `(0,0,0)` through
`FPDFAnnot_GetFontColor` (`fpdf_annot_embeddertest.cpp:2594`) — that API has
its own `(0,0,0)` default and does not contradict this.

`GetColorARGB()` (:141-172) converts for the API: Gray → `g = c1*255 + 0.5`
truncated; Rgb likewise per channel; Cmyk → `1 - min(1, c + k)` per channel,
then `*255 + 0.5` truncated. Not used by AP generation.

Pinned by `cpdf_defaultappearance_unittest.cpp:32-61` — eleven
`FindTagParamFromStart` cases including the final-position assertions, e.g.
`("Tj", "Tj", 1)` → false with position **2**, and
`("er^ 2 (34) (5667) Tj", "Tj", 2)` → true with position **5**.

#### 1.13.5 The variable-text layout engine — `cpvt_variabletext.cpp`, `cpvt_section.cpp`

This is what turns a field value into positioned glyphs. It is a small
paragraph engine: a document is a `Vec<Section>` (paragraphs), each section is
a `Vec<WordInfo>` (one entry **per character**) plus a `Vec<LineInfo>`
(computed).

**Coordinate space.** Everything internal is y-**down** from the plate's
top-left; `InToOut` (`cpvt_variabletext.cpp:971-973`) converts to PDF y-up:
`(x + plate.left, plate.top - y)`. `GetContentRect()` (:637-639) is
`InToOut(content_rect_)` which flips top/bottom (:979-984). So an internal
`fWordY` grows downward and a content rect's `Height()` is positive.

**Constants** (:28-33):

```
kFontScale   = 0.001               // 1000-unit font space
kReturnLength = 1                  // a section break counts as one "word" for indices
kFontSizeSteps = [4, 6, 8, 9, 10, 12, 14, 18, 20, 25, 30, 35, 40,
                  45, 50, 55, 60, 70, 80, 90, 100, 110, 120, 130, 144]   // 25 entries
```

**Metrics** (:645-695), all through the `Provider`:

```
word_width(w)  = char_width(font_index, sub_word ? sub_word : word) * font_size * 0.001 + word_tail
font_ascent(i, size)  = provider.type_ascent(i)  * size * 0.001
font_descent(i, size) = provider.type_descent(i) * size * 0.001
line_leading = 0.0                 // line_leading_ is never assigned anywhere
line_indent  = 0.0                 // GetLineIndent() returns a literal 0
```

`GetCharWidth` substitutes `sub_word` (the password `*`) for the real word
**for width purposes only** (:930-938). `line_leading_` and `GetLineIndent()`
being constant zero collapses a lot of arithmetic — keep the terms so the
formulas match the source, but note they are dead.

`Provider::GetCharWidth(i, word)` (:44-57): `font_map.font(i)?`,
`char_code_from_unicode(word)`, `kInvalidCharCode` → 0, else
`font.char_width(charcode)`. `GetTypeAscent/Descent` (:59-67) are the font's
`/Ascent`/`/Descent` (or the face-derived values — font brief §1.2). Note
these are **not** scaled per font index; index 1 is always absent for us
(§1.0), so effectively one font.

`Provider::GetWordFontIndex(word, charset, hint)` (:69-83): index 0 if the
default font maps the char, else index 1 if the system font does, else **−1**.
A −1 font index then flows into `GetCharWidth` → `font_map.font(-1)` → none →
width 0, and into `GetPDFWordString` → empty. Port the −1, do not clamp.

**`SetText`** (`cpvt_variabletext.cpp:300-345`) — the tokenizer:

```
delete everything;  wp = (0, 0, -1);  section[0].rect = zero
for i in 0..text.len() {
    if limit_char > 0 && count >= limit_char { break }
    if char_array > 0 && count >= char_array { break }
    let mut word = text[i];                    // a UTF-16 code unit (u16)
    match word {
        0x0D => if multi_line { if text[i+1] == 0x0A { i += 1 }  wp.advance_section(); add_section(wp) },
        0x0A => if multi_line { if text[i+1] == 0x0D { i += 1 }  wp.advance_section(); add_section(wp) },
        0x09 => { word = 0x20; fallthrough }
        _    => wp = insert_word(wp, word, Charset::Default),
    }
    count += 1;                                 // INCREMENTED EVEN FOR LINE BREAKS
}
```

Traps: CRLF **and** LFCR are both single breaks; a lone CR or LF in a
**single-line** field is consumed and produces *nothing* (not a space);
TAB becomes a space; the character counter advances for line breaks too, so
`/MaxLen 5` on `"ab\ncd"` admits `a b <break> c` and stops. Text is iterated as
**UTF-16 code units**, so a supplementary character becomes two independent
"words" (each a lone surrogate) with independent widths — D15 keeps this.

`InsertWord` (:222-239) re-checks both caps, then picks the font index:
`sub_word > 0 ? default_font_index() : word_font_index(word, charset, default)`
— i.e. a password field always uses font 0.

**`Rearrange`** (:835-847) is the entry point:

```
if !initialized { content_rect = zero; return }
if auto_font_size { set_font_size(auto_font_size());  range = whole document }
content_rect = rearrange_sections(range)
```

**Auto font size** (`GetAutoFontSize`, :849-877):

```
if plate_width() <= 0 { return 0 }
let span = if multi_line { &STEPS[..25/4] } else { &STEPS[..] };     // 25/4 == 6
let it = lower_bound(span, |size| !is_bigger(size));                 // first size that FITS
if it == end   { return span.last() }        // nothing fits -> the largest candidate
if it == begin { return span[0] }            // even the smallest fits -> smallest?!
span[it - 1]
```

Two things to get right. **Multi-line fields only ever consider the first six
steps `[4, 6, 8, 9, 10, 12]`** — a multi-line field can never auto-size above
12. And the `std::lower_bound` predicate is `!IsBigger(size)`, i.e. the
sequence is partitioned into "too big" then "fits"; `lower_bound` returns the
first *fitting* size, and the function then returns the one **before** it —
which is a size that does **not** fit. The `it == begin` guard returns
`span[0]` in that case. Net effect: the chosen size is the largest one that
*overflows*, except at the ends. This looks like an off-by-one, it is
pixel-visible, and it is the behavior — port it verbatim (D16).

`IsBigger(size)` (:879-891) accumulates `max(width)` and `sum(height)` over
`section.section_size(size)` and returns true as soon as either exceeds
`plate_width()` / `plate_height()` **using `is_float_bigger`** (epsilon 1e-4).

`RearrangeSections(range)` (:893-928) walks every section (not just the range),
laying them out top-to-bottom with a running `fPosY`; sections inside the range
are re-`Rearrange()`d, those after it keep their old height. The returned rect
is the union of all section rects, with `rcRet = rcSec` for `s == 0` (so a
document whose first section is empty still contributes its zero-width rect).

**Section layout** (`cpvt_section.cpp`). `Rearrange()` (:241-246) dispatches:
`char_array > 0` → `RearrangeCharArray()` (the comb field), else
`RearrangeTypeset()` = `line_array.clear(); OutputLines(SplitLines(true, 0.0))`.

**`SplitLines(typeset, font_size)`** (:542-681) — line breaking. When
`typeset` is false it only *measures* (used by `GetSectionSize` for auto-fit);
when true it also `AddLine`s.

Empty section (:543-562): one line with `nBeginWordIndex = nEndWordIndex = -1`,
width 0, ascent/descent from the **default font at the current size** (typeset)
or at `font_size` (measure); returns a rect of height
`line_leading + ascent - descent`.

The main loop, with `typeset_width = max(plate_width - line_indent, 0)`:

```
i = 0;  bOpened = false;  bFullWord = false;  nCharIndex = 0;  nLineFullWordIndex = 0
nLineHead = 0;  fLineWidth = 0;  fLineAscent = 0;  fLineDescent = 0
while i < total {
    let word = &words[i];  let old = if i > 0 { &words[i-1] } else { word };
    fLineAscent  = max(fLineAscent,  ascent(word));
    fLineDescent = min(fLineDescent, descent(word));
    fWordWidth   = width(word);
    if !bOpened {
        if is_open_style_punctuation(word) { bOpened = true; bFullWord = true }
        else if need_division(old.word, word.word) { bFullWord = true }
    } else if !is_space(word) && !is_open_style_punctuation(word) { bOpened = false }
    if bFullWord {
        bFullWord = false;
        if nCharIndex > 0 { nLineFullWordIndex += 1 }
        nWordStartPos = i;
        fBackupLineWidth = fLineWidth; fBackupLineAscent = fLineAscent; fBackupLineDescent = fLineDescent;
    }
    nCharIndex += 1;
    if auto_return && typeset_width > 0 && fLineWidth + fWordWidth > typeset_width {
        if nLineFullWordIndex > 0 {                     // rewind to the last word boundary
            i = nWordStartPos;
            fLineWidth = fBackupLineWidth; fLineAscent = fBackupLineAscent; fLineDescent = fBackupLineDescent;
        }
        if nCharIndex == 1 { fLineWidth = fWordWidth; i += 1 }   // a single char wider than the line
        nLineTail = i - 1;
        if typeset { add_line(nLineHead, nLineTail, fLineWidth, fLineAscent, fLineDescent) }
        fMaxY += fLineAscent + line_leading;  fMaxY -= fLineDescent;
        fMaxX = max(fLineWidth, fMaxX);
        nLineHead = i;
        fLineWidth = 0; fLineAscent = 0; fLineDescent = 0;
        nCharIndex = 0; nLineFullWordIndex = 0; bFullWord = false;
    } else {
        fLineWidth += fWordWidth;  i += 1;
    }
}
if nLineHead <= total - 1 { /* flush the tail line the same way */ }
return FloatRect(0, 0, fMaxX, fMaxY)
```

Notes: `fLineAscent` starts at **0**, not at the font's ascent, so a line's
ascent is `max(0, per-word ascents)` — a font with a negative ascent yields 0.
`fLineDescent` starts at 0 and is a `min`, so it is ≤ 0. The `i` is *not*
incremented on the wrap branch (except in the `nCharIndex == 1` case), which is
what makes the rewind work. `bFullWord` is evaluated **before** `nCharIndex`
increments, so the very first character of a section never sets
`nLineFullWordIndex`.

**The character classifier** (`cpvt_section.cpp:25-174`) — a 128-entry bit
table plus range tests:

```
kSpecialChars[128], bits: 0x01 = Latin, 0x04 = OpenStylePunctuation,
                          0x08 = Punctuation, 0x20 = ConnectiveSymbol
```

The table is transcribed verbatim (:25-37). Predicates:

- `IsLatin(w)`: `w <= 0x7F` → bit 0x01; else the ranges
  `00C0–00FF, 0100–024F, 1E00–1EFF, 2C60–2C7F, A720–A7FF, FF21–FF3A, FF41–FF5A`.
- `IsDigit(w)`: `0030–0039`.
- `IsCJK(w)`: `1100–11FF, 2E80–2FFF, 3040–9FBF, AC00–D7AF, F900–FAFF,
  FE30–FE4F, 20000–2A6DF, 2F800–2FA1F`; plus, within `3000–303F`, the explicit
  set `{3005,3006,3021..3029,3031..3035}`; plus `FF66–FF9D`.
- `IsPunctuation(w)`: `<= 0x7F` → bit 0x08; `0080–00FF` → an explicit set that
  **contains the bug `word <= 0x0094`** (:84) where every other term is
  `word ==`, so *every* code point from 0x80 to 0x94 is punctuation;
  `2000–206F`, `3000–303F`, `FE50–FE6F`, `FF00–FFEF` → explicit sets.
- `IsConnectiveSymbol(w)`: `<= 0x7F` and bit 0x20 — from the table, only
  `0x06` (ACK) has that bit. Effectively dead.
- `IsOpenStylePunctuation(w)`: `<= 0x7F` → bit 0x04; else
  `{300A,300C,300E,3010,3014,3016,3018,301A,FF08,FF3B,FF5B,FF62}`.
- `IsCurrencySymbol(w)`: `{0024,0080,00A2,00A3,00A4,00A5, 20A0–20CF,
  FE69,FF04,FFE0,FFE1,FFE5,FFE6}`. `IsPrefixSymbol` = that ∪ `{2116}`.
- `IsSpace(w)`: `{0020, 3000}`.

`NeedDivision(prev, cur)` (:150-174) — **the line-break opportunity test**,
in order:

```
if (Latin(prev)||Digit(prev)) && (Latin(cur)||Digit(cur)) { false }   // inside a word
if Space(cur) || Punctuation(cur)                          { false }
if Connective(prev) || Connective(cur)                     { false }
if Space(prev) || Punctuation(prev)                        { true }   // after a space
if PrefixSymbol(prev)                                      { false }  // "$" binds right
if PrefixSymbol(cur) || CJK(cur)                           { true }
if CJK(prev)                                               { true }
false
```

Transcribe the table and the predicates mechanically; the `word <= 0x0094`
line is not a typo to fix (D17).

**`RearrangeCharArray`** (:462-535) — comb fields (`/Ff` bit 24). Each of the
`char_array` cells is `plate_width / char_array` wide; the alignment decides a
starting cell offset:

```
node = plate_width / max(char_array, 1)
y = line_leading + font_ascent(default, size)
match alignment {
    Left   => line.x = node * 0.5,
    Center => { start = (char_array - words.len()) / 2;  line.x = node*start - node*0.5 }
    Right  => { start =  char_array - words.len();       line.x = node*start - node*0.5 }
}
for w in 0..min(words.len(), char_array) {
    next_width = words.get(w+1).map(|n| { n.word_tail = 0; width(n) }).unwrap_or(0);
    words[w].word_tail = 0;
    let ww = width(words[w]);
    x = node * (w + start + 0.5) - ww * 0.5;              // computed in DOUBLE
    words[w].x = x;  words[w].y = y;
    if w == 0 { line.x = x }                              // overrides the alignment value
    words[w].word_tail = if w != words.len()-1 { max(node - (ww + next_width)*0.5, 0) } else { 0 };
    x += ww;
    line_ascent = max(line_ascent, ascent(w));  line_descent = min(line_descent, descent(w));
}
line = { begin: 0, end: words.len()-1, y, width: x - line.x, ascent, descent }
return FloatRect(0, 0, x, y - line_descent)
```

The `(float)(fNodeWidth * (w + nStart + 0.5) - fWordWidth * 0.5f)` is
explicitly a `double` computation cast to `float` (:510) — do the arithmetic in
`f64` and cast once, or the last cell drifts. `word_tail` is written back into
the word and then read by `GetWordWidth`, so it is state, not a local. The
`if w == 0 { line.x = x }` makes the alignment-derived `line.x` dead whenever
there is at least one word.

`RearrangeCharArray` also **ignores `line_array` beyond the first entry** and
requires it to be non-empty (:463-465) — `Initialize()` guarantees that.

**`OutputLines(rect)`** (:683-768) — final placement and the bidi pass:

```
let typeset_width = max(plate_width - line_indent, 0);
let offset = |w| match alignment { Left => 0, Center => (typeset_width - w)*0.5, Right => typeset_width - w };
let fMinX = offset(rect.width());   let fMaxX = fMinX + rect.width();
let fMinY = 0.0;                    let fMaxY = rect.height();
let bidi = if !words.is_empty() { BidiResolver::new(words.iter().map(|w| w.word as u16).collect(), direction) } else { None };
let mut fPosY = 0.0;
for line in &mut lines {
    let mut fPosX = offset(line.width) + line_indent;
    fPosY += line_leading;  fPosY += line.ascent;
    line.x = fPosX - fMinX;  line.y = fPosY - fMinY;
    if line.begin < 0 { fPosY -= line.descent; continue }        // the empty-section line
    let len = line.end - line.begin + 1;                         // end is INCLUSIVE
    let mut runs = bidi.visual_runs(line.begin, len);
    if runs.is_empty() { runs = vec![(line.begin, len, false)] }  // fallback: one LTR run
    for (start, length, is_rtl) in runs {
        let span = words[start .. start+length];                 // clamped
        for w in if is_rtl { span.rev() } else { span } {
            w.is_rtl = is_rtl;  w.x = fPosX - fMinX;  w.y = fPosY - fMinY;  fPosX += width(w);
        }
    }
    fPosY -= line.descent;
}
FloatRect(fMinX, fMinY, fMaxX, fMaxY)
```

The alignment offset is computed **twice with different widths** — once from
the whole section's width (for `fMinX`) and once per line (for `fPosX`) — and
the per-word x is `fPosX - fMinX`, so for Center/Right alignment the two
partially cancel and short lines shift relative to the section box. That is
the behavior.

`GetWordRangeSpan(start, length)` (:798-805) clamps
`end = clamp(start+length, 0, size)` then `begin = clamp(start, 0, end)`, so
an out-of-range bidi run yields an empty span rather than a panic.

**The bidi dependency.** `CFX_BidiResolver` (`core/fxcrt/cfx_bidi_resolver.*`)
wraps **ICU's `ubidi_*`**: `ubidi_setPara` over the whole section's code units
with `UBIDI_DEFAULT_LTR` / `UBIDI_LTR` / `UBIDI_RTL`, then per line
`ubidi_setLine` + `ubidi_countRuns` + `ubidi_getVisualRun`. This is
`CPVT_VariableText::text_direction_`, which **nothing in `core/` ever sets**
(`SetTextDirection` has no caller outside tests), so it is always `kAuto` =
`UBIDI_DEFAULT_LTR`. See **Escalation E5** — this is a dependency-set question,
not a behavior question.

Failure modes to match: `Create` returns null for empty text (so `bidi` is
`None` and the `CHECK(bidi_resolver)` at :744 would fire — but a section with
zero words takes the `line.begin < 0` path first, so it is unreachable);
`GetVisualRunsForLine` returns an empty vector for `line_start < 0`,
`line_length <= 0`, an overflowing or out-of-range end, or `run_count <= 0`,
and the caller then falls back to one LTR run. `cpvt_section_unittest.cpp:308`
pins that fallback with `U+001D` (Bidi_Class=B), for which ICU reports zero
runs.

Pinned orderings (`cpvt_section_unittest.cpp:39-47`, stub font, every char
width 10):

| content | direction | visual order (indices into the logical array) |
|---|---|---|
| `A_B_C_ℵ_ℶ` | Auto | `0 1 2 3 4 5 8 7 6` |
| same | LTR | `0 1 2 3 4 5 8 7 6` |
| same | RTL | `8 7 6 5 0 1 2 3 4` |
| `ℵ_ℶ_ℷ_A_B` | Auto | `6 7 8 5 4 3 2 1 0` |
| same | LTR | `4 3 2 1 0 5 6 7 8` |
| same | RTL | `6 7 8 5 4 3 2 1 0` |

And `cpvt_variabletext_unittest.cpp:44-142` pins `CaretX` for `"hello"` at
`0.1 .. 0.5` and for `שלום` at `0.3, 0.2, 0.1, 0.0` (stub widths make each
char 0.1 wide at size 10 with `char_width` 10 — note `10 * 10 * 0.001 = 0.1`).

#### 1.13.6 `GenerateEditAP` — laid-out text to `Td`/`Tf`/`Tj`

`cpdf_generateap.cpp:315-410`. Walks the layout iterator emitting the smallest
text-positioning stream it can. `use_continuous_formatting` groups consecutive
same-line words into one `Tj`; it is **bypassed per-word for RTL words**
regardless of the flag.

```
let (mut old_pt, mut new_pt) = (Point::ZERO, Point::ZERO);
let mut cur_font = -1i32;   let mut words = Vec::new();
let (mut edit, mut line) = (String::new(), String::new());
let mut oldplace = WordPlace::default();          // (-1, -1, -1)
it.set_at(0);
while it.next_word() {
    let place = it.word_place();
    let (has_word, word) = it.get_word();
    let is_rtl = has_word && word.is_rtl;
    if continuous && !is_rtl {
        if place.line_cmp(oldplace) != 0 {                       // NEW LINE
            if !words.is_empty() { line += &render(words); edit += &line; line.clear(); words.clear() }
            new_pt = if has_word { word.location + offset }
                     else if let Some(l) = it.get_line() { l.pt + offset } else { new_pt };
            if new_pt != old_pt { line += &format_point(new_pt - old_pt) + " Td\n"; old_pt = new_pt }
        } else if words.is_empty() && has_word {                 // first word of a run
            new_pt = word.location + offset;
            if new_pt != old_pt { line += &format_point(new_pt - old_pt) + " Td\n"; old_pt = new_pt }
        }
        if has_word {
            if word.font_index != cur_font {
                if !words.is_empty() { line += &render(words); words.clear() }
                line += &font_set(word.font_index, word.font_size);
                cur_font = word.font_index;
            }
            words += &pdf_word_string(cur_font, word.word, sub_word);
        }
    } else {                                                     // per-word path
        if !words.is_empty() { line += &render(words); edit += &line; line.clear(); words.clear() }
        if has_word {
            new_pt = word.location + offset;
            if new_pt != old_pt { edit += &format_point(new_pt - old_pt) + " Td\n"; old_pt = new_pt }
            if word.font_index != cur_font { edit += &font_set(word.font_index, word.font_size); cur_font = word.font_index }
            edit += &render(pdf_word_string(cur_font, word.word, sub_word));
        }
    }
    oldplace = place;
}
if !words.is_empty() { line += &render(words); edit += &line }
edit
```

Details that change bytes:

- **`Td` is relative** (`new_pt - old_pt`) and is emitted only when the delta
  is non-zero — compared as exact `CFX_PointF` equality, i.e. bitwise-ish
  float `==` on both components. Formatter **A** via `WritePoint`.
- The non-continuous branch writes `Td` and the font directly into `edit`,
  while the continuous branch writes into `line` and flushes `line` into
  `edit` only at line boundaries. When a font change occurs mid-line in the
  continuous branch, the pending `words` are flushed to `line` but `line` is
  **not** flushed to `edit` — so ordering is preserved but the buffering
  differs. Reproduce the buffering exactly; the concatenation order is
  observable when a line ends immediately after a font change.
- `word.font_index != cur_font` in the *non*-continuous branch emits the font
  **after** the `Td`, in the continuous branch **before** the word text but
  possibly after a `Td` from the same iteration.
- `it.set_at(0)` is `SetAt(int32)` → `WordIndexToWordPlace(0)`
  (`cpvt_variabletext.cpp:382-411`), which for index 0 returns
  `GetBeginWordPlace()` = `(0, 0, -1)` when the first section is non-empty, or
  the end place when the document is empty. `NextWord` then advances before
  the first `GetWord`, so word index 0 is the first character.
- `place.LineCmp(oldplace)` (`cpvt_wordplace.h`) compares `(nSecIndex,
  nLineIndex)` lexicographically. `oldplace` starts as `(-1,-1,-1)`, so the
  first iteration always takes the "new line" branch.

**`GetPDFWordString(font_map, font_index, word, sub_word)`** (:60-88):

```
if sub_word > 0 { return format!("{}", sub_word as u8 as char) }   // "%c" — TRUNCATES to a byte
let font = font_map?.font(font_index)?;                             // else ""
if font.base_font_name() == "Symbol" || == "ZapfDingbats" {
    return format!("{}", word as u8 as char)                        // "%c" again — byte truncation
}
let cc = font.char_code_from_unicode(word);
if cc == INVALID { return "" }
font.append_char(&mut out, cc);  out
```

`ByteString::Format("%c", u16)` writes **one byte** — the low byte after the
default argument promotion. So a Symbol/ZapfDingbats word above U+00FF is
silently truncated. This is the *only* "ZapfDingbats glyph" handling in
`core/`: the check is on the **base font name**, and it emits the raw code
point as a byte, relying on the font's built-in encoding. There is no
named-glyph table and no `/Encoding` consultation. (The named-glyph checkbox
shapes the task description refers to live in `cpdfsdk_appstream.cpp` and are
**paths**, not glyphs — §1.15 and Escalation E1.)

`AppendChar` (`cpdf_font.cpp:98-100`) is a single byte for simple fonts; for
CID fonts it is `CPDF_CMap::AppendChar` (`cpdf_cmap.cpp:448-480`): OneByte → 1
byte; TwoBytes → big-endian 2; MixedTwoBytes → 1 byte if `< 0x100` and not a
leading byte, else 2; MixedFourBytes → zero-padded to the codespace size.

**`GetWordRenderString(words)`** (:90-95): empty → `""`; else
`PDF_EncodeString(words) + " Tj\n"`. `PDF_EncodeString`
(`fpdf_parser_decode.cpp:628-649`) wraps in `(...)`, maps `0x0A → \n`,
`0x0D → \r`, backslash-escapes `(`, `)`, `\`, and passes every other byte —
including NUL and 8-bit bytes — through **verbatim**.

**`GetFontSetString(font_map, index, size)`** (:107-115) →
`StringFromFontNameAndSize(font_map.alias(index), size)` (:97-105), which emits
`"/" + name + " " + <A:size> + " Tf\n"` **only when the name is non-empty and
the size is > 0**; otherwise the empty string. A size of 0 (auto-size that
resolved to 0 because the plate had no width) therefore emits no `Tf` at all,
and the text inherits whatever font the surrounding stream set.

`SetVtFontSize(size, vt)` (:117-123): `is_float_zero(size)` → `set_auto_font_size(true)`;
else `set_font_size(size)`. A **negative** size is not zero, so it is used
directly — `text_form_negative_fontsize.pdf` keeps `-12.0`
(`fpdf_annot_embeddertest.cpp:2577`), and `StringFromFontNameAndSize`'s
`size > 0` gate then suppresses the `Tf`.

#### 1.13.7 The ExtGState, resources, and stream assembly

`GenerateExtGStateDict(annot_dict, blend_mode)` (:726-743) builds
`<< /GS << /Type /ExtGState /CA <op> /ca <op> /AIS false /BM <blend> >> >>`
where `op = if dict.contains(/CA) { dict.float(/CA) } else { 1 }`. Both the
stroke and fill alpha get the same value. The wrapper dict's single key is
`"GS"` (`kGSDictName`, :47), which is why every generator starts with
`/GS gs`.

`GenerateResourcesDict(gs, font)` (:745-757) assembles
`<< /ExtGState <gs> /Font <font> >>`, omitting either when absent.
`GenerateResourceFontDict(name, objnum)` (:659-667) is `<< <name> <ref> >>`.

`GenerateAndSetAPDict(doc, dict, stream, resources, is_text_markup)`
(:759-782):

```
stream_dict = << /FormType 1 /Type /XObject /Subtype /Form
                 /Matrix [1 0 0 1 0 0]
                 /BBox <if is_text_markup { bounding_rect_from_quad_points(dict) }
                        else { dict.rect(/Rect) }>
                 /Resources <resources> >>
new_stream = doc.new_indirect(stream_dict);  new_stream.set_data(stream);
dict.get_or_create(/AP)[/N] = Ref(new_stream)
```

`SetDataFromStringstream` (`cpdf_stream.cpp:144-150`) writes the bytes and sets
`/Length`; **an empty stream writes zero bytes** (`tellp() <= 0` → `SetData({})`).
`SetMatrixFor` writes the six numbers with the object model's own float
formatting (SPEC §11's `fmt_number`), not either of §1.13.1's formatters —
this is a *dictionary* value, not stream content.

Note the `/BBox` is the annot's `/Rect` **as written at that moment**, which
for `GenerateTextAP` and `GenerateInkAP` is the rect they just rewrote.

`GenerateEmptyAP` (:1585-1593) produces a valid form XObject with a zero-length
stream — used by `fpdfsdk` only.

#### 1.13.8 The ten `GenerateAnnotAP` cases

`GenerateAnnotAP(doc, dict, subtype)` (:1596-1623) dispatches on subtype;
everything not listed returns **false** (no AP generated):

| subtype | generator | line | markup? | blend |
|---|---|---|---|---|
| Circle | `GenerateCircleAP` | :977 | no | Normal |
| FreeText | `GenerateFreeTextAP` | :1047 | no | Normal |
| Highlight | `GenerateHighlightAP` | :1151 | **yes** | **Multiply** |
| Ink | `GenerateInkAP` | :1182 | no | Normal |
| Popup | `GeneratePopupAP` | :1282 | no | Normal |
| Square | `GenerateSquareAP` | :1323 | no | Normal |
| Squiggly | `GenerateSquigglyAP` | :1365 | **yes** | Normal |
| StrikeOut | `GenerateStrikeOutAP` | :1415 | **yes** | Normal |
| Text | `GenerateTextAP` | :1233 | no | Normal |
| Underline | `GenerateUnderlineAP` | :1252 | **yes** | Normal |

Every one begins `"/GS gs "` — **with a trailing space, not a newline**
(except `GeneratePopupAP`, which uses `"/GS gs\n"`, :1284). All use formatter
**B** except FreeText.

**Circle** (:977-1045):
```
/GS gs <fill: /IC or Transparent> <stroke: /C or Rgb(0,0,0)>
<if border_width > 0:>  <B:width> w <dash pattern string>
```
then the rect: `dict.rect(/Rect).normalize()`, deflated by `width/2` on both
axes when stroking. With `kL = 0.5523f` (:1014) and
`dx = kL * rect.Width() / 2.0`, `dy = kL * rect.Height() / 2.0` (**computed in
`double` then narrowed**, :1015-1016):
```
<mx> <top> m\n
<mx+dx> <top> <right> <my+dy> <right> <my> c\n
<right> <my-dy> <mx+dx> <bottom> <mx> <bottom> c\n
<mx-dx> <bottom> <left> <my-dy> <left> <my> c\n
<left> <my+dy> <mx-dx> <top> <mx> <top> c\n
<paint op>\n
```
`paint op` = `GetPaintOperatorString(stroke, fill)` (:669-674):
`b` (stroke+fill), `s` (stroke only), `f` (fill only), `n` (neither).
`is_fill = /IC exists && !/IC.is_empty()` — an `/IC []` is "present but empty"
so no fill; `/IC [0 0 0]` fills black.

**Square** (:1323-1363): identical preamble and rect handling, then
```
<left> <bottom> <Width()> <Height()> re <paint op>\n
```
Note the single ` re ` with spaces and the paint operator on the same line.

**Highlight** (:1151-1180): `/GS gs ` then `GetColorStringWithDefault(/C,
Rgb(1,1,0), Fill)`. Per quad `i` (rect normalized):
```
<l> <t> m <r> <t> l <r> <b> l <l> <b> l h f\n
```
all on one line, then the ExtGState with blend **Multiply** and
`is_text_markup = true` (so the BBox is the quadpoint bounding box).

**Underline** (:1252-1280): stroke default `Rgb(0,0,0)`; if `/QuadPoints`
exists, first `1 w ` (an integer via `<<`), then per quad
```
<l> <b+1> m <r> <b+1> l S\n
```

**Squiggly** (:1365-1413): stroke default black; `1 w ` then per quad, with
`kDelta = 2`, `top = b + 2`, `bottom = b`:
```
<l> <top> m
<x> <top|bottom> l    for x = l+2, l+4, ... while x < right, alternating starting at bottom
<right> <bottom+rem | top-rem> l S\n
```
where `rem = right - (x - 2)` after the loop and the branch is on the final
`isUpwards`. Everything on one line per quad. The loop condition is `x <
rect.right` on floats, so a quad of width < 2 emits only the `m` and the final
segment.

**StrikeOut** (:1415-1444): stroke default black; per quad,
`y = (top + bottom) / 2` and
```
1 w <l> <y> m <r> <y> l S\n
```
Note the `1 w` is emitted **inside** the per-quad loop here, unlike Underline
and Squiggly.

**Ink** (:1182-1231): returns **false** (no AP at all) when `/InkList` is
absent or empty, or when `GetBorderWidth() <= 0`. Then
```
/GS gs <stroke: /C or Rgb(0,0,0)> <B:width> w <dash string>
```
and **mutates `/Rect`**: `rect.inflate(width/2, width/2)` written back
(:1205-1207). Per sub-array of `/InkList` with `len >= 2`:
```
<x0> <y0> m <x0> <y0> l <x1> <y1> l ... S\n
```
— note the first point is emitted **twice**, once as `m` and again as the
first `l`, because the `l` loop starts at `j = 0` (:1218). A sub-array of odd
length drops nothing: the loop runs `j < len-1` stepping 2, so an odd trailing
element is skipped. Sub-arrays with `len < 2` are skipped entirely.

**Text** (:1233-1250): **rewrites `/Rect`** to
`(left, bottom, left + 20, bottom + 20)` using the *raw, unnormalized* rect's
left/bottom (:1237-1241), then emits `GenerateTextSymbolAP(note_rect)`.

`GenerateTextSymbolAP(rect)` (:676-724) — the sticky-note icon:
```
<fill Rgb(1,1,0)> <stroke Rgb(0,0,0)> 1 w\n
```
then with `half = 0.5`, `tip = 4`:
```
outer1 = rect.deflate(0.5, 0.5);  outer1.bottom += 4
outer2 = outer1;  outer2.left += 4;  outer2.right = outer2.left + 4;
                  outer2.top = outer2.bottom - 4;   mid = (outer2.left + outer2.right)/2
<outer1.l, outer1.b> m\n  <outer1.l, outer1.t> l\n  <outer1.r, outer1.t> l\n
<outer1.r, outer1.b> l\n  <outer2.r, outer2.b> l\n  <mid, outer2.t> l\n
<outer2.l, outer2.b> l\n  <outer1.l, outer1.b> l\n
line = outer1;  dx = 2;  dy = (line.top - line.bottom)/4
line.left += 2;  line.right -= 2
for _ in 0..3 { line.top -= dy;  <line.l, line.t> m\n  <line.r, line.t> l\n }
B*\n
```
Formatter **A** for the points, integer for the `1 w`. Note `outer2.top` is
*below* `outer2.bottom` (an inverted rect used as a coordinate pair) — the
points are read out directly, never normalized.

**Popup** (:1282-1321): `"/GS gs\n"` (newline!), then
```
<fill Rgb(1,1,0)> <stroke Rgb(0,0,0)> 1 w\n
<B:l> <B:b> <B:Width()> <B:Height()> re b\n
```
with the rect normalized then deflated by 0.5. Then it builds a **fallback
Helvetica font** (`GenerateFallbackFontDict`, :633-641:
`<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>`,
where `CFX_Font::kDefaultAnsiFontName == "Helvetica"`), aliases it `"FONT"`,
and appends `GetPopupContentsString` (:600-631):

```
value = decode_text(dict[/T]) + "\n" + decode_text(dict[/Contents])
vt: plate = dict.rect(/Rect);  font_size = 12;  auto_return = true;  multi_line = true
vt.initialize();  vt.set_text(value);  vt.rearrange_all()
content = generate_edit_ap(&map, vt.iter(), offset=(3.0, -3.0), continuous=false, sub_word=0)
if content.is_empty() { return "" }
"BT\n" + <fill Rgb(0,0,0)> + content + "ET\n" + "Q\n"
```

**The trailing `Q\n` has no matching `q`.** It is an unbalanced restore in the
generated stream; the content-stream interpreter tolerates it (page brief
§1.2). Emit it.

**FreeText** (:1047-1149) — the only per-subtype generator that uses the form
machinery. Requires: a catalog; an `/AcroForm` (creating one via
`InitAcroFormDict` if absent — a **document mutation**, D1); a
`GetDefaultAppearanceInfo` (§1.13.4) that is `Some`; a `/DR` dict; and a
`ValidateFontResourceDict(/DR /Font)` — `fpdf_parser_utility.cpp:167-186`:
every value in the dict must resolve to a dict whose `/Type` is the **Name**
`Font`. **An empty `/Font` dict passes** (vacuous truth); a missing one fails
(`!dict` → false). Any failure returns false with no AP.

```
font_dict = dr_font[name] or a new fallback Helvetica registered under `name`   // :643-657
appearance = "/GS gs "
bsi = GetBorderStyleInfo(dict[/BS]);  half = bsi.width / 2
rect = dict.rect(/Rect)                       // NOT normalized
background = rect.deflate(half, half);   body = background.deflate(half, half)
if let Some(c) = dict[/C] as array {
    appearance += "q\n" + <fill CFXColorFromArray(c)> + <A:WriteRect(background)> + " re f\nQ\n"
}
let border = GenerateBorderAP(rect, bsi, text_color);      // note: TEXT colour, not /C
if !border.is_empty() { appearance += "q\n" + border + "Q\n" }
vt: plate = body;  alignment = ToAlignment(dict.int(/Q));  SetVtFontSize(da.size)
vt.initialize();  vt.set_text(decode_text(dict[/Contents]));  vt.rearrange_all()
let content = vt.content_rect();
let offset = (0.0, (content.height() - body.height()) / 2.0);
let body_str = generate_edit_ap(map, vt.iter(), offset, continuous=true, sub_word=0);
if !body_str.is_empty() {
    appearance += "/Tx BMC\n" + "q\n";
    if content.width() > body.width() || content.height() > body.height() {
        appearance += <A:WriteRect(body)> + " re\nW\nn\n"
    }
    appearance += "BT\n" + <fill text_color> + body_str + "ET\n" + "Q\nEMC\n"
}
```

`ToAlignment(q)` (`cpvt_variabletext.h:80-86`) clamps out-of-range to `Left`.
The clip is emitted **only on overflow**, which is why a short FreeText has no
`W n`.

#### 1.13.9 `GenerateFormAP` — text field, combo box, list box

`cpdf_generateap.cpp:1449-1582`. Reached only from §1.11.1 (so, in the oracle,
never — but it is the `/NeedAppearances` contract and the form-cluster pixel
tests exercise it through `fpdfsdk`).

**Preconditions**, all silent `return`s: a catalog; an existing `/AcroForm`
(**unlike FreeText, this one does not create it**); a `Some` default-appearance
info; a `/DR`; a valid `/DR /Font`; and a loadable font.

**Rotation and the BBox** (`GetAnnotationDimensionsAndColor`, :215-253):

```
let mk = GetAppearanceCharacteristics(dict[/MK]);     // :183-206
let r = dict.rect(/Rect);                             // NOT normalized
let (w, h) = (r.right - r.left, r.top - r.bottom);    // can be negative
match mk.rotation % 360 {                             // C++ % keeps the sign!
    0   => (bbox = (0,0,w,h),  matrix = identity),
    90  => (bbox = (0,0,h,w),  matrix = ( 0, 1,-1, 0, w, 0)),
    180 => (bbox = (0,0,w,h),  matrix = (-1, 0, 0,-1, w, h)),
    270 => (bbox = (0,0,h,w),  matrix = ( 0,-1, 1, 0, 0, h)),
    _   => (bbox = zero, matrix = identity),          // falls through the switch!
}
```

`GetAppearanceCharacteristics` reads `/MK /R` via `GetIntegerFor`, `/MK /BC`
and `/MK /BG` via `CFXColorFromArray`. **A negative `/R`**: C++ `%` truncates
toward zero, so `/R -90` gives `-90`, matching no case → `bbox` stays the
default-constructed **zero rect**. Rust's `%` on `i64` behaves identically;
port it as a plain `match` on `r % 360` with a `_ => zero` arm.

**The stream body:**

```
let d = GetAnnotationDimensionsAndColor(dict);
let mut app = String::new();
let background = <fill d.background_color>;
if !background.is_empty() { app += "q\n" + &background + &<A:WriteRect(d.bbox)> + " re f\nQ\n" }
let bsi = GetBorderStyleInfo(dict[/BS]);
let border = GenerateBorderAP(d.bbox, bsi, d.border_color);
if !border.is_empty() { app += "q\n" + &border + "Q\n" }
let body_rect = d.bbox.deflate(bsi.width, bsi.width);       // FULL width, not half
```

Then the AP stream is located or created **before** the body is generated,
because the font map needs the stream's `/Resources` (:1510-1528):

```
let ap = dict.get_or_create(/AP);
let mut normal = ap.stream(/N);                              // Stream-typed
let resources = if let Some(s) = &normal {
    let cloned = clone_resources_if_missing(s.dict, dr_dict);   // :276-286
    if !cloned && !validate_or_create_font_resources(s.dict, font_dict, name) { return }   // :288-309
    s.dict[/Resources]
} else {
    normal = doc.new_indirect_stream(empty_dict);  ap[/N] = Ref(normal);  None
};
```

`CloneResourcesDictIfMissingFromStream` **deep-clones the whole `/DR`** into
the stream when it has no `/Resources`, returning whether it did.
`ValidateOrCreateFontResources` gets-or-creates `/Resources /Font`, runs
`ValidateFontResourceDict` on it (**returning false and aborting the whole
generation on failure**), and registers `name → Ref(font_dict)` if absent.

Then the per-type body, and finally (:1570-1581):

```
normal.set_data_and_remove_filter(&app);            // clears /Filter and /DecodeParms
normal.dict[/Matrix] = d.matrix;
normal.dict[/BBox]   = d.bbox;
let cloned = clone_resources_if_missing(normal.dict, dr_dict);
if cloned { return }
validate_or_create_font_resources(normal.dict, font_dict, name);   // return value IGNORED here
```

The resources handling runs **twice** — once before the body and once after —
because the second call is the one that fixes up a freshly created stream.
Port both calls.

**Text field** (`GenerateTextFieldAP`, :784-831):

```
let value  = field_attr(dict, "V").map(unicode_text).unwrap_or_default();
let align  = field_attr(dict, "Q").map(int).unwrap_or(0);
let flags  = field_attr(dict, "Ff").map(int).unwrap_or(0);
let maxlen = field_attr(dict, "MaxLen").map(int).unwrap_or(0);
vt.plate = body_rect;  vt.alignment = ToAlignment(align);  SetVtFontSize(font_size, vt);
let multiline = flags & TextMultiline != 0;
if multiline { vt.multi_line = true; vt.auto_return = true }
let sub_word = if flags & TextPassword != 0 { vt.set_password_char(b'*'); b'*' } else { 0 };
let comb = flags & TextComb != 0;
if comb { vt.set_char_array(maxlen) } else { vt.set_limit_char(maxlen) }
vt.initialize();  vt.set_text(value);  vt.rearrange_all();
let offset = if multiline { (0,0) } else { (0.0, (vt.content_rect().height() - body_rect.height()) / 2.0) };
generate_edit_ap(map, vt.iter(), offset, /*continuous=*/ !comb, sub_word)
```

Wrapped by the caller (:1536-1553) as
```
/Tx BMC\nq\n [<A:WriteRect(body_rect)> re\nW\nn\n if content overflows]
BT\n <fill text_color> <body> ET\nQ\nEMC\n
```
A comb field passes `continuous = false`, forcing one `Td`+`Tj` per character —
which is exactly what a comb needs.

**Combo box** (`GenerateComboBoxAP`, :833-896):

```
let value = field_attr(dict, "V").map(unicode_text).unwrap_or_default();
let button = { let mut b = body_rect; b.left = b.right - 13; b.normalize(); b };
let edit   = { let mut e = body_rect; e.right = button.left; e.normalize(); e };
vt.plate = edit;  SetVtFontSize(font_size, vt);
vt.initialize();  vt.set_text(value);  vt.rearrange_all();
let offset = (0.0, (vt.content_rect().height() - edit.height()) / 2.0);
let text = generate_edit_ap(map, vt.iter(), offset, /*continuous=*/true, 0);
if !text.is_empty() {
    out += "/Tx BMC\nq\n" + <A:WriteRect(edit)> + " re\nW\nn\n"
         + "BT\n" + <fill text_color> + text + "ET\n" + "Q\nEMC\n";
}
let button_fill = <fill Rgb(220/255, 220/255, 220/255)>;     // MakeRGBBytes(220,220,220)
if !button_fill.is_empty() && !button.is_empty() {
    out += "q\n" + &button_fill + <A:WriteRect(button)> + " re f\nQ\n";
    let bb = GenerateBorderAP(button, BorderStyleInfo{ width: 2, style: Beveled, dash: (3,0,0) },
                              Gray(0.0));
    if !bb.is_empty() { out += "q\n" + &bb + "Q\n" }
    let c = ((button.left+button.right)/2, (button.top+button.bottom)/2);
    if is_float_bigger(button.width(), 6) && is_float_bigger(button.height(), 6) {
        out += "q\n0 g\n"
             + <A:(c.x-3, c.y+1.5)> + " m\n" + <A:(c.x+3, c.y+1.5)> + " l\n"
             + <A:(c.x,   c.y-1.5)> + " l\n" + <A:(c.x-3, c.y+1.5)> + " l f\n"
             + &button_fill + "Q\n";
    }
}
```

The dropdown arrow is a down-pointing triangle 6 wide and 3 tall; the
`button_fill` string is emitted **again** after the triangle (a no-op fill
colour change before `Q`). The `13`-point button width and the `6`-point
minimum are hard constants. Note the `/Tx BMC` clip here is **unconditional**,
unlike the text field's overflow-gated one.

**List box** (`GenerateListBoxAP`, :898-975): returns `""` when there is no
`/Opt`.

```
let opts = field_attr(dict, "Opt") as array?;                 // else ""
let sel  = field_attr(dict, "I") as array;
let top  = field_attr(dict, "TI").map(int).unwrap_or(0);
let mut fy = body_rect.top;
for i in top..opts.len() {
    if is_float_smaller(fy, body_rect.bottom) { break }
    let opt = opts.direct_at(i);  if none { continue }        // note: continue, not break
    let item = match opt { Str(s) => decode_text(s),
                           Array(a) => a.direct_at(1).map(unicode_text).unwrap_or_default(),
                           _ => "" };
    let selected = sel.map_or(false, |s| s.iter().any(|v| v.int() >= 0 && v.int() as usize == i));
    vt.plate = FloatRect(body_rect.left, 0.0, body_rect.right, 0.0);   // ZERO HEIGHT
    vt.font_size = if is_float_zero(font_size) { 12.0 } else { font_size };
    vt.initialize();  vt.set_text(item);  vt.rearrange_all();
    let h = vt.content_rect().height();
    if selected {
        out += "q\n" + <fill Rgb(0/255, 51/255, 113/255)>
             + <A:WriteRect(FloatRect(body_rect.left, fy - h, body_rect.right, fy))> + " re f\nQ\n"
             + "BT\n" + <fill Gray(1.0)>
             + generate_edit_ap(map, vt.iter(), (0.0, fy), true, 0) + "ET\n";
    } else {
        out += "BT\n" + <fill text_color>
             + generate_edit_ap(map, vt.iter(), (0.0, fy), true, 0) + "ET\n";
    }
    fy -= h;
}
```

The plate rect has **zero height**, so `auto_return` (off here) is irrelevant
and every item is one line; the height comes from the font metrics. The
selection highlight is `RGB(0, 51, 113)` and the selected label is white. The
offset passed to `generate_edit_ap` is `(0, fy)`, i.e. the y offset is applied
per item rather than by moving the plate — so the `Td` deltas accumulate down
the list. `TI` out of range makes the loop body never run. Wrapped by the
caller (:1559-1567) as an **unconditional** clip:
`/Tx BMC\nq\n <A:WriteRect(body_rect)> re\nW\nn\n <body> Q\nEMC\n`.

`GenerateDefaultAppearanceWithColor` (:1626-1664) rewrites an annot's `/DA` to
`"<font+size><colour>"` and does **not** regenerate the AP (there is an
in-tree TODO). Only `fpdfsdk/fpdf_annot.cpp`'s `SetFontColor` uses it; include
it for the facade's benefit, it is six lines.

### 1.14 AcroForm — `cpdf_interactiveform.cpp`, `cpdf_formfield.cpp`, `cpdf_formcontrol.cpp`

#### 1.14.1 The inherited-attribute walk

`GetFieldAttrRecursive(dict, name, level)` (`cpdf_formfield.cpp:36-53`):

```
if dict.is_none() || level > 32 { return None }               // kGetFieldMaxRecursion
if let Some(a) = dict.direct(name) { return Some(a) }         // GetDirectObjectFor: resolves 1 level
GetFieldAttrRecursive(dict[/Parent] as dict, name, level + 1)
```

**No cycle guard** — a `/Parent` loop terminates only via the depth cap.
`GetDictFor` on `/Parent` means a non-dict parent ends the walk. This is the
`field_attr` used everywhere above.

`GetFullNameForDict(dict)` (:92-113) — the fully-qualified name:

```
let mut full = String::new();
let mut visited = HashSet::new();
let mut level = Some(dict);
while let Some(d) = level {
    visited.insert(d);
    let short = d.unicode_text(/T);                            // decode_text; absent => ""
    if !short.is_empty() {
        full = if full.is_empty() { short } else { short + "." + &full };
    }
    level = d[/Parent] as dict;
    if visited.contains(level) { break }                       // CHECKED AFTER the assignment
}
full
```

Levels with an empty `/T` are **skipped without a separator**, so
`grandparent.T="a"`, `parent.T=""`, `child.T="b"` gives `"a.b"`. The visited
check happens *after* advancing, so the cycle's entry node is processed once
more than a naive guard would allow — pinned by
`cpdf_formfield_unittest.cpp:71-107`, where a four-node cycle
`root→dict1→dict2→dict3→root` yields `"qux.bar.foo"` from `root`,
`"foo.qux.bar"` from `dict1`, and `"bar.foo.qux"` from **both** `dict2` and
`dict3`. Reproduce with an `ObjRef`-and-slot identity set (inline dicts are
distinct nodes, as in C++).

#### 1.14.2 Field discovery and the field tree

`CPDF_InteractiveForm::CPDF_InteractiveForm(doc)` (:589-609):
`catalog[/AcroForm]` as a dict (absent → an empty form), then `/Fields` as an
array (absent → empty), then `LoadField(fields.dict_at(i), 0)` for each.

`LoadField(dict, level)` (:854-887):

```
if level > 32 || dict.is_none() { return }
let parent_objnum = dict.objnum();
let kids = dict[/Kids] as array;
if kids.is_none() { return add_terminal_field(dict) }
let first = kids.dict_at(0);  if none { return }               // ABORTS the whole subtree
if !first.contains(/T) && !first.contains(/Kids) { return add_terminal_field(dict) }
for kid in kids {
    let k = kid as dict;
    if k.is_some() && k.objnum() != parent_objnum { load_field(k, level + 1) }
}
```

Three quirks: a `/Kids` whose **first** element is not a dict abandons the
node entirely (no terminal field, no recursion); the "is this terminal?" test
looks only at the first kid; and the self-loop guard is `objnum` equality, so
an *inline* kid identical to its parent (objnum 0 == 0) is **also** skipped —
including a legitimately inline kid of an inline field. Port the objnum test.

`AddTerminalField(dict)` (:903-982):

```
if !dict.contains(/FT) {
    let p = dict[/Parent] as dict;
    if p.is_none() || !p.contains(/FT) { return }               // no field type anywhere
}
let name = GetFullNameForDict(dict);   if name.is_empty() { return }
if tree.get(&name).is_none() {
    let mut owner = dict;
    if !dict.contains(/T) && dict.name(/Subtype) == b"Widget" {
        owner = dict[/Parent] as dict or dict;                  // the merged-widget case
    }
    if owner != dict && !owner.contains(/FT) {
        if let Some(ft) = dict.direct(/FT) { owner.set(/FT, ft.clone()) }     // MUTATION
        if let Some(ff) = dict.direct(/Ff) { owner.set(/Ff, ff.clone()) }     // MUTATION
    }
    let field = FormField::new(form, owner);
    if let Some(t) = dict.raw(/T) {                             // GetObjectFor: NON-resolving
        if t.is_reference() {
            let c = t.clone_direct();                           // resolve + deep clone
            dict.set(/T, if c.is_string() { c } else { String::new_empty() });   // MUTATION
        }
    }
    if !tree.set(&name, field) { return }
}
let kids = dict[/Kids] as array;
if kids.is_none() {
    if dict.name(/Subtype) == b"Widget" { add_control(field, dict) }
    return;
}
for kid in kids { if kid.name(/Subtype) == b"Widget" { add_control(field, kid) } }
```

Note `GetNameFor("Subtype")` is **Name-typed and non-resolving** here (unlike
`CPDF_AnnotList`, which uses `GetByteStringFor`) — a String `/Subtype (Widget)`
is *not* a widget for form purposes but *is* for annotation purposes.

The `/T` normalization (:952-962) is pinned by
`cpdf_interactiveform_unittest.cpp:23-76`: an indirect `/T` pointing at a
String is flattened to a direct String with the same text; one pointing at a
Name or a Stream becomes an **empty String**. Three `[spec]`-relevant
mutations happen here — see D1.

`CFieldTree` (:400-587) is a trie keyed on the `.`-separated FQN components,
depth-capped at `kMaxRecursion = 32` (:49, :495). `SetField` fails for an empty
name or when the path resolves back to the root. `CountFields`/`GetFieldAtIndex`
are a pre-order walk counting nodes that carry a field (:434-461) — so the
**field index order is the trie's insertion order per level**, not the
`/Fields` array order.

`FixPageFields(page)` (:889-901) re-runs `LoadField` over every `/Annots`
entry whose `/Subtype` is the **Name** `Widget` — used by the SDK when a page
loads after the form. Include it; it is four lines and the facade wants it.

`AddControl` (:984-999) memoizes one `FormControl` per widget dict and appends
it to the field's control list, so a widget appearing twice yields one control.
`GetControlAtPoint` (:695-727) iterates `/Annots` **backwards**, looks each
dict up in the control map, and returns the first whose `GetRect().Contains(point)`
— the same z-order-preserving pattern as `LinkList`.

#### 1.14.3 Field type and flags

`InitFieldFlags` (`cpdf_formfield.cpp:123-161`) runs once at construction:

```
let ft = field_attr(dict, "FT").map(|o| o.as_byte_string()).unwrap_or_default();
let flags = field_attr(dict, "Ff").map(int).unwrap_or(0);
required  = flags & 0x02 != 0;      no_export = flags & 0x04 != 0;
match ft {
  b"Btn" => if flags & (1<<15) != 0 { RadioButton; unison = flags & (1<<25) != 0 }
            else if flags & (1<<16) != 0 { PushButton }
            else { CheckBox; unison = true },
  b"Tx"  => if flags & (1<<20) != 0 { File } else if flags & (1<<25) != 0 { RichText } else { Text },
  b"Ch"  => { if flags & (1<<17) != 0 { ComboBox }
              else { ListBox; multi_select = flags & (1<<21) != 0 }
              use_selected_indices = compute_use_selected_indices() },
  b"Sig" => Sign,
  _      => Unknown,
}
```

**Bit 25 is overloaded**: `kButtonRadiosInUnison` and `kTextRichText` are the
same bit, disambiguated by `/FT`. A checkbox is **always** unison. The
`kTextFileSelect` (bit 20) check precedes the rich-text one, so a field with
both is `File`.

`GetFieldType()` (:281-302) maps to the public `FormFieldType`: `Text`,
`RichText` and `File` all collapse to `TextField`; `Sign` → `Signature`.

Pinned: `multiple_form_types.pdf` annot indices 1..5 give ComboBox, ListBox,
TextField, CheckBox, RadioButton, and index 0 (a Link) gives **−1**
(`fpdf_annot_embeddertest.cpp:2799`). `combobox_form.pdf` gives
`Combo|Edit`, `Combo`, `ReadOnly|Combo` (`:1833`).

#### 1.14.4 Values

`GetValue(default)` (:333-365):

```
if type is CheckBox | RadioButton { return get_check_value(default) }
let mut v = if default { field_attr(/DV) } else { field_attr(/V) };
if v.is_none() && !default && type != Text { v = field_attr(/DV) }   // NOTE the type gate
let v = v?;                                                          // else ""
match v { Str | Stream => v.unicode_text(),
          Array(a)     => a.direct_at(0).map(unicode_text).unwrap_or_default(),
          _            => "" }
```

The `type_ != kText` gate means a **plain text field with no `/V` reports the
empty string even when `/DV` exists**, while a RichText or File field falls
back to `/DV`. An Array `/V` reports only its first element. A Stream `/V`
decodes its raw bytes.

`SetValue(value, default, notify)` (:375-447), per type:

- **CheckBox / RadioButton** → `SetCheckValue` (§1.14.5), always `true`.
- **File / RichText / Text / ComboBox**: write a `CPDF_String` to `/DV` or
  `/V`; then, for a ComboBox only, `index = FindOption(value)`. If
  `index < 0`: for a **RichText** non-default write, mirror the value into
  `/RV` (`dict[/RV] = dict[key].clone()`); and unconditionally
  `dict.remove(/I)`. If `index >= 0` and not default:
  `ClearSelection` then `SetItemSelection(index)`.
- **ListBox**: `FindOption(value)`; `< 0` → **return false** (the only
  failing path); `default && index == GetDefaultSelectedItem()` → return
  false; else clear+select as above.
- anything else → `true` with no effect.

Note `SetValue` writes `/V` **on `dict_`**, the field's own dict, which for a
merged widget is the *parent* (§1.14.2's `owner`).

`ResetField()` (:176-257):

- CheckBox/RadioButton: `CheckControl(i, control.is_default_checked())` for
  every control, then notify.
- ComboBox/ListBox: clear selection; `index = GetDefaultSelectedItem()`;
  `csValue = if index >= 0 { GetOptionLabel(index) } else { "" }`; notify;
  `SetItemSelection(index)` — **with a possibly negative index**, which
  `SetItemSelection` then rejects (:556-560), so the field ends up cleared.
- everything else: compare `/DV` and `/V` as text; if `/RV` is **absent** and
  they are equal, return early; else set `/V` (and `/RV` when it was present)
  from a clone of `/DV`, or **remove both** when `/DV` is absent.

`GetMaxLen()` (:454-471): `field_attr("MaxLen")` as an integer; if absent,
scan the controls for the first widget dict that **has** the key and use
`GetIntegerFor` on it. Pinned by `text_form_multiple.pdf`: annots 0 and 1 have
none, annot 2 has `10` (`fpdf_annot_embeddertest.cpp:1524`).

#### 1.14.5 Checkbox / radio group logic

`CPDF_FormControl::GetOnStateName()` (`cpdf_formcontrol.cpp:56-76`): the first
key of `widget[/AP][/N]` (as a dict) that is not `"Off"` — **in the dict's
iteration order**, which in C++ is sorted (Escalation E2 applies here too,
though only for a multi-state `/N`, which is malformed anyway). Missing `/AP`
or `/N` → `""`.

`GetCheckedAPState()` (:78-89): `GetOnStateName()`, but if the field has an
`/Opt` **array** it is replaced by `FormatInteger(control_index)`; empty →
`"Yes"`.

`GetExportValue()` (:91-103): `GetOnStateName()`, but with an `/Opt` array it
is `opt.byte_string_at(control_index)`; empty → `"Yes"`; then `decode_text`.

`IsChecked()` (:105-111): `widget.byte_string(/AS) == GetOnStateName()`.
`IsDefaultChecked()` (:113-124): `field_attr("DV").as_byte_string() ==
GetOnStateName()`, and **false when `/DV` is absent** (so a control whose on-state
is `""` is not "default checked" merely because `/DV` is missing).

`CheckControl(checked)` (:126-138): `old = widget.byte_string(/AS)` defaulting
to `"Off"`; `new = if checked { on_state_name() } else { "Off" }`; write a
**Name** `/AS` only when they differ.

`CPDF_FormField::CheckControl(index, checked, notify)` (:683-740) — the group
semantics:

```
let control = self.control(index)?;
if !checked && control.is_checked() == checked { return }      // already off: no-op
let target_export = control.export_value();
for c in self.controls() {
    if unison {
        if c.export_value() == target_export {
            if c.on_state_name() == control.on_state_name() { c.check_control(checked) }
            else if checked { c.check_control(false) }
        } else if checked { c.check_control(false) }
    } else {
        if c is control { c.check_control(checked) } else if checked { c.check_control(false) }
    }
}
if field_attr("Opt") is not an Array {
    let export_bytes = encode_text(&target_export);
    if checked { dict[/V] = Name(export_bytes) }
    else if field_attr(/V).map(as_byte_string) == Some(export_bytes) { dict[/V] = Name("Off") }
} else if checked {
    dict[/V] = Name(FormatInteger(index));
}
```

So: with no `/Opt`, `/V` holds the **export value as a Name**; with an `/Opt`
array, `/V` holds the **control index** as a decimal Name. Unchecking only
clears `/V` when it currently names *this* control's export value. Radios in
unison share state by export value.

`GetCheckValue(default)` (:742-756): the export value of the first checked (or
default-checked) control, else `"Off"`.

`SetCheckValue(value, default, notify)` (:758-779): for each control, compare
its export value to `value`; when not `default`, `CheckControl(index, matched)`;
`break` on the **first match**. Note the non-default path calls `CheckControl`
for every control *until* the match, so preceding controls are explicitly
unchecked.

Pinned by `click_form.pdf` (8 annots): checkbox states
`[true, false, false, false, true, false, false, true]`, annot 3 belongs to a
3-control field at control index 1, annot 6's export value is `"value2"` and
annot 0's is `"Yes"` (`fpdf_annot_embeddertest.cpp:2702-3222`).

#### 1.14.6 Choice fields: `/V` vs `/I` vs `/Opt`

`CountOptions` / `GetOptionText(i, sub)` / `GetOptionValue(i)` /
`GetOptionLabel(i)` (:635-672): `field_attr("Opt")` as an array;
`GetOptionText(i, sub)` reads `opt[i]`, and if that is an **Array**, its
element `sub`; then a **`ToString` type filter** — a Name option yields `""`.
`GetOptionValue` is `sub = 0`, `GetOptionLabel` is `sub = 1`. So a
string option is both its own value *and* its own label, while a
`[value label]` pair splits.

`UseSelectedIndicesObject()` (:863-950) — the tie-break, run once at
construction and re-run after each `SetItemSelection`:

```
let i_obj = field_attr("I")?;   if none { return false }        // no /I: use /V
let v_obj = field_attr("V");    if none { return true }         // no /V: use /I
let n = match i_obj { Array(a) => a.len(), Number => 1, _ => return false };
let mut values: HashMap<String, usize> = HashMap::new();
match v_obj {
    Array(a) => { if a.len() != n { return false }
                  for o in a { if o.is_string() { *values.entry(o.unicode_text()).or_default() += 1 } } }
    Str(s)   => { if n != 1 { return false }  *values.entry(s.unicode_text()).or_default() += 1 }
    _ => {}                                                      // a Number /V: values stays EMPTY
}
let num_options = self.count_options();
if let Array(a) = i_obj {
    for o in a {
        if !o.is_number() { return false }
        let idx = o.int();  if idx < 0 || idx >= num_options { return false }
        let label = self.option_value(idx);
        let e = values.get_mut(&label)?;  *e -= 1;  if *e == 0 { values.remove(&label) }
        // a missing key => return false
    }
    return values.is_empty();
}
let idx = i_obj.int();
if idx < 0 || idx >= num_options { return false }
values.contains_key(&self.option_value(idx))
```

In words: `/I` is trusted only when it is a well-formed index set whose
referenced option *values* are exactly the multiset in `/V`. Any mismatch —
different lengths, an out-of-range index, a non-number entry, a value not in
`/V` — falls back to `/V`. Pinned exhaustively by
`cpdf_formfield_unittest.cpp:109-336` (sixteen cases) and by `listbox_form.pdf`
(`fpdf_annot_embeddertest.cpp:2378`): `/I`-only → `{1,3}`; `/V`-array-only →
`{2,4}`; conflicting different-length → `{0,2}` (i.e. `/V` wins).

`IsItemSelected(i)` (:546-554): out of range → false; else
`if use_selected_indices { is_selected_index(i) } else { is_selected_option(option_value(i)) }`.

`IsSelectedOption(v)` (:806-824): a `/V` array containing a **String** equal to
`v`, **or** a `/V` that is itself a String equal to `v`.
`IsSelectedIndex(i)` (:826-845): a `/I` array containing a **Number** equal to
`i`, or a `/I` that is itself a Number equal to `i`.

`GetSelectedIndex(index)` (:486-525) is a different, hairier accessor used by
the SDK: `GetValueOrSelectedIndicesObject()` (`/V` if present, else `/I`); a
Number → its integer directly; a String → only valid for `index == 0`; an
Array → element `index`. Then the found *text* is looked up in `/Opt`: first
try `GetSelectedOptionIndex(index)`'s option value, else a linear scan of all
options; no match → −1.

`SetItemSelection(index, notify)` (:556-603): out of range → no-op. For a
non-ListBox: `dict[/V] = String(option_value)` and `dict[/I] = [index]`.
For a ListBox: `SelectOption(index)` — an **insertion-sorted** append into
`/I` (:847-861) that returns early if already present — then, single-select,
`dict[/V] = String(option_value)`; multi-select, rebuild `/V` as an array of
the option values of every `i` where `i == index || IsItemSelected(i)`.

`GetDefaultSelectedItem` (:605-621): the index whose option **value** equals
`decode_text(/DV)`, or −1.

#### 1.14.7 `CPDF_FormControl` and the `/MK` dictionary

`GetRect()` (:52-54) = `widget.rect(/Rect)`, unnormalized.

`GetDefaultAppearance()` (:193-205) — a three-rung ladder that is **not** the
same as §1.13.4's: `widget[/DA]` if the key exists (via `GetByteStringFor`);
else `field_attr(field.dict, "DA")` as a string; else the form's
`acroform[/DA]`.

`GetControlAlignment()` (:267-279): `widget[/Q]` if the key exists; else
`field_attr(field.dict, "Q")`; else `acroform[/Q]` defaulting to 0.

`GetHighlightingMode()` (:140-150): `widget.byte_string(/H)` defaulting to
`"I"`, matched against `['N','I','O','P','T']` → `None|Invert|Outline|Push|Toggle`;
no match → `Invert`.

`GetDefaultControlFont()` (:216-265) — the font lookup ladder:

1. `field_attr(widget, "DR")` as a dict → `/Font` (validated) → `[name]` →
   load. Note `DR` is looked up through the **inherited walk from the widget**,
   so a widget's own `/DR` wins over the AcroForm's.
2. `form.GetFormFont(name)` (:782-809) — `PDF_NameDecode(name)`, then
   `acroform[/DR][/Font][alias]` validated as a `/Type /Font` dict.
3. `widget[/P]` → `field_attr(page_dict, "Resources")` → `/Font` (validated)
   → `[name]` → load.

`CPDF_ApSettings` (`cpdf_apsettings.cpp`) is the `/MK` reader — a thin,
nullable dict wrapper:

| accessor | key | behavior |
|---|---|---|
| `has(entry)` | any | `dict.contains(entry)` |
| `rotation()` | `/R` | `GetIntegerFor`, 0 |
| `color_argb(entry)` | `/BC`,`/BG` | length 1/3/4 → Gray/RGB/CMYK **pre-multiplied to bytes**, else Transparent |
| `original_color_component(i, entry)` | | `array.float_at(i)`, 0.0 |
| `original_color(entry)` | | `CFXColorFromArray` (§1.0.1) |
| `caption(entry)` | `/CA`,`/RC`,`/AC` | `GetUnicodeTextFor` |
| `icon(entry)` | `/I`,`/RI`,`/IX` | `GetMutableStreamFor` |
| `icon_fit()` | `/IF` | `CPDF_IconFit(dict[/IF])` |
| `text_position()` | `/TP` | `GetIntegerFor` clamped to `0..=6`, default 0 |

`color_argb` (:32-65) converts CMYK as `(1 - min(1, c+k)) * 255` per channel —
**a different formula from `--annot`'s** `255*(1-c)*(1-k)` (§1.12.2). Both
exist; keep them separate.

`CPDF_IconFit` (`cpdf_iconfit.cpp`): `/SW` → `Bigger|Smaller|Never|Always`
(default `Always`, matching `"A"` and anything unrecognized);
`/S != "A"` → proportional (so the default `"P"` and any junk are
proportional); `/A` → a 2-element position array defaulting to
`(0.5, 0.5)` for `GetIconBottomLeftPosition` but **`(0, 0)` for
`GetIconPosition`** (two accessors, two defaults, :51-90); `/FB` boolean.
`GetScale` / `GetImageOffset` (:92-141) are the fitting math; they are
data-model completeness only (§1.0).

### 1.15 `cpdfsdk_appstream` — the widget appearance generator (scope decision)

SPEC §10 names `cpdfsdk_appstream` as in scope. PLAN.md §1 and §3 exclude
`fpdfsdk` wholesale. Both cannot hold; **Escalation E1** puts the decision to
the user. The evidence:

- **It is oracle-reachable.** `CPDFSDK_PageView::NewAnnot`
  (`fpdfsdk/cpdfsdk_pageview.cpp:97-123`) calls
  `CPDFSDK_Widget::ResetAppearance` whenever `CPDF_InteractiveForm::
  NeedConstructAP()` — i.e. `acroform[/NeedAppearances]` is a true Boolean —
  and `pdfium_test` always builds pages through the form-fill environment.
  `ResetAppearance` (`fpdfsdk/cpdfsdk_widget.cpp:671-700`) dispatches to
  `SetAsPushButton | SetAsCheckBox | SetAsRadioButton | SetAsComboBox |
  SetAsListBox | SetAsTextField`.
- **It is small in the corpus.** Exactly **five** files in
  `testing/corpus` + `testing/resources` contain `NeedAppearances`, all under
  `testing/resources/pixel/`: `bug_394767650.in`, `bug_733528.in`,
  `bug_40643646.in`, `password.in`, `text_form_multiline.in`. All are Tier-B
  pixel tests; none is a Tier-A `--annot` or `--show-structure` target.
- **It drags real dependencies.** `cpdfsdk_appstream.cpp` includes
  `fpdfsdk/pwl/cpwl_edit_impl.h` and uses `CPWL_EditImpl` as its layout engine
  for text/combo/list bodies (`GetEditAppStream`, :639) — a *second*,
  different variable-text implementation from `CPVT_VariableText`. It also
  needs `CPDFSDK_Widget::GetRotatedRect`/`GetMatrix`
  (`cpdfsdk_widget.cpp:1022-1060`) and `CPDF_BAFontMap`, which §1.0 excludes.
- **What is genuinely useful in it** is the six checkbox/radio glyph shapes
  (`GetAP_Check` :209-258, `GetAP_Circle` :260-298, `GetAP_Cross` :300-309,
  `GetAP_Diamond` :311-324, `GetAP_Square` :326-336, `GetAP_Star` :338-361,
  `GetAP_HalfCircle` :363-398), selected by `CheckStyleFromCaption`
  (:1146-1165) from the **first character of `/MK /CA`** using ZapfDingbats
  code points: `'4'`→Check, `'8'`→Cross, `'H'`→Star, `'l'`→Circle,
  `'n'`→Square, `'u'`→Diamond, anything else → Check (checkbox default) or
  Circle (radio default, :1447). These are **Bézier/line paths**, not font
  glyphs — the "ZapfDingbats" in the comment refers only to which code point
  names which shape. `GetCheckBoxAppStream` (:566-587) centres the shape via
  `GetCenterSquare()` and scales it by `2/3` for every style except Check and
  Cross; `GetRadioButtonAppStream` (:590-611) uses `1/2` for Circle instead.
  `FXSYS_BEZIER = 0.5522847498308f` (`fx_system.h:45`) is the circle constant.
  `GetAP_Check`'s eight-row control-point table (:213-232) is a fixed
  normalized outline scaled into the bbox.

**Recommendation (default if the escalation is not answered): defer to M6+,
not v1.** The five affected files are pixel tests whose Tier-B thresholds can
carry a documented waiver, and the alternative is importing a second layout
engine that duplicates §1.13.5. The checkbox/radio *shape* tables are cheap and
self-contained, so `ap/shapes.rs` ships them behind a
`generate_widget_appearance` entry point that is wired up but not called by
the default options — the same "implemented, off by default" shape as D12.

### 1.16 Consolidated limit and constant table

| Constant | Value | C++ source |
|---|---|---|
| `kNameTreeMaxRecursion` | 32 | `cpdf_nametree.cpp:26` |
| `kGetFieldMaxRecursion` | 32 | `cpdf_formfield.cpp:40` |
| `kStructTreeMaxRecursion` | 32 | `cpdf_structtree.cpp:112` |
| `CFieldTree` / `LoadField` `kMaxRecursion` | 32 | `cpdf_interactiveform.cpp:49` |
| `kMaxMetaDataDepth` | 128 | `cpdf_metadata.cpp:23` |
| number-tree depth | **uncapped** | `cpdf_numbertree.cpp` (we add 32, D4) |
| `/Next` action chain depth | **uncapped** | `cpdf_action.cpp:206` (we add 32, D4) |
| popup rect | 200 × 200 | `cpdf_annotlist.cpp:104` |
| Text-annot note rect | 20 × 20 | `cpdf_generateap.cpp:1238` |
| text-symbol tip delta | 4 | `cpdf_generateap.cpp:687` |
| text-symbol inner lines | 3, inset 2 | `cpdf_generateap.cpp:711-720` |
| combo-box button width | 13 | `cpdf_generateap.cpp:845` |
| combo-box arrow min size | 6 (exclusive, epsilon) | `cpdf_generateap.cpp:885-886` |
| combo-box arrow half-extent | 3 × 1.5 | `cpdf_generateap.cpp:888-891` |
| combo-box button fill | RGB bytes (220, 220, 220) | `cpdf_generateap.cpp:869` |
| combo-box button border | width 2, Beveled, dash (3,0,0), Gray(0) | `cpdf_generateap.cpp:875-878` |
| list-box selection colour | RGB bytes (0, 51, 113) | `cpdf_generateap.cpp:955` |
| list-box default font size | 12 | `cpdf_generateap.cpp:945` |
| popup font size | 12 | `cpdf_generateap.cpp:612` |
| popup text offset | (3.0, −3.0) | `cpdf_generateap.cpp:619` |
| default border width | 1 | `cpdf_generateap.cpp:129`, `:563` |
| default dash pattern | (3, 0, 0) | `cpdf_generateap.cpp:130` |
| dash array cap | 10 elements | `cpdf_generateap.cpp:588` |
| Beveled/Inset width multiplier | 2 | `cpdf_generateap.cpp:154`, `:158` |
| Beveled greys | 1.0 / 0.5 | `cpdf_generateap.cpp:491`, `:503` |
| Inset greys | 0.5 / 0.75 | `cpdf_generateap.cpp:491`, `:503` |
| circle Bézier `kL` | 0.5523f | `cpdf_generateap.cpp:1014` |
| `FXSYS_BEZIER` | 0.5522847498308f | `fx_system.h:45` |
| squiggly delta / line width | 2 / 1 | `cpdf_generateap.cpp:1376-1377` |
| underline / strikeout line width | 1 | `cpdf_generateap.cpp:1263`, `:1433` |
| `kFontScale` | 0.001 | `cpvt_variabletext.cpp:28` |
| `kReturnLength` | 1 | `cpvt_variabletext.cpp:29` |
| `kFontSizeSteps` | 25 entries, 4…144 | `cpvt_variabletext.cpp:31-33` |
| auto-size multi-line span | first 6 steps (`25/4`) | `cpvt_variabletext.cpp:851`, `:861` |
| float epsilon | 1e-4 (as `double`) | `fx_system.h:36` |
| `MatchRect` degenerate epsilon | 0.001 | `fx_coordinates.cpp:433`, `:436` |
| `MakeRoman` modulus | 1 000 000 | `cpdf_pagelabel.cpp:27` |
| `MakeLetters` repeat modulus | 1000 | `cpdf_pagelabel.cpp:48` |
| `MakeLetters` alphabet | 26 | `cpdf_pagelabel.cpp:49` |
| `kMaximumFloatToDecimalLength` | 49 | `cpdf_contentstream_write_utils.cpp:17` |
| `CFX_Font::kDefaultAnsiFontName` | `"Helvetica"` | `core/fxge/fx_font.h` |

`pdfrum-common::Limits` gains one field (additive, SPEC §1's list is
*(abridged)*): `max_name_tree_depth: u32 = 32`, covering the name tree, the
number tree, the field-attribute walk, the field trie, the struct tree, and
the `/Next` chain. All six use 32 in C++ or are uncapped there; one knob is
enough and no real file approaches it.

### 1.17 Diagnostics mapping

Every recovery gets a `Diagnostic` (severity `Recovered` unless noted):

| Event | `DiagKind` (proposed) |
|---|---|
| outline/`/Next`/field-parent cycle cut | `NavigationCycle` |
| name/number/struct/field depth cap hit | `TreeDepthExceeded` (Suspicious) |
| `/Limits` shorter than 2, or swapped | `NameTreeLimitsRepaired` |
| leaf `/Names` odd length (trailing key with no value) | `NameTreeMalformed` (Suspicious) |
| named dest resolved only via the old-style `/Dests` dict | `LegacyNamedDest` |
| dest page index out of range / unresolvable | `DestPageUnresolved` (Suspicious) |
| `/Subtype` unknown → `Annot::Unknown` | `AnnotSubtypeUnknown` |
| AP generated for an annot (per subtype) | `AppearanceGenerated` |
| AP generation declined (hidden, or `/AP /N` present) | *(none — normal)* |
| `/QuadPoints` length not a multiple of 8 (tail ignored) | `QuadPointsTruncated` (Suspicious) |
| `/InkList` sub-array with odd length or `len < 2` | `InkPathDropped` (Suspicious) |
| `/DA` unparsable (`Tf` not found) → empty font, size 0 | `DefaultAppearanceMalformed` (Suspicious) |
| `/DR /Font` fails `ValidateFontResourceDict` → no AP | `FormResourcesInvalid` (Suspicious) |
| field dropped: no `/FT` on it or its parent | `FieldSkippedNoType` |
| field dropped: empty FQN | `FieldSkippedNoName` |
| `/Kids[0]` not a dict → subtree abandoned | `FieldKidsMalformed` (Suspicious) |
| indirect `/T` flattened, or replaced by an empty string | `FieldNameNormalized` |
| `/I` rejected in favour of `/V` | `ChoiceIndicesIgnored` |
| `/MK /R` not a multiple of 90 → zero BBox | `WidgetRotationInvalid` (Suspicious) |
| struct element dropped (page mismatch, unlinkable parent) | `StructElementDropped` |
| `/K` slot reserved but never populated | `StructKidUnresolved` (Suspicious) |
| page label `/S` unrecognized → prefix only | `PageLabelStyleUnknown` |
| auto font size resolved to 0 (no plate width) → no `Tf` | `AutoFontSizeZero` (Suspicious) |
| a UTF-16 string truncated at a lone surrogate for output | `TextTruncatedAtSurrogate` (Suspicious) |

`Diagnostic.at` is the annotation's or field's `ObjRef` number where one
exists, encoded in the existing `Option<u64>` byte-offset slot as the object's
offset when the parser can supply it, else `None`.

---

## 2. Divergences

1. **AP generation never mutates parsed objects.** The C++ writes `/AP`,
   `PDFIUM_HasGeneratedAP`, a rewritten `/Rect` (Text, Ink), a normalized `/T`,
   an inherited `/FT`+`/Ff`, a copied-down `/AS`, a padded/swapped `/Limits`,
   and — for FreeText — a whole new `/AcroForm` dict, all into the document.
   SPEC §2's `Object` is a value type and the parser's store is
   `OnceLock`-immutable, so instead `generate_appearances(doc, page, &opts)`
   returns an **`AnnotOverlay`**:

   ```rust
   pub struct AnnotOverlay {
       /// Per annotation index: the generated appearance and the dict edits it implies.
       entries: Vec<Option<GeneratedAp>>,
   }
   pub struct GeneratedAp {
       pub stream: Vec<u8>,          // the content-stream bytes
       pub bbox: Rect,
       pub matrix: Affine,
       pub resources: Dict,
       pub rect_override: Option<Rect>,   // Text: the 20x20 box; Ink: the inflated rect
       pub as_override: Option<Name>,     // the /NeedAppearances button case
   }
   ```

   Every reader in this crate (`--annot`, the render path, the facade) takes
   `Option<&AnnotOverlay>` and consults `rect_override` before `/Rect`, and
   `entries[i]` before `/AP`. `pdfrum-edit` applies an overlay to produce the
   mutated document when saving. This is the single biggest structural
   divergence and it is what makes the Tier-A goldens reachable without an
   interior-mutability object model (STYLE §2).

2. **`Bookmark::color()` returns `None` instead of dereferencing null.**
   `cpdf_bookmark.cpp:65-67` calls `GetNumberAt(i)->GetNumber()` with no null
   check; a 3-element `/C` of non-numbers is an upstream crash. Pixel- and
   Tier-A-invisible (no corpus file has one; `bookmarks_color.pdf` covers the
   *rejection* shapes, all of which fail the length or range test first).

3. **String ordering in the name tree is by Unicode scalar value.** C++
   compares `WideString`s with `wmemcmp` over 32-bit `wchar_t` on Linux; Rust
   `str`/`char` comparison gives the same total order for every well-formed
   input. Lone surrogates (which `decode_text` can produce from a malformed
   UTF-16BE string) have no `char`; we store keys as `Vec<u32>` code points for
   comparison purposes, which reproduces `wmemcmp` exactly.

4. **Depth caps added where C++ has none**: the number tree, the `/Next` action
   chain. Both are unbounded recursion in the oracle (stack-overflow bugs). The
   cap is `Limits.max_name_tree_depth` (32) and exceeding it yields "not
   found" plus a diagnostic — never an error. Unobservable on any real file.

5. **The name-tree mutation API is not implemented in v1.**
   `AddValueAndName`, `DeleteValueAndName`, `GetNodeAncestorsLimits`,
   `UpdateNodesAndLimitsUponDeletion`, `CreateWithRootNameArray`. No `core/`
   caller; the only consumer is `fpdfsdk/fpdf_attachment.cpp`. Their unittest
   assertions are catalogued in §4 for `pdfrum-edit`'s M7 port.

6. **Font index 1 (the "system font") is permanently absent.**
   `CPVT_FontMap::SetupAnnotSysPDFFont` (`cpvt_fontmap.cpp:32-51`) depends on
   `GetNativeFontName`, which is `#if BUILDFLAG(IS_WIN)`-only and returns
   `""` on Linux — the oracle's platform. So `GetPDFFont(1)` is always null
   and `GetWordFontIndex` never returns 1. We model `FontMap` as
   `{ default_font, default_alias }` with `font(0)` the only hit and
   `font(_) → None`, matching Linux exactly. Reintroducing index 1 would
   require a whole `CPDF_BAFontMap` port for zero Linux-observable effect.

7. **`decode_file_name`/`encode_file_name` implement the Linux (identity)
   branch only.** The Windows and Apple branches are documented in §1.5 and
   `#[cfg]`-gated out; the Windows *decode* branch is additionally an
   out-of-bounds read for 1–2-character paths that we would not reproduce even
   if enabled.

8. **`Dest::xyz` returns `Option<Xyz>` with per-field `Option`s** rather than
   C++'s six out-parameters plus a bool. The "outputs left untouched on
   absence" behavior the embedder test relies on is an artifact of the C API
   and has no Rust analogue.

9. **Struct-element kid identity is `(ObjRef | parent-slot index)`**, not a raw
   pointer. C++ matches `Kid::dict_ == dict` by pointer, which for indirect
   objects is objnum-equivalent and for inline dicts is
   allocation-equivalent — and an inline dict reached twice through
   `GetDirectObjectAt` *is* the same pointer. Our slot-path key selects the
   same set of nodes.

10. **Wide-string output stops at the first unpaired surrogate.** The oracle
    goes UTF-16 → `wchar_t[]` → `printf("%ls")`, and glibc's `%ls` aborts the
    conversion at the first code unit with no scalar value, truncating the
    line. We reproduce the truncation point rather than the mechanism. (The
    text brief's D3 makes the same call for `--txt`; this keeps them
    consistent.)

11. **No XMP/XML parsing.** `CPDF_Metadata::CheckForSharedForm` feeds an
    "unsupported feature" callback that `pdfium_test` prints to **stderr**, so
    it contributes to no golden. `Metadata` exposes the decoded `/Metadata`
    bytes; the shared-form scan is a documented non-feature. Revisit only if a
    consumer appears.

12. **`IsUpdateAPEnabled` becomes an option field, defaulting to `false`.**
    The C++ process-global (`cpdf_interactiveform.cpp:613`) is set to `true` at
    startup and flipped to `false` by `CPDFSDK_PageView::LoadFXAnnots` around
    every list construction, so the oracle behavior is "off". STYLE §1 bans the
    global; `DocOptions { generate_widget_ap: bool }` defaults to `false` and
    the on-path is implemented for the facade.

13. **An unknown annotation subtype dumps as `Unknown`, not a crash.**
    `write.cc:129` `NOTREACHED()`s for `FPDF_ANNOT_UNKNOWN` and has no `Redact`
    arm at all. If the golden store contains a file that trips this, the
    golden itself is a crash artifact — see Escalation E3.

14. **Float→unsigned colour conversion saturates at 0.** `--annot` writes
    `*R = color.fColor1 * 255.f` into an `unsigned int`; a negative component
    is UB in C++ and wraps to a huge value in practice. We saturate. No corpus
    file has a negative colour component.

15. **Layout iterates UTF-16 code units.** `CPVT_VariableText::SetText` walks
    `WideString` elements, which on Linux are 32-bit — so a supplementary
    character is **one** word there and **two** (a surrogate pair) if we
    iterated UTF-16. We therefore iterate **`char`s**, matching Linux. The
    `%c` byte truncation in `GetPDFWordString` then applies to the scalar
    value's low byte, as in C++.

16. **The auto-font-size off-by-one is ported verbatim.**
    `GetAutoFontSize` returns the step *before* the first fitting one
    (`cpvt_variabletext.cpp:876`), i.e. a size that overflows the plate. It is
    pixel-visible and it is the behavior.

17. **The `IsPunctuation` `word <= 0x0094` bug is ported verbatim.**
    `cpvt_section.cpp:84` — every code point in `0x80..=0x94` is punctuation,
    which changes line-break opportunities for Latin-1 text. Pixel-visible.

18. **Two float formatters, both hand-written.** Formatter A is `ryu` shortest
    with the leading zero stripped and scientific notation forced off;
    formatter B is a ~40-line 6-significant-digit `%g`. DEPS.md already
    contains `ryu`; no new dependency (STYLE §5).

19. **No `NotifierIface`.** Mutating APIs return a `FieldChange` record
    describing what changed instead of calling back into an observer.

20. **`ExportToFDF` deferred to `pdfrum-edit`.** It constructs a `CFDF_Document`,
    which is a writer concern.

---

## 3. Module plan

```
crates/pdfrum-doc/src/
  lib.rs            // re-exports: Outline, Bookmark, NameTree, NumberTree, Dest, Action,
                    // Link, FileSpec, Annotation, AnnotList, AnnotOverlay, Form, Field,
                    // Control, StructTree, StructElement, page_label, ViewerPrefs,
                    // DocOptions, Error

  geom.rs           // the CFX_FloatRect semantics of §1.0.1 over kurbo::Rect:
                    // normalize, is_empty, inflate, deflate, union, intersect, contains,
                    // center_square, scale_from_center, match_rect; plus is_float_{zero,
                    // bigger,smaller}. NOTHING else in the crate touches kurbo directly.
  color.rs          // enum Color + from_array (§1.0.1) + nearly_eq + to_argb/to_rgb_bytes
                    // (the two different CMYK formulas of §1.12.2 and §1.14.7, named)

  nav/
    mod.rs          // re-exports; the Dest/Action/Link/FileSpec records
    outline.rs      // Bookmark record + `walk(doc) -> impl Iterator<Item = Bookmark>`
                    // (pre-order, ObjRef visited set) + `find(doc, title)` (§1.1)
    name_tree.rs    // NameTree { root: Dict } + lookup / lookup_by_index / count /
                    // named_dest (the two-rung ladder); the seen-objnum sets (§1.2.1)
    number_tree.rs  // find / lower_bound (§1.2.2)
    dest.rs         // Dest + ZoomMode enum + xyz/params/page_index (§1.3)
    action.rs       // ActionKind (names!-generated) + the per-type accessors +
                    // `next_actions()` iterator with a cycle guard; AActionType (§1.4)
    filespec.rs     // file_name / file_stream / params + decode/encode_file_name (§1.5)
    link.rs         // Link + `page_links(page)` (the null-preserving vector) +
                    // link_at_point (§1.6)

  annot/
    mod.rs          // Annotation record, Subtype (names!-generated), flags (§1.10)
    quad.rs         // quad_point_count / rect_from_quad_points{,_array} /
                    // bounding_rect_from_quad_points (§1.10.2)
    appearance.rs   // the /AP /N|/R|/D + /AS ladder, both fallback modes (§1.10.3);
                    // annot_matrix (§1.10.4)
    list.rs         // AnnotList: the /Annots walk, popup filtering, popup synthesis,
                    // the display-pass filter (§1.11)

  ap/
    mod.rs          // generate_appearances(doc, page, &opts) -> AnnotOverlay (D1);
                    // the per-subtype dispatch of §1.13.8
    fmt.rs          // THE TWO FLOAT FORMATTERS (§1.13.1). `fmt_shortest` (A) and
                    // `fmt_g6` (B). Every emitter takes one explicitly; there is no
                    // default. This file is ~90 lines and is the crate's sharpest edge.
    emit.rs         // ContentBuilder: a String sink with `color(op)`, `point(A|B)`,
                    // `rect(A|B)`, `op(name)`. The site table of §1.13.1 is encoded
                    // here as which method each generator calls.
    border.rs       // GetBorderStyleInfo + GenerateBorderAP's five styles (§1.13.3);
                    // border_width / dash_array / dash_pattern_string
    da.rs           // SimpleParser (the DA tokenizer) + find_tag_param_from_start +
                    // DefaultAppearance { font_name, font_size, color } (§1.13.4)
    markup.rs       // circle, square, highlight, underline, squiggly, strikeout, ink,
                    // text(+text_symbol), popup — the formatter-B generators (§1.13.8)
    freetext.rs     // the one formatter-A per-subtype generator (§1.13.8)
    form_ap.rs      // GenerateFormAP: rotation/bbox, the resources dance,
                    // text_field / combo_box / list_box bodies (§1.13.9)
    shapes.rs       // the six check/radio path tables from cpdfsdk_appstream
                    // (§1.15) — compiled, wired, off by default
    stream.rs       // ext_gstate_dict / resources_dict / assemble (§1.13.7)

  vt/               // the variable-text layout engine (§1.13.5)
    mod.rs          // VariableText: the config record + `layout(text) -> Layout`
    classify.rs     // kSpecialChars + is_latin/digit/cjk/punctuation/... /need_division
                    // — pure functions over u32, no state
    split.rs        // split_lines: the measure/typeset line breaker
    place.rs        // output_lines: alignment, the bidi pass, per-word placement
    comb.rs         // rearrange_char_array (comb fields)
    autosize.rs     // the kFontSizeSteps ladder + is_bigger (incl. the D16 off-by-one)
    bidi.rs         // the ParagraphDirection enum + the run resolver seam (E5)
    edit_ap.rs      // GenerateEditAP: Layout -> Td/Tf/Tj bytes (§1.13.6)

  form/
    mod.rs          // Form { fields: Vec<Field>, controls: Vec<Control> } + load (§1.14.2)
    attr.rs         // field_attr (the inherited walk) + full_name (the FQN) (§1.14.1)
    field.rs        // Field record, FieldKind, flags, value get/set, reset (§1.14.3-.4)
    check.rs        // checkbox/radio group logic: export values, check_control,
                    // check_value (§1.14.5)
    choice.rs       // options, use_selected_indices, is_item_selected,
                    // set_item_selection (§1.14.6)
    control.rs      // Control + ApSettings (/MK) + IconFit + the DA/Q/font ladders (§1.14.7)

  structure/
    mod.rs          // StructTree::load_page + the bottom-up build (§1.7.1)
    element.rs      // StructElement + Kid + the five /K shapes (§1.7.2)
    dump.rs         // the --show-structure emitter (§1.7.3) — lives here, not in
                    // pdfrum-tool, because the ordering rules are behavior

  page_label.rs     // roman / letters / the label assembly (§1.8)
  prefs.rs          // ViewerPrefs (§1.9)
  metadata.rs       // raw XMP bytes only (D11)
                    // (annot_dump.rs was here; WP2 moved the --annot emitter to
                    //  pdfrum-tool. structure/dump.rs's reasoning does not carry:
                    //  its ordering rules are the *structure tree's* behaviour, so
                    //  a library caller needs them; the --annot line format is the
                    //  CLI's own and had no caller outside pdfrum-tool.)
  error.rs          // Error (thiserror)
```

### Key types beyond SPEC

SPEC §10 names `Bookmarks`, `NameTree`, `Link`/`Action`/`Dest`, `Annotation`,
`Form { fields }`, `Field { kind, value, flags, kids }`, `set_value()`, the
appearance generator, and the struct-tree reader. Filling in:

```rust
pub struct DocOptions {
    /// The /NeedAppearances widget path (D12). Oracle behavior is `false`.
    pub generate_widget_ap: bool,
    /// The cpdfsdk_appstream shapes (§1.15, E1). Default `false`.
    pub generate_widget_shapes: bool,
    pub limits: Limits,
}   // Default + struct-update, per STYLE §4

pub struct Annotation {
    pub subtype: Subtype,
    pub rect: Rect,               // raw /Rect, UNNORMALIZED (§1.10.2)
    pub flags: AnnotFlags,        // a bitflags-free u32 newtype with named predicates
    pub dict: Dict,               // the source, for the long tail of per-subtype keys
    pub quad_points: Option<Array>,
}

/// The result of `ap::generate_appearances`. See D1.
pub struct AnnotOverlay { entries: Vec<Option<GeneratedAp>> }

/// One laid-out paragraph run, the output of `vt::layout`.
pub struct Layout { pub sections: Vec<Section>, pub content_rect: Rect, pub font_size: f32 }
pub struct Section { pub words: Vec<Word>, pub lines: Vec<Line>, pub rect: Rect }
pub struct Word { pub ch: u32, pub font_index: i32, pub x: f32, pub y: f32,
                  pub tail: f32, pub is_rtl: bool }
pub struct Line { pub begin: i32, pub end: i32,   // INCLUSIVE, -1/-1 for an empty line
                  pub x: f32, pub y: f32, pub width: f32, pub ascent: f32, pub descent: f32 }

/// The §1.13.5 provider seam, satisfied by pdfrum-font. NOT a trait — a record of
/// closures would be a trait in disguise; this is a plain struct the caller fills.
pub struct FontMetrics<'a> {
    pub font: &'a Font,
    pub alias: Name,
}
impl FontMetrics<'_> {
    fn char_width(&self, u: u32) -> i32;      // §1.11.1 gap — see OQ-3
    fn ascent(&self) -> i32;
    fn descent(&self) -> i32;
}

pub struct FieldChange { pub value: bool, pub selection: bool, pub checked: bool }
```

`Subtype`, `ActionKind`, `AActionType`, `FieldKind`, `ZoomMode`, `BorderStyle`,
`HighlightMode`, `TextPosition`, `ScaleMethod`, `CheckStyle` are all closed
enums with exhaustive matches and no `_ =>` arms (STYLE §1). The `names!` macro
generates the byte-string ↔ variant tables for `Subtype`, `ActionKind` and
`AActionType`, so the three-way duplication in the C++
(`cpdf_annot.cpp` × 2 + `write.cc`) collapses to one declaration.

### Data flow

```
Document ──catalog──▶ Outline / NameTree / StructTree / PageLabels / ViewerPrefs
                          │
page_dict[/Annots] ──▶ AnnotList ──generate_appearances──▶ AnnotOverlay
                          │                                     │
                          │                              (stream bytes)
                          │                                     │
                          ▼                                     ▼
            pdfrum-tool::annot_dump                  pdfrum-page::build_page
                    (--annot golden)                          │
                                                              ▼
                                                       pdfrum-render

catalog[/AcroForm] ──▶ Form ──▶ Field ──set_value──▶ FieldChange
                                  │
                                  └──(re-generate)──▶ AnnotOverlay
```

`vt::layout` is a pure function `(&VariableText, &str, &FontMetrics) -> Layout`;
`ap::emit` is a pure function `(&Layout, offset, continuous, sub_word) ->
String`. Neither touches a document. That is what makes the layout engine
unit-testable against `cpvt_*_unittest.cpp` with a stub `FontMetrics`.

### Decomposition notes (STYLE §7)

- `CPDF_InteractiveForm` (1153 lines, a god class holding the form dict, the
  field trie, two maps, a notifier, and a static) decomposes into: the `Form`
  record (`form_dict`, `fields`, `controls`), `form::load` (the discovery
  walk), `form::attr` (two free functions), and `form::control` — none of which
  holds a back-pointer to the others. The `CFieldTree` trie survives as a
  private `FieldTrie` inside `form/mod.rs` because the FQN ordering is
  observable.
- `CPVT_VariableText` (984 lines) + `CPVT_Section` (828) hold a mutual
  `UnownedPtr` web and mutate each other's rects. The Rust shape is a
  **config record in, a `Layout` value out**, with `split`/`place`/`comb` as
  free functions taking `(&Config, &mut Vec<Word>, &FontMetrics)`. The
  editing API (`InsertWord`, `DeleteWords`, `BackSpaceWord`, `SearchWordPlace`,
  `GetUpWordPlace`, the `WordPlace`/`WordRange` algebra — roughly 400 of the
  984 lines) exists **only** for the interactive `pwl/` editor and is dropped;
  `SetText` + `RearrangeAll` is the whole surface `core/` uses.
- `CPDF_Annot` (516 lines, a record plus a form cache plus a popup
  back-pointer plus two draw methods) becomes the `Annotation` record, the
  `appearance` module's free functions, and an `ap_forms: HashMap<ObjRef,
  Page>` cache owned by the render session — never by the annotation.
- `CPDF_GenerateAP` (1664 lines of static methods over `ostringstream`)
  becomes `ap/` with one file per generator family and a shared
  `ContentBuilder`. The `ostringstream` becomes a `String` sink; the
  `ByteString` return values become `&str`/`String` and the ubiquitous
  `if (s.GetLength() > 0)` gates become `if !s.is_empty()`.
- The three tokenizers in play (`CPDF_SimpleParser` for `/DA`, the main lexer,
  the content lexer) stay separate: `ap/da.rs` owns the first, and the brief
  documents (§1.13.4) exactly how it differs so nobody unifies them.

---

## 4. Test plan

### 4.1 Ported unittest assertions

Restated over our types; the C++ file is the oracle for the values.

**`cpdf_dest_unittest.cpp:15-67` → `nav::dest`**
Array `[0 /XYZ 4]`: `xyz()` is `None` (length < 5). Append `5`, `6`:
`Some { x: Some(4.0), y: Some(5.0), zoom: Some(6.0) }`. Set index 4 to `0`:
`zoom` becomes `None`, x/y unchanged. Set indices 2, 3, 4 to `Null`: all three
`None` but the result is still `Some`. A null dest: `None`.

**`cpdf_annot_unittest.cpp:27-138` → `annot::quad`**
`rect_from_quad_points_array([0..15], 0)` → `(4, 5, 2, 3)`; index 1 →
`(4, 3, 6, 5)`.
`bounding_rect_from_quad_points`: empty dict → zero; a 3-element array → zero;
`[0..7]` → `(4, 5, 2, 3)`; the 24-element array → `(2, 3, 6, 7)`.
`rect_from_quad_points(dict, i)`: out of range → zero; the 24-element array at
0/1/2 → `(4,5,2,3)`, `(4,3,6,5)`, `(3,6,5,7)`.
`quad_point_count`: 0 for lengths 0..7, 1 for 8..15, 8 for 65.

**`cpdf_action_unittest.cpp:55-127` → `nav::action`**
All 18 spellings resolve, with `/Type /Action` present and with `/Type` absent.
`/Type /Lights` → `Unknown` for all 18. A **String** `/S` → `Unknown` for all
18, with and without `/Type`. `"Camera"`, `"Javascript"`, `"Unknown"` →
`Unknown`.

**`cpdf_defaultappearance_unittest.cpp:32-61` → `ap::da`**
Eleven `find_tag_param_from_start` cases asserting both the boolean and the
resulting parser position: `("", "Tj", 1)` → false/0; `("", "", 1)` →
false/0; `("  T j", "", 1)` → false/**5**; `("Tj", "Tj", 1)` → false/**2**;
`("(Tj", "Tj", 1)` → false/3; `("\r12\t34  56 78Tj", "Tj", 1)` → false/15;
`("\r\0abd Tj", "Tj", 1)` → true/**0**; `("12 4 Tj 3 46 Tj", "Tj", 1)` →
true/2; `("er^ 2 (34) (5667) Tj", "Tj", 2)` → true/5;
`("<344> (232)\t343.4\n12 45 Tj", "Tj", 3)` → true/11;
`("1 2 3 4 5 6 7 8 cm", "cm", 6)` → true/3.

**`cpdf_filespec_unittest.cpp:23-237` → `nav::filespec`**
Linux `encode`/`decode` identity for `"./docs/test.pdf"`,
`"../test_docs/test.pdf"`, `"/usr/local/home/test.pdf"`, `""`, `"test.pdf"`.
`GetFileName`: a String object round-trips; a dict with the five keys set in
reverse-precedence order changes answer each time; `/FS URL` returns the `/UF`
value **undecoded**; a Name object → empty; **Name-valued** `Unix`/`Mac`/`DOS`/
`F`/`UF` all → empty (crbug.com/959183).
`GetFileStream`: null for a non-dict, for a dict with no `/EF`, and for an
empty `/EF`; then the five-key precedence with `/FS URL` cutting the list to 2.
`GetParamsDict`: null without a stream; present once the stream dict has
`/Params`; `Size` reads `6`.

**`cpdf_formfield_unittest.cpp:71-336` → `form::attr`, `form::choice`**
`full_name(None)` → empty. The four-node cycle:
`root(T=foo)` alone → `"foo"`; with `Parent → dict1(T=bar)` → `"bar.foo"`;
adding an inline `dict2` with no `/T` → still `"bar.foo"`; `dict2 → dict3(T=qux)`
→ `"qux.bar.foo"`; closing the cycle `dict3 → root` → `root` still
`"qux.bar.foo"`, `dict1` → `"foo.qux.bar"`, `dict2` **and** `dict3` →
`"bar.foo.qux"`.
`IsItemSelected` over a 5-option list box (`Alpha Beta Gamma Delta Epsilon`),
sixteen cases: no `/V`/`/I` → nothing; `/V "Gamma"` → `{2}`; `/V "Omega"` →
`{}`; `/V ["Beta"]` → `{1}`; `/V ["Beta","Epsilon"]` → `{1,4}`;
`/V ["Beta","Epsilon","Omega"]` → `{1,4}`; `/I 3` → `{3}` with
`use_indices = true`; `/I 26` → `{}` with `use_indices = true`; `/I [0]` →
`{0}`; `/I [0,2,3]` → `{0,2,3}`; `/I [0,2,3,-5,12,42]` → `{0,2,3}`;
`/V ["Beta","Epsilon"]` + `/I [0,2,3]` (length mismatch) → `{1,4}` with
`use_indices = **false**`; `/V ["Alpha","Epsilon"]` + `/I [2,3]` (same length,
wrong values) → `{0,4}` false; `/V [3 strings]` + `/I [1,4]` → `{1,4}` false;
`/V [2]` + `/I [1,4,26]` → `{1,4}` false; `/V [3]` + `/I [0,2,3,26]` →
`{1,4}` false; `/V "Gamma"` + `/I 4` → `{2}` false. Indices `-1` and `5` are
never selected.

**`cpdf_pagelabel_unittest.cpp:169-204` → `page_label`**
The full three-level tree of §1.8 with 10001 pages; 24 assertions from
`GetLabel(-1) == None` through `GetLabel(10001) == None`, including the two
long repeated-letter strings (`abc` + 17×`N` at index 525; 103×`c` at 7654;
115×`j` at 7999).

**`cpdf_annotlist_unittest.cpp:76-126` → `annot::list`**
A `Text` annot with `/Contents {'A','a',0xE4,0xA0}` → list count **2**, the
popup's raw contents byte-identical, decoded to `"Aaä€"`. With the UTF-16BE
contents `{FE FF 00 41 00 61 00 E4 20 AC D8 3C DF A8}` → count 2, decoded
`"Aaä€🎨"`. Empty contents → count **1**. `{FE FF}` → count 1.
`{FE FF 00 1B 'j' 'a' 00 1B}` → count 1 (the language-code region strips to
nothing).

**`cpdf_nametree_unittest.cpp:108-179` → `nav::name_tree` (v1)**
`GetUnicodeNameWithBOM`: a hex key `<FEFF0031>` reads back as `"1"` from
`lookup_by_index(0)` and `lookup("1")` finds the value `100`.
`GetFromTreeWithLimitsArrayWith4Items`: with 4-element `/Limits` on an
intermediate and a leaf node, two `lookup` calls find `999` and `222` and
**leave both arrays at length 4**.

**`cpdf_nametree_unittest.cpp:181-415` → deferred to `pdfrum-edit` (D5)**
`AddIntoNames`, `AddIntoEmptyNames`, `AddIntoKids`, `DeleteFromKids` —
recorded verbatim in this brief's git history for the M7 port. The
`AddIntoKids` limit assertions (`"0.txt".."99.txt"` propagating up three
levels) and `DeleteFromKids`'s node-removal cascade are the valuable parts.

**`cpdf_interactiveform_unittest.cpp:23-76` → `form::load`**
Three `Btn` widgets whose `/T` is an indirect reference to, respectively, a
String, a Name and a Stream. After `Form::load`, the `/T` overrides are a
direct String `"good_string"`, an **empty** String, and an **empty** String.

**`cpvt_variabletext_unittest.cpp:44-199` → `vt`** (stub metrics: every char
width 10, ascent 10, descent −2, font index 0)
`LTRTextLayout`: `"hello"` at size 10 in a 100×100 plate — the five words in
logical order have `CaretX` `0.1, 0.2, 0.3, 0.4, 0.5` and strictly increasing
`location().x`.
`RTLTextLayout`: `שלום` — `CaretX` `0.3, 0.2, 0.1, 0.0` and strictly
**decreasing** `location().x`.
`GetLineCaretX`: empty text → `0.0` (Left), `50.0` (Center), `100.0` (Right);
`"hello"` → `0.0`; the Hebrew string → `0.4`. Each case also asserts the
iterator position is restored to `(0,0,-1)`.

**`cpvt_section_unittest.cpp:127-325` → `vt::place`, `vt::split`**
`ClearLeftWords`/`ClearRightWords`/`ClearMidWords`/`ClearAllWords` over
`"hello"` — these exercise the editing API we are dropping (D-note in §3), so
port them as tests of `Section::clear_words` only if that helper survives;
otherwise waive with a note.
`OutputLines` (parameterized, 6 cases): the visual orders of §1.13.5's table.
`OutputLines_Multiline_EnglishAndHebrew`: plate width 0.45, auto-return on →
3 lines, with words 0–3 and 4–7 each strictly increasing in x.
`OutputLines_EmptySection`: `Rearrange` on a wordless section yields
`height > 0`, **1** line, `begin == end == -1`, `width == 0.0`.
`OutputLines_ParagraphSeparator`: `[0x001D, 0x1D1D]` → 2 words with increasing
x (the zero-runs fallback).

### 4.2 Ported embeddertest assertions

These need real PDFs; the resource files live in
`pdfium-c++/testing/resources/` and are already in the golden store's input
set. Each becomes a `#[test]` that opens the file through
`pdfrum::Document::open` and asserts the value.

**Destinations** (`named_dests.pdf`): page indices `1, 1, 11, -1` for
`First / Next / FirstAlternate / LastAlternate`. View modes and params:
`XYZ(3) = [0,0,1]`, `Fit(0)`, `XYZ(3) = [200,400,800]`, `XYZ(3) = [0,0,-200]`.
`First`'s location: `has_x/y/zoom` all true, `(0, 0, 1)`. Six named dests, two
of which (indices 2, 3) have no resolvable dest. `NamedDestsByName`: `""` →
none, `"WrongType"` → none, `"Bogus"` → none.
`named_dests_old_style.pdf`: 2 dests, both resolving.
`bug_821454.pdf`: two links at (150,360) and (150,420) with z-orders 0 and 1,
both dests on page 0, both `has_zoom == false`, `(100,200)` and `(150,250)`.
`bug_1506.pdf`: `"First"` → page 3 under three different page-load orders.
`bug_680376.pdf`: the dest exists but resolves to `-1`.

**Actions**: `launch_action.pdf` → `Launch`, path `"test.pdf"`.
`uri_action.pdf` → `URI`, `"https://example.com/page.html"`.
`uri_action_nonascii.pdf` → the raw bytes `https://example.com/\xA5octal\xC7chars`.
`goto_action.pdf` → `GoTo` with a dest and no path/URI.
`gotoe_action.pdf` → `GoToE`, dest `Fit` with 0 params, path `"ExampleFile.pdf"`.
`nonesuch_action.pdf` → `Unknown`.
`get_page_aaction.pdf` page 0 `/AA /O` → `GoToE` with path
`\\127.0.0.1\c$\Program Files\test.exe`; page 1 has none.

**Bookmarks** (`bookmarks.pdf`): four nodes with titles
`"A Good Beginning"`, `"Open Middle"`, `"A Good Closed Ending"`,
`"Open Middle Descendant"`; counts `0, 1, -2, 0`; the first has neither `/Dest`
nor `/A`, the second has `/A` only, the fourth has `/Dest` only.
Case-insensitive find hits `"A Good Beginning"` and misses `"A BAD Beginning"`.
`bookmarks_circular.pdf`: find terminates and returns none.
`bookmarks_color.pdf`: five rejection titles → `None`, `"Valid Color Array"`
→ `(0.1, 0.2, 0.3)`.
`bookmarks_style.pdf`: `0, 0, 15, 0, 3`.
`utf-8.pdf`: first title `"Titlè 1"`.

**Page labels** (`page_labels.pdf`, 7 pages): `-2, -1` → none;
`i, ii, 1, 2, zzA, zzB, ""`; `7, 8` → none.
`about_blank.pdf`: no labels.

**Viewer prefs** (`viewer_ref.pdf`): `print_scaling == true`,
`num_copies == 5`, `duplex == "None"`, `generic_name("foo")` → none,
`generic_name("Foo")` → `"foo"`, `generic_name("HideToolbar")` → none
(Boolean), `generic_name("NumCopies")` → none (Integer),
`Direction == "R2L"`, `ViewArea == "CropBox"`, print page range
`[0, 2, 4, 4]`.
`about_blank.pdf`: the four defaults, and `print_page_range` → none.

**Annotations** (`annots.pdf`): page 0 has exactly nine annots in the order
`Link Link Link Link Highlight Highlight Popup Highlight Widget`; annot 3 is a
Link whose URI is
`"https://cs.chromium.org/chromium/src/third_party/pdfium/public/fpdf_text.h"`;
annot 4 (Highlight) has no link. Page 1 has 3 annots, index 2 a `Square`.
`annotation_highlight_long_content.pdf`: 1 Highlight; colour `(255,255,0,255)`;
`/T` = `"Jae Hyun Park"`; `/Contents` is 1344 characters ending
`"longLong long long. END"`; quad set 0 = `(115.802643, 718.913940)` …
`(157.211166, 706.264404)`.
`annotation_ink_multiple.pdf`: 3 annots, index 2 an `Ink` with colour
`(0,0,255,76)` and rect `(351.820435, 583.830750, 475.336121, 681.535034)`;
the three rect lefts are `86.1971, 149.8127, 351.8204`.
`annotation_highlight_square_with_ap.pdf`: 4 annots; annot 0 Highlight with
quads `(72.0, 720.792)…(132.055, 704.796)` and **derived rect**
`(67.7299, 704.296, 136.325, 721.292)` — this one is the AP-generation rect
rule and is the single best regression test for §1.13.8; annot 2 is a Square
with 0 attachment points. Its `/Popup` linked annot is on page **1** with
`rect.left == 612.0`, `rect.bottom == 578.792`.
`annotation_markup_multiline_no_ap.pdf`: 3 quad sets.
`annotation_highlight_rollover_ap.pdf`: flags = `Print` only.
`line_annot.pdf`: annot 0 `/Border` → `(0.25, 0.5, 2.0)`; annot 1's is too
short. `/L` → `(159.0, 296.0)` to `(472.0, 243.42)`.
`polygon_annot.pdf` / `ink_annot.pdf`: three vertices
`(159,296) (350,411) (472,243.42)` and `(259,396) (450,511) (572,343)`, with an
odd trailing element dropped in the second annot of each.
`redact_annot.pdf`: 1 `Redact`.
`bad_annots_entry.pdf`: count 1, annot 0 unresolvable.
`annotation_stamp_with_ap.pdf`: annot 0's Normal AP is **73970 bytes** with
MD5 `be903df0343fd774fadab9c8900cdf4a`; its `/AAPL:Hash` is the Name
`395fbcb98d558681742f30683a62a2ad`; `/M` is `"D:201706071721Z00'00'"`.
`text_form_multiple.pdf`: annots 0/1 have no `MaxLen`, annot 2 has `10`.

**Forms**: `multiple_form_types.pdf` field types
`-1, ComboBox, ListBox, TextField, CheckBox, RadioButton`.
`text_form.pdf`: field name `"Text Box"`; hit at `(120,120)`,
miss at `(0,0)`; font size `12.0`.
`text_form_multiple.pdf`: annot 0 no flags, annot 1 `ReadOnly`, annot 3
`TextPassword`; values `""` and `"Elephant"`.
`text_form_negative_fontsize.pdf`: `-12.0`.
`text_form_color.pdf`: font colour `(0xff, 0x80, 0x00)`.
`combobox_form.pdf`: flags `Combo|Edit`, `Combo`, `ReadOnly|Combo`; option
counts 3 and 26; labels `"Foo"`, `"Apple"`, `"Zucchini"` (index 25); values
`""` and `"Banana"`; field name `"Combo_Editable"`; selection: annot 0 nothing,
annot 1 `{1}`.
`listbox_form.pdf`: option counts 3 and 26; annot 3 `/I`-only → `{1,3}`;
annot 4 `/V`-array → `{2,4}`; annot 5 conflicting → `{0,2}`.
`click_form.pdf`: 8 annots; checked states
`[T,F,F,F,T,F,F,T]`; annot 3 → control count 3, index 1; annot 0 → count 1,
index 0; export values `"Yes"` (annot 0) and `"value2"` (annot 6); annot 0's
`/TU` is `"readOnlyCheckbox"`.
`annot_javascript.pdf`: the Format AA is
`AFDate_FormatEx("yyyy-mm-dd");`; the KeyStroke AA is present but empty.

**Struct tree**: `tagged_alt_text.pdf` → `Document` / `"TitleText"` /
`"symbol: 100k"` / `/Alt "Black Image"`; three levels each with one child and
MCID −1; `GetParent(root_child)` is **none**.
`tagged_actual_text.pdf` → `"Actual Text"`; the grandchild's single attribute
has 5 keys, `Height` a Number with `count_children == -1`, `BBox` an Array with
4 Number children.
`tagged_expansion.pdf` → `/E "Expansion"` on the child, absent on the
Document.
`tagged_table.pdf` → Document(`en-US`) → Table(`node12`, `hu`, `/Summary` =
present-but-empty) → 2 rows; row 0's `/ID` is present-but-empty and its
`/Lang` is **absent** (no inheritance); its first `TH` has 2 attribute objects,
one `{O: "Table", Scope: "Row"}` and one `{ColSpan: 2.0, O: "Table"}`; the
second `TR`'s `TD` has an attribute reached **through a reference** with 4 keys
including `ColProp` (String `"Sum"`, blob `S u m`), `CurUSD` (Boolean true) and
`RowSpan` (a **reference to** the Number 3).
`tagged_table_bad_elem.pdf` → the table is retrievable without `/Type`; a
grandchild has no `/Type` (→ empty `obj_type`); a great-grandchild has
`/Type /NotStructElem` (read back verbatim); `GetStringAttribute(table,
"Summary")` → empty because the attribute object is malformed.
`tagged_marked_content.pdf` → 4 children exercising the four `/K` shapes with
MCID counts `1, 1, 2, -1` and values `[0]`, `[1]`, `[2,3]`, `[-1]`.
`tagged_mcr_multipage.pdf` → the same element tree from both pages;
`child_marked_content_id(doc, 0)` is `0` on page 0 and `-1` on page 1, and vice
versa for index 1.
`marked_content_id.pdf` → root child MCID 0.
`tagged_nested.pdf`, `bug_1768.pdf`, `bug_1296920.pdf`,
`tagged_table_bad_parent.pdf` → must not panic; counts 1 each.

### 4.3 Hand-written tests pinning behavior with no C++ unittest

- **The two float formatters** (§1.13.1). A table test over
  `{0.0, -0.0, 0.5, -0.5, 0.25, 1.0, 12.0, 1e-7, 1e7, 123456789.0, f32::MIN_POSITIVE,
   f32::MAX, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 0.1, 1.0/3.0}` asserting
  formatter A gives `0 0 .5 -.5 .25 1 12 .0000001 10000000 123456792
  .000000000000000000000000000000000000011754944 340282350000000000000000000000000000000
  340282350000000000000000000000000000000 -340282350000000000000000000000000000000 0
  .1 .33333334` and formatter B gives
  `0 -0 0.5 -0.5 0.25 1 12 1e-07 1e+07 1.23457e+08 1.17549e-38 3.40282e+38 inf -inf nan
  0.1 0.333333`. (Generate the expected column once by running a tiny C++
  program against the oracle's own headers, and commit it with provenance —
  the cmap-blob precedent.)
- **The complete `--annot` emitter** against the golden store: a
  data-driven test walking `conformance/goldens/*/input.pdf.N.annot.txt` and
  byte-comparing our output. This is the Tier-A gate; it belongs in the
  conformance harness, but a 20-file smoke subset lives here as a nextest
  target so the crate is not green while `--annot` is broken.
- **The `--show-structure` emitter** likewise, over `*/structure.txt` —
  including the zero-byte case for untagged documents.
- **`GetBorderStyleInfo`'s first-byte dispatch**: `/S /Solid`, `/S /Dashed`,
  `/S /Beveled`, `/S /Inset`, `/S /Underline`, `/S /Squiggly` (→ Solid, since
  `'S'`), `/S /Dotted` (→ Dash), `/S` absent, `/S ()` → the five styles plus
  the width doubling for B/I.
- **`GenerateBorderAP` byte snapshots** (`insta`) for all five styles at
  width 1, 2 and 0 (empty), with a Transparent, Gray, RGB and CMYK colour.
- **Per-subtype AP snapshots** (`insta`): one minimal annotation dict per
  generator in §1.13.8, with the generated content stream as the snapshot. This
  is where formatter-A-vs-B mistakes surface immediately.
- **`GenerateEditAP` buffering**: a two-line, two-font layout asserting that a
  mid-line font change flushes `words` but not `line`, and that the resulting
  concatenation order matches. Hand-computed from §1.13.6.
- **The auto-font-size off-by-one** (D16): a plate that exactly fits step
  `k`, asserting the chosen size is step `k-1`; plus the multi-line 6-step cap.
- **`NeedDivision`'s eight rungs**: one case per `return` in
  `cpvt_section.cpp:150-174`, including the `word <= 0x0094` punctuation bug
  (`0x0085` and `0x0090` must both be punctuation).
- **`MakeLetters` cliffs**: `0 → ""`, `1 → "a"`, `26 → "z"`, `27 → "aa"`,
  `26_000 → ""` (the `% 1000` zero), `-1 → ""`.
- **`MakeRoman`**: `0 → ""`, `1 → "i"`, `4 → "iv"`, `1900 → "mcm"`,
  `1_000_000 → ""` (the modulus), `1_000_001 → "i"`, `-5 → ""`.
- **`GetAnnotAPInternal`'s ladder**: sixteen dict shapes — no `/AP`; `/AP` not
  a dict; `/AP /N` a stream; `/AP /N` a state dict with `/AS` naming a present
  state, a missing state, and absent `/AS` with `/V` on the annot, on the
  parent, and nowhere; `/AP /R` present-but-null with `fallback = true`; the
  same with `fallback = false`.
- **`UseSelectedIndicesObject`'s `/V` Number case**: a `/V` that is a Number
  leaves `values` empty, so an `/I` array of any length returns false. Not
  covered by the C++ unittest.
- **`GetFullNameForDict`'s empty-`/T` skip**: `a` → (no `/T`) → `b` yields
  `"a.b"`, not `"a..b"`.
- **`AddTerminalField`'s first-kid gate**: a field whose `/Kids[0]` is a
  Number is dropped entirely, kids 1..n included.
- **The name-tree `IsArrayWithTraversedObject` side effect**: two sibling
  leaves sharing one indirect `/Names` array — the second must see it as
  traversed and behave as if it had no `/Names`.
- **`FindLowerBound`'s `num >= max` re-descent** returning a key with a `None`
  value, and `page_label` then falling back to `index + 1` while ignoring the
  key.
- **Rect primitive parity** (§1.0.1): `deflate` on an inverted rect
  (normalizes first), `is_empty` on an inverted rect (does **not**), `contains`
  on the four edges.

### 4.4 Snapshot, fuzz, and conformance

**Snapshots (`insta`)**: the per-subtype AP streams (above); the
`--show-structure` output for `tagged_table.pdf` and `tagged_mcr_objr.pdf`;
the `--annot` output for `annots.pdf`, `annotation_ink_multiple.pdf` and
`annotation_highlight_square_with_ap.pdf`; a `Layout` debug dump for the six
bidi orderings.

**Fuzz targets** (`cargo-fuzz`, seeded from `pdfium-c++/testing/fuzzers/`):

- `fuzz_generate_ap` — an arbitrary annotation dict (built from a fuzzer-driven
  `Object` generator) through `ap::generate_appearances`. The oracle has
  `pdf_cpdf_generateap_fuzzer` (verify the name at M6) whose corpus seeds this.
- `fuzz_name_tree` — an arbitrary catalog through `named_dest` and
  `lookup_by_index` over `0..n`.
- `fuzz_vt_layout` — arbitrary text × plate rect × flags through `vt::layout`,
  asserting termination and no panic. The `SplitLines` rewind loop is the
  interesting target: `i = nWordStartPos` can revisit indices, and a
  pathological width could loop.
- `fuzz_struct_tree` — an arbitrary catalog + page dict through
  `StructTree::load_page` and the dump emitter.
- `fuzz_form_load` — an arbitrary `/AcroForm` through `Form::load`, exercising
  the `/Parent` walk and the trie.

All five must run clean for 24h before M6 exits.

**Conformance clusters** (`--triage` tags this crate owns):

| tag | gate |
|---|---|
| `annot-dump` | Tier A: `--annot` byte-exact ≥ 98% (PLAN.md M6) |
| `structure-dump` | Tier A: `--show-structure` byte-exact ≥ 98% |
| `annot-ap-markup` | Tier B: the ten `GenerateAnnotAP` subtypes' pixel tests |
| `form-appearance` | Tier B: the form-field pixel cluster |
| `form-needappearances` | Tier B: the five `NeedAppearances` files (E1) |
| `pagelabel` | Tier A, via `--show-metadata`-adjacent checks |
| `nav` | no direct dump; covered by the facade's API tests |

The `annot-dump` gate is the one that can move early: it depends on
`pdfrum-page`'s `build_page` only for the object *count*, so a page crate that
parses content correctly but rasterizes nothing already unblocks it.

---

## 5. Open questions

### Escalations (SPEC §0 — these change SPEC.md, PLAN.md or DEPS.md)

**E1 — `cpdfsdk_appstream` is in SPEC §10 but excluded by PLAN.md §1/§3.**
SPEC §10 says "**appearance-stream generation** (port `cpdfsdk_appstream`/
variable-text layout)". PLAN.md §1 says fpdfsdk "is mostly the C-ABI shim +
interactive widget UI (`pwl/`, `formfiller/`) that only matters with JS event
handling → **not ported**", and §3 gives `pdfrum-doc` no fpdfsdk row.

The facts (§1.15): it is oracle-reachable via `/NeedAppearances`; exactly
**five** corpus/resource files trigger it, all Tier-B pixel tests; it needs
`CPWL_EditImpl` — a *second* variable-text engine — plus `CPDF_BAFontMap` and
`CPDFSDK_Widget` geometry.

Options: **(a)** narrow SPEC §10 to `cpdf_generateap` + `cpvt_*` and waive the
five pixel tests with a documented threshold (my recommendation; ship
`ap/shapes.rs` and the `generate_widget_shapes` option so the door stays open);
**(b)** port the widget generators using `CPVT_VariableText` in place of
`CPWL_EditImpl`, accepting an unquantified pixel divergence on those five
files; **(c)** port `cpwl_edit_impl` too, which is a second 1000-line layout
engine for five files. Either (a) or (b) is a `[spec]` edit to SPEC §10.
**Default if unanswered: (a).**

**E2 — dict key ordering is Tier-A-visible in `--show-structure`.**
SPEC §2 states: "C++ stores dicts in a sorted `std::map` … we deliberately
keep insertion order … written-file byte layout is not an oracle target
(round-trip fidelity is semantic), so this divergence is accepted and
permanent."

That reasoning holds for the *writer*. It does not hold here:
`FPDF_StructElement_Attr_GetName(attr, i, …)` (`fpdf_structtree.cpp:379-405`)
returns the *i*-th key **in dict iteration order**, and `dump.cc:122-137`
prints the attributes in that order. The golden store confirms sorted output —
e.g. `9eec3b69ac6cbf8c/structure.txt` prints `O` then `Scope`, and
`5ff0e3d4c00be4d2/structure.txt` prints
`EndIndent, O, SpaceAfter, SpaceBefore, StartIndent, TextAlign, TextIndent,
WritingMode` — alphabetical, not the file's order. `CPDF_FormControl::
GetOnStateName` (§1.14.5) has the same exposure for multi-state `/AP /N`
dicts, though only for malformed files.

Options: **(a)** sort keys at the two emission sites only (a 2-line
`sort_unstable` in `structure/dump.rs` and `form/check.rs`) and add a sentence
to SPEC §2 recording that ordering *is* observable through these two APIs;
**(b)** make `Dict` sorted, which contradicts SPEC §2 and changes every crate.
**Recommendation: (a).** It is a `[spec]` edit to SPEC §2's parenthetical, not
a design change.

**E3 — `--annot` on an unknown or `Redact` subtype crashes the oracle.**
`AnnotSubtypeToCString` (`write.cc:53-131`) has no arm for
`FPDF_ANNOT_UNKNOWN` and none for `FPDF_ANNOT_REDACT`, falling into
`NOTREACHED()`. A corpus file with `/Subtype /Redact` (`redact_annot.pdf`
exists in `testing/resources`) would abort the tool. Question for the
orchestrator: **does the golden store contain an `.annot.txt` for a file with
an unknown or Redact subtype?** If yes, that golden is a crash artifact and the
harness needs a waiver; if no (likely — the generator would have failed), our
`Unknown`/`Redact` output is unconstrained and D13 stands. This is a
five-minute grep over `conformance/goldens/`, but it needs the golden-store
manifest to interpret, so I am escalating rather than guessing.

**E4 — `%.3f` and `%f` tie-breaking.** The `--annot` rectangles use `%.3f` and
the struct-tree numbers use `%f` (6 decimals), both on `double`-promoted
`float`s, with glibc's round-half-to-**even**. Rust's `{:.3}` rounds
half-**away-from-zero**. A `float` value landing exactly on a decimal-half
boundary at the 3rd or 6th place is possible (e.g. `0.0625` → `%.3f` is
`0.062` in glibc, `0.063` in Rust). The golden store is the arbiter: §4.4's
data-driven test over every `*.annot.txt` and `*/structure.txt` will find any
occurrence in one run. **If it finds one**, we need a hand-written
round-half-to-even fixed-point formatter (~30 lines) and this becomes a
divergence note rather than an escalation. Flagging it now so the M6 agent does
not assume `{:.3}` is safe.

**E5 — the bidi dependency.** `CPVT_Section::OutputLines` now calls
`CFX_BidiResolver`, which wraps **ICU's `ubidi_*`** (`core/fxcrt/
cfx_bidi_resolver.cpp`). DEPS.md chose `unicode-bidi` (Servo's) as the ICU
replacement and scoped it to *text extraction*; the text brief then noted it
"may end up unused there since PDFium's four-way bucket predates the UBA".
This changes that calculus: **AP generation genuinely needs the UBA**, with
`UBIDI_DEFAULT_LTR` paragraph resolution and per-line `ubidi_setLine` +
`ubidi_getVisualRun` reordering.

`unicode-bidi` provides `BidiInfo::new(text, Some(Level))` and
`visual_runs(para, line_range)`, which is the same shape. The open items:
(1) confirm `unicode-bidi`'s `visual_runs` matches ICU's run splitting for the
six pinned orderings of §1.13.5 — the `U+001D` zero-runs case in particular,
where ICU returns 0 runs and `unicode-bidi` may return 1; (2) confirm the
`kAuto` → `UBIDI_DEFAULT_LTR` mapping is `unicode_bidi::Level` `None` (auto
paragraph detection); (3) note that `text_direction_` is never set anywhere in
`core/`, so **only `kAuto` is reachable in the oracle** — the LTR/RTL forcing
paths are unittest-only and can be implemented without conformance risk.
Action: keep `unicode-bidi` pinned, add `pdfrum-doc` as a second consumer in
DEPS.md, and validate against the six orderings in M6 before writing
`vt/place.rs`. If it diverges, the fallback is a first-party UBA implementation
(~400 lines), which would be a `[spec]` change to DEPS.md.

### Questions resolved in this brief (recorded, not escalated)

**OQ-1 — Where does `--annot`'s "Number of objects" come from?**
Resolved: `FPDFAnnot_GetObjectCount` parses the *Normal* AP stream as a form
XObject and counts page objects (§1.12.2). So `pdfrum-doc` depends on
`pdfrum-page`'s `parse_content` + `build_page`, but only for a count — no
rendering. This is a real crate-graph edge (`pdfrum-doc → pdfrum-page`), which
PLAN.md §3's DAG already permits (fpdfdoc sits above fpdfapi/page).

**OQ-2 — Does `--annot` see AP-generation mutations?**
Resolved: **yes.** `pdfium_test` loads pages through `FORM_OnAfterLoadPage`,
which builds a `CPDF_AnnotList`, which runs `GenerateAPIfNeeded` on every
annotation before `WriteAnnot` reads the dicts (§1.12.1). Verified against the
golden store: `Text` annots show 20×20 rects. Hence D1's overlay.

**OQ-3 — `pdfrum-font` does not expose what the layout engine needs.**
§1.13.5's `Provider` needs three things the font brief's public API
(`docs/design/pdfrum-font.md:2926-2941`) does not have:

- `char_code_from_unicode(u: u32) -> CharCode` — `CPDF_Font::
  CharCodeFromUnicode` (ToUnicode reverse) plus the per-subclass fallbacks
  (`cpdf_simplefont.cpp:135-141` adds the encoding's reverse scan;
  `cpdf_cidfont.cpp:360-416` adds the CMap paths). The brief *documents* all of
  these (§1.9, §1.10.5) but does not export them.
- `char_width(code: CharCode) -> i32` — `CPDF_FaceBasedSimpleFont::
  GetCharWidth` (`:118-130`, with the `> 0xff → 0` clamp and the `0xffff`
  lazy-load sentinel) and `CPDF_CIDFont::GetCharWidth` (`:573-586`, with the
  `ansi_widths_fixed_` 500/0 special case). `CharItem::width` gives the width
  of a *decoded* item, not of an arbitrary charcode.
- `append_char(code: CharCode, out: &mut Vec<u8>)` — one byte for simple
  fonts, the CMap encoding for CID fonts (`cpdf_cmap.cpp:448-480`).
- `base_font_name() -> &[u8]` — for the `Symbol`/`ZapfDingbats` test in
  `GetPDFWordString`.

Plus `Font::load_standard(name)` for `GenerateFallbackFontDict`'s Helvetica —
the font brief already anticipates this ("Used by `pdfrum-doc`'s appearance
generation", `pdfrum-font.md:88-96`) but does not name the function.

**Proposal (additive to `pdfrum-font`, no shape change):** add these five to
the `impl Font` block. They are all already implemented internally (the brief
describes each algorithm); this is an export, not new behavior. It needs a
one-line addition to `docs/design/pdfrum-font.md`'s §3.1 API block when
`pdfrum-font` is implemented — flagging it here so the font agent adds them
rather than the doc agent discovering the gap at M6.

**OQ-4 — Does the `/NeedAppearances` widget path ever run in the oracle?**
Resolved: **no.** `CPDFSDK_PageView::LoadFXAnnots` sets the
`IsUpdateAPEnabled` global to `false` around the `CPDF_AnnotList` construction
(`fpdfsdk/cpdfsdk_pageview.cpp:595-600`), and `pdfium_test` always goes through
that path. So `cpdf_annotlist.cpp`'s `GenerateAP` (§1.11.1) is dead in the
oracle, while `cpdfsdk_appstream`'s path (E1) is live. D12 records the
resulting default.

**OQ-5 — Is `Limits` growing a field acceptable?**
`max_name_tree_depth: u32 = 32`. SPEC §1's field list is marked *(abridged)*
and §1 explicitly says "new fields are additive", so this needs no `[spec]`
commit — recording it here for the changelog.

---

## Appendix — C++ file inventory and disposition

| File | LOC | Disposition |
|---|---|---|
| `cpdf_generateap.cpp` | 1664 | §1.13 → `ap/` |
| `cpdf_interactiveform.cpp` | 1153 | §1.14.2 → `form/` (minus Win font code, FDF) |
| `cpdf_formfield.cpp` | 1008 | §1.14.3-.6 → `form/{field,check,choice}.rs` |
| `cpvt_variabletext.cpp` | 984 | §1.13.5 → `vt/` (minus ~400 lines of editing API) |
| `cpvt_section.cpp` | 828 | §1.13.5 → `vt/{classify,split,place,comb}.rs` |
| `cpdf_nametree.cpp` | 665 | §1.2.1 → `nav/name_tree.rs` (mutation API deferred, D5) |
| `cpdf_annot.cpp` | 516 | §1.10 → `annot/` |
| `cpdf_bafontmap.cpp` | 469 | **dropped** (§1.0, D6) |
| `cpdf_formcontrol.cpp` | 279 | §1.14.7 → `form/control.rs` |
| `cpdf_annotlist.cpp` | 282 | §1.11 → `annot/list.rs` |
| `cpdf_action.cpp` | 235 | §1.4 → `nav/action.rs` |
| `cpdf_filespec.cpp` | 213 | §1.5 → `nav/filespec.rs` |
| `cpdf_structelement.cpp` | 191 | §1.7.2 → `structure/element.rs` |
| `cpdf_defaultappearance.cpp` | 180 | §1.13.4 → `ap/da.rs` |
| `cpdf_structtree.cpp` | 178 | §1.7.1 → `structure/mod.rs` |
| `cpdf_dest.cpp` | 168 | §1.3 → `nav/dest.rs` |
| `cpdf_iconfit.cpp` | 141 | §1.14.7 → `form/control.rs` (data model only) |
| `cpdf_pagelabel.cpp` | 140 | §1.8 → `page_label.rs` |
| `cpdf_numbertree.cpp` | 131 | §1.2.2 → `nav/number_tree.rs` |
| `cpdf_apsettings.cpp` | 123 | §1.14.7 → `form/control.rs` |
| `cpdf_metadata.cpp` | 98 | **dropped** (D11) |
| `cpvt_fontmap.cpp` | 94 | §1.13.5 → collapsed into `FontMetrics` (D6) |
| `cpdf_action.cpp`'s AA half / `cpdf_aaction.cpp` | 71 | §1.4 → `nav/action.rs` |
| `cpdf_linklist.cpp` | 75 | §1.6 → `nav/link.rs` |
| `cpdf_bookmark.cpp` | 77 | §1.1 → `nav/outline.rs` |
| `cpdf_viewerpreferences.cpp` | 64 | §1.9 → `prefs.rs` |
| `cpdf_bookmarktree.cpp` | 47 | §1.1 → `nav/outline.rs` |
| `cpdf_color_utils.cpp` | 33 | §1.0.1 → `color.rs` |
| `cpdf_link.cpp` | 32 | §1.6 → `nav/link.rs` |
| `cpvt_stub_provider.cpp` | 32 | test-only → a `FontMetrics` stub in `vt/tests` |
| `cpdf_icon.cpp` | 30 | **dropped** (§1.0) |
| `cpvt_word{,info}.cpp` | 56 | records → `vt/mod.rs` |
| **out-of-tree but in scope** | | |
| `fpdfsdk/cpdfsdk_appstream.cpp` | 1937 | **E1** — shapes only (`ap/shapes.rs`), off by default |
| `testing/pdfium_test/write.cc` (annot half) | ~110 | §1.12.2 → `pdfrum-tool`'s `annot_dump.rs` |
| `testing/pdfium_test/dump.cc` (structure half) | ~120 | §1.7.3 → `structure/dump.rs` |
| `fpdfsdk/fpdf_doc.cpp` (semantics only) | ~150 | §1.1/§1.3/§1.6 — the `/Dest`-then-`/A` ladders and the colour clamp |
| `fpdfsdk/fpdf_annot.cpp` (semantics only) | ~250 | §1.12.2 — the colour defaults and attachment-point predicate |
| `fpdfsdk/fpdf_structtree.cpp` (semantics only) | ~200 | §1.7.3 — the attribute and MCID accessors |
| `core/fxcrt/cfx_bidi_resolver.cpp` | 120 | **E5** → `vt/bidi.rs` over `unicode-bidi` |
| `core/fpdfapi/edit/cpdf_contentstream_write_utils.cpp` | 141 | §1.13.1 → `ap/fmt.rs` |

Net: roughly **8 900 LOC of C++** in scope, of which ~1 700 is dropped
outright (bafontmap, metadata, icon, the VT editing API, the FDF exporter, the
Windows font paths). Estimated Rust: **6 000–7 000 LOC** including tests.

