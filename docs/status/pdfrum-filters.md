# `pdfrum-filters` status

**Updated:** 2026-08-29 · **State:** implemented, all gates green

Third library crate of M1. Contract: SPEC.md §4; behavior:
`docs/design/pdfrum-filters.md`.

## What landed

The whole of SPEC §4 plus the chain executor the brief §3.4 specified, in the
seven modules the brief's §3 module plan named:

| module | contents |
|---|---|
| `lib.rs` | `Filter` (10 variants), `from_name` / `canonical_name` / `is_image_codec` / `is_chainable`, `decode`, `DecodeOutput`, `NeedsImageCodec` |
| `ascii.rs` | `decode_ascii85`, `decode_ascii_hex` |
| `runlength.rs` | `decode_run_length`, `RUN_LENGTH_MAX_OUTPUT` |
| `flate.rs` | `decode_flate` over `miniz_oxide`'s streaming inflate |
| `lzw.rs` | `decode_lzw` over `weezl` |
| `predictor.rs` | `predictor`, `PredictorParams`, `PredictorKind` |
| `ccitt.rs` | `decode_ccitt`, `CcittParams`, `CcittImage` over `hayro-ccitt` |
| `chain.rs` | `decode_chain`, `decoder_list`, `validate_pipeline`, `DecodedStream` |

### The behaviors that mattered most

- **Flate's "inflate's return value is only a loop-exit condition"**
  (brief §1.4). `miniz_oxide::inflate::stream::inflate` with `MZFlush::None`
  reproduces it exactly: every one of the six C++ reference vectors matches,
  `b"preposterous nonsense"` included — empty output, two bytes read, success,
  which is what routes a corrupt stream to its own compressed bytes through
  the chain executor's fourth fallback. No error is ever returned for malformed
  input; only the output cap fails.
