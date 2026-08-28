# Design Brief — `pdfrum-filters`

**Behavior source (read-only oracle):** `/mnt/data2/pdfium/pdfium-c++`
- `core/fpdfapi/parser/fpdf_parser_decode.h` / `.cpp` — filter-name resolution, chain
  validation, chain execution, A85/AHx/RLE decoders, the image-codec punt
- `core/fxcodec/flate/flatemodule.h` / `.cpp` — Flate, LZW, PNG + TIFF predictors
- `core/fxcodec/fax/faxmodule.cpp` — CCITT parameter clamping and error tolerance
- `core/fpdfapi/parser/cpdf_stream_acc.cpp` — the raw-data fallback on decode failure
- `core/fpdfapi/page/cpdf_streamparser.cpp` — inline-image decode limits
- `core/fxge/calculate_pitch.cpp` — the row-size (`pitch`) computation predictors use
- Tests: `core/fpdfapi/parser/fpdf_parser_decode_unittest.cpp`,
  `core/fxcodec/flate/flatemodule_unittest.cpp`,
  `core/fxcodec/basic/a85_unittest.cpp`, `core/fxcodec/basic/rle_unittest.cpp`

**Contract:** SPEC.md §4. **Style:** STYLE.md. **Deps:** DEPS.md (`miniz_oxide`,
`weezl`, `hayro-ccitt`; A85/AHx/RLE and both predictors written in-crate).

This brief is written to be sufficient on its own: an implementing agent should
need SPEC.md + STYLE.md + this file, and never the C++.

---

## 1. Behavior inventory

### 1.1 Filter-name resolution, including abbreviations

There is **no filter-name table** in PDFium. `PDF_DataDecode`
(`fpdf_parser_decode.cpp:446-534`) is a chain of string comparisons, and the
*terminal `else`* swallows everything unrecognized. The complete set of names
the chain executor recognizes:

| accepted names | action |
|---|---|
| `Crypt` | **skipped entirely** — `continue` (`:463-465`) |
| `FlateDecode`, `Fl` | flate decode (`:466-475`) |
| `LZWDecode`, `LZW` | LZW decode (`:476-480`) |
| `ASCII85Decode`, `A85` | A85 decode (`:481-484`) |
| `ASCIIHexDecode`, `AHx` | AHx decode (`:485-488`) |
| `RunLengthDecode`, `RL` | RLE decode (`:489-497`) |
| `BrotliDecode` | only under `PDF_ENABLE_BROTLI` (`:499-511`) — **out of scope** |
| **anything else** | stop, punt to the image path (`:512-522`) |

The punt branch also expands two abbreviations, **and only there**:
`DCT` → `DCTDecode`, `CCF` → `CCITTFaxDecode` (`:514-518`).
`CCITTFaxDecode`, `DCTDecode`, `JPXDecode`, `JBIG2Decode` are never named in
this function — they reach the image path purely by falling through, exactly
like a garbage name such as `/FooBar`. This has a consequence worth stating
plainly: **an unknown filter name is not an error.** It produces a "needs an
image codec named `FooBar`" result, and the consumer
(`CPDF_DIB::CreateDecoder`, `cpdf_dib.cpp:463-523`) is what finally fails when
no codec matches.

SPEC §4's `Filter::from_name` must therefore be understood as a *classification*
helper, not a gate: `None` means "not one of ours", which is the image-punt
case, not an error.

### 1.2 `/Filter` and `/DecodeParms` → a decoder list

`GetDecoderArray`, `fpdf_parser_decode.cpp:393-426`:

- No `/Filter` key ⇒ **empty list**, `Some(vec![])` — not an error (`:395-398`).
- `/Filter` neither an array nor a name ⇒ `None` (`:400-402`). The unittest pins
  a `/Filter` that is a *string* `"RL"` as invalid
  (`fpdf_parser_decode_unittest.cpp:212-217`).
- Name form: one entry, params = `/DecodeParms` resolved to a dict if it is one
  (`:419-423`). A `/DecodeParms` that is an array in the name form yields
  `pParams->GetDict()`, i.e. **null** — parameters are silently dropped.
- Array form: validated by `ValidateDecoderPipeline` first (`:409-411`), then
  entry `i` takes params from `/DecodeParms[i]` **only if `/DecodeParms` is
  itself an array** (`:413-417`). A single dict `/DecodeParms` alongside an
  array `/Filter` yields **null params for every entry**.
- `/Filter` elements are resolved through indirect references
  (`GetDirectObjectAt`), so `/Filter [ 5 0 R /LZW ]` works if object 5 is a name
  (unittest `:147-200`).

**`ValidateDecoderPipeline`** (`:95-122`) is the chain-shape gate:

1. Empty array ⇒ **valid** (`:96-99`).
2. Every element must resolve to a `Name`, else invalid (`:101-106`). This is
   checked for *all* sizes, including size 1.
3. A single-element array is **always valid** once the type check passes —
   including unknown names (`FooBar`) and image codecs (`DCTDecode`)
   (`:108-110`, unittest `:58-63`).
4. For arrays of size ≥ 2, every element **except the last** must be in this
   exact allowlist (`:113-115`):
   `FlateDecode`, `Fl`, `LZWDecode`, `LZW`, `ASCII85Decode`, `A85`,
   `ASCIIHexDecode`, `AHx`, `RunLengthDecode`, `RL`.

Consequences the unittests pin (`fpdf_parser_decode_unittest.cpp:46-145`):
- `[DCTDecode, CCITTFaxDecode]` ⇒ invalid (two image codecs).
- `[DCTDecode, FlateDecode]` ⇒ invalid (image codec not last).
- `[Fl, Fl, DCTDecode, Fl, Fl]` ⇒ invalid.
- `[RL, A85, Fl, LZW, DCTDecode]` ⇒ **valid** (image codec last).
- `[A85, A85]` ⇒ valid (repeats are fine).
- **`Crypt` is not in the allowlist.** So `/Filter [/Crypt /FlateDecode]` is
  *rejected* by the pipeline validator, even though `PDF_DataDecode` would
  happily skip the `Crypt` entry. `Crypt` is only tolerated as the sole filter
  or as the last one.

Validation failure ⇒ `GetDecoderArray` returns `None` ⇒ `CPDF_StreamAcc` uses
the **raw, undecoded** bytes (§1.9).

### 1.3 Chain execution and the `estimated_size` hint

`PDF_DataDecode` walks the list left to right, feeding each decoder the previous
one's output (`:454`, `:527-528`). Two details:

- **`estimated_size` is passed only to the last filter** (`:457`):
  `int estimated_size = i == nSize - 1 ? last_estimated_size : 0;`
  Every earlier filter gets 0. It is a *buffer-sizing hint only* (§1.4) and is
  consumed by Flate alone — LZW ignores it entirely.
- Any decoder returning `bytes_consumed == FX_INVALID_OFFSET` (`0xFFFFFFFF`,
  `core/fxcrt/fx_extension.h:24`) aborts the whole chain with `None`
  (`:523-525`).

`estimated_size` originates from two call sites:
- `cpdf_docpagedata.cpp:495-502` — an embedded font stream: `/Length1 + /Length2
  + /Length3`, saturating to 0 on overflow.
- `cpdf_dib.cpp:799-806` — an image: `CalculatePitch8(bpc, components, width) *
  height`, with the whole load abandoned on overflow.

Anything else passes 0.

### 1.4 FlateDecode

`FlateModule::FlateOrLZWDecode` → `FlateUncompress`, `flatemodule.cpp:496-548`.
zlib `inflate` with `Z_SYNC_FLUSH`, chunked:

- Initial chunk size, `EstimateFlateUncompressBufferSize` (`:488-494`):
  `guess = estimated_size != 0 ? estimated_size : src_len * 2`, then
  `min(guess, kMaxInitialAllocSize)` where **`kMaxInitialAllocSize = 10_000_000`**
  (decimal ten million, not 10 MiB). This is *only* the chunk size; the loop
  allocates further chunks of the same size until inflate stops producing
  output (`:512-522`).
- **There is no output cap for Flate.** The only ceiling is the saturation in
  `FlateGetPossiblyTruncatedTotalOut` (`:60-63`):
  `min(saturated_u32(total_out), kMaxTotalOutSize)` with
  **`kMaxTotalOutSize = 1024 * 1024 * 1024` (1 GiB)** (`:58`). Output beyond
  1 GiB is **silently truncated** to 1 GiB — the chunks were allocated, the
  reported size is clamped.
- `bytes_consumed` = `saturated_u32(total_in)` (`:65-67`), i.e. how much of the
  compressed input zlib actually read.

