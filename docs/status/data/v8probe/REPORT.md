# PDFium+V8 JavaScript resource-bounding probe

**Question.** When a PDF's document JavaScript (a) loops forever, (b) doubles a
string toward ~1 GiB (`var s='a'; for(var i=0;i<30;i++) s=s+s;`), or (c) runs a
catastrophic-backtracking regex (`/(a+)+$/.test('a'.repeat(N)+'b')`), does
PDFium *built with V8* bound the work? Claim under test: it bounds **none** of
the three — no execution timeout, no heap limit, no regex budget; the only
JS resource cap in the tree is a 256 MiB per-`ArrayBuffer` allocator cap in
`fxjs/cfx_v8_array_buffer_allocator.cpp`. This turns that source reading into a
measurement.

## Binary / version used

- Release: **`bblanchon/pdfium-binaries` tag `chromium/8021`**, asset
  `pdfium-v8-linux-x64.tgz` (downloaded with `gh release download`).
- `VERSION`: MAJOR=154 MINOR=0 **BUILD=8021** PATCH=0.
- `args.gn`: `pdf_enable_v8 = true`, `pdf_enable_xfa = true`, `is_debug = false`,
  `v8_enable_i18n_support = false`, `pdf_use_partition_alloc = false`,
  `target_cpu = "x64"`, linux.
- Ships `lib/libpdfium.so` + `include/*.h` (public C API only; **no V8 headers,
  no V8 static lib**). `libpdfium.so` exports `FPDF_InitLibraryWithConfig`,
  `FPDF_GetArrayBufferAllocatorSharedInstance`, `FPDF_GetRecommendedV8Flags`.

## Driver

- Source: **`/home/r13921098/.claude/jobs/89471cbc/tmp/v8probe/driver.cc`**
  (compiled to `./driver`).
- Uses **only the public C API**. Mirrors
  `testing/pdfium_test/pdfium_test.cc`: `ExampleAppAlert` prints
  `"<title>: <msg>"` to stdout; same `FORM_DoDocumentJSAction` ->
  `FORM_DoDocumentOpenAction` -> `FPDF_LoadPage(0)` -> `FORM_OnAfterLoadPage`
  -> `FORM_DoPageAAction(OPEN)` sequence.
- The prebuilt binary ships no V8 headers, so the driver sets
  `FPDF_LIBRARY_CONFIG.m_pIsolate = NULL` and `m_pPlatform = NULL`, which
  fpdfview.h:263 documents as "NULL to force PDFium to create one". PDFium
  builds its own `v8::Isolate` + `v8::Platform` internally -> a faithful
  default embedding.
- Faithfulness detail: this is an **XFA build**, so `CheckFormfillVersion()`
  (fpdfsdk/fpdf_formfill.cpp) rejects `FPDF_FORMFILLINFO.version < 2`. The
  driver uses `version = 2`, `xfa_disabled = 0` (XFA inert for our plain
  AcroForm/JS PDFs). With version 1, `FPDFDOC_InitFormFillEnvironment` returns
  NULL and no JS runs.

Build:
```
g++ -std=c++17 -O2 driver.cc -o driver \
    -I <extract>/include -L <extract>/lib -lpdfium -Wl,-rpath,<extract>/lib
```

## Test PDFs

`gen_pdfs.py` writes `.in` templates in the pdfium fixture shape and expands
them with the oracle's `testing/tools/fixup_pdf_template.py` (run read-only
from the oracle checkout, output into this dir; oracle unmodified). Each PDF
has an `/OpenAction` `/JavaScript` running the payload. The control's
`Alert: hi` confirms the JS pipeline works end to end.

Run harness per case:
```
/usr/bin/time -v timeout --signal=KILL <T> ./driver <case>.pdf
```
`ulimit -v` **unset** (default) unless a row says otherwise. `rc=137` = 128+9
= SIGKILL by `timeout` (never self-terminated). Peak RSS = `/usr/bin/time -v`
"Maximum resident set size".

## Results

