#!/usr/bin/env python3
"""Extract the fourteen base-14 Foxit CFF blobs from the C++ oracle checkout.

The oracle (`/mnt/data2/pdfium/pdfium-c++`) stores the base-14 substitution
faces as C++ `std::array<uint8_t, N>` initializers, one per translation unit
under `core/fxge/fontdata/chromefontdata/`. This script turns them back into
the bare CFF byte streams `pdfrum` embeds. It is the base-14 sibling of
`scripts/extract-foxit-mm.py`, which does the same for the two Multiple-Master
PFB fallbacks.

Usage:
    uv run scripts/extract_fontdata.py [ORACLE_ROOT] [OUT_DIR]

Writes `<OUT_DIR>/{FoxitFixed,...,FoxitDingbats}.cff` (default `./fontdata`).
All files are PDFium-BSD licensed (see PROVENANCE.md alongside them).
"""

from __future__ import annotations

import pathlib
import re
import sys

# cpp file -> (C++ symbol, output name, expected length from chromefontdata.h)
FONTS = {
    "FoxitFixed.cpp": ("kFoxitFixedFontData", "FoxitFixed.cff", 17597),
    "FoxitFixedBold.cpp": ("kFoxitFixedBoldFontData", "FoxitFixedBold.cff", 18055),
    "FoxitFixedBoldItalic.cpp": (
        "kFoxitFixedBoldItalicFontData",
        "FoxitFixedBoldItalic.cff",
        19151,
    ),
    "FoxitFixedItalic.cpp": (
        "kFoxitFixedItalicFontData",
        "FoxitFixedItalic.cff",
        18746,
    ),
    "FoxitSans.cpp": ("kFoxitSansFontData", "FoxitSans.cff", 15025),
    "FoxitSansBold.cpp": ("kFoxitSansBoldFontData", "FoxitSansBold.cff", 16344),
    "FoxitSansBoldItalic.cpp": (
        "kFoxitSansBoldItalicFontData",
        "FoxitSansBoldItalic.cff",
        16418,
    ),
    "FoxitSansItalic.cpp": ("kFoxitSansItalicFontData", "FoxitSansItalic.cff", 16339),
    "FoxitSerif.cpp": ("kFoxitSerifFontData", "FoxitSerif.cff", 19469),
    "FoxitSerifBold.cpp": ("kFoxitSerifBoldFontData", "FoxitSerifBold.cff", 19395),
    "FoxitSerifBoldItalic.cpp": (
        "kFoxitSerifBoldItalicFontData",
        "FoxitSerifBoldItalic.cff",
        20733,
    ),
    "FoxitSerifItalic.cpp": (
        "kFoxitSerifItalicFontData",
        "FoxitSerifItalic.cff",
        21227,
    ),
    "FoxitSymbol.cpp": ("kFoxitSymbolFontData", "FoxitSymbol.cff", 16729),
    "FoxitDingbats.cpp": ("kFoxitDingbatsFontData", "FoxitDingbats.cff", 29513),
}

# Unlike the MM sources, these blobs are written with minimal-width hex
# literals (`0x1`, `0xd`, `0x43`), so accept one or two digits.
BYTE = re.compile(rb"0[xX]([0-9a-fA-F]{1,2})\b")


def extract(source: bytes, symbol: str) -> bytes:
    start = source.index(symbol.encode())
    body = source[source.index(b"{{", start) + 2 : source.index(b"}}", start)]
    return bytes(int(m.group(1), 16) for m in BYTE.finditer(body))


def main() -> int:
    oracle = pathlib.Path(
        sys.argv[1] if len(sys.argv) > 1 else "/mnt/data2/pdfium/pdfium-c++"
    )
    src = oracle / "core/fxge/fontdata/chromefontdata"
    out = pathlib.Path(sys.argv[2] if len(sys.argv) > 2 else "fontdata").resolve()
    out.mkdir(parents=True, exist_ok=True)

    # Cross-check every expected length against the header's array bounds, so a
    # rebased oracle that changed a blob fails here rather than silently.
    header = (src / "chromefontdata.h").read_text()
    bounds = {
        m.group(2): int(m.group(1))
        for m in re.finditer(r"std::array<uint8_t,\s*(\d+)>\s+(\w+);", header)
    }

    failed = False
    for cpp, (symbol, name, expected_len) in FONTS.items():
        if bounds.get(symbol) != expected_len:
            print(
                f"{symbol}: chromefontdata.h declares {bounds.get(symbol)}, "
                f"script expects {expected_len}",
                file=sys.stderr,
            )
            failed = True
            continue
        blob = extract((src / cpp).read_bytes(), symbol)
        if len(blob) != expected_len:
            print(
                f"{cpp}: got {len(blob)} bytes, expected {expected_len}",
                file=sys.stderr,
            )
            failed = True
            continue
        (out / name).write_bytes(blob)
        print(f"{name}: {len(blob)} bytes, first 4 = {blob[:4].hex(' ')}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
