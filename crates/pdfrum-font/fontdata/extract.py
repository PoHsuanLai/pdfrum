#!/usr/bin/env python3
"""Extract the fourteen base-14 Foxit CFF blobs from the C++ oracle checkout.

Writes `Foxit*.cff` next to this file. Sibling of
`crates/pdfrum-type1/tests/fixtures/extract.py` (the two MM fallbacks).

Usage, from anywhere:

    uv run crates/pdfrum-font/fontdata/extract.py [ORACLE_ROOT]
"""

from __future__ import annotations

import os
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



def oracle_checkout(argv_index: int = 1) -> pathlib.Path:
    """The read-only C++ PDFium checkout.

    One place, three inputs, in order: an explicit argument, then
    `$PDFRUM_ORACLE_CHECKOUT`, then `<repo>/../pdfium-c++`.
    """
    if len(sys.argv) > argv_index:
        return pathlib.Path(sys.argv[argv_index])
    repo = pathlib.Path(__file__).resolve().parents[3]
    return pathlib.Path(os.environ.get("PDFRUM_ORACLE_CHECKOUT", repo.parent / "pdfium-c++"))


def main() -> int:
    oracle = oracle_checkout()
    src = oracle / "core/fxge/fontdata/chromefontdata"
    out = pathlib.Path(
        sys.argv[2] if len(sys.argv) > 2 else pathlib.Path(__file__).resolve().parent
    ).resolve()
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
