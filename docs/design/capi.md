# `libpdfrum` — the C ABI

**Contract:** PLAN.md §M22.2 and §M22.3. **Status:** landed 2026-09-05, M22
phase 2.

`crates/pdfrum-capi` builds `libpdfrum.so` and `libpdfrum.a`, and generates
`include/pdfrum.h`, so a program in C — or in Go, C#, Zig, Swift, or anything
else that speaks the C ABI — can use pdfrum the way it uses libpdfium. This
file states the five rules the boundary is built on. Each is a rule rather
than a description because each is a thing a future change could break without
any gate noticing, if it were not written down.

The crate holds **no logic**. Every exported function is a handle plus one call
into the facade, and where this document says "the library decides" it means
the facade already decided and this layer forwards.

## 1. The unsafe rule

The workspace sets `unsafe_code = "forbid"` (STYLE.md §3). This crate alone
overrides it to `allow`, in its own `[lints.rust]`. That is a real weakening of
a real guarantee, and it is bounded by a rule narrower than the lint it
replaces:

- **`unsafe` appears only inside an `extern "C"` boundary function**, or inside
  a private helper in this crate that exists to serve one. Nothing below the
  boundary is unsafe: the facade and everything under it stay under
  `forbid`, and no `unsafe` block in this crate calls into them with anything
  but ordinary Rust values.
- **Every `extern "C"` function carries a `# Safety` section** naming what the
  caller must guarantee about each pointer it is passed. cbindgen copies the
  doc comment into the header, so the contract a C programmer reads is the same
  text a Rust reviewer reads.
- **Every `unsafe` block carries a `// SAFETY:` comment** saying which of those
  guarantees it is leaning on. A block whose justification is only "the caller
  promised" says so, and names the promise.
- **Pointer validity is the caller's contract. Pointer nullness is ours.** C
  cannot prove a pointer is live and neither can we, so a dangling handle is
  undefined and the header says so. A *null* handle is not: every pointer this
  library reads is null-checked, and a null is answered with an error return —
  `NULL`, `false`, `0`, or a filled `pdfrum_error` — never a crash. The C test
  exercises that for the whole surface.

The practical reason for the rule, rather than a blanket `allow`: a bug in a
translation layer is found by reading the layer, and a bug that could be
anywhere is found by reading everything. Keeping `unsafe` at the boundary keeps
the search small.

## 2. The type rule

The boundary is type-driven on the Rust side, not a set of conventions about
what a `void *` currently points at.

- **Each handle is its own newtype.** `pdfrum_document(Arc<Document>)`,
  `pdfrum_page(OwnedPage)`, `pdfrum_form { .. }`, `pdfrum_words { .. }`. A
  function that takes a page takes a `*const pdfrum_page`, so passing a
  document where a page belongs is a Rust type error rather than a runtime
  surprise. There is no `void *` handle anywhere in the crate.
- **The C-visible contract is `#[repr(C)]` structs and `#[repr(u32)]` enums.**
  `pdfrum_code` names the failure kinds as `PDFRUM_CODE_*` and
  `pdfrum_field_kind` the field kinds as `PDFRUM_FIELD_KIND_*`. Both are the
  *actual field types* of the structs that carry them, not `uint32_t` with the
  meanings written in a comment, so a C caller switches on a name.
- **Each such enum converts from the facade by a total match**, never a numeric
  cast. A variant added to `pdfrum::FieldKind` is a compile error here.
  `pdfrum::ErrorCode` is `#[non_exhaustive]` so its match needs a wildcard, and
  that arm is commented with what it does and why it cannot be dropped.
- **Options are structs.** `pdfrum_limits`, `pdfrum_save_options` — never a
  positional list of booleans a caller has to count. A zeroed struct is the
  default in every case, so `memset` and `= {0}` mean "as if I had not asked".
- **One typed helper carries the null check and the error fill.**
  `with_handle(ptr: *const T, err, f: impl FnOnce(&T) -> Result<R>)`, its
  `_mut` twin, and the bare `catch` for a body with no handle. Each `extern
  "C"` function is a one-liner around a typed core, which is what puts the null
  check in one place instead of fifty-four.

