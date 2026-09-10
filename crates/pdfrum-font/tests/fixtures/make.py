#!/usr/bin/env python3
"""Generate the minimal TrueType fixtures used by `pdfrum-font`'s test suite.

Every font this script emits is synthesized from scratch: no byte of any output
is copied from the PDFium oracle checkout. What *is* borrowed is the shape of
the idea — `core/fpdfapi/font/cpdf_truetypefont_unittest.cpp` hand-assembles a
508-byte TTF whose only interesting content is a single cmap subtable, and
proves that two such fonts differing solely in the subtable's platform and
encoding IDs behave identically. The fixtures below generalize that: a fixed
skeleton (head/hhea/hmtx/maxp/loca/glyf/post) plus whatever cmap the test under
examination needs.

Usage:

    python3 crates/pdfrum-font/tests/fixtures/make.py [OUT_DIR]

OUT_DIR defaults to this directory. Output is deterministic.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

UNITS_PER_EM = 1000

DEFAULT_OUT_DIR = Path(__file__).resolve().parent


# --------------------------------------------------------------------------
# small helpers
# --------------------------------------------------------------------------


def pad4(data: bytes) -> bytes:
    """Pad to a 4-byte boundary, as the SFNT table directory requires."""
    return data + b"\0" * (-len(data) % 4)


def checksum(data: bytes) -> int:
    """The SFNT table checksum: sum of big-endian u32 words, mod 2**32."""
    data = pad4(data)
    total = 0
    for (word,) in struct.iter_unpack(">I", data):
        total = (total + word) & 0xFFFFFFFF
    return total


# --------------------------------------------------------------------------
# tables
# --------------------------------------------------------------------------


def make_head() -> bytes:
    """`head`, 54 bytes. `checkSumAdjustment` is patched in by `build_sfnt`."""
    return struct.pack(
        ">IIIIHHqqhhhhHHhhh",
        0x00010000,  # version 1.0
        0x00010000,  # fontRevision 1.0
        0,  # checkSumAdjustment (patched later)
        0x5F0F3CF5,  # magicNumber
        0b0000_0000_0000_0011,  # flags: baseline at y=0, lsb at x=0
        UNITS_PER_EM,
        0,  # created (seconds since 1904-01-01; fixed for determinism)
        0,  # modified
        0,  # xMin
        0,  # yMin
        700,  # xMax
        700,  # yMax
        0,  # macStyle
        8,  # lowestRecPPEM
        2,  # fontDirectionHint
        0,  # indexToLocFormat: short loca
        0,  # glyphDataFormat
    )


def make_hhea(num_h_metrics: int) -> bytes:
    """`hhea`, 36 bytes."""
    return struct.pack(
        ">IhhhHhhhhhhhhhhhH",
        0x00010000,  # version 1.0
        800,  # ascender
        -200,  # descender
        0,  # lineGap
        700,  # advanceWidthMax
        0,  # minLeftSideBearing
        0,  # minRightSideBearing
        700,  # xMaxExtent
        1,  # caretSlopeRise
        0,  # caretSlopeRun
        0,  # caretOffset
        0,  # reserved x4
        0,
        0,
        0,
        0,  # metricDataFormat
        num_h_metrics,
    )


def make_hmtx(num_glyphs: int) -> bytes:
    """`hmtx` with one longHorMetric per glyph: uniform 600/0."""
    return b"".join(struct.pack(">Hh", 600, 0) for _ in range(num_glyphs))


def make_maxp(num_glyphs: int) -> bytes:
    """`maxp` version 1.0, 32 bytes; generous limits for a triangle."""
    return struct.pack(
        ">IHHHHHHHHHHHHHH",
        0x00010000,  # version 1.0
        num_glyphs,
        16,  # maxPoints
        4,  # maxContours
        0,  # maxCompositePoints
        0,  # maxCompositeContours
        2,  # maxZones
        0,  # maxTwilightPoints
        0,  # maxStorage
        0,  # maxFunctionDefs
        0,  # maxInstructionDefs
        0,  # maxStackElements
        0,  # maxSizeOfInstructions
        0,  # maxComponentElements
        0,  # maxComponentDepth
    )


def make_post_v3() -> bytes:
    """`post` version 3.0: metrics only, no glyph names."""
    return struct.pack(
        ">IIhhIIIII",
        0x00030000,  # version 3.0
        0,  # italicAngle
        -100,  # underlinePosition
        50,  # underlineThickness
        0,  # isFixedPitch
        0,  # minMemType42
        0,  # maxMemType42
        0,  # minMemType1
        0,  # maxMemType1
    )


# The 258 standard Macintosh glyph names occupy post-v2 indices 0..257; a
# `glyphNameIndex` of 258+k names the k-th string in this table's own array.
STANDARD_MAC_GLYPH_NAMES = {".notdef": 0, ".null": 1, "nonmarkingreturn": 2}


def make_post_v2(names: list[str]) -> bytes:
    """`post` version 2.0 carrying an explicit name for every glyph."""
    header = struct.pack(
        ">IIhhIIIII",
        0x00020000,  # version 2.0
        0,  # italicAngle
        -100,  # underlinePosition
        50,  # underlineThickness
        0,  # isFixedPitch
        0,
        0,
        0,
        0,
    )
    indices = bytearray()
    strings = bytearray()
    custom = 0
    for name in names:
        std = STANDARD_MAC_GLYPH_NAMES.get(name)
        if std is not None:
            indices += struct.pack(">H", std)
        else:
            indices += struct.pack(">H", 258 + custom)
            custom += 1
            encoded = name.encode("ascii")
            assert len(encoded) < 256, name
            strings += bytes([len(encoded)]) + encoded
    return header + struct.pack(">H", len(names)) + bytes(indices) + bytes(strings)


def make_triangle_glyph() -> bytes:
    """A simple one-contour, three-point glyph: a filled triangle."""
    xs = [50, 650, 350]
    ys = [0, 0, 700]
    header = struct.pack(">hhhhh", 1, min(xs), min(ys), max(xs), max(ys))
    body = struct.pack(">H", 2)  # endPtsOfContours: last point index
    body += struct.pack(">H", 0)  # instructionLength
    body += bytes([0x01, 0x01, 0x01])  # flags: on-curve, 16-bit deltas
    prev = 0
    for x in xs:
        body += struct.pack(">h", x - prev)
        prev = x
    prev = 0
    for y in ys:
        body += struct.pack(">h", y - prev)
        prev = y
    return pad4(header + body)


ARG_1_AND_2_ARE_WORDS = 0x0001
ARGS_ARE_XY_VALUES = 0x0002
MORE_COMPONENTS = 0x0020
WE_HAVE_INSTRUCTIONS = 0x0100


def make_composite_glyph(base: int, instructions: bytes) -> bytes:
    """A two-component composite of `base`, optionally carrying bytecode.

    Both components are plain translations. `instructions` is written only
    when non-empty, and `WE_HAVE_INSTRUCTIONS` is set to match, so one builder
    covers the instructed and uninstructed cases the predicate separates.
    """
    header = struct.pack(">hhhhh", -1, 0, 0, 1000, 1000)
    body = b""
    for i, (dx, dy) in enumerate(((0, 0), (300, 200))):
        last = i == 1
        flags = ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES
        if not last:
            flags |= MORE_COMPONENTS
        elif instructions:
            flags |= WE_HAVE_INSTRUCTIONS
        body += struct.pack(">HHhh", flags, base, dx, dy)
    if instructions:
        body += struct.pack(">H", len(instructions)) + instructions
    return pad4(header + body)


def make_glyf_and_loca(
    num_glyphs: int,
    filled: set[int],
    composites: dict[int, tuple[int, bytes]] | None = None,
) -> tuple[bytes, bytes]:
    """Build `glyf` plus a matching short-format `loca` (offsets/2)."""
    composites = composites or {}
    glyf = bytearray()
    offsets = [0]
    for gid in range(num_glyphs):
        if gid in composites:
            base, instructions = composites[gid]
            glyf += make_composite_glyph(base, instructions)
        elif gid in filled:
            glyf += make_triangle_glyph()
        offsets.append(len(glyf))
    # Short loca stores offset/2, so every offset must be even; the triangle is
    # already padded to 4 bytes, and empty glyphs add nothing.
    assert all(off % 2 == 0 for off in offsets)
    loca = b"".join(struct.pack(">H", off // 2) for off in offsets)
    return bytes(glyf), loca


# --------------------------------------------------------------------------
# cmap
# --------------------------------------------------------------------------


def cmap_format4_table(mapping: dict[int, int]) -> bytes:
    """A complete format 4 subtable. An empty mapping yields the degenerate
    table holding only the mandatory 0xFFFF terminator segment."""
    segments: list[tuple[int, int, int]] = []
    for code in sorted(mapping):
        gid = mapping[code]
        delta = (gid - code) & 0xFFFF
        if segments and segments[-1][1] == code - 1 and segments[-1][2] == delta:
            start, _, _ = segments[-1]
            segments[-1] = (start, code, delta)
        else:
            segments.append((code, code, delta))
    segments.append((0xFFFF, 0xFFFF, 1))  # required terminator segment

    seg_count = len(segments)
    seg_count_x2 = seg_count * 2
    search_range = 2
    entry_selector = 0
    while search_range * 2 <= seg_count_x2:
        search_range *= 2
        entry_selector += 1
    range_shift = seg_count_x2 - search_range

    arrays = struct.pack(
        ">HHHH", seg_count_x2, search_range, entry_selector, range_shift
    )
    arrays += b"".join(struct.pack(">H", end) for _, end, _ in segments)
    arrays += struct.pack(">H", 0)  # reservedPad
    arrays += b"".join(struct.pack(">H", start) for start, _, _ in segments)
    arrays += b"".join(
        struct.pack(">h", delta - 0x10000 if delta > 0x7FFF else delta)
        for _, _, delta in segments
    )
    arrays += b"".join(struct.pack(">H", 0) for _ in segments)  # idRangeOffset

    length = 6 + len(arrays)  # format + length + language + arrays
    return struct.pack(">HHH", 4, length, 0) + arrays


def cmap_format0_table(mapping: dict[int, int]) -> bytes:
    """A format 0 subtable: the flat 256-byte byte-encoding array."""
    table = bytearray(256)
    for code, gid in mapping.items():
        assert 0 <= code < 256, code
        assert 0 <= gid < 256, gid
        table[code] = gid
    return struct.pack(">HHH", 0, 262, 0) + bytes(table)


def make_cmap(subtables: list[tuple[int, int, bytes]]) -> bytes:
    """Assemble a `cmap` from (platform_id, encoding_id, subtable) triples,
    preserving the given order of the encoding records."""
    header = struct.pack(">HH", 0, len(subtables))
    records = b""
    body = b""
    offset = len(header) + 8 * len(subtables)
    for platform_id, encoding_id, subtable in subtables:
        records += struct.pack(">HHI", platform_id, encoding_id, offset + len(body))
        body += subtable
        # Keep each subtable 4-byte aligned so offsets stay tidy.
        body += b"\0" * (-len(body) % 4)
    return header + records + body


# --------------------------------------------------------------------------
# SFNT assembly
# --------------------------------------------------------------------------


def make_name(family: str) -> bytes:
    """A minimal `name` table declaring `family` as the Windows family name.

    Only nameID 1 on platform 3 (Windows), encoding 1 (Unicode BMP), language
    0x0409 — the record `skrifa`'s `english_or_first` reads, and the one
    FreeType compares against its hint-reliant list. The string is UTF-16BE
    because that is what platform 3 requires.
    """
    value = family.encode("utf-16-be")
    count = 1
    storage_offset = 6 + 12 * count
    record = struct.pack(">HHHHHH", 3, 1, 0x0409, 1, len(value), 0)
    return pad4(struct.pack(">HHH", 0, count, storage_offset) + record + value)


def build_sfnt(tables: dict[str, bytes]) -> bytes:
    """Lay out an SFNT: sorted table directory, per-table checksums, and a
    `head.checkSumAdjustment` patched to make the whole file sum to 0xB1B0AFBA."""
    tags = sorted(tables)
    num_tables = len(tags)

    search_range = 16
    entry_selector = 0
    while search_range * 2 <= num_tables * 16:
        search_range *= 2
        entry_selector += 1
    range_shift = num_tables * 16 - search_range

    header = struct.pack(
        ">IHHHH",
        0x00010000,  # sfntVersion: TrueType outlines
        num_tables,
        search_range,
        entry_selector,
        range_shift,
    )

    directory = bytearray()
    body = bytearray()
    offset = len(header) + 16 * num_tables
    head_body_offset = None
    for tag in tags:
        data = tables[tag]
        if tag == "head":
            head_body_offset = len(body)
        directory += struct.pack(
            ">4sIII", tag.encode("ascii"), checksum(data), offset + len(body), len(data)
        )
        body += pad4(data)

    font = bytearray(header + bytes(directory) + bytes(body))

    # checkSumAdjustment lives 8 bytes into `head`.
    assert head_body_offset is not None
    adj_pos = len(header) + 16 * num_tables + head_body_offset + 8
    struct.pack_into(">I", font, adj_pos, 0)
    total = checksum(bytes(font))
    struct.pack_into(">I", font, adj_pos, (0xB1B0AFBA - total) & 0xFFFFFFFF)
    return bytes(font)


def make_font(
    num_glyphs: int,
    filled: set[int],
    cmap: bytes | None,
    post: bytes,
    composites: dict[int, tuple[int, bytes]] | None = None,
    family: str | None = None,
) -> bytes:
    glyf, loca = make_glyf_and_loca(num_glyphs, filled, composites)
    tables = {
        "head": make_head(),
        "hhea": make_hhea(num_glyphs),
        "hmtx": make_hmtx(num_glyphs),
        "maxp": make_maxp(num_glyphs),
        "loca": loca,
        "glyf": glyf,
        "post": post,
    }
    if cmap is not None:
        tables["cmap"] = cmap
    if family is not None:
        tables["name"] = make_name(family)
    return build_sfnt(tables)


# --------------------------------------------------------------------------
# the fixture catalogue
# --------------------------------------------------------------------------

UNICODE_MAP = {0x002E: 1}  # U+002E FULL STOP -> glyph 1
SYMBOL_MAP = {0xF041: 5}  # the F0xx private-use "symbolic" convention
MACROMAN_MAP = {0x41: 7}
# A GB2312 double-byte code, for the legacy-charmap rung of `UseCIDCharmap`.
SJIS_MAP = {0x889F: 3}


def fixtures() -> dict[str, bytes]:
    post3 = make_post_v3()
    fmt4_unicode = cmap_format4_table(UNICODE_MAP)
    fmt4_symbol = cmap_format4_table(SYMBOL_MAP)
    fmt4_empty = cmap_format4_table({})
    fmt0_macroman = cmap_format0_table(MACROMAN_MAP)
    fmt0_empty = cmap_format0_table({})

    out: dict[str, bytes] = {}

    out["tt_unicode_31.ttf"] = make_font(
        2, {1}, make_cmap([(3, 1, fmt4_unicode)]), post3
    )
    out["tt_unicode_03.ttf"] = make_font(
        2, {1}, make_cmap([(0, 3, fmt4_unicode)]), post3
    )
    out["tt_symbol_30.ttf"] = make_font(
        6, {5}, make_cmap([(3, 0, fmt4_symbol)]), post3
    )
    out["tt_macroman_10.ttf"] = make_font(
        8, {7}, make_cmap([(1, 0, fmt0_macroman)]), post3
    )
    out["tt_custom_40.ttf"] = make_font(2, set(), make_cmap([(4, 0, fmt0_empty)]), post3)
    # Glyph 1 simple, glyph 2 an uninstructed composite, glyph 3 an instructed
    # one: the three cases the instructed-composite predicate separates.
    # `0x4B` is `MIAP[0]`, enough to make the stream non-empty.
    out["tt_composite_instructions.ttf"] = make_font(
        4,
        {1},
        make_cmap([(3, 1, fmt4_unicode)]),
        post3,
        composites={2: (1, b""), 3: (1, bytes([0x4B]))},
    )
    # The same shapes under a family name on FreeType's hint-reliant list, so
    # a test can assert the face-level predicate that decides whether the
    # interpreter runs. "DFKai-SB" is the name the DynaLab Kai faces carry.
    out["tt_hint_reliant.ttf"] = make_font(
        4,
        {1},
        make_cmap([(3, 1, fmt4_unicode)]),
        post3,
        composites={2: (1, b""), 3: (1, bytes([0x4B]))},
        family="DFKai-SB",
    )
    out["tt_symbol_and_macroman.ttf"] = make_font(
        8,
        {5, 7},
        make_cmap([(3, 0, fmt4_symbol), (1, 0, fmt0_macroman)]),
        post3,
    )
    out["tt_unicode_03_and_symbol.ttf"] = make_font(
        6,
        {1, 5},
        make_cmap([(0, 3, fmt4_unicode), (3, 0, fmt4_symbol)]),
        post3,
    )
    out["tt_unicode_31_and_symbol.ttf"] = make_font(
        6,
        {1, 5},
        make_cmap([(3, 1, fmt4_unicode), (3, 0, fmt4_symbol)]),
        post3,
    )
    out["tt_symbol_empty.ttf"] = make_font(
        6, set(), make_cmap([(3, 0, fmt4_empty)]), post3
    )
    out["tt_macroman_empty.ttf"] = make_font(
        8, set(), make_cmap([(1, 0, fmt0_empty)]), post3
    )
    # The legacy CJK rung: a Shift-JIS `(3,2)` subtable beside a Unicode one,
    # so a test can prove the coding scheme really chooses between them.
    out["tt_sjis_and_unicode.ttf"] = make_font(
        6,
        {1, 3},
        make_cmap([(3, 1, fmt4_unicode), (3, 2, cmap_format4_table(SJIS_MAP))]),
        post3,
    )
    out["tt_named_no_cmap.ttf"] = make_font(
        3, {1, 2}, None, make_post_v2([".notdef", "A", "B"])
    )
    return out


def main(argv: list[str]) -> int:
    out_dir = Path(argv[1]).resolve() if len(argv) > 1 else DEFAULT_OUT_DIR
    out_dir.mkdir(parents=True, exist_ok=True)
    for name, data in sorted(fixtures().items()):
        (out_dir / name).write_bytes(data)
        print(f"{name}: {len(data)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