**Damage tolerance — this is the key behavior.** `FlateOutput` (`:93-104`)
returns `inflate(...) == Z_OK` but **the return value is only used as a
loop-exit condition, never as an error**. A `Z_DATA_ERROR`, `Z_STREAM_END`, or
`Z_BUF_ERROR` all simply stop the loop, and whatever bytes were produced so far
are returned as a **success** with a valid `bytes_consumed`. Concretely:

| input | output | `bytes_consumed` |
|---|---|---|
| `""` | `""` | 0 |
| `"preposterous nonsense"` (not a zlib stream) | `""` | **2** |
| a valid stream truncated mid-block | the prefix that inflated | bytes read |
| a valid stream with trailing garbage | the full content | bytes up to the stream end |
| a valid stream with a corrupt tail | the prefix before the corruption | bytes read |

The `"preposterous nonsense"` case is a real unittest assertion
(`flatemodule_unittest.cpp:19`): `'p' = 0x70`, `'r' = 0x72` is not a valid zlib
header, inflate consumes the 2 header bytes, fails, and PDFium reports **empty
output, 2 bytes consumed, success**. Downstream, `CPDF_StreamAcc` sees empty
data and falls back to the raw bytes (§1.9) — so a corrupt flate stream ends up
delivering its *compressed* bytes to the consumer. That chain is the single most
important damage-tolerance path in this crate.

`FlateOutput` also **zero-fills the unwritten tail** of each chunk (`:101`), so
a short final inflate leaves zeros rather than uninitialized memory. Since the
buffer is then truncated to `total_out`, this is not observable in
`FlateOrLZWDecode` output — but it *is* observable in the scanline path
(§1.11), where a starved scanline reads as all-zero.

### 1.5 LZWDecode

`CLZWDecoder`, `flatemodule.cpp:120-307`. This is a from-scratch LZW, not TIFF's
library, and it has behavioral details our `weezl` integration must match.

Fixed state: initial code width **9 bits**; a 4000-byte decode stack
(`:151`); a dictionary of **5021** `u32` entries (`:153`), each packing
`(prefix_code << 16) | append_char` (`:161`) — prefix codes are implicitly
capped at 16 bits. Codes are read **MSB-first** (`:216-237`). Output buffer
starts at 512 bytes and grows by `max(len/2, needed)` (`:195-205`).

**Special codes:** `< 256` = literal; **`256` = clear** (reset width to 9,
`current_code_ = 0`, forget `old_code`; the dictionary array is *not* zeroed,
just logically reset) (`:255-260`); **`257` = EOD** (`break`) (`:261-263`);
`>= 258` = dictionary reference.

**EarlyChange.** Read at `fpdf_parser_decode.cpp:377-380`:
`bEarlyChange = !!params.int("EarlyChange", 1)` — **default 1 (true)**, both
when `/DecodeParms` is absent and when the key is absent. It is stored as an
integer 0 or 1 and used as an **arithmetic offset**, not a boolean
(`flatemodule.cpp:152`, `:156-167`):

```
AddCode(prefix, ch):
    if current_code + early_change == 4094 { return }        // table full: stop adding
    codes[current_code++] = (prefix << 16) | ch
    if current_code + early_change ==  512 - 258 { width = 10 }   //  254
    if current_code + early_change == 1024 - 258 { width = 11 }   //  766
    if current_code + early_change == 2048 - 258 { width = 12 }   // 1790
```

`current_code` counts entries *beyond* 258, so the absolute next code is
`current_code + 258`. With `early_change == 1` the width grows one code early
(at absolute 511/1023/2047 instead of 512/1024/2048) — the PDF/TIFF-compatible
behavior, which is exactly what `weezl`'s TIFF variant implements. With
`early_change == 0` it grows at the "textbook" boundary; `weezl` exposes this
by choosing between its TIFF and GIF/LSB constructors, or via its
`with_tiff_size_switch` entry point — verify during implementation
(Open Question Q2).

**Table-full behavior** (`:156-158`): once `current_code + early_change == 4094`
(absolute 4095 with early change, 4096 without), `AddCode` returns without
adding. Decoding **continues at width 12 with a frozen dictionary** — there is
no error and no implicit clear. A stream that never emits code 256 simply
degrades.

**Error and truncation behavior** — every row here is damage tolerance:

| condition | behavior | `flatemodule.cpp` |
|---|---|---|
| fewer than `code_len` bits remain | `break` — partial trailing code **silently discarded**, decode ends successfully | `:216-218` |
| first code after start or after a clear is ≥ 258 | **`return false`** — the only outright rejection | `:266-268` |
| `code - 258 >= current_code` (the KwKwK case) | emit `last_char`, then expand `old_code`; no error | `:272-276` |
| prefix chain exceeds the 4000-byte stack | **silently truncated** to 4000 bytes, no error | `:181-183`, `:188-190` |
| prefix chain index out of range (cyclic/dangling) | chain walk terminates naturally | `:175-178` |
| `old_code >= 258 && old_code - 258 >= current_code` after emitting | `break` — decode ends successfully | `:299-301` |
| output buffer growth overflows `size_t` | clear the buffer, `return false` | `:196-201` |
| `dest_byte_pos + stack_len` overflows `u32` | `return false` | `:281-285` |
| **decode produced zero bytes** | **`return false`** | `:306` |

That last row is a genuine quirk: a valid LZW stream consisting of just an EOD
code decodes to nothing, and PDFium reports it as a **failure**, which cascades
to `FX_INVALID_OFFSET` → chain abort → raw-bytes fallback.

`bytes_consumed` on success = `(src_bit_pos + 7) / 8` (`:125`), i.e. the byte
containing the last bit read, rounded up.

**LZW has no output cap** beyond `size_t` overflow.

### 1.6 ASCII85Decode

`A85Decode`, `fpdf_parser_decode.cpp:124-209`. Two passes.

Pass 1 (`:129-141`) scans forward counting `z` characters and stops at the first
byte that is not: `z`, in `'!'..='u'`, a line ending, `' '`, or `'\t'`. Note
`~` (0x7E, the first byte of the `~>` terminator) is outside `'!'..='u'`
(`u` = 0x75), so the scan stops there. If the scan stops at position 0
(empty input, or input starting with an illegal byte) ⇒ **empty output,
0 bytes consumed** (`:143-145`).

Buffer sizing (`:147-157`): `space_for_non_zeroes = (pos - zcount) / 5 * 4 + 4`,
`size = zcount * 4 + space_for_non_zeroes`, with `FX_SAFE_UINT32` overflow ⇒
`FX_INVALID_OFFSET`.

Pass 2 (`:161-193`) accumulates base-85:
- whitespace (line ending, space, tab) skipped;
- `z` emits **four zero bytes** and resets the group — even mid-group, which is
  not legal ASCII85 but is accepted;
- a byte outside `'!'..='u'` ⇒ `break`;
- otherwise `res = res * 85 + (ch - 33)`; after the **fifth** byte
  (`state` reaches 4 then the 5th falls through) emit 4 bytes MSB-first
  (`GetA85Result`, `:58-60`).

**Partial-group handling** (`:194-203`): a trailing group of `state` bytes
(1..4) is padded with `(5 - state)` copies of the value 84 (i.e. `'u'`), and
`state - 1` bytes are emitted. This is the standard ASCII85 tail rule. A
trailing group of exactly **one** byte emits **zero** bytes.

**Terminator handling** (`:204-206`): after the loop, if the byte at `pos` is
`'>'`, consume one more. Note this checks for `>` *alone* — the `~` that
normally precedes it was what stopped the loop, and `pos` has already advanced
past it. So `~>` consumes both; a bare `>` after a legal character also
consumes.

Unittest assertions (`fpdf_parser_decode_unittest.cpp:268-294`), `(input,
output, bytes_consumed)`:

| input | output | consumed |
|---|---|---|
| `""` | `""` | 0 |
| `"~>"` | `""` | 0 |
| `"FCfN8~>"` | `"test"` | 7 |
| `"FCfN8~>FCfN8"` | `"test"` | 7 |
| `"\t F C\r\n \tf N 8 ~>"` | `"test"` | 17 |
| `"@3B0)DJj_BF*)>@Gp#-s"` (no terminator) | `"a funny story :)"` | 20 |
| `"12A"` (non-multiple) | `"2k"` | 3 |
| `"FCfN8FCfN8vw"` (stops at unknown) | `"testtest"` | 11 |

The last row is instructive: `v` (0x76) is past `u`, so the loop breaks at index
10, and then `src[10] == 'v' != '>'` so nothing extra is consumed — but pass 1
had already stopped at index 10 too. The reported 11 comes from `pos` being
post-incremented before the range check (`:165`). Reproduce by simulation, not
by reasoning about the arithmetic.

