#!/usr/bin/env python3
"""Independently verify tables/cmaps.bin against the C++ source arrays.

This is a *second* implementation on purpose. `build.rs` writes the blob; this
reads it back with a separate parser, in a different language, and compares
every value against the C++ it claims to have come from. A bug shared between
writer and reader cannot hide from it, which a Rust-side round-trip test could
not promise.

    python3 crates/pdfrum-cmap/tables/verify_blob.py [ORACLE_ROOT]

`ORACLE_ROOT` defaults to `$PDFRUM_ORACLE_CHECKOUT`, itself defaulting to
`<repo>/../pdfium-c++`.

Exits non-zero on the first mismatch, naming the symbol. Run it whenever the
blob is regenerated or the oracle checkout moves.
"""

import os
import re
import struct
import sys
from pathlib import Path

BLOB_PATH = Path(__file__).parent / "cmaps.bin"
# `crates/pdfrum-cmap/tables/` -> the repository root.
REPO_ROOT = Path(__file__).resolve().parents[3]
# One place, three inputs: an explicit argument, then `$PDFRUM_ORACLE_CHECKOUT`,
# then the sibling directory README.md says the checkout lives in.
# `scripts/env.nu` resolves the same variable with the same default.
DEFAULT_ORACLE = os.environ.get("PDFRUM_ORACLE_CHECKOUT", REPO_ROOT.parent / "pdfium-c++")

# (directory, index .inc, index symbol, CID2Unicode .inc, CID2Unicode symbol)
REGISTRIES = [
    ("GB1", "cmaps_gb1.inc", "kGB1_cmaps",
     "Adobe-GB1-UCS2_5.inc", "kGB1CID2Unicode_5"),
    ("CNS1", "cmaps_cns1.inc", "kCNS1_cmaps",
     "Adobe-CNS1-UCS2_5.inc", "kCNS1CID2Unicode_5"),
    ("Japan1", "cmaps_japan1.inc", "kJapan1_cmaps",
     "Adobe-Japan1-UCS2_4.inc", "kJapan1CID2Unicode_4"),
    ("Korea1", "cmaps_korea1.inc", "kKorea1_cmaps",
     "Adobe-Korea1-UCS2_2.inc", "kKorea1CID2Unicode_2"),
]

ROW_RE = re.compile(
    r'\{\s*"([^"]+)"\s*,\s*(\w+)\s*,\s*(\w+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,'
    r'\s*CMap::Type::(\w+)\s*,\s*(-?\d+)\s*\}'
)


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return re.sub(r"//[^\n]*", "", text)


def array_body(src, symbol):
    """The text inside `symbol[...] = { ... }`, or None."""
    m = re.search(re.escape(symbol) + r"\s*\[[^\]]*\]\s*=\s*\{", src)
    if not m:
        return None
    open_at = m.end() - 1
    depth = 0
    for i in range(open_at, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return src[open_at + 1:i]
    return None


def literals(text):
    return [
        int(t, 16) if t.lower().startswith("0x") else int(t)
        for t in re.findall(r"0[xX][0-9a-fA-F]+|\d+", text)
    ]


def fail(msg):
    print(f"MISMATCH: {msg}", file=sys.stderr)
    sys.exit(1)


def main():
    oracle = Path(sys.argv[1] if len(sys.argv) > 1 else DEFAULT_ORACLE)
    cmaps = oracle / "core/fpdfapi/cmaps"
    if not cmaps.is_dir():
        print(f"no oracle at {cmaps}; pass its root as argv[1]", file=sys.stderr)
        return 2
    blob = BLOB_PATH.read_bytes()

    magic, version, count = struct.unpack_from("<IHH", blob, 0)
    index_off, words_off, dwords_off, c2u_off, total, names_off = \
        struct.unpack_from("<6I", blob, 8)
    if magic != 0x504D4331:
        fail(f"magic {magic:#x}")
    if version != 1 or count != 4 or total != len(blob):
        fail(f"header version={version} registries={count} len={total}/{len(blob)}")

    def name_at(off):
        at = names_off + off
        return blob[at + 1:at + 1 + blob[at]].decode()

    words_checked = dwords_checked = 0
    for r, (d, inc, sym, uinc, usym) in enumerate(REGISTRIES):
        src = strip_comments((cmaps / d / inc).read_text())
        rows = ROW_RE.findall(src)
        entry_off, entry_count = struct.unpack_from("<IHH", blob, index_off + r * 8)[:2]
        if entry_count != len(rows):
            fail(f"{sym}: blob has {entry_count} entries, source has {len(rows)}")

        sources = {p: strip_comments(p.read_text()) for p in (cmaps / d).glob("*.cpp")}

        def find_array(symbol):
            for text in sources.values():
                body = array_body(text, symbol)
                if body is not None:
                    return literals(body)
            fail(f"array {symbol} not found under {d}/")

        for i, (nm, wsym, dsym, wc, dc, ty, uo) in enumerate(rows):
            noff, woff, doff, b_wc, b_dc, is_range, b_uo = \
                struct.unpack_from("<IIIHHbb", blob, entry_off + i * 20)
            if name_at(noff) != nm:
                fail(f"{sym}[{i}]: name {name_at(noff)!r} != {nm!r}")
            if (b_wc, b_dc, b_uo) != (int(wc), int(dc), int(uo)):
                fail(f"{nm}: counts/use_offset {(b_wc, b_dc, b_uo)} != "
                     f"{(int(wc), int(dc), int(uo))}")
            if is_range != (1 if ty == "kRange" else 0):
                fail(f"{nm}: record type")

            values = find_array(wsym)
            stride = 3 if is_range else 2
            if len(values) != b_wc * stride:
                fail(f"{wsym}: {len(values)} u16 != {b_wc} records x {stride}")
            got = struct.unpack_from(f"<{len(values)}H", blob, words_off + woff)
            if list(got) != values:
                fail(f"{wsym}: word data differs")
            words_checked += len(values)

            if dsym == "nullptr":
                if doff != 0xFFFFFFFF:
                    fail(f"{nm}: dword offset set on an entry with no dword table")
            else:
                dvalues = find_array(dsym)
                if len(dvalues) != b_dc * 4:
                    fail(f"{dsym}: {len(dvalues)} u16 != {b_dc} records x 4")
                got = struct.unpack_from(f"<{len(dvalues)}H", blob, dwords_off + doff)
                if list(got) != dvalues:
                    fail(f"{dsym}: dword data differs")
                dwords_checked += len(dvalues)

        uvalues = literals(array_body(strip_comments((cmaps / d / uinc).read_text()), usym))
        uoff, ulen = struct.unpack_from("<II", blob, c2u_off + r * 8)
        if ulen != len(uvalues):
            fail(f"{usym}: {ulen} entries != {len(uvalues)}")
        got = struct.unpack_from(f"<{ulen}H", blob, uoff)
        if list(got) != uvalues:
            fail(f"{usym}: CID2Unicode data differs")
        print(f"{d}: {len(rows)} entries, CID2Unicode {ulen} entries — OK")

    print(f"VERIFIED: {words_checked} word + {dwords_checked} dword u16 "
          f"byte-identical to the C++ source ({len(blob)} byte blob)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
