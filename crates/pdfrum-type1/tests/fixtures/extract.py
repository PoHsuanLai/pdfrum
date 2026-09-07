#!/usr/bin/env python3
"""Extract the two Foxit Multiple-Master PFB blobs from the C++ oracle checkout.

The oracle (`$PDFRUM_ORACLE_CHECKOUT`, default `<repo>/../pdfium-c++`) stores
the terminal-rung fallback fonts as C++ `std::array<uint8_t, N>` initializers.
This script turns them back into the PFB byte streams `pdfrum-type1`'s tests
consume.

Writes `FoxitSansMM.pfb` and `FoxitSerifMM.pfb` next to this file.

Usage:

    uv run crates/pdfrum-type1/tests/fixtures/extract.py [ORACLE_ROOT]
"""

import os
import pathlib
import re
import sys

FONTS = {
    "FoxitSansMM.cpp": ("kFoxitSansMMFontData", "FoxitSansMM.pfb", 66919),
    "FoxitSerifMM.cpp": ("kFoxitSerifMMFontData", "FoxitSerifMM.pfb", 113417),
}

BYTE = re.compile(rb"0[xX]([0-9a-fA-F]{2})")


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
    repo = pathlib.Path(__file__).resolve().parents[4]
    return pathlib.Path(os.environ.get("PDFRUM_ORACLE_CHECKOUT", repo.parent / "pdfium-c++"))


def main() -> int:
    oracle = oracle_checkout()
    src = oracle / "core/fxge/fontdata/chromefontdata"
    out = pathlib.Path(__file__).resolve().parent
    out.mkdir(parents=True, exist_ok=True)
    for cpp, (symbol, name, expected_len) in FONTS.items():
        blob = extract((src / cpp).read_bytes(), symbol)
        if len(blob) != expected_len:
            print(f"{cpp}: got {len(blob)} bytes, expected {expected_len}", file=sys.stderr)
            return 1
        (out / name).write_bytes(blob)
        print(f"{name}: {len(blob)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
