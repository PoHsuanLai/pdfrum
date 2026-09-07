# Upstream issue draft — `hayro-jbig2`

Ready to file against the `hayro-jbig2` repository. Everything below the rule
is the issue text; nothing above it is meant to be posted.

**Status:** drafted, not yet filed. **Re-checked 2026-09-05:** hayro-jbig2 0.3.0 is still
the latest release, and no issue in the hayro tracker mentions segment
lengths or truncated streams — the draft stands as written. The corpus file it comes from is
`testing/resources/pixel/bug_867501.pdf` in the PDFium tree, which renders at
SSIM 0.646 against PDFium's own output and is the only file in our 1675-file
corpus whose residual is a codec gap. **Re-verified 2026-09-08** against
`hayro-jbig2` 0.3.0: the segment table below was re-derived from the stream's
actual bytes, `Image::new_embedded` was observed returning
`Parse(UnexpectedEof)`, and a one-field control isolates the zero declared
length as the cause on its own.

**Repro files:** `bug_867501.pdf` in `../repro/`.
**Confidence:** observed output, against `hayro-jbig2` 0.3.0.
**Host:** Ubuntu 22.04.5 LTS, x86-64.

**Why it is not fixed locally.** The behaviour PDFium relies on is structural —
where segment bodies are read from, and when a truncated stream is an error —
and neither half can be reached from a wrapper around the crate's public API.
Both halves are small, both look like defects rather than design, and a
first-party JBIG2 port is not justified by one file (asks for the
narrowest faithful fix). So the fix belongs upstream.

---

## Truncated JBIG2 stream: segment bodies are read at their declared length, not continuously

### Summary

`hayro-jbig2` refuses a JBIG2 embedded stream that PDFium decodes
successfully. Two independent behaviours are involved, and a file can need
either or both:

1. **Segment bodies are sliced to their declared data length.** A segment
   header that declares a body length of `0` yields an empty body, and
   `parse_page_information` (and the region parsers) return `UnexpectedEof`.
   PDFium ignores the declared length entirely for this purpose and parses
   every segment body from one continuous stream, at the offset where the
   header ended.
2. **A truncated segment sequence is an error rather than an end.** PDFium
   stops when fewer than eleven bytes remain — the minimum segment header size
   — and reports success for whatever it decoded. `parse_segments_sequential`
   loops to `at_end()` and propagates the error, discarding a partially
   decoded page.

Real-world JBIG2 embedded in PDF appears to rely on both. A PDF viewer is
expected to render what a damaged image yields rather than drop the image, so
the practical effect is that a page renders blank where other viewers show
content.

### Minimal reproduction

The whole codestream is 77 bytes (this is the `/JBIG2Decode` stream of
PDFium's `testing/resources/pixel/bug_867501.pdf`, a 1x3 `/DeviceGray` image):

```
00 00 00 00 30 00 00 00 00 00 00 00 00 00 00 26
00 00 00 00 00 00 00 00 00 01 00 00 00 02 00 00
00 00 00 00 00 00 00 02 00 00 00 00 00 00 00 00
00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00 00 00 00 00 00 00 00 00 00 00 00 00
```

base64:

```
AAAAADAAAAAAAAAAAAAAJgAAAAAAAAAAAAEAAAACAAAAAAAAAAAAAgAAAAAAAAAAAAAAAAAAAAAA
AAAAAAAAAAAAAAAAAAAAAAAAAAA=
```

Its three segment headers are:

| offset | segment | type | declared data length | note |
|---|---|---|---|---|
| 0 | page information | 48 | **0** | body would be 19 bytes |
| 11 | immediate generic region | 38 | **0** | body is a 1x2 region at (0, 0) |
| 22 | symbol dictionary | 0 | **33 554 432** | only 44 bytes remain |

The image is declared 1 wide and 3 tall.

```rust
// with `hayro-jbig2 = "=0.3.0"` as the only dependency
fn main() {
    let data: &[u8] = /* the 77 bytes above */;
    match hayro_jbig2::Image::new_embedded(data, None) {
        Ok(img) => println!("OK {}x{}", img.width(), img.height()),
        Err(e) => println!("ERR {e:?}"), // observed: ERR Parse(UnexpectedEof)
    }
    // expected: Ok(_) whose 3 rows decode white, black, white
}
```

The failure happens inside `new_embedded`, before any region is decoded: it
calls `parse_segments_sequential` and propagates that error, so `from_segments`
and the page bitmap are never reached.

No prefix of this stream parses either — the longest run of whole segments is
the 22 bytes holding only the two zero-length ones, and truncating the input to
22 or 33 bytes gives the same `Parse(UnexpectedEof)` — so feeding a prefix is
not a workaround.

**The declared length alone is enough to cause it.** Building a single,
otherwise well-formed page-information segment — an 11-byte header followed by a
correct 19-byte body declaring a 1x3 page — and varying only the declared data
length isolates the behaviour from everything else in the file:

| declared data length | result |
|---|---|
| 19 (correct) | `Ok`, `width = 1`, `height = 3` |
| 0 | `Err(Parse(UnexpectedEof))` |

The bytes are identical apart from that one field, and neither case has a
segment that overruns. So the rejection is caused by the zero-length body being
sliced to nothing, independently of the third segment's oversized declaration —
the two behaviours in the summary above are separable, and this is the first.

### Expected output

PDFium produces a 1x3 greyscale bitmap reading **`255 0 255`** — white, black,
white. Rendered into the page (`94.78 x 450` device pixels) that is a white
top third, a black middle third, and a white bottom third, which is what
`testing/resources/pixel/bug_867501_expected.pdf.0.png` in the PDFium tree
contains.

How PDFium reaches it, for reference:

- `ParseSegmentData` reads each body from the shared stream at the offset the
  header ended, so the zero-length page-information segment still consumes its
  19 bytes (from offset 11) and the zero-length region segment still reads its
  own (from offset 22), where it finds a generic region **1 pixel wide and 2
  tall at (0, 0)**.
- The third segment's declared length runs the offset past the end;
  `while (getByteLeft() >= kMinSegmentSize)` ends the loop and
  `DecodeSequential` **returns success**.
- The destination began zeroed and is inverted at the end, so the third row —
  which no region ever reached — comes back white.

### Suggested fixes

The two halves are independent and either is useful alone:

- **Continuous bodies.** Parse each segment body from the position the header
  ended rather than from a slice of the declared length, at minimum when the
  declared length is `0` or would overrun the stream. (The JBIG2 spec's
  "unknown length" convention is `0xFFFFFFFF`; a declared `0` on a segment
  type whose body cannot be empty is in practice a producer bug that PDFium's
  reading tolerates.)
- **Truncation as an end.** In `parse_segments_sequential`, stop when fewer
  bytes remain than a segment header needs and return the page decoded so far,
  instead of propagating the parse error. This matches PDFium and, we believe,
  the general expectation that a partially decoded JBIG2 image renders.

### Context

Found while building a Rust PDF engine that uses `hayro-jbig2` for
`/JBIG2Decode`, checked against PDFium as an oracle over a 1675-file corpus.
This is the only file in that corpus where the two disagree on JBIG2, so the
gap is narrow — but it is a genuine "renders in every other viewer, blank
here" case.
