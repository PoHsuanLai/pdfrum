#!/usr/bin/env python3
"""Generate `tiny.ttf` next to this file.

Same SFNT skeleton as `crates/pdfrum-font/tests/fixtures/make.py`, but every
glyph carries real outlines so the subsetter has something to drop.

Usage:

    python3 crates/pdfrum-edit/tests/files/make.py [OUT_DIR]
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_OUT_DIR = HERE
FONT_MAKE = HERE.parents[2] / "pdfrum-font" / "tests" / "fixtures" / "make.py"

NUM_GLYPHS = 8
# Glyph 0 is `.notdef` and stays empty, as every font's does; the rest carry
# outlines so a subset has something to leave behind.
FILLED = {1, 2, 3, 4, 5, 6}


def load_builders():
    path = FONT_MAKE
    spec = importlib.util.spec_from_file_location("tt_fixtures", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> None:
    tt = load_builders()
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT_DIR
    out_dir.mkdir(parents=True, exist_ok=True)

    glyf, loca = tt.make_glyf_and_loca(NUM_GLYPHS, FILLED)
    font = tt.build_sfnt(
        {
            "head": tt.make_head(),
            "hhea": tt.make_hhea(NUM_GLYPHS),
            "hmtx": tt.make_hmtx(NUM_GLYPHS),
            "maxp": tt.make_maxp(NUM_GLYPHS),
            "loca": loca,
            "glyf": glyf,
            "post": tt.make_post_v3(),
        }
    )

    target = out_dir / "tiny.ttf"
    target.write_bytes(font)
    print(f"{target}: {len(font)} bytes, {NUM_GLYPHS} glyphs")


if __name__ == "__main__":
    main()