## 3. The memory rule

**Every pointer the library hands out is freed by exactly one named library
function, and the function's own documentation names it.** A caller never calls
`free(3)` on library memory, and never passes its own pointer to a library
free.

There are three kinds of pointer and three answers:

| What | Freed by |
|---|---|
| A handle (`pdfrum_document`, `pdfrum_page`, `pdfrum_form`, `pdfrum_cancel`, the six list handles) | Its own `pdfrum_close` / `pdfrum_page_close` / `*_free` |
| A `char *` or a `pdfrum_buffer::data` | `pdfrum_free` |
| A `pdfrum_metadata`'s eight strings | `pdfrum_metadata_free`, all at once — **not** individually |

Underneath, there is **one allocator and one owner type per kind**, and no
`Box::from_raw` on a guessed type anywhere. `Bytes` is the sole allocator of
caller-owned blocks; `CString` and `Buffer` are its two typed faces.

Because Rust's allocator needs the exact layout a block was made with, and a
`pdfrum_buffer` of arbitrary bytes cannot recover its length by scanning for a
NUL, **`Bytes` writes a length word ahead of the payload** and hands out a
pointer to the payload. That is what lets a single `pdfrum_free(void *)` answer
for every string and every buffer alike. The cost is one word per allocation;
the gain is a header with one free function whose contract a reader can hold in
their head, instead of `pdfrum_free_string` and `pdfrum_free_buffer` and the
first caller who mixes them up.

**Lists own their items' strings, so there is no per-item free.** A
`pdfrum_word` carries `text` and `font` as `const char *`, and both point
*into* the `pdfrum_words` handle. One `pdfrum_words_free` releases the items
and every string at once. The same shape holds for links, bookmarks,
attachments and hits. A caller that needs a string to outlive its handle copies
it, and the header says so on every getter.

**A form field's `name` and `value` are borrowed and short-lived**: they live
in the form handle's scratch, and the *next* call on that handle replaces them.
That is the one place in the header where a borrow does not last as long as its
handle, and it is documented on `pdfrum_form_field` because the alternative —
accumulating every string a thousand-field walk ever asked for — is a leak
shaped like a feature.

## 4. The thread rule

**A `pdfrum_document *` may be used from any thread and from several at once.**
Every function taking a `const pdfrum_document *` is safe to call
concurrently, because `pdfrum::Document` is `Sync` and the handle is an `Arc`
of one.

**Everything else is single-threaded**: a page, a form, and every list handle
is used by one thread at a time, though which thread may change. This is the
same division PDFium draws, and for the same reason.

The intended shape, and the one the C test proves with eight pthreads, is **one
document shared and one page per worker**. `pdfrum_cancel` is the single
exception in the other direction: it is meant to be raised from another thread
while a render is running, and `pdfrum_cancel_stop` is the one function that
may be called concurrently with any other on the same handle.

A handle's `*_free` always needs the handle to itself, as every free does.
Closing a document while its pages are still open is nonetheless fine: each
handle holds its own reference, so the bytes stay until the last one goes, and
the order a caller frees things in does not matter.

## 5. The error rule

**Every function that can fail takes a trailing `pdfrum_error *`, which may be
null**, and returns `NULL`, `false` or `0` on failure. When the pointer is not
null it is filled with a `pdfrum_code` and a heap-allocated UTF-8 message; on
success neither field is touched.

`pdfrum_code` is the facade's `ErrorCode` one for one — `PDFRUM_CODE_IO` 1,
`PDFRUM_CODE_OPEN` 2, `PDFRUM_CODE_WRONG_PASSWORD` 3, and so on, the numbers
pinned by a table test in `crates/pdfrum/src/error.rs` — plus two of this
layer's own: `PDFRUM_CODE_OK` 0, which a zeroed error reads, and
`PDFRUM_CODE_ARGUMENT` 100, for a call whose arguments were not usable at all:
a null handle, an index past the end, a string that is not UTF-8.

`pdfrum_error_free` releases the message, sets it back to null and the code
back to `PDFRUM_CODE_OK`, so calling it twice is defined and does nothing the
second time. A caller may reuse one `pdfrum_error` down a whole function as
long as it frees between failures.

