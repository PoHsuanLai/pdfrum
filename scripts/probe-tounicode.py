#!/usr/bin/env python3
"""Build minimal PDFs that isolate the pdfrum-font brief's OQ-3 questions.

Run from the workspace root with the oracle checkout present:

    python3 scripts/probe-tounicode.py /tmp/probes
    ORACLE=/mnt/data2/pdfium/pdfium-c++
    for f in /tmp/probes/*.pdf; do
      "$ORACLE/out/Release/pdfium_test" --time=1399672130 --txt \\
        --croscore-font-names \\
        --font-dir="$(readlink -f "$ORACLE/third_party/test_fonts")" "$f"
    done
    # then read the UTF-32LE output back:
    python3 -c "import struct,sys;b=open(sys.argv[1],'rb').read();\\
      print([hex(x) for x in struct.unpack('<%dI'%(len(b)//4), b)])" /tmp/probes/*.0.txt

The measured answers are recorded in docs/status/pdfrum-font.md and pinned as
tests in crates/pdfrum-font/src/tounicode_tests.rs.


Each probe is a one-page PDF with a Type0/Identity-H font whose /ToUnicode CMap
is the exact byte string under test.  `pdfium_test --txt` then reports what the
oracle's CPDF_ToUnicodeMap::Lookup produced for each shown charcode, in
UTF-32LE, which answers the question by measurement rather than by reading the
C++.
"""

import os
import sys

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))


def build(name, tounicode_cmap, shown_codes):
    """One page showing `shown_codes` (list of 2-byte big-endian charcodes)."""
    hexstr = "".join("%04X" % c for c in shown_codes)
    content = b"BT /F1 12 Tf 20 700 Td <" + hexstr.encode() + b"> Tj ET"

    objs = {}
    objs[1] = b"<< /Type /Catalog /Pages 2 0 R >>"
    objs[2] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"
    objs[3] = (
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 750] "
        b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
    )
    objs[4] = b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream"
    objs[5] = (
        b"<< /Type /Font /Subtype /Type0 /BaseFont /Helvetica "
        b"/Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 7 0 R >>"
    )
    objs[6] = (
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Helvetica "
        b"/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
        b"/DW 1000 >>"
    )
    objs[7] = (
        b"<< /Length %d >>\nstream\n" % len(tounicode_cmap)
        + tounicode_cmap
        + b"\nendstream"
    )

    out = bytearray(b"%PDF-1.7\n")
    offsets = {}
    for num in sorted(objs):
        offsets[num] = len(out)
        out += b"%d 0 obj\n" % num + objs[num] + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n" % (len(objs) + 1)
    out += b"0000000000 65535 f \n"
    for num in sorted(objs):
        out += b"%010d 00000 n \n" % offsets[num]
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objs) + 1,
        xref,
    )

    path = os.path.join(OUT, name + ".pdf")
    with open(path, "wb") as f:
        f.write(bytes(out))
    return path


def cmap(body):
    return (
        b"/CIDInit /ProcSet findresource begin\n"
        b"12 dict begin\nbegincmap\n"
        b"/CMapName /Probe def\n/CMapType 2 def\n"
        b"1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n"
        + body
        + b"\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"
    )


PROBES = []

# OQ-3(a).  `1 beginbfrange<0010><00ff><fff0>endbfrange` is the C++ unittest's
# HandleBeginBFRangeDestLargeValue.  The single-dest form increments an
# unclamped u32, so:
#   code 0x10 -> 0xFFF0        (a normal char)
#   code 0x1F -> 0xFFFF        (collides with the multi-char indicator)
#   code 0x20 -> 0x10000       ((v & 0xffff) == 0 -> a NUL char, or empty?)
# Showing 0x0010, 0x001F and 0x0020 makes all three visible in --txt.
PROBES.append(
    (
        "oq3a_dest_large_value",
        cmap(b"1 beginbfrange\n<0010> <00ff> <fff0>\nendbfrange"),
        [0x0010, 0x001F, 0x0020, 0x0011],
    )
)

# OQ-3(b).  StringDataAdd's carry cases, via the multi-dest bfrange form.
# A 2-unit destination increments as base-65536 big-endian, and a position that
# carries is documented to emit U+0000 rather than the wrapped value.
#   <0041 0041> over 3 codes -> [0041 0041], [0041 0042], [0041 0043]
PROBES.append(
    (
        "oq3b_stringdataadd_plain",
        cmap(b"1 beginbfrange\n<0030> <0032> <00410041>\nendbfrange"),
        [0x0030, 0x0031, 0x0032],
    )
)

#   <0041 FFFF> -> next should carry: the low unit wraps to 0 and, per the
#   algorithm, is emitted as U+0000 with the carry continuing into 0x41 -> 0x42.
PROBES.append(
    (
        "oq3b_stringdataadd_carry",
        cmap(b"1 beginbfrange\n<0030> <0032> <0041FFFF>\nendbfrange"),
        [0x0030, 0x0031, 0x0032],
    )
)

#   <FFFF FFFF> -> a full carry out of the leading unit, which the algorithm
#   says prepends U+0001, lengthening the string to three units.
PROBES.append(
    (
        "oq3b_stringdataadd_full_carry",
        cmap(b"1 beginbfrange\n<0030> <0031> <FFFFFFFF>\nendbfrange"),
        [0x0030, 0x0031],
    )
)

# Control: a plain single-unit bfchar, to confirm the harness reads what we
# think it reads.
PROBES.append(
    (
        "oq3_control",
        cmap(b"1 beginbfchar\n<0041> <0042>\nendbfchar"),
        [0x0041],
    )
)

# The U+FFFF indicator collision on its own, via bfchar rather than a range.
PROBES.append(
    (
        "oq3c_ffff_bfchar",
        cmap(b"2 beginbfchar\n<0041> <FFFF>\n<0042> <0043>\nendbfchar"),
        [0x0041, 0x0042],
    )
)

if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, cm, codes in PROBES:
        p = build(name, cm, codes)
        print(p, file=sys.stderr)
        print(name)