| Case | JS payload | Timeout | Wall | Exit | Peak RSS | Alert / V8 output |
|---|---|---|---|---|---|---|
| control | `app.alert('hi')` | 120s | 0.02s | 0 | 16.6 MB | `Alert: hi` |
| **(a) loop** | `while(true){}` | 120s | **120.00s** | **137 SIGKILL(timeout)** | 2.0 MB | `before-loop` only; never returns |
| (a) loop confirm | `while(true){}` | 600s | **600.01s** | **137 SIGKILL(timeout)** | 2.0 MB | `before-loop` only; never returns |
| **(b) string** | `s='a'; 30x s=s+s` | 120s | **0.04s** | **0** | 17.2 MB | `before-string` only -- silent RangeError, no `after` |
| (b) string N=28 | 28x (len 2^28) | 120s | 0.00s | 0 | 17.4 MB | completes: `after-string len=268435456` |
| (b) string N=29 | 29x (len 2^29) | 120s | 0.00s | 0 | 17.4 MB | `before-string` only -- silent RangeError |
| (b) heap-array | 200k x `Array(1000)` | 120s | 0.93s | 0 | **860 MB** | completes: `after-heap blocks=200000` |
| (b) heap-array big | 2M x `Array(1000)` | 120s | 3.82s | **133 SIGTRAP, core dumped** | **1.58 GB** | `before-heap` only; V8 fatal `Reached heap limit` |
| **(c) regex 20** | `/(a+)+$/.test('a'*20+'b')` | 120s | 0.02s | 0 | 17.7 MB | completes: `after false` |
| **(c) regex 24** | ...*24 | 120s | 0.18s | 0 | 17.7 MB | completes: `after false` |
| **(c) regex 26** | ...*26 | 120s | 0.80s | 0 | 17.4 MB | completes: `after false` |
| **(c) regex 28** | ...*28 | 120s | 3.13s | 0 | 17.7 MB | completes: `after false` |
| (c) regex 32 | ...*32 | 120s | 56.98s | 0 | 17.2 MB | completes: `after false` |
| (c) regex 34 | ...*34 | 120s | **120.00s** | **137 SIGKILL(timeout)** | 2.0 MB | `before` only; never returns |

Regex wall time grows ~4x per +2 length (0.18 -> 0.80 -> 3.13 -> 56.98s): pure
exponential backtracking with no engine budget; it "finishes" only while 2^n is
still tractable, and length 34 blew past the external 120s wall. RSS stays
~17 MB (backtracking is CPU-bound).

### External-cap side runs (`ulimit -v 2000000`, a 2 GB *virtual*-memory cap)

| Case | Wall | Exit | Peak RSS | Output |
|---|---|---|---|---|
| string | 0.15s | 134 SIGABRT | 8 MB | fatal at init: `Oilpan: CagedHeap reservation` |
| heap-array big | 0.09s | 134 SIGABRT | 8 MB | fatal at init: `Fatal process out of memory: Oilpan: CagedHeap reservation` |

`ulimit -v` is the wrong external control for this V8: it reserves a huge
*virtual* address space up front (VSZ ~1.4 TB while RSS is tens of MB), so a
2 GB VM cap aborts V8 **during library init, before any JS runs** -- the alert
never prints. An RSS-based cgroup `memory.max` is the correct external cap;
`ulimit -v` cannot be used with this V8 build.

## Public-API knob audit (grep of shipped headers)

- `timeout`, `budget`, `quota`, `gas`: **0 matches** anywhere.
- `limit`: only image-cache (`FPDF_RENDER_LIMITEDIMAGECACHE`) and mail-field
  length comments -- nothing about JS/CPU/heap.
- `terminate`: only "string terminated by NUL" comments -- **no execution
  Terminate hook exposed**.
- Entire V8-related public surface is three items, none of which bounds runtime
  cost:
  - `FPDF_LIBRARY_CONFIG.{m_pIsolate,m_pPlatform,m_v8EmbedderSlot}` -- bring your
    own isolate/platform. An embedder linking V8 itself could set
    `Isolate::CreateParams` heap limits / call `TerminateExecution` / add a
    near-heap-limit callback, but **none is exposed by PDFium**, and the
    prebuilt ships no V8 headers to reach it.
  - `FPDF_GetRecommendedV8Flags()` returns **`"--jitless"`** only -- no
    `--max-old-space-size`, no `--stack-size`, no time/regex flag.
  - `FPDF_GetArrayBufferAllocatorSharedInstance()` -- allocator whose
    `kMaxAllowedBytes = 0x10000000` = **256 MiB per-`ArrayBuffer` cap**
    (`fxjs/cfx_v8_array_buffer_allocator.{h,cpp}`), confirmed. Caps a single
    `ArrayBuffer` only; not total heap, CPU, or string length; irrelevant to
    (a)/(b)/(c).