The library **never panics across the boundary** and never aborts on a
condition a caller could have caused. `panic`, `unwrap` and `expect` are denied
in this crate as everywhere else in the workspace.

## Naming

- Every exported symbol begins `pdfrum_`; every macro and enum constant begins
  `PDFRUM_`.
- A function on a handle is `pdfrum_<handle>_<verb>`:
  `pdfrum_page_render`, `pdfrum_form_set`, `pdfrum_words_count`.
- The four constructors are the exceptions, because they have no handle to
  name yet: `pdfrum_open`, `pdfrum_open_with`, `pdfrum_open_file`,
  `pdfrum_cancel_new`.
- **A type and a function may not share a name.** C has one namespace for both,
  so a function named for a type shadows the typedef and no caller can declare
  that type afterwards. The accessor for a page is therefore
  `pdfrum_document_page`, not `pdfrum_page`. This is not a style preference —
  it is a rule the C test discovered by failing to compile, and it is why the
  C test exists.
- The Rust types carry their C names (`pdfrum_document`, not `PdfrumDocument`),
  with `non_camel_case_types` allowed crate-wide for that reason. A grep for a
  name finds both sides of the boundary.

## What is not here

- **Scripts.** The facade's `javascript` feature is not forwarded and there is
  no `pdfrum_script_*`. Running a document's own JavaScript is a different
  security proposition from rendering it, and a C caller who wants it links the
  Rust facade with that feature. The header's preamble says so.
- **Page-object editing.** PLAN.md §M22.2 scopes it out of v1.
- **An `fpdf_*` compatibility shim.** PLAN.md §M22 declines it, with reasons.
- **A backend choice.** `pdfrum_page_render` uses `vello-cpu`, the facade's own
  default. Choosing a rasterizer is a decision about which crate to compile,
  and a C caller links whatever this library was built with.

## The gates

Three, and they check different things:

1. **`cargo clippy -p pdfrum-capi --all-targets -- -D warnings`**, pedantic,
   with `missing_docs` on — so an exported function without a documented
   contract does not compile.
2. **`scripts/capi-header.nu check`** — the header is what cbindgen makes of
   the source (no drift), *and* its declarations match the library's exported
   symbols in both directions (`nm -D`). Both halves were proved to fail on a
   planted fault before being trusted.
3. **`crates/pdfrum-capi/ctest/run.sh`** — a real C program, compiled with
   `-std=c11 -Wall -Werror` against the generated header and linked against the
   built library. It is the only gate that can see a C-level defect, and it
   found one on its first run.

`scripts/ci.nu` runs all three, skipping 2 and 3 with a printed note when
`cbindgen` or a C compiler is absent — both are tools, not dependencies
(DEPS.md, "The C library's tools").

## Packaging

`cargo cinstall --release --prefix <dir>` installs, verified 2026-09-05:

```
<prefix>/include/pdfrum.h
<prefix>/lib/<triple>/libpdfrum.a
<prefix>/lib/<triple>/libpdfrum.so -> libpdfrum.so.0.1.0
<prefix>/lib/<triple>/libpdfrum.so.0.1 -> libpdfrum.so.0.1.0
<prefix>/lib/<triple>/libpdfrum.so.0.1.0
<prefix>/lib/<triple>/pkgconfig/pdfrum.pc
```

The `.pc` names the package `pdfrum`, so `pkg-config --cflags --libs pdfrum`
is all a consumer's build needs; it was compiled and run against for this
document. cargo-c generates the header from the same `cbindgen.toml`, and the
installed header differs from the committed one only by the `PDFRUM_MAJOR` /
`PDFRUM_MINOR` / `PDFRUM_PATCH` macros cargo-c adds — which is why
`scripts/capi-header.nu` regenerates with `cbindgen` directly rather than
diffing against a `cinstall` output.

The empty `capi` feature in `Cargo.toml` exists because `cargo cbuild` passes
`--features capi` and fails on a manifest without it. It gates nothing: the C
ABI is what this crate is, not something a feature adds.