**No output cap.**

### 1.7 ASCIIHexDecode

`HexDecode`, `fpdf_parser_decode.cpp:211-254`.

Sizing pass: scan to the first `'>'` or end, allocate `i / 2 + 1` bytes
(`:216-222`). Decode pass (`:225-247`):
- whitespace skipped;
- `'>'` ⇒ consume it and `break`;
- **non-hex-digit bytes are silently skipped** (`:235-237`) — not an error, not
  a terminator;
- hex digits accumulate high-then-low nibble.

**Odd trailing nibble** (`:248-251`): if a high nibble was written but no low
nibble followed, the byte is kept with the low nibble **zero** — `"12A"` ⇒
`0x12 0xA0`. This matches ISO 32000's rule.

`bytes_consumed` is the loop index `i`, which is *past* the `'>'` when one was
found (`:232`).

Unittest assertions (`:296-322`):

| input | output | consumed |
|---|---|---|
| `""` | `""` | 0 |
| `">"` | `""` | 1 |
| `"\t   \r\n>"` | `""` | 7 |
| `"12Ac>zzz"` | `12 AC` | 5 |
| `"12 Ac\t02\r\nBF>zzz>"` | `12 AC 02 BF` | 13 |
| `"12A>zzz"` | `12 A0` | 4 |
| `"12tk  \tAc>zzz"` | `12 AC` | 10 |
| `"12AcED3c3456"` (no terminator) | `12 AC ED 3C 34 56` | 12 |

**No output cap.**

### 1.8 RunLengthDecode

`RunLengthDecode`, `fpdf_parser_decode.cpp:256-316`. Two passes, and the only
filter in this crate with an explicit output cap.

Pass 1 sizes the output (`:257-278`):
- byte `128` ⇒ **EOD**, stop;
- byte `n < 128` ⇒ literal run of `n + 1` bytes; advance `i += n + 2`;
- byte `n > 128` ⇒ repeat run of `257 - n` bytes (so `129` ⇒ 128 copies,
  `255` ⇒ 2 copies); advance `i += 2`.
- `u32` wraparound of `dest_size` ⇒ `FX_INVALID_OFFSET` (`:267-269`, `:273-275`).
- **`if (dest_size >= kMaxStreamSize) return FX_INVALID_OFFSET;`** (`:279-281`)
  with **`kMaxStreamSize = 20 * 1024 * 1024` (20 MiB)** (`:41`). Note the `>=`:
  exactly 20 MiB is rejected. **This is the only hard output cap in the whole
  filter path.**

Pass 2 fills (`:285-313`), and this is where truncation tolerance lives:
- A literal run whose source bytes run past the end of the input is **copied as
  far as possible and the remainder zero-filled** (`:292-304`):
  ```
  copy_len = n + 1;  buf_left = src.len - i - 1;
  if (buf_left < copy_len) { fill dest[dest_count + buf_left .. +delta] with 0; copy_len = buf_left; }
  ```
  Crucially `dest_count` still advances by the **full** `n + 1`, so the output
  keeps its pass-1 length.
- A repeat run whose fill byte is past the end uses **0** as the fill value
  (`:306`).

`bytes_consumed` = `min(i + 1, src.len)` (`:314-315`) — the `+1` accounts for
the EOD byte, clamped when the input ended without one.

The unittests (`rle_unittest.cpp:20-100`) are all encoder/decoder round trips;
port them as round trips against our own encoder if we ever write one, and
otherwise as direct decode vectors. The interesting decode cases (truncated runs,
missing EOD, the 20 MiB cap) are **not** covered by the C++ tests and are ours
to add (§4.4).

### 1.9 The `CPDF_StreamAcc` fallback ladder — where damage tolerance lands

`cpdf_stream_acc.cpp:124-169`. This is not strictly in `pdfrum-filters`, but the
crate's error contract is meaningless without it. Four **silent** fallbacks to
the raw, undecoded bytes:

1. raw stream size 0 ⇒ nothing loaded at all (`:126-129`);
2. `GetDecoderArray` returned `None` (bad `/Filter` type or invalid pipeline) or
   an empty list ⇒ **raw bytes** (`:146-151`);
3. `PDF_DataDecode` returned `None` (some decoder produced
   `FX_INVALID_OFFSET`) ⇒ **raw bytes** (`:153-158`);
4. decode "succeeded" but produced **empty** data ⇒ **raw bytes** (`:163-166`).

Fallback 4 is doing double duty. It is what delivers the compressed bytes when
flate chokes on the first byte (§1.4), *and* it is the normal path for an image
stream whose only filter is `DCTDecode`: `PDF_DataDecode` punts immediately with
empty `data`, so `GetSpan()` yields the raw JPEG bytes while `image_decoder_`
names the codec. The image encoding and params are recorded at `:160-161`
**before** the empty-data check, so they survive the fallback.

There is **no error channel to the caller at all** — a consumer sees only
`GetSpan()` / `GetSize()` and the image-decoder name.

### 1.10 The image-codec punt, precisely