## Verdict

- **(a) Infinite loop -- NOT bounded.** PDFium+V8 runs `while(true){}` forever.
  Survived a 120s SIGKILL and a confirming 600s SIGKILL, both at ~2 MB RSS,
  never returning. No execution timeout, no interrupt, no `TerminateExecution`
  wired to the public API. Only an *external* wall-clock kill stops it. (Our
  boa engine's ~10 ms runtime-limit stop has no analogue here.)
- **(b) Unbounded memory -- NOT bounded by any PDFium/V8 *heap* policy.** The
  `s=s+s` payload stops at ~0.04s / 17 MB, but *not* from a heap bound: it hits
  V8's intrinsic `String::kMaxLength` (2^29 chars; N=28 succeeds at len 2^28,
  N=29 throws) as a **catchable RangeError that PDFium silently swallows** (no
  alert, no error text, no crash). A workload that grows the heap via many
  allocations instead of one giant string is *not* caught: an array-of-arrays
  reached 860 MB and completed cleanly, and scaling up drove RSS to 1.58 GB and
  then a **V8 fatal `Reached heap limit` abort (SIGTRAP, core dump)** at V8's
  default ~1.4 GB old-space limit. That default is not a graceful document
  bound -- it kills the **entire host process**, and PDFium exposes no callback
  to survive it. So: no per-document memory bound; the only ceilings are
  V8-internal (string length -> catchable; heap limit -> process crash).
- **(c) Catastrophic-backtracking regex -- NOT bounded.** V8 Irregexp has no
  backtracking budget here. Runtime is purely exponential in input length
  (0.18/0.80/3.13/56.98s at 24/26/28/32) at flat ~17 MB RSS; it "completes"
  only while 2^n stays under the wall clock, and length 34 ran past 120s and
  was SIGKILLed. Nothing internal stops it.

**Overall:** claim confirmed. PDFium built with V8 bounds none of (a)/(b)/(c)
through any timeout, heap-limit, or regex budget of its own. The only
mechanisms that ever intervened were V8-intrinsic and non-graceful -- a
catchable string-length RangeError (silently dropped by PDFium) and a
process-fatal heap-limit abort -- plus the sole PDFium-authored JS cap, the
256 MiB per-`ArrayBuffer` allocator limit, irrelevant to these three attacks.
Effective mitigation requires the embedder to run its own external wall-clock
and RSS (cgroup) limits; `ulimit -v` specifically cannot be used, as it kills
V8 at init.

## What could not be run / caveats

- **`ulimit -v <cap>` could not exercise the memory workload**: V8's huge
  up-front virtual reservation makes any practical VM cap abort V8 during init
  (before JS runs); the alert never prints. The heap-growth measurement was
  taken with `ulimit -v` unset (RSS observed directly). This is itself a
  finding, not a gap.
- The prebuilt binary ships **no V8 headers/lib**, so an embedder that installs
  its own `Isolate` heap limit / `TerminateExecution` / near-heap-limit
  callback could not be tested. The audit shows PDFium exposes no path to those
  through its public API regardless; a from-source build linking V8 would be
  needed to probe that hypothetical (out of scope).
- `heap`/`heapbig` are extra probes added to distinguish "V8 string-length
  ceiling" from "V8 heap OOM crash"; the required string-doubling PDF
  (`string.pdf`, 30 iterations) is present and reported.

## Reproduce

```
cd /home/r13921098/.claude/jobs/89471cbc/tmp/v8probe
# binary already extracted (lib/, include/); tag chromium/8021
g++ -std=c++17 -O2 driver.cc -o driver -Iinclude -Llib -lpdfium -Wl,-rpath,lib
python3 gen_pdfs.py
/usr/bin/time -v timeout --signal=KILL 120s ./driver loop.pdf
g++ -std=c++17 -DPDF_ENABLE_V8 flags.cc -o flags -Iinclude -Llib -lpdfium -Wl,-rpath,lib && ./flags
```

Raw captures: `results/*.out` (stdout) and `results/*.time` (`/usr/bin/time -v`).
