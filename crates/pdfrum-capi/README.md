# `libpdfrum`

C ABI over [`pdfrum`](https://crates.io/crates/pdfrum): `libpdfrum.so`, `libpdfrum.a`, `pdfrum.h`.
Every export is an opaque handle plus one call into the facade. This is the
one workspace crate where `unsafe` is allowed, and only inside `extern "C"`
(or a private helper of one). Zeroed option structs are the defaults.

```sh
cargo build -p pdfrum-capi --release
cargo cinstall -p pdfrum-capi --release --prefix /usr/local   # needs cargo-c
cc myprogram.c $(pkg-config --cflags --libs pdfrum) -o myprogram
```

```c
#include <pdfrum.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    pdfrum_error error = {0};
    pdfrum_document *doc = pdfrum_open_file("in.pdf", NULL, &error);
    if (!doc) {
        fprintf(stderr, "(%u) %s\n", (unsigned)error.code, error.message);
        pdfrum_error_free(&error);
        return 1;
    }
    pdfrum_page *page = pdfrum_document_page(doc, 0, &error);
    uint32_t w, h;
    size_t stride;
    pdfrum_page_render_size(page, 2.0, &w, &h, &stride, &error);
    unsigned char *rgba = malloc(stride * h);
    pdfrum_page_render(page, 2.0, rgba, stride, NULL, NULL, &error);
    char *text = pdfrum_page_text(page, &error);
    printf("%u pages\n%s\n", pdfrum_page_count(doc), text);
    free(rgba);
    pdfrum_free(text);          /* not free(3) */
    pdfrum_page_close(page);
    pdfrum_close(doc);
    return 0;
}
```

- Fallible functions take a trailing `pdfrum_error *` (nullable). Failure is
  `NULL` / `false` / `0`.
- Handles: `pdfrum_close`, `pdfrum_page_close`, `*_free`. Strings and
  `pdfrum_buffer::data`: `pdfrum_free`. Never `free(3)` on library memory.
- List strings live in the list handle. One `pdfrum_words_free` frees them.
- `pdfrum_document *` is `Sync`. A page, form, or list is one thread at a
  time. Parallel render: one page per worker, shared document.
- Render: `pdfrum_page_render_size`, allocate `stride * height`, then
  `pdfrum_page_render`. RGBA8, straight alpha, top-down.

## Features

| feature | default | adds |
|---|:---:|---|
| `markdown` | off | `pdfrum_page_markdown`, behind `#ifdef PDFRUM_MARKDOWN` |
| `capi` | — | required by `cargo cbuild` / `cinstall`; gates nothing |

No JavaScript: the facade's `javascript` feature is not forwarded, so a
document's scripts are data here whatever the Rust build does.

`publish = false` — nothing `cargo add`s a cdylib. The published surface is
[`include/pdfrum.h`](https://github.com/PoHsuanLai/pdfrum/blob/main/crates/pdfrum-capi/include/pdfrum.h),
which a CI snapshot gate diffs, and
[`ctest/run.sh`](https://github.com/PoHsuanLai/pdfrum/blob/main/crates/pdfrum-capi/ctest/run.sh)
is the checked example.

MIT OR Apache-2.0