`PDF_DataDecode`'s result (`fpdf_parser_decode.h:79-91`) is:
`data` (bytes decoded so far), `image_encoding` (a filter name, or empty), and
`image_params` (that filter's `/DecodeParms`).

Punt happens in three situations:

1. **Unrecognized name** (`:512-522`) — at any position in the chain. Remaining
   decoders are silently dropped. Abbreviations `DCT`/`CCF` expanded here only.
2. **`bImageAcc == true` and the filter is the last one** and is `FlateDecode`
   (`:466-471`) or `RunLengthDecode` (`:489-494`) — the decode is *deliberately
   skipped* so the image path can run a scanline decoder instead of buffering
   the whole image. `bImageAcc` comes from `CPDF_StreamAcc::LoadAllDataImageAcc`,
   used only by `cpdf_dib.cpp:806`.
3. Never for `Crypt`, `ASCII85Decode` or `ASCIIHexDecode` — those always run.

The consumer, `CPDF_DIB::CreateDecoder` (`cpdf_dib.cpp:463-523`), dispatches on
the recorded name: empty ⇒ already decoded; `JPXDecode`, `JBIG2Decode`,
`CCITTFaxDecode`, `FlateDecode`, `RunLengthDecode`, `DCTDecode` each to their
codec; **anything else ⇒ `LoadState::kFail`**. It also *overrides* the image's
declared depth (`ValidateDictParam`, `:982-1005`): `CCITTFaxDecode` and
`JBIG2Decode` force `bpc = 1, components = 1`; `DCTDecode` forces `bpc = 8`;
`JPXDecode` skips the bpc check entirely.

SPEC §4's `DecodeOutput::Image(NeedsImageCodec)` is exactly this result. The
`NeedsImageCodec` payload must carry the (expanded) filter name and the params
dict so `pdfrum-page` can dispatch identically.

### 1.11 Predictors

Both live in `flatemodule.cpp` and are applied **after** Flate *or* LZW, using
the same code (`:826-842`). Selection, `GetPredictor` (`:550-559`):
`predictor >= 10` ⇒ **PNG**; `predictor == 2` ⇒ **TIFF**; anything else
(including 1, 0, negative) ⇒ **none**. The specific PNG predictor value
(10–15) is *not* used — the per-row tag byte decides.

**Parameter defaults**, read at `fpdf_parser_decode.cpp:378-386`:
`Predictor` = 0, `Colors` = **1**, `BitsPerComponent` = **8**, `Columns` = **1**.
Validated by `CheckFlateDecodeParams` (`:43-56`):
- any of `Colors`, `BitsPerComponent`, `Columns` negative ⇒ **fail**;
- `Columns * Colors * BitsPerComponent` must not overflow `i32` and must be
  `<= INT_MAX - 7` ⇒ else **fail**.
Failure ⇒ `FX_INVALID_OFFSET` from `FlateOrLZWDecode` ⇒ chain abort ⇒ raw
fallback. Note these are checked **before** decoding, so a bad predictor param
discards even a perfectly good flate stream.

**Row size** is `CalculatePitch8(bpc, colors, columns)` =
`(bpc * colors * columns + 7) / 8` computed in checked `u32`
(`calculate_pitch.cpp:13-22`), returning `None` on overflow.

**PNG predictor** (`PNG_Predictor`, `:387-428`; `PNG_PredictLine`, `:339-385`):

- `row_size = pitch(bpc, colors, columns)`; **`row_size == 0` ⇒ fail**
  (`:392-395`).
- `src_row_size = row_size + 1` (the per-row tag byte).
- `row_count = (src_len + row_size) / src_row_size` — note the `+ row_size`,
  which **rounds up**, so a truncated final row still gets a row. `row_count ==
  0` ⇒ fail (`:403-406`).
- `dest_size = row_size * row_count`, minus `src_row_size - last_row_size` when
  `last_row_size = src_len % src_row_size` is nonzero (`:408-412`) — i.e. the
  short final row contributes only its actual bytes.
- `bytes_per_pixel = (colors * bpc + 7) / 8` (`:417`).
- Per row, `remaining_row_size = min(row_size, remaining_src.len - 1)` (`:419-420`)
  — a **truncated final row is predicted over the bytes that exist**, not
  rejected.

`PNG_PredictLine` filter tags (`:347-384`):
| tag | rule |
|---|---|
| 1 (Sub) | `out[i] = src[i] + out[i - bpp]` (0 when `i < bpp`) |
| 2 (Up) | `out[i] = src[i] + prev[i]` (0 when there is no previous row) |
| 3 (Average) | `out[i] = src[i] + (prev[i] + out[i - bpp]) / 2` — **integer division on the u8 sum**, i.e. `(up + left) / 2` where both are `u8` promoted to `int`; no overflow since max is 510 |
| 4 (Paeth) | `out[i] = src[i] + paeth(left, up, upper_left)` |
| **anything else, including 0** | **verbatim copy** (`:380-383`) |

Tag 0 (None) falling into the `default` copy arm is correct behavior, but note
that **an invalid tag such as 7 or 200 is also treated as None** — no error, no
diagnostic. The Paeth helper (`:328-337`) is the standard
`p = a + b - c; pa=|p-a|; pb=|p-b|; pc=|p-c|; pa<=pb && pa<=pc ? a : (pb<=pc ? b : c)`.

`GetUpValue` (`:315-317`) returns 0 when the previous row span is empty;
`GetUpperLeftValue` (`:319-326`) returns 0 when `i < bpp` **or** the previous
row is empty.

**TIFF predictor** (`TIFF_Predictor`, `:469-486`; `TIFF_PredictLine`, `:430-467`):

- `row_size = pitch(bpc, colors, columns)`; **`row_size == 0` ⇒ `return false`**
  ⇒ `FX_INVALID_OFFSET` (`:473-476`, `:838-841`).
- Operates **in place** on the decoded buffer, row by row, with the final row
  clamped to whatever bytes remain (`:479-484`) — no failure on a partial row.
- `bpc == 1` (`:434-451`): a bitwise XOR-accumulate across the row, bounded by
  `min(bpc * colors * columns, dest.len * 8)` bits.
- `bpc == 16` (`:454-461`): `BytesPerPixel = bpc * colors / 8`; big-endian u16
  addition, loop `for i in (BytesPerPixel..).step_by(2) while i + 1 < len`.
- otherwise (`:462-466`): `BytesPerPixel = bpc * colors / 8`; byte-wise
  `dest[i] += dest[i - BytesPerPixel]` for `i >= BytesPerPixel`.

Note the `bpc == 8` path computes `BytesPerPixel = colors`, and for `bpc == 2`
or `4` it computes `bpc * colors / 8` which is **0** when `colors` is small —
`for i in 0..len { dest[i] += dest[i - 0] }` would double every byte. The C++
loop is `for (size_t i = BytesPerPixel; i < dest_span.size(); i++)`, so with
`BytesPerPixel == 0` it starts at 0 and does `dest[0] += dest[0]`, then
`dest[1] += dest[1]`, … — every byte doubles (mod 256). Bizarre but
reproducible; see Open Question Q3.

**Failure semantics differ between the two** (`:826-842`):
- PNG failure ⇒ returns the **un-predicted** buffer with `FX_INVALID_OFFSET`
  (`:833-835`). The data is moved out but the sentinel aborts the chain.
- TIFF failure ⇒ same shape (`:838-841`).
Either way the caller discards the result and falls back to raw bytes.

**One more failure mode:** `PNG_Predictor` computes `dest_size` via
`Fx2DSizeOrDie(row_size, row_count)` (`:409`), which **aborts the process** on
overflow rather than returning an error. We return an `Error` (Divergence D4).

SPEC §4's `predictor(data, params) -> Result<Vec<u8>, Error>` covers both; note
the TIFF variant is in-place in the C++ and so should take ownership of the
`Vec` (which SPEC's signature already does).

### 1.12 CCITTFaxDecode parameters and tolerance

PDFium reads these in `CreateFaxDecoder`, `fpdf_parser_decode.cpp:318-342` — the
only site. Defaults are C++ initializers set *before* the `if (params)` guard,
so a **missing `/DecodeParms` entirely** yields the same values as a present-but-
empty dict:

| key | default | notes |
|---|---|---|
| `K` | `0` | `< 0` = pure G4; `0` = pure G3 1-D; `> 0` = mixed, one mode bit per row |
| `EndOfLine` | `false` | read as `!!int` |
| `EncodedByteAlign` | `false` | read as `!!int` |
| `BlackIs1` | `false` | read as `!!int`; applied by inverting the output buffer |
| `Columns` | **`1728`** | explicit two-arg default |
| `Rows` | `0` | **`Rows > 65535` ⇒ reset to `0`** (`:336-338`) |
| `DamagedRowsBeforeError` | — | **never read; PDFium does not implement it** |

Clamping happens in `FaxModule::CreateDecoder` (`faxmodule.cpp:666-692`):
```
actual_width  = Columns != 0 ? Columns : image_width
actual_height = Rows    != 0 ? Rows    : image_height
if actual_width <= 0 || actual_height <= 0 { return null }
if actual_width > 65535 || actual_height > 65535 { return null }
```
with `kFaxMaxImageDimension = 65535` (`faxmodule.cpp:54`). So a **negative
`Columns` passes the parser but is rejected here**, and `Columns == 0` means
"use the image's `/Width`". `Rows` and `Columns` **override** the image
dimensions when nonzero.

Row pitch is `CalculatePitch32OrDie(1, width)` (`faxmodule.cpp:584`) —
**32-bit-word aligned**, unlike Flate/RLE which use the byte-aligned
`CalculatePitch8`. `kFaxBpc = 1`, `kFaxComps = 1` (`:56-57`).

Error tolerance:
- `Rewind` fills the reference line with `0xFF` = **all white** (`:598-602`).
- `GetNextLine` fills the scanline with `0xFF` before decoding (`:604-648`), so
  **any row that fails to decode comes back all white** rather than erroring.
- `FaxGet1DLine` returns early on `bitpos >= bitsize` or on an invalid run code
  (`FaxGetRun` returning −1) (`:504-506`, `:516-523`) — the rest of the row
  stays white. **No error is propagated.**
- `EncodedByteAlign` has a self-disabling quirk (`:629-643`): while skipping to
  the byte boundary, if any set bit is found before the boundary,
  `byte_align_` is set to **false permanently** for the rest of the stream.
- `FaxSkipEOL` (`:482-494`) rewinds to the start position if the zero-run before
  the sync bit is `<= 11` bits, i.e. it was not a real EOL.
- `GetSrcOffset` = `min((bitpos + 7) / 8, src.len)` (`:650-653`).

Per DEPS.md, CCITT rides on `hayro-ccitt`. This section is the **behavioral
contract that integration must meet**, not a port plan: the parameter defaults
and clamps above are ours to implement in the wrapper, and the "damaged row
comes back white, never an error" property is what we must verify `hayro-ccitt`
provides (or emulate around it).

### 1.13 Inline images

`cpdf_streamparser.cpp` (referenced because it is the other consumer of these
decoders, and it has its own limits):
- `original_size = CalculatePitch8(bpc, components, width) * height` in checked
  `u32`; overflow ⇒ abandon the inline image (`:200-212`). With no colorspace
  object present, `bpc = 1, components = 1` (`:190-199`).
- **Unfiltered:** `original_size = min(original_size, remaining_bytes)`
  (`:215-220`).
- **Filtered:** the decoded byte count must fit in an `int`, which also rejects
  `FX_INVALID_OFFSET` (`:225-227`).
- `DecodeInlineStream` passes `estimated_size = orig_size` for `FlateDecode` but
  **hardcodes 0 for `LZWDecode`** (`:98-106`) — harmless, since LZW ignores it.
- Any unrecognized filter ⇒ `FX_INVALID_OFFSET` ⇒ inline image abandoned
  (`:142`). Note this is **stricter than the stream path**, which punts to the
  image codec instead.
- `kMaxStringLength = 32767` (`:43`) applies to content-stream string tokens,
  not to filters.

### 1.14 Every size cap, in one table

| constant | value | scope | source |
|---|---|---|---|
| `kMaxStreamSize` | `20 * 1024 * 1024` (20 MiB) | **RunLengthDecode output only**; rejected at `>=` | `fpdf_parser_decode.cpp:41`, used `:279` |
| `kMaxTotalOutSize` | `1024 * 1024 * 1024` (1 GiB) | Flate reported output size, **silently clamped** | `flatemodule.cpp:58` |
| `kMaxInitialAllocSize` | `10_000_000` | Flate initial chunk size **only**, not a cap | `flatemodule.cpp:490` |
| `kFaxMaxImageDimension` | `65535` | CCITT width and height | `faxmodule.cpp:54` |
| `Rows` clamp | `> 65535 ⇒ 0` | CCITT `/Rows` | `fpdf_parser_decode.cpp:336-338` |
| LZW decode stack | `4000` bytes | max expanded string length | `flatemodule.cpp:151` |
| LZW dictionary | `5021` entries | code table | `flatemodule.cpp:153` |
| LZW table-full | absolute code `4095`/`4096` | stops adding entries | `flatemodule.cpp:156` |
| predictor param guard | `cols*colors*bpc <= INT_MAX - 7` | pre-decode validation | `fpdf_parser_decode.cpp:55` |
| `kMaxImageDimension` (DIB) | `0x01FFFF` = 131071 | image `/Width`, `/Height` | `cpdf_dib.cpp:54-55` |

**Flate, LZW, A85 and AHx have no output cap.** That is a real property of the
oracle and a real zip-bomb exposure; see Divergence D2.

---

## 2. Divergences

**D1 — `decode` returns `Result`, and the raw fallback moves up a layer.**
PDFium's damage tolerance is spread across two files: the decoders return a
sentinel, and `CPDF_StreamAcc` turns it into "use the raw bytes". SPEC §4's
`decode` returns `Result<DecodeOutput, Error>`, so **the caller
(`pdfrum-parser`'s stream accessor) owns the fallback ladder of §1.9.** This
brief specifies the ladder precisely (§3.4) so the parser brief can implement it
without rediscovering it. The alternative — burying the fallback inside
`decode` — would make the function unable to distinguish "decoded to nothing"
from "gave up", which the image punt depends on.

**D2 — a real output cap, from `Limits`.** PDFium's absence of a Flate/LZW
output cap is a denial-of-service surface we decline to inherit. `Limits` gains
`max_decoded_stream_len`, defaulting to **1 GiB** to match `kMaxTotalOutSize`'s
effective ceiling, and exceeding it returns `Error::OutputTooLarge` rather than
silently truncating. Rationale: at 1 GiB the oracle's *reported* size stops
growing, so a document whose decoded stream exceeds 1 GiB already behaves
differently from its content in the C++; failing cleanly is closer to
observable behavior than truncating, and it is the only value that cannot
change any currently-passing corpus file. RunLength keeps its exact 20 MiB cap
(`kMaxStreamSize`, `>=`) as a *separate* filter-specific constant, because that
one is behaviorally load-bearing — files exist that rely on the rejection.
Adding a field to `Limits` touches SPEC §1; see Open Question Q1.

**D3 — no scanline decoders in v1.** PDFium's `FlateScanlineDecoder` /
`FlatePredictorScanlineDecoder` (`flatemodule.cpp:561-796`) exist so
`CPDF_DIB` can pull one image row at a time. SPEC §4 says "no streaming v1;
C++ also decodes to memory", and the `bImageAcc` punt (§1.10 case 2) exists
purely to feed them. In v1, `pdfrum-page`'s image path calls `decode` normally
and slices rows itself; the `bImageAcc` flag has **no analogue** in our API.
The observable consequence is memory, not output: a `FlateDecode` image is
buffered whole. Note this leaves one behavior unported — the zero-fill of a
starved scanline (`flatemodule.cpp:101`), which in the C++ makes a truncated
image render as black-to-the-bottom rather than short. `pdfrum-page` must
zero-pad a short decoded image buffer to `pitch * height` to match; recorded
here as a cross-crate obligation.

**D4 — no aborts.** `Fx2DSizeOrDie` (`flatemodule.cpp:409`) and
`CalculatePitch8OrDie` terminate the process on overflow. Per STYLE.md §3 these
become `Error` returns. Every one is reachable from attacker-controlled
`/DecodeParms`.

**D5 — LZW via `weezl`, CCITT via `hayro-ccitt`.** Per DEPS.md. §1.5 and §1.12
document the *behavior* those integrations must match. Two known gaps to verify
during implementation, both listed as open questions: `weezl`'s treatment of a
zero-byte decode (PDFium calls it a failure, §1.5) and of a frozen full table
(PDFium continues at width 12, §1.5). If `weezl` diverges, the wrapper adapts
its result rather than us hand-writing LZW.

**D6 — `Filter::from_name` classifies, it does not gate.** Per §1.1, `None`
from `from_name` means "image codec or unknown", which is a *successful* punt in
PDFium, not an error. To keep this from being a trap, the enum carries the
image codecs as real variants (SPEC §4 already lists `CcittFax`, `Jbig2`, `Dct`,
`Jpx`) and `from_name` returns `Some` for them; a genuinely unknown name returns
`None` and the *chain executor* turns it into `DecodeOutput::Image` with the
unrecognized name attached, exactly as the C++ does.

**D7 — abbreviation expansion is uniform.** PDFium expands `DCT` → `DCTDecode`
and `CCF` → `CCITTFaxDecode` **only** in the punt branch, so a
`/Filter /DCT` chain reports `DCTDecode` while `Filter::from_name` in our design
resolves both spellings up front. Since the expanded name is what the consumer
dispatches on, and PDFium always expands before recording it, the behavior is
identical; ours is just less accidental.

---

## 3. Module plan

```
crates/pdfrum-filters/src/
├── lib.rs        // Filter, decode(), DecodeOutput, Error — the whole surface
├── ascii.rs      // A85 + AHx (both ~60 lines, share the whitespace predicate)
├── runlength.rs  // RLE, two-pass with the 20 MiB cap
├── flate.rs      // miniz_oxide wrapper with the partial-output contract
├── lzw.rs        // weezl wrapper + the EarlyChange / zero-output adaptations
├── predictor.rs  // PNG + TIFF
└── ccitt.rs      // hayro-ccitt wrapper + PDFium's parameter defaults and clamps
```

### 3.1 `lib.rs` — public surface

```rust
#![forbid(unsafe_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    Flate, Lzw, AsciiHex, Ascii85, RunLength,
    CcittFax, Jbig2, Dct, Jpx,
    Crypt,
}

impl Filter {
    /// Resolve a /Filter name, including PDF's abbreviations.
    /// Returns `None` for a name we do not know — which is NOT an error:
    /// the chain executor treats it as an unknown image codec (see `decode`).
    #[must_use]
    pub fn from_name(n: &Name) -> Option<Filter>;

    /// True for filters that must be handed to the image path rather than
    /// decoded here: Dct, Jpx, Jbig2, CcittFax.
    #[must_use]
    pub fn is_image_codec(self) -> bool;
}
```

`from_name` table (both spellings for the five basic filters, per §1.1):

| name | variant |
|---|---|
| `FlateDecode`, `Fl` | `Flate` |
| `LZWDecode`, `LZW` | `Lzw` |
| `ASCII85Decode`, `A85` | `Ascii85` |
| `ASCIIHexDecode`, `AHx` | `AsciiHex` |
| `RunLengthDecode`, `RL` | `RunLength` |
| `CCITTFaxDecode`, `CCF` | `CcittFax` |
| `DCTDecode`, `DCT` | `Dct` |
| `JPXDecode` | `Jpx` |
| `JBIG2Decode` | `Jbig2` |
| `Crypt` | `Crypt` |
| anything else | `None` |

```rust
#[derive(Debug)]
pub enum DecodeOutput {
    Bytes(Vec<u8>),
    /// The chain stopped at a filter this crate does not decode.
    /// `data` holds whatever the preceding filters produced (empty if this was
    /// the first filter, in which case the caller uses the raw stream bytes).
    Image(NeedsImageCodec),
}

#[derive(Debug)]
pub struct NeedsImageCodec {
    /// The filter, if we recognize it. `None` for a name we do not know —
    /// the consumer then fails, exactly as CPDF_DIB does.
    pub filter: Option<Filter>,
    /// The name as written, with DCT/CCF expanded, for diagnostics.
    pub name: Name,
    /// That filter's /DecodeParms entry, already resolved to a dict.
    pub params: Dict,
    /// Bytes produced by the filters before this one.
    pub data: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("decoded output would exceed the {limit}-byte limit")]
    OutputTooLarge { limit: usize },
    #[error("run-length stream decodes to {size} bytes, over the 20 MiB limit")]
    RunLengthTooLarge { size: u64 },
    #[error("LZW stream is malformed: {0}")]
    LzwMalformed(&'static str),
    #[error("predictor parameters are invalid: {0}")]
    BadPredictorParams(&'static str),
    #[error("size computation overflowed")]
    SizeOverflow,
    #[error("CCITT parameters are out of range: {0}")]
    BadCcittParams(&'static str),
    #[error("CCITT decoding failed")]
    CcittFailed,
}
```

Entry points, per SPEC §4:

```rust
/// Decode ONE filter. Filter chains are the caller's business (SPEC §4).
pub fn decode(
    filter: Filter, input: &[u8], params: &Dict, r: &impl Resolve,
    limits: &Limits, diags: &mut Diagnostics,
) -> Result<DecodeOutput, Error>;

/// Apply a PNG or TIFF predictor to already-decompressed data.
pub fn predictor(data: Vec<u8>, params: PredictorParams) -> Result<Vec<u8>, Error>;
```

`decode` dispatch:
- `Crypt` ⇒ `Ok(Bytes(input.to_vec()))`. The chain executor's `continue`
  (§1.1) is equivalent to an identity decode; making it identity here keeps
  `decode` total.
- `Flate`, `Lzw`, `Ascii85`, `AsciiHex`, `RunLength` ⇒ their modules.
- `CcittFax`, `Dct`, `Jpx`, `Jbig2` ⇒
  `Ok(Image(NeedsImageCodec { filter: Some(f), .. }))` with `data` empty.
  Note `CcittFax` is in this list: PDFium punts it to the image path too
  (§1.1), and `ccitt.rs` is reached from `pdfrum-page`, not from `decode`.

`PredictorParams` is a plain record with `Default`, per STYLE.md §4:

```rust
#[derive(Debug, Clone, Copy)]
pub struct PredictorParams {
    pub kind: PredictorKind,
    pub colors: u32,             // /Colors,           default 1
    pub bits_per_component: u32, // /BitsPerComponent, default 8
    pub columns: u32,            // /Columns,          default 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictorKind { None, Tiff, Png }

impl PredictorParams {
    /// Read /Predictor, /Colors, /BitsPerComponent, /Columns with PDFium's
    /// defaults, and apply CheckFlateDecodeParams' validation.
    pub fn from_dict(d: &Dict, r: &impl Resolve) -> Result<Self, Error>;
}
```

`PredictorKind::from_predictor(n: i64)`: `n >= 10 ⇒ Png`, `n == 2 ⇒ Tiff`,
else `None` (§1.11). Validation in `from_dict`: negative `Colors`/`bpc`/
`Columns` ⇒ `BadPredictorParams`; `columns * colors * bpc` checked in `i32` and
`<= i32::MAX - 7` ⇒ else `BadPredictorParams`. **This runs even when
`kind == None`**, matching `CheckFlateDecodeParams` being called before the
predictor branch (§1.11).

### 3.2 `flate.rs`

`miniz_oxide`'s `inflate::stream::inflate` (the low-level streaming API) is the
right primitive because we need "give me whatever inflated before the error".
The high-level `decompress_to_vec_zlib` returns `Err` on truncation and
discards the partial output — unusable for §1.4.

```rust
/// Inflate `input`, returning whatever decompressed before any error.
/// `estimated_size` is a capacity hint only (0 = unknown).
/// Never fails on malformed input: a corrupt stream yields a short (possibly
/// empty) Vec, matching PDFium. Fails only on the output limit.
pub fn decode_flate(input: &[u8], estimated_size: usize, limits: &Limits,
                    diags: &mut Diagnostics) -> Result<Vec<u8>, Error>;
```

Implementation shape:
- initial capacity `min(if estimated_size != 0 { estimated_size } else { input.len() * 2 }, 10_000_000)`;
- loop: inflate into the tail of a growing `Vec`; stop on `Done`, on any error,
  or when no progress is made;
- on early stop with a nonempty result, push a
  `Diagnostic { severity: Recovered, what: DiagKind::TruncatedFlate, .. }`;
- on `output.len() > limits.max_decoded_stream_len` ⇒ `OutputTooLarge`.

Behavior table this must satisfy (from §1.4, and pinned by tests):

| input | result |
|---|---|
| `[]` | `Ok(vec![])` |
| `b"preposterous nonsense"` | `Ok(vec![])` + a `TruncatedFlate` diagnostic |
| a valid zlib stream | `Ok(content)`, no diagnostic |
| a valid stream truncated at any byte | `Ok(prefix)` + diagnostic |
| a valid stream with trailing garbage | `Ok(content)`, no diagnostic |

`bytes_consumed` is **not** part of our API. PDFium threads it around only to
signal `FX_INVALID_OFFSET`; nothing reads the real value except the inline-image
path (§1.13), which is `pdfrum-page`'s problem and can use `input.len()`.
Recorded as Open Question Q4.

### 3.3 `lzw.rs`

`weezl::decode::Decoder` with the TIFF/MSB configuration. Wrapper duties:

```rust
pub fn decode_lzw(input: &[u8], early_change: bool, limits: &Limits,
                  diags: &mut Diagnostics) -> Result<Vec<u8>, Error>;
```

- `early_change` read by the caller as `params.int("EarlyChange", 1) != 0`
  (default **true**, §1.5).
- `weezl` reports errors as a status; a truncated or invalid stream must yield
  the **prefix decoded so far**, plus a `Recovered` diagnostic — never an `Err`,
  matching §1.5's `break` cases.
- **`Err(LzwMalformed)`** only for the two genuine rejections: a dictionary code
  before any literal (`old_code` unset), and a zero-byte result. The zero-byte
  rule is odd but load-bearing: it is what routes an empty LZW stream to the
  raw-bytes fallback in the C++.
- Output cap from `limits`, as with Flate.

If `weezl` cannot express `early_change = false`, the fallback is a ~120-line
in-crate decoder following §1.5 exactly; DEPS.md permits writing code rather
than adding a crate, and the C++ algorithm is fully specified above. Decide
during implementation (Open Question Q2).

### 3.4 The chain executor — where it lives

SPEC §4 says "filter chains applied left-to-right by the caller". The caller is
`pdfrum-parser`'s stream accessor. To keep the behavior from being reinvented,
here is the algorithm it must implement, transcribed from §1.2, §1.3, §1.9 and
§1.10:

```
fn decoded_stream(stream, r, limits, diags) -> (Vec<u8>, Option<NeedsImageCodec>)
    raw = stream.data
    if raw.is_empty()                     { return (vec![], None) }
    filters = match decoder_list(stream.dict, r) {
        Err(_)                            => return (raw.to_vec(), None),   // fallback 2
        Ok(list) if list.is_empty()       => return (raw.to_vec(), None),   // fallback 2
        Ok(list)                          => list,
    }
    let mut cur = raw.to_vec();
    for (i, (filter_name, params)) in filters.enumerate() {
        let est = if i == filters.len() - 1 { estimated_size } else { 0 };
        match Filter::from_name(filter_name) {
            None => return image_punt(filter_name, params, cur),            // §1.10 case 1
            Some(f) if f.is_image_codec()
                 => return image_punt(filter_name, params, cur),            // §1.10 case 1
            Some(f) => match decode(f, &cur, params, r, limits, diags) {
                Err(_)          => return (raw.to_vec(), None),             // fallback 3
                Ok(Bytes(b))    => cur = b,
                Ok(Image(need)) => return (vec![], Some(need)),
            }
        }
    }
    if cur.is_empty() { return (raw.to_vec(), None) }                       // fallback 4
    (cur, None)
```

with `decoder_list` implementing §1.2 including `ValidateDecoderPipeline`, and
`image_punt` producing a `NeedsImageCodec` whose `data` is `cur` (empty on the
first filter, which is exactly what makes fallback 4 hand the raw bytes to the
image codec).

Two deliberate simplifications versus the C++:
- The `bImageAcc` early punt for `FlateDecode`/`RunLengthDecode` (§1.10 case 2)
  is omitted (Divergence D3). Those filters always decode.
- `Crypt` decodes as identity rather than being skipped, which is equivalent.

### 3.5 Data flow

```
Stream { dict, data } ──┬─→ decoder_list(dict) ──→ [(Filter, Dict)]
                        │                                │
                        └──────── raw bytes ─────────────┼──→ fallback on any failure
                                                          ▼
                             decode(f, bytes, params) per filter, left to right
                                    │                       │
                            Bytes ──┘                       └── Image(NeedsImageCodec)
                                    │                                    │
                                    ▼                                    ▼
                          predictor(bytes, params)              pdfrum-page image path
                                    │                              (DCT/JPX/JBIG2/CCITT)
                                    ▼
                          decoded stream bytes
```

`predictor` is applied *inside* `decode` for `Flate` and `Lzw` (matching
`FlateOrLZWDecode`, `flatemodule.cpp:826-842`) and is also public for direct
use — the xref-stream reader wants it without a flate round trip when
`/Filter` is absent but `/DecodeParms/Predictor` is set.

---

## 4. Test plan

### 4.1 Ported directly from `fpdf_parser_decode_unittest.cpp`

**T1 — `ValidateDecoderPipeline`** (`:46-145`). Every case in §1.2, restated
over `Vec<Object>`:
- `[]` valid; `[Name("FlateEncode")]` valid; `[Name("FooBar")]` valid;
- `[Name("AHx"), Name("LZWDecode")]` valid;
- `[Name("ASCII85Decode"), Name("ASCII85Decode")]` valid;
- `[A85, A85, RL, Fl, RL]` (5, all basic) valid;
- `[RL, A85, Fl, LZW, DCTDecode]` valid (image codec last);
- `[Str("FlateEncode")]` **invalid** (wrong type, even at length 1);
- `[DCTDecode, CCITTFaxDecode]` invalid;
- `[DCTDecode, FlateDecode]` invalid;
- `[Str("AHx"), Name("LZWDecode")]` invalid;
- `[Fl, Fl, DCTDecode, Fl, Fl]` invalid;
- `[A85, A85, RL, Fl, Str("RL")]` invalid.
Plus the indirect-reference cases (`:147-200`): a `Ref` to a `Name` behaves as
that name; a `Ref` to a `Str` is invalid; a `Ref` to `DCTDecode` in a non-last
position is invalid.

**Add** (not in the C++, but implied by §1.2): `[Crypt, FlateDecode]` is
**invalid**, `[Crypt]` and `[FlateDecode, Crypt]` are valid.

**T2 — `decoder_list`** (`:203-266`):
- no `/Filter` ⇒ `Ok([])`;
- `/Filter` as a `Str` ⇒ `Err`;
- `/Filter /RL` ⇒ one entry, `RL`;
- `/Filter []` ⇒ `Ok([])`;
- `/Filter [/FooBar]` ⇒ one entry, name preserved verbatim;
- `/Filter [/AHx /LZWDecode]` ⇒ two entries in order;
- `/Filter [/DCTDecode /CCITTFaxDecode]` ⇒ `Err`.
**Add:** `/Filter [/Fl /Fl]` with `/DecodeParms <<...>>` (a dict, not an array)
⇒ both entries get **empty** params (§1.2).

**T3 — A85** (`:268-294`). All eight rows of §1.6's table, asserting output
bytes **and** `bytes_consumed`. If `bytes_consumed` is dropped from our API
(Q4), keep it as an internal assertion in the unit test.

**T4 — AHx** (`:296-322`). All eight rows of §1.7's table, same treatment.

**T5 — Flate** (`flatemodule_unittest.cpp:16-46`). Six vectors, called with
`early_change=false, predictor=0, colors=0, bpc=0, columns=0, estimated=0`:

| input (hex) | output | consumed |
|---|---|---|
| *(empty)* | `""` | 0 |
| `"preposterous nonsense"` (ASCII) | `""` | **2** |
| `789c030000000001` | `""` | 8 |
| `789c53000000210021` | `" "` | 9 |
| `789c3334320600012d0097` | `"123"` | 11 |
| `789c63f80f0001010100` | `\x00\xff` | 10 |
| the 96-byte content stream at `:25-35` | the 111-char content stream at `:32-34` | 96 |

Transcribe the long case verbatim from the C++; it is a real page content
stream and exercises a multi-block inflate.

**T6 — RLE round trips** (`rle_unittest.cpp:20-100`). The C++ tests are
encoder→decoder round trips. Port them as **decode vectors** by running the C++
encoder's output shape by hand, or simply as round trips if we implement an
encoder in `pdfrum-edit`. The direct decode assertions worth keeping now:
- `[0, 1, 128]` ⇒ `[1]` (the `RLEShortInput` case, `:25-32`);
- `[2,2,2,2,4,4,4,4,4,4]` and its RLE form round-trip;
- 260-byte inputs exercising the >128 run split, all four match/non-match
  combinations (`:62-100`).

**T7 — A85 *encode*** (`a85_unittest.cpp:19-122`) is for `pdfrum-edit`, not this
crate. Noted so a later agent finds it: the `z` shortcut, the 1/2/3-leftover
tail lengths, and the 75/76-column line breaking with `\r\n` at offsets 75 and
153 of a 166-byte output.

### 4.2 Damage-tolerance tests (ours to write)

These are the highest-value tests in the crate; the C++ has none of them.

**T8 — Flate corruption matrix.** For a known-good zlib stream `S` of a
1000-byte payload:
| mutation | expected |
|---|---|
| `S[..0]` (empty) | `Ok(vec![])` |
| `S[..1]`, `S[..2]` | `Ok(vec![])` |
| `S[..k]` for k = 3, 10, half, len−1 | `Ok(prefix)`, prefix length monotonic in k, diagnostic recorded |
| `S` with byte 0 corrupted | `Ok(vec![])` + diagnostic |
| `S` with a mid-stream byte corrupted | `Ok(prefix)` + diagnostic, never a panic |
| `S ++ b"garbage"` | `Ok(full payload)`, **no** diagnostic |
| a 100 MiB zlib bomb with `max_decoded_stream_len = 1 MiB` | `Err(OutputTooLarge)` |

**T9 — LZW truncation and malformation.**
- a stream ending mid-code ⇒ the prefix, no error;
- a first code ≥ 258 ⇒ `Err(LzwMalformed)`;
- a stream that decodes to zero bytes (bare EOD) ⇒ `Err(LzwMalformed)`;
- `EarlyChange` 0 vs 1 over a stream long enough to cross the 511/512 boundary
  ⇒ **different** output; assert both against fixtures generated by a reference
  encoder (`weezl`'s encoder with and without the TIFF size switch);
- a stream that fills the dictionary past 4095 without a clear ⇒ decodes to
  completion at width 12, no error;
- a prefix chain longer than 4000 ⇒ truncated to 4000, no error.

**T10 — A85 malformation.** Beyond the ported vectors:
- input consisting only of `z` characters ⇒ 4 zero bytes per `z`;
- `z` appearing mid-group (illegal per spec) ⇒ 4 zeros, group reset;
- a group of exactly one trailing character ⇒ **no output byte**;
- input starting with an illegal byte ⇒ empty, 0 consumed;
- a very long run of `!` (which is 0) ⇒ no overflow, no panic.

**T11 — AHx malformation.**
- an odd number of hex digits ⇒ final low nibble zero;
- interleaved garbage (`"1g2h3i>"`) ⇒ garbage skipped, `0x12 0x30`;
- no terminator ⇒ consumes everything;
- input of only whitespace ⇒ empty.

**T12 — RLE truncation and the cap.**
- a literal run declaring 128 bytes with only 5 present ⇒ 5 bytes copied,
  **123 zero bytes appended**, total output length unchanged from the pass-1
  computation;
- a repeat run whose fill byte is missing (input ends after the count byte) ⇒
  the run fills with **0**;
- no EOD byte at all ⇒ decodes to the end, `bytes_consumed == input.len()`;
- a stream declaring exactly `20 * 1024 * 1024` output ⇒ `Err(RunLengthTooLarge)`
  (the `>=` boundary);
- a stream declaring `20 * 1024 * 1024 - 1` ⇒ `Ok`, length exactly that;
- a stream whose declared size overflows `u32` ⇒ `Err`, no allocation.

**T13 — predictor parameter matrix.** Table-driven over `PredictorParams::from_dict`:
| `/Predictor` | `/Colors` | `/BitsPerComponent` | `/Columns` | expected |
|---|---|---|---|---|
| absent | absent | absent | absent | `None`, (1, 8, 1) |
| 1 | — | — | — | `None` |
| 2 | 3 | 8 | 4 | `Tiff` |
| 10 | 3 | 8 | 4 | `Png` |
| 15 | 3 | 8 | 4 | `Png` (all of 10–15 are the same) |
| 100 | 1 | 8 | 1 | `Png` (`>= 10`) |
| 12 | **−1** | 8 | 4 | `Err(BadPredictorParams)` |
| 12 | 1 | **−8** | 4 | `Err(BadPredictorParams)` |
| 12 | 1 | 8 | **−1** | `Err(BadPredictorParams)` |
| 12 | `0x1_0000` | 32 | `0x1_0000` | `Err(BadPredictorParams)` (i32 overflow) |
| **0** | −1 | 8 | 1 | `Err` — validation runs even for `None` |
| 12 | 0 | 8 | 4 | `row_size == 0` ⇒ `Err` at predict time |

**T14 — PNG predictor.**
- each tag 0..=4 over a known 3-row, 3-color, 8-bpc image, comparing against
  hand-computed expected output;
- **tag 7 and tag 200 ⇒ verbatim copy**, no error (§1.11);
- a truncated final row (input length not a multiple of `row_size + 1`) ⇒ the
  short row is predicted over its actual bytes and the output is
  `row_size * (rows - 1) + last_row_len`;
- input shorter than one full row ⇒ one row of `input.len() - 1` bytes;
- `bpc = 16, colors = 3` ⇒ `bytes_per_pixel = 6`;
- `bpc = 1, colors = 1, columns = 8` ⇒ `row_size = 1`, `bytes_per_pixel = 1`.

**T15 — TIFF predictor.**
- `bpc = 8, colors = 3` ⇒ byte-wise add with stride 3;
- `bpc = 16, colors = 1` ⇒ big-endian u16 add with stride 2;
- `bpc = 1` ⇒ the XOR-accumulate path, bounded by `min(bits, len * 8)`;
- a final partial row is predicted over its actual bytes, no error;
- `row_size == 0` ⇒ `Err`;
- **the `bpc = 4, colors = 1` case where `BytesPerPixel == 0`** — assert
  whatever the oracle does (see Q3); until resolved, assert we do not panic and
  mark the test `#[ignore]` with a pointer to the question.

**T16 — the chain executor** (a `pdfrum-parser` test, specified here so it is
not forgotten). One test per fallback in §3.4:
- invalid pipeline ⇒ raw bytes out;
- `/Filter` a string ⇒ raw bytes out;
- flate failure mid-chain ⇒ raw bytes out (not the partial result);
- flate producing empty output ⇒ raw bytes out;
- `/Filter [/A85 /DCTDecode]` ⇒ `Image` with `data` = the A85 output;
- `/Filter /DCTDecode` ⇒ `Image` with empty `data`, so the caller substitutes
  raw bytes;
- `/Filter /FooBar` ⇒ `Image` with `filter: None`, `name: "FooBar"`;
- `/Filter /DCT` ⇒ `Image` with `name` reported as `DCTDecode`;
- `/Filter [/Fl /Fl]` where the inner flate output is itself flate ⇒ both run.

**T17 — CCITT parameters.** Against the `ccitt.rs` wrapper, before any decode:
| params | image w×h | resolved | expected |
|---|---|---|---|
| absent | 100×50 | K=0, cols=1728, rows=0, all flags false | width 1728, height 50 |
| `<< >>` | 100×50 | same as absent | width 1728, height 50 |
| `<< /Columns 100 >>` | 100×50 | cols=100 | width 100 |
| `<< /Columns 0 >>` | 100×50 | cols=0 ⇒ use image width | width 100 |
| `<< /Columns -1 >>` | 100×50 | passes the parser | `Err(BadCcittParams)` |
| `<< /Columns 70000 >>` | — | > 65535 | `Err(BadCcittParams)` |
| `<< /Rows 70000 >>` | 100×50 | **> 65535 ⇒ 0** ⇒ use image height | height 50 |
| `<< /Rows 20 >>` | 100×50 | rows=20 overrides | height 20 |
| `<< /K -1 >>` | — | pure G4 | — |
| `<< /BlackIs1 true >>` | — | output inverted | — |
Plus: a truncated G4 stream ⇒ the decoded rows plus **white** rows to fill,
never an error (§1.12). If `hayro-ccitt` errors instead, the wrapper catches it
and pads.

### 4.3 Snapshot, fuzz and conformance

- **Snapshots (`insta`):** none of these outputs are dump-shaped; skip. The one
  candidate is a predictor debug dump for a 3×3 image, which a plain assertion
  covers better.
- **Fuzz targets** (one per byte-consuming entry point, per SPEC §0):
  `fuzz_flate`, `fuzz_lzw`, `fuzz_a85`, `fuzz_ahx`, `fuzz_rle`,
  `fuzz_predictor` (bytes split into params + data), and `fuzz_chain` (bytes →
  a synthetic `/Filter` array + payload). All must run clean with
  `max_decoded_stream_len` set low enough that the fuzzer finds logic bugs
  rather than OOMs — **1 MiB** is the right target-side value. Seed corpora
  from `testing/fuzzers/` — note that `pdf_lzw_fuzzer.cc` targets the **GIF**
  LZW decompressor, not this one, so its corpus is only weakly relevant;
  `pdf_codec_a85_fuzzer.cc` and `pdf_codec_rle_fuzzer.cc` target the
  **encoders**. The genuinely useful seeds are the corpus PDFs' own stream
  payloads, extracted by a small harness script.
- **Conformance clusters:** `filters` is not a natural `--triage` cluster on its
  own — filter bugs surface as parse failures or image mismatches. The signal
  to watch is M1's "100% of corpus+resources load without crash" and M4's
  "decoded-image Tier-A ≥ 95%". A dedicated smoke check is worth adding: for
  every corpus PDF, decode every stream and assert no panic and no
  `OutputTooLarge` at the default 1 GiB limit.

---

## 5. Open questions

**Q1 — `Limits.max_decoded_stream_len`.** Divergence D2 proposes a new field on
`Limits` (SPEC §1 marks the struct *(abridged)*, so adding a field is in bounds,
but the *value* is a behavioral decision). Proposal: **1 GiB** default, matching
`kMaxTotalOutSize`'s effective ceiling, so no currently-decodable stream
changes behavior; the conformance harness may lower it. Also proposed:
`max_runlength_len = 20 * 1024 * 1024` as a separate constant, since RunLength's
cap is a *rejection* the oracle actually performs and files may depend on it.
**Needs a decision** before implementation, because the default value is
observable.

**Q2 — `weezl` configuration for EarlyChange.** §1.5 specifies the exact
semantics (an arithmetic offset that shifts the width-growth boundary by one
code). `weezl` implements the TIFF variant, which is the `early_change = 1`
behavior; whether it can be configured for `early_change = 0` needs checking
against the crate's API. Two additional adaptations are needed regardless: a
zero-byte decode must become `Err` (§1.5), and a truncated stream must yield its
prefix rather than an error. Resolve by reading `weezl`'s API during
implementation; the fallback (a ~120-line in-crate decoder, fully specified by
§1.5) is acceptable under DEPS.md and does not need escalation.

**Q3 — the TIFF predictor with `BytesPerPixel == 0`.** For `bpc ∈ {2, 4}` with
small `colors`, `bpc * colors / 8` is 0, and the C++ loop then does
`dest[i] += dest[i]` for every byte — doubling each byte mod 256 (§1.11). This
is almost certainly unintended upstream. Options: (a) reproduce it exactly;
(b) treat `BytesPerPixel == 0` as 1. Proposal: **reproduce exactly** (it costs
nothing and is what the oracle does), but flag it, because a corpus file with
`/Predictor 2 /BitsPerComponent 4` would otherwise silently diverge. Resolve by
grepping the corpus for such a dict once the oracle is built; if none exists,
either choice is safe and (a) stands.

**Q4 — does anything need `bytes_consumed`?** PDFium threads it through every
decoder, but the only consumer of the *value* (rather than the
`FX_INVALID_OFFSET` sentinel) is the inline-image path, which needs to know
where `EI` begins (`cpdf_streamparser.cpp:225-227`). Proposal: keep it out of
`decode`'s signature (SPEC §4 has no place for it) and give `pdfrum-page`'s
inline-image reader a separate `decode_inline(filter, input, ...) -> (Vec<u8>,
usize)` entry point when it needs one. **Escalate if** the inline-image work
finds that `EI` detection genuinely requires the exact consumed count from
Flate — miniz_oxide reports `total_in`, so it is obtainable, just not through
this API.

**Q5 — the missing scanline zero-fill.** Divergence D3 drops the scanline
decoders, which means we also drop `FlateOutput`'s zero-fill of a starved
scanline (`flatemodule.cpp:101`). The equivalent behavior — a truncated image
buffer zero-padded to `pitch * height` — must land in `pdfrum-page`'s image
path instead. Recorded here as a cross-crate obligation rather than a question,
but flagged because it is exactly the kind of behavior that gets lost between
two briefs. The `pdfrum-page` brief must pick it up.
