#!/usr/bin/env python3
"""Extract the two Foxit Multiple-Master PFB blobs from the C++ oracle checkout.

The oracle (`/mnt/data2/pdfium/pdfium-c++`) stores the terminal-rung fallback
fonts as C++ `std::array<uint8_t, N>` initializers. This script turns them back
into the PFB byte streams `pdfrum-type1`'s tests consume.

Usage:
    uv run scripts/extract-foxit-mm.py [ORACLE_ROOT]

Writes `crates/pdfrum-type1/tests/fixtures/{FoxitSansMM,FoxitSerifMM}.pfb`.
Both files are PDFium-BSD licensed (see the fixtures' PROVENANCE.md).
"""

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


def main() -> int:
    oracle = pathlib.Path(
        sys.argv[1] if len(sys.argv) > 1 else "/mnt/data2/pdfium/pdfium-c++"
    )
    src = oracle / "core/fxge/fontdata/chromefontdata"
    out = pathlib.Path(__file__).resolve().parent.parent / (
        "crates/pdfrum-type1/tests/fixtures"
    )
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
