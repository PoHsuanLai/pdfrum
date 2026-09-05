# `libpdfrum` — pdfrum for C

The C ABI over the [`pdfrum`](../pdfrum) facade: `libpdfrum.so`,
`libpdfrum.a`, and a generated `pdfrum.h`. Anything that speaks the C ABI — C,
C++, Go, C#, Zig, Swift, Python via `ctypes` — can use it.

The rules it is built on are in [`docs/design/capi.md`](../../docs/design/capi.md):
who owns which pointer, who frees what, which handle may cross a thread, and
the one rule under which this crate is allowed `unsafe` at all.

## Build and install

```sh
cargo build -p pdfrum-capi --release          # target/release/libpdfrum.{so,a}
cargo cinstall -p pdfrum-capi --release --prefix /usr/local
```

`cinstall` needs [`cargo-c`](https://github.com/lu-zero/cargo-c)
(`cargo install cargo-c --locked`) and lays down the header, both libraries and
a `pdfrum.pc`, so a consumer's build is:

```sh
cc myprogram.c $(pkg-config --cflags --libs pdfrum) -o myprogram
```

## Twenty lines

```c
#include <pdfrum.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    pdfrum_error error = {0};
    pdfrum_document *doc = pdfrum_open_file("in.pdf", NULL, &error);
    if (!doc) {                                     /* null means it failed */
        fprintf(stderr, "(%u) %s\n", (unsigned)error.code, error.message);
        pdfrum_error_free(&error);                  /* frees error.message */
        return 1;
    }
    printf("%u pages\n", pdfrum_page_count(doc));

    pdfrum_page *page = pdfrum_document_page(doc, 0, &error);
    uint32_t w, h;
    size_t stride;
    pdfrum_page_render_size(page, 2.0, &w, &h, &stride, &error);
    unsigned char *rgba = malloc(stride * h);       /* the buffer is yours */
    pdfrum_page_render(page, 2.0, rgba, stride, NULL, NULL, &error);

    char *text = pdfrum_page_text(page, &error);    /* UTF-8, yours to free */
    printf("%s\n", text);

    free(rgba);
    pdfrum_free(text);                              /* not free(3) */
    pdfrum_page_close(page);
    pdfrum_close(doc);
    return 0;
}
```

A real program checks every return; the example omits it after the first to
stay at twenty lines. `crates/pdfrum-capi/ctest/test.c` is the version that
checks everything.

## The five things to know

**Errors.** Every fallible function takes a trailing `pdfrum_error *`, which
may be `NULL` if you do not care why. Failure is `NULL`, `false` or `0`, and
the error carries a `pdfrum_code` (`PDFRUM_CODE_WRONG_PASSWORD` and friends)
beside a message. `pdfrum_error_free` releases the message and resets the
struct, so one error can serve a whole function.

**Freeing.** A handle is closed by its own function — `pdfrum_close`,
`pdfrum_page_close`, `pdfrum_words_free`. A `char *` or a `pdfrum_buffer`'s
`data` goes to `pdfrum_free`. Never `free(3)` on library memory. Every free
tolerates `NULL`, so a pointer a failed call left behind needs no guard.

**Lists own their strings.** `pdfrum_page_words` returns a handle, not an
array, and each `pdfrum_word`'s `text` and `font` point into it. One
`pdfrum_words_free` releases the lot — there is no per-word free — and the
strings die with the handle, so copy any you need to keep. Links, bookmarks,
attachments and search hits work the same way.

**Threads.** A `pdfrum_document *` is safe to use from any thread and from
several at once. A page, a form and a list are not: one thread at a time. So
the way to render in parallel is a page per worker off one shared document,
which is what the C test does with eight of them.

**Rendering.** Ask `pdfrum_page_render_size` first, allocate `stride * height`
yourself, then `pdfrum_page_render` into it. The output is RGBA8 with
**straight** (not premultiplied) alpha, row-major, top-down: pixel *(x, y)*
starts at `rgba[y * stride + x * 4]`. The buffer stays yours throughout; the
library keeps no pointer to it.

## What is in the header

Fifty-four functions over ten opaque handles. Opening (from bytes or a path,
with a password, with limits and a cancellation flag), pages and their sizes,
rendering, text, words, links, search, bookmarks, attachments, metadata,
images, forms (read, fill, save), saving, and cancellation.

`pdfrum_page_markdown` exists only in a build with the `markdown` feature and
is declared under `#ifdef PDFRUM_MARKDOWN`, so one header serves both.

Not here: the facade's `javascript` feature. A document's own scripts are never
run by this library under any build; a caller who wants that links the Rust
facade with that feature instead.

## Testing it

```sh
crates/pdfrum-capi/ctest/run.sh
```

Compiles `ctest/test.c` with `-std=c11 -Wall -Werror` against the generated
header, links the built library, and runs 44 checks — including eight pthreads
each rendering their own page of one shared document and asserting every pixmap
matches a single-threaded render byte for byte. `scripts/ci.nu` runs it, and
skips it with a note where there is no C compiler.