- **The four-rung fallback ladder** (brief §1.9, §3.4) is `decode_chain`, and
  every rung has a test: an invalid pipeline, a `/Filter` of the wrong type, a
  filter that fails mid-chain (which discards the chain's partial work, not
  just that stage's), and a chain that succeeds while producing nothing.
- **`/EarlyChange` as an arithmetic offset** (brief §1.5) maps cleanly onto
  `weezl`'s two decoder configurations — Q2 resolved in the brief's preferred
  direction, no hand-written LZW needed. `Configuration::with_tiff_size_switch`
  is `early_change = 1`, `Configuration::new` is `0`, and a test over a stream
  long enough to cross code 511 asserts the two genuinely disagree.
- **LZW's zero-byte-decode-is-failure quirk** is reproduced, because it is what
  routes a bare-EOD stream to the raw-bytes fallback rather than leaving a
  consumer with an empty buffer.
- **Predictors**, including the invalid-tag-is-a-verbatim-copy rule (tags 0, 7
  and 200 all take the same branch), the truncated-final-row behavior (the row
  count rounds up, and the short row is predicted over the bytes it has), and
  the geometry validation that runs *before* the predictor branch so a bad
  `/Colors` discards a perfectly good Flate stream.
- **CCITT's white prefill.** Rows start `0xff` and a row nothing decodes stays
  that way, so a truncated fax comes back short-but-white rather than as an
  error. `/DamagedRowsBeforeError` is not implemented, matching the C++.
  Row pitch is 32-bit-word aligned, unlike every other filter here.

## Tests

`cargo nextest run -p pdfrum-filters`: **105** unit tests.
`cargo test --doc -p pdfrum-filters`: **14** doctests.
Workspace additions: `pdfrum-common` gains 0 tests (one assertion updated).

Per module: `ascii` 10, `ccitt` 14, `chain` 22, `flate` 9, `lzw` 10,
`predictor` 22, `runlength` 12, crate-level 6.

All 17 test sets from the brief §4 are covered:

| set | where | notes |
|---|---|---|
| T1 `ValidateDecoderPipeline` | `chain` | all 12 shape cases plus the four indirect-reference cases, plus the brief's three added `/Crypt` cases |
| T2 `decoder_list` | `chain` | all seven, plus the added lone-dictionary-`/DecodeParms` case and its two siblings |
| T3 A85 | `ascii` | all eight rows, output **and** consumed count |
| T4 AHx | `ascii` | all eight rows, same |
| T5 Flate | `flate` | all six short vectors plus the 96-byte content stream, transcribed verbatim and asserted against its 111-character expansion |
| T6 RLE | `runlength` | the `RLEShortInput` case and the mixed-run round trip as decode vectors |
| T8 Flate corruption matrix | `flate` | all seven rows, including the zip bomb at a 1 MiB limit |
| T9 LZW | `lzw` | all six rows |
| T10 A85 malformation | `ascii` | all five rows |
| T11 AHx malformation | `ascii` | all four rows |
| T12 RLE truncation and cap | `runlength` | all six rows, including both sides of the `>=` boundary |
| T13 predictor parameters | `predictor` | all twelve rows |
| T14 PNG predictor | `predictor` | all six rows |
| T15 TIFF predictor | `predictor` | all six rows, Q3's case included and **not** `#[ignore]`d |
| T16 chain executor | `chain` | all nine rows (specified in the brief as a parser test; it lives here because the executor does) |
| T17 CCITT parameters | `ccitt` | all ten rows plus the truncated-stream row |

T7 (A85 *encode*) is `pdfrum-edit`'s, as the brief says.

Three brief test expectations were wrong and are corrected in the ported tests,
each with the reasoning in a comment:

- A85 `>` (0x3E) is *inside* `'!'..='u'`, so it is a code character, never a
  terminator on its own. The post-loop `>` check only ever fires on the `>` of
  a `~>`, because the `~` is what stopped the scan.
- RunLength `255` repeats **twice** (`257 - 255`), not three times.
- Paeth with equal-ish neighbours favours `a`, then `b`; the brief's prose is
  right, but hand-computed expectations for it are easy to get backwards.

## `[spec]` changes made

One `[spec]` commit, amending SPEC §4:

1. **The public surface is written out.** SPEC §4 named five items; the crate
   ships those plus `NeedsImageCodec`, `PredictorParams`/`PredictorKind`,
   `Filter::canonical_name`/`is_image_codec`/`is_chainable`, the seven
   per-filter entry points, and the CCITT types. All were implied by the brief
   but not by SPEC, and callers read SPEC.
2. **The chain executor lives here, not in `pdfrum-parser`.** SPEC §4 said
   "filter chains applied left-to-right by the caller"; the brief §3.4 then
   spelled out the algorithm the caller must implement, in this crate's brief,
   precisely because getting it wrong is invisible. Splitting the four
   fallbacks from the decoders whose return values they interpret is the shape
   SPEC §0 exists to prevent, so `decode_chain` is public here and the parser's
   stream accessor is a call to it. `decode` stays public for a caller holding
   one already-classified filter.
3. **RunLength's 20 MiB cap is a constant, not a `Limits` field.** The brief's
   Q1 proposed `max_runlength_len` beside `max_decoded_stream_len`. It is not
   configurable here: unlike the 1 GiB cap (which we invented), this one is a
   rejection the oracle really performs and files depend on, so making it
   tunable would let a caller silently change observable behavior.
4. **`NeedsImageCodec` carries no `data` field.** The brief §3.1 gave it one
   holding "bytes produced by the filters before this one", with the chain
   executor's pseudocode then returning `(vec![], Some(need))` so that the
   *caller* resolves empty-means-raw. That is the C++'s fallback 4 leaking into
   every consumer. `DecodedStream::data` is already the codec's input in both
   cases, so the field would be a second, sometimes-wrong copy; it is gone.
5. **`bytes_consumed` is reported by three filters, not none and not all.**
   Q4 proposed dropping it entirely. RLE, A85 and AHx return it — the
   inline-image reader needs it to find `EI`, and it is the sharpest way to pin
   those three decoders' edge cases — while Flate and LZW do not, exactly as
   Q4 argued.

## Additive change to `pdfrum-common`

`Limits.max_decoded_stream_len` already existed but defaulted to **20 MiB**
with a doc comment citing `kMaxStreamSize`. That conflated two different caps:
`kMaxStreamSize` is `RunLengthDecode`'s alone, while the field SPEC §4
describes is the crate-wide Flate/LZW ceiling with a **1 GiB** default. The
default and the doc comment are corrected; the field itself is unchanged, so
this is a value fix inside an already-agreed contract rather than a new
divergence. The RunLength cap moved to `pdfrum_filters::RUN_LENGTH_MAX_OUTPUT`
where it belongs.

## Open questions, resolved

- **Q1** (the `Limits` field): settled by SPEC §4's orchestrator decision at
  1 GiB; the field's default now matches. `max_runlength_len` declined, see
  `[spec]` item 3.
- **Q2** (`weezl` and `/EarlyChange`): `weezl` expresses both settings. No
  hand-written LZW.
- **Q3** (TIFF with `BytesPerPixel == 0`): **reproduced exactly**, as the brief
  proposed. For 2 or 4 bits per component with one colour the stride is zero
  and every byte doubles mod 256. The test asserts it rather than ignoring it,
  and the code comment says plainly that it is nonsense we are matching on
  purpose. Still worth a corpus grep for `/Predictor 2 /BitsPerComponent 4`
  once that sweep is cheap.
- **Q4** (`bytes_consumed`): see `[spec]` item 4.
- **Q5** (the scanline zero-fill): unchanged — a cross-crate obligation on
  `pdfrum-page`, restated here so it is not lost. A truncated image buffer must
  be zero-padded to `pitch * height` there.

## Not in scope here (next crates)

- **Fuzz targets.** The brief §4.3 asks for seven (`fuzz_flate`, `fuzz_lzw`,
  `fuzz_a85`, `fuzz_ahx`, `fuzz_rle`, `fuzz_predictor`, `fuzz_chain`) at a
  1 MiB target-side output limit. The `fuzz/` workspace does not exist yet;
  they land with it, alongside the parser's. In the meantime each
  byte-consuming entry point has a hand-written "arbitrary bytes never panic"
  test sweeping a few dozen synthetic inputs.
- **The corpus smoke check** (decode every stream of every corpus PDF, assert
  no panic and no `OutputTooLarge` at the 1 GiB default) needs `pdfrum-parser`
  to reach the streams. It belongs in the conformance harness once that lands.
- **Scanline decoding** stays out per brief divergence D3; the `bImageAcc`
  early punt has no analogue and `FlateDecode` images buffer whole.
