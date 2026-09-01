#!/usr/bin/env python3
"""Generate test PDFs by writing .in templates in the pdfium fixture shape and
expanding them with the oracle's testing/tools/fixup_pdf_template.py (which is
run read-only from the oracle checkout; output goes to OUT_DIR here).

Each PDF has an OpenAction that runs a JS program. We embed the JS payload
directly in the object 11 stream.
"""
import os
import subprocess
import sys

OUT_DIR = os.path.dirname(os.path.abspath(__file__))
ORACLE = "/mnt/data2/pdfium/pdfium-c++"
FIXUP = os.path.join(ORACLE, "testing", "tools", "fixup_pdf_template.py")

TEMPLATE = """{{header}}
{{object 1 0}} <<
  /Type /Catalog
  /Pages 2 0 R
  /OpenAction 10 0 R
>>
endobj
{{object 2 0}} <<
  /Type /Pages
  /Count 1
  /Kids [ 3 0 R ]
>>
endobj
{{object 3 0}} <<
  /Type /Page
  /Parent 2 0 R
  /MediaBox [0 0 612 792]
>>
endobj
{{object 10 0}} <<
  /Type /Action
  /S /JavaScript
  /JS 11 0 R
>>
endobj
{{object 11 0}} <<
  {{streamlen}}
>>
stream
%(JS)s
endstream
endobj
{{xref}}
{{trailer}}
{{startxref}}
%%%%EOF
"""

CASES = {
    "control": "app.alert('hi');",
    "loop": "app.alert('before-loop'); while(true){} app.alert('after-loop');",
    "string": (
        "app.alert('before-string');\n"
        "var s='a'; for(var i=0;i<30;i++){ s=s+s; }\n"
        "app.alert('after-string len='+s.length);"
    ),
    # Catastrophic backtracking regex at several lengths.
    "regex20": "app.alert('before'); var r=/(a+)+$/.test('a'.repeat(20)+'b'); app.alert('after '+r);",
    "regex24": "app.alert('before'); var r=/(a+)+$/.test('a'.repeat(24)+'b'); app.alert('after '+r);",
    "regex26": "app.alert('before'); var r=/(a+)+$/.test('a'.repeat(26)+'b'); app.alert('after '+r);",
    "regex28": "app.alert('before'); var r=/(a+)+$/.test('a'.repeat(28)+'b'); app.alert('after '+r);",
}


def main():
    for name, js in CASES.items():
        in_path = os.path.join(OUT_DIR, name + ".in")
        pdf_path = os.path.join(OUT_DIR, name + ".pdf")
        with open(in_path, "w") as f:
            f.write(TEMPLATE % {"JS": js})
        subprocess.run([sys.executable, FIXUP, "--output-dir", OUT_DIR,
                        in_path], check=True)
        print(f"wrote {pdf_path} ({os.path.getsize(pdf_path)} bytes)")


if __name__ == "__main__":
    main()
