# `pdfrum-text` unicode tables — provenance

`unicode.bin` is a **committed build product**. `cargo build` only reads it;
nothing in a normal build touches the C++ oracle. This file records where every
byte came from and how to reproduce it, so the blob is auditable rather than
opaque (the arrangement `crates/pdfrum-cmap/tables/PROVENANCE.md` sets out for
the CJK CMaps).

Oracle checkout: `/mnt/data2/pdfium/pdfium-c++` @ `6f2272e`.

## What is in the blob

| Section | Rows | Source |
|---|---|---|
| `UCDR` | 65 536 packed `(mirror << 5) \| bidi_class` words, run-length encoded | `core/fxcrt/fx_ucddata.inc`, read through the `CHARPROP____` macro's first and third arguments; the bidi-class numbering is `BidiClass` in `core/fxcrt/fx_unicode.h` |
| `MIRR` | 366 mirror characters | `kFXTextLayoutBidiMirror` in `core/fxcrt/fx_unicode.cpp` |
| `NRMR` | 65 536 normalization index words, run-length encoded | `kUnicodeDataNormalization` in `core/fpdftext/unicodenormalizationdata.cpp` |
| `NRM1`…`NRM4` | 5 376 / 1 724 / 1 164 / 488 words | `kUnicodeDataNormalizationMap1`…`Map4`, same file |
| `ALPH` | `u_isalpha` code-point ranges | the oracle's ICU (see below) |
| `ALNM` | `u_isalnum` code-point ranges | ditto |
| `LOWR` | `u_tolower` runs of constant delta | ditto |

## Why not a Unicode crate

`pdfrum-text` is Tier-A byte-exact against the oracle, and three of its
decisions — whether a hyphen's neighbour is a letter (`IsHyphen`), whether a
mail-link character is alphanumeric (`CheckMailLink`), and how a search needle
is case-folded (`MakeLower`) — read ICU directly. A crate tracking a newer
Unicode revision classifies some code points differently from the ICU the
oracle links, and every disagreement is a whole-page Tier-A failure. So the
values come from the oracle's own ICU, not from the newest data available
(design brief D1, SPEC.md §9's 2026-08-29 ruling).

The bidi data is a second, independent reason: PDFium does **not** run the
Unicode Bidirectional Algorithm. It buckets raw bidi classes four ways and
reverses runs. `unicode-bidi` implements the real UBA and would produce
different output, so this crate does not link it (design brief D1; a DEPS.md
observation, not a change).

## Regenerating `unicode.bin`

```bash
PDFRUM_REGEN_TEXT_TABLES=1 \
PDFRUM_ORACLE=/mnt/data2/pdfium/pdfium-c++ \
  cargo build -p pdfrum-text
```

`build.rs` re-reads the three C++ sources and `icu-properties.txt`, re-checks
every structural invariant (row counts, mirror indices in bounds, ranges
ascending and disjoint) and rewrites `unicode.bin` plus
`unicode.manifest.json`. A violated invariant aborts the build naming the
table.

## Regenerating `icu-properties.txt`

The three ICU predicates are not in any oracle *source* file — they are calls
into ICU — so they are captured by linking a probe against the oracle's own
built `libicuuc`. Its ICU is **78.2, i.e. Unicode 17.0**
(`third_party/icu/source/common/unicode/uvernum.h`), which the built
`pdfium_test` statically links: `nm out/Release/pdfium_test` shows
`u_isalpha_78`.

Probe (`probe.cpp`):

```cpp
#include <cstdio>
#include <cstdint>
#include "unicode/uchar.h"

int main() {
  for (uint32_t c = 0; c <= 0x10FFFF; ++c) {
    bool a = u_isalpha((UChar32)c);
    bool n = u_isalnum((UChar32)c);
    UChar32 lo = u_tolower((UChar32)c);
    if (a || n || lo != (UChar32)c)
      printf("%X %d %d %X\n", c, (int)a, (int)n, (uint32_t)lo);
  }
}
```

Built with the checkout's own toolchain, because the ICU objects carry CREL
relocations that older GNU `ld` cannot read and the headers must resolve to the
checkout's ICU rather than the system's:

```bash
cd /mnt/data2/pdfium/pdfium-c++
CLANG=third_party/llvm-build/Release+Asserts/bin
ICU=third_party/icu/source/common
cp buildtools/third_party/libc++/__config_site /tmp/cxxcfg/
$CLANG/clang++ -O1 -std=c++20 -I$ICU -DU_STATIC_IMPLEMENTATION \
  -D_LIBCPP_HARDENING_MODE=_LIBCPP_HARDENING_MODE_NONE \
  -nostdinc++ -isystem /tmp/cxxcfg -isystem third_party/libc++/src/include \
  -fuse-ld=lld -B$CLANG -o /tmp/icuprobe /tmp/probe.cpp \
  out/Release/obj/third_party/icu/icuuc_private/*.o \
  out/Release/obj/buildtools/third_party/libc++/libc++.a \
  out/Release/obj/buildtools/third_party/libc++abi/libc++abi.a
/tmp/icuprobe > /tmp/icuprops.txt
```

`icu-properties.txt` is that dump collapsed to ranges — 146 484 lines become
684 alphabetic ranges, 736 alphanumeric ranges and 678 lowercase runs, which
is small enough to read and to diff. `src/unicode.rs`'s tests re-derive the
predicates from the blob and check them against hand-written expectations for
the boundary cases the crate actually depends on.

## Spot checks

Values a reviewer can confirm by hand:

- `u_tolower('A') == 'a'`, `u_tolower(U+0102) == U+0103` (Latin Extended-A,
  the `TextSearchLatinExtended` fixture's pair).
- U+0660 ARABIC-INDIC DIGIT ZERO is alphanumeric but not alphabetic.
- U+10400 DESERET CAPITAL LONG I lowercases to U+10428 — a supplementary-plane
  mapping, which is why the tables cover the whole code space rather than the
  BMP alone.
- Bidi: U+05D0 HEBREW ALEF is `R`, U+0627 ARABIC ALEF is `AL`, U+0028 LEFT
  PARENTHESIS is `ON` and mirrors to U+0029.
- Normalization: U+FB01 LATIN SMALL LIGATURE FI decomposes to `f` `i`.
