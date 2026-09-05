# `pdfrum` on the web — the WebAssembly binding

**Contract:** PLAN.md §M22.4. **Status:** landed 2026-09-05, M22 phase 4.

`crates/pdfrum-wasm` builds a WebAssembly module and the JavaScript that loads
it, so a browser or a Node program can open a PDF, render a page onto a canvas,
pull its text out, fill its form and save the result. It is the same library
`libpdfrum` is, reached the other way.

It is the C ABI's sibling and it is built on the same four rules — the type
rule, the memory rule, the thread rule, the error rule (docs/design/capi.md).
This file states them again where JavaScript changes the answer, and states the
two things this target takes away.

The crate holds **no logic**. Every export is a handle plus one call into the
facade, and where this document says "the library decides" it means the facade
already decided.

## 1. The shape

Four handles, twelve value types and one function. The `.d.ts` wasm-bindgen
generates from the same source is the authority; this is the shape of it.

```ts
class Document {
  static open(bytes: Uint8Array, password?: string): Document;
  static openWith(bytes: Uint8Array, password?: string,
                  options?: OpenOptions): Document;
  readonly pageCount: number;
  page(index: number): Page;
  metadata(): Metadata;
  bookmarks(): Bookmark[];
  attachments(): Attachment[];
  save(options?: SaveOptions): Uint8Array;
  free(): void;
}

class Page {
  readonly width: number;   // PDF points
  readonly height: number;
  render(scale: number, options?: RenderOptions): RenderResult;
  text(): string;
  words(): Word[];
  links(): Link[];
  search(needle: string, ignoreCase?: boolean): Hit[];
  markdown(): string;
  images(): ImageInfo[];
  free(): void;
}

class Form {
  static open(doc: Document): Form;
  fields(): Field[];
  set(name: string, value: string): void;
  setChecked(name: string, on: boolean): void;
  save(options?: SaveOptions): Uint8Array;
  free(): void;
}

class Cancel { constructor(); stop(): void; free(): void; }

function version(): string;
```

The values are `RenderResult`, `Word`, `Link`, `Hit`, `Bookmark`,
`Attachment`, `Metadata`, `ImageInfo`, `Field` and the `FieldKind` string
union, plus the three option objects `OpenOptions`, `RenderOptions` and
`SaveOptions`.

**`RenderResult.data` is a `Uint8ClampedArray`**, not a `Uint8Array`, and that
is the whole reason the type exists: `new ImageData(r.data, r.width, r.height)`
requires a clamped array, so returning the plain one would make every caller
copy the buffer a second time for nothing. The pixels are RGBA8 with straight
(not premultiplied) alpha, row-major, top-down — `ImageData`'s own layout. The
facade's pixmap is premultiplied and the conversion happens in the binding
rather than being left to a caller who would have to know it was needed.
`ImageInfo.data` has the same shape, so the same line displays a page image.

### Why typed classes and not plain objects

`serde-wasm-bindgen` would have been fewer lines and would have reached
TypeScript as `any`. The TypeScript types are part of what PLAN.md §M22.4 asks
for, so every structured return is a `#[wasm_bindgen(getter_with_clone)]`
struct instead: sixteen named classes with typed fields, so a caller's editor
knows a `Word` has a `text` and a `size` before the code has run. The cost is
one `pub` field per value.

The same reasoning makes the option objects classes with constructors rather
than plain objects: a plain object arrives in Rust as a `JsValue` this crate
would have to reflect fields out of one string at a time, and arrives in
TypeScript as `any`. `new OpenOptions()` with typed setters is the same trade
`pdfrum_limits` makes in the C header, where a zeroed struct is the default.

## 2. The memory rule

**Every handle has a `free()`, and a caller is expected to call it.**

JavaScript's garbage collector sees a small wrapper object. It does not see the
parsed object store, the font cache and the file bytes that wrapper points at
inside the module's linear memory, so it has no reason to collect promptly and
no way to price not doing so. This is wasm-bindgen's own convention for every
exported struct and the binding keeps it rather than inventing a second one.

The handles whose `free()` matters are `Document`, `Page`, `Form` and `Cancel`.
The *values* are plain data already copied out of the module; freeing one is
allowed and pointless.

**A handle keeps its document alive, so the order does not matter.** Freeing a
document while a page of it is still open is defined — each handle holds its
own `Arc`, and the bytes stay until the last one goes. This is the same promise
`pdfrum_close` makes in C, and there is a test for it.

The bytes handed to `Document.open` are **copied** into the module, so the
caller's `Uint8Array` may be reused or detached as soon as the call returns.

## 3. The error rule

**A failed call throws a JavaScript `Error`** whose `message` is the facade's
own `Display` and whose `code` is a number to branch on.

```js
try {
  Document.open(bytes, "wrong");
} catch (e) {
  if (e.code === 3) console.log("bad password:", e.message);
}
```

The codes are the facade's `ErrorCode`, one for one, which is the same table
`pdfrum.h` names `PDFRUM_CODE_*`: `Io` 1, `Open` 2, `WrongPassword` 3, `Read`
4, `Render` 5, `Doc` 6, `Save` 7, `Text` 8, `Limit` 9 — plus **100** for an
argument this layer rejected before the facade saw it (a scale that is not a
positive finite number, a field name that is not in the form, a document with
no form at all). One table, two bindings, and the numbers are pinned by a table
test in `crates/pdfrum/src/error.rs`.

There is exactly **one** `From<Failure> for JsValue` in the crate and no
per-function string anywhere. It builds a real `Error` — not a string — so a
caller gets `e.stack` and `instanceof Error` for free, and sets `code` on it
with `Reflect::set`, which is how a JavaScript library carries a machine
kind beside a human message.

## 4. Threads, and time

**There are no threads.** A WebAssembly module without the threads proposal
runs on one, and this binding compiles the facade with no thread pool. Two
workers each want their own module instance and their own copy of the bytes.
The C library's "one document shared, one page per worker" shape has no
equivalent here and needs none.

**There is no clock.** `Instant::now()` panics on `wasm32-unknown-unknown`, so
the facade's `Deadline::after` — a wall-clock budget the library polls — cannot
be constructed at all, and this binding has no `timeLimitMs` option to offer.

The one stopping mechanism is `Cancel`, the host-set flag, built on
`Deadline::manual`. A caller raises it from its own `setTimeout`, an abort
button, or a message from the main thread, and work in progress stops at its
next check and reports `Limit` — `.code === 9`. This is not a lesser
substitute: the facade checks the flag at exactly the points it would have
checked a clock, so the difference is only *who owns the timer*, and on the web
the host owns it already.

`Cancel` is the one handle that may be touched while another call is running,
which is the only way it is useful.

## 5. The features, and the two that are out

The facade is compiled with `default-features = false` and five named back:

| Feature | Why |
|---|---|
| `vello-cpu` | The rasterizer. Pure Rust, no threads, no GPU handshake. |
| `codecs-all` | JPX, JBIG2 and CCITT. A PDF on the web is no less likely to carry them than one on a disk. |
| `forms` | Reading and filling a form. |
| `edit` | Saving the result, which is the half that makes filling worth anything. |
| `markdown` | `Page.markdown()` — cheap to carry, and the one text shape a web page actually wants to paste. |

Two are **off**, and PLAN.md §M22.4 says why:

- **`system-fonts`.** There is no host font directory in a browser. The bundled
  Foxit base-14 faces carry text, which is what the facade's own `wasm32` check
  has assumed since it was written.
- **`javascript`.** Two reasons and either is sufficient. `boa`'s `getrandom`
  needs its `wasm_js` feature on this target, so the build does not even
  resolve without extra work. And running a document's own script inside a page
  is a different security proposition from rendering it — the same judgement
  the C library makes, for the same reason. A caller who wants it builds their
  own module.

Page-object editing is out of v1, as it is in C (PLAN.md §M22.2).

## 6. The size budget

The module a browser downloads is a deliverable in its own right, so it is
measured and held to a number.

**The profile is `[profile.wasm]` at the workspace root**, not in the member:
cargo ignores a `[profile.*]` table in a workspace member, which is worth
knowing because the manifest accepts it silently. It is a named profile rather
than a change to `release`, because `lto` and one codegen unit cost native
build time that nothing else in this workspace is asking to spend.

**`opt-level = 3`, measured.** On the shipped `_bg.wasm`:

| `opt-level` | raw | after `wasm-bindgen` | after `wasm-opt -Oz` |
|---|---|---|---|
| `3` | 5,650,552 | 5,097,256 | **4,827,012** |
| `"s"` | 5,959,819 | 5,368,249 | 4,932,482 |
| `"z"` | 6,040,761 | 5,409,739 | 4,982,040 |

`3` wins at every stage, by 3% over `"z"` on the artefact that ships. That is
the opposite of the usual WebAssembly advice, which is why it was measured
rather than assumed: this crate's bulk is a rasterizer and three image codecs,
whose inner loops `"z"` declines to unroll or vectorize and then pays for in
call overhead and spill code. No render-time tie-break was needed, `3` being
both the smallest and the fastest.

**The budget is 5,792,414 bytes**, 20% above the 4,827,012 measured, checked by
`scripts/wasm-package.nu` and failing the build when the optimized module
exceeds it. Twenty percent leaves room for ordinary growth in the facade and
still catches a step change — a codec accidentally compiled in, a panic
formatter, a `system-fonts` that crept back. Raising it is a deliberate commit
with a sentence about what got bigger and why it is worth it.

## 7. Packaging

`nu scripts/wasm-package.nu` builds `crates/pdfrum-wasm/pkg` (git-ignored):

```
pkg/pdfrum_bg.wasm      the module
pkg/pdfrum.js           the loader, ES module
pkg/pdfrum.d.ts         the TypeScript types, generated from the Rust
pkg/pdfrum_bg.wasm.d.ts the raw import types
pkg/package.json        name `pdfrum`, type module, main and types
pkg/README.md           the fifteen-line canvas example
```

The script is three tools in a row — `cargo build --profile wasm`,
`wasm-bindgen --target web`, `wasm-opt -Oz` — and it records the size before
and after the optimizer and checks the budget.

### Why not `wasm-pack`

It was the first choice and it cannot drive this crate. wasm-pack runs
`wasm-opt` itself with a hardcoded `-O` and no feature flags, and **that
invocation fails here**: rustc's `wasm32-unknown-unknown` emits bulk-memory
(`memory.copy`, `memory.fill`) and non-trapping float-to-int, and `wasm-opt`'s
validator rejects both unless told they are allowed — it still defaults to the
2017 MVP.

The flags cannot be supplied through the manifest either. wasm-pack reads
`[package.metadata.wasm-pack.profile.<name>]` only for the three profiles it
knows — `dev`, `release`, `profiling` — and this crate builds under the
workspace's own `wasm` profile, under which it consults no metadata at all.
Both `wasm-opt = false` and a full flag list were tried under `.wasm` and under
`.release`; neither is read.

So the three tools are called directly. It costs one script and buys the
before/after measurement the budget check needs. The `.d.ts` is wasm-bindgen's
own output either way — wasm-pack never generated it.

## 8. The gates

| Gate | What it catches |
|---|---|
| `cargo clippy -p pdfrum-wasm --all-targets` on both targets, `-D warnings` | pedantic plus `missing_docs`, so an export without a documented contract does not compile |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps -p pdfrum-wasm` | a broken intra-doc link, invisible to every other gate |
| `cargo test --target wasm32-unknown-unknown`, from inside the crate | the binding, where it actually runs |
| `nu scripts/wasm-package.nu` | the size budget |

The test stage is in `scripts/ci.nu` after the C test, skipped with a printed
note when `node`, the `wasm32-unknown-unknown` target or
`wasm-bindgen-test-runner` is absent — the bargain `cargo deny` and the C test
already get.

**Running the tests from inside the crate is load-bearing.** The runner is
named in `crates/pdfrum-wasm/.cargo/config.toml`, and cargo reads a
`.cargo/config.toml` relative to the *invocation* directory rather than the
manifest. From the workspace root with `--manifest-path` the tests build and
then fail to execute with `Exec format error`, because cargo tries to run the
`.wasm` file itself.

Thirteen tests, and each is there for a failure it can see:

- two pages counted, and `version()` matching the crate;
- a render at scale 1 with **a non-white pixel asserted** — a render that drew
  nothing at all would pass a size check alone;
- `Hello, world!` in the text; words non-empty and indexing into that same
  string;
- `text_form.pdf` filled, saved, and **reopened to read the value back** — a
  save that wrote into a place nothing reads would pass every assertion above
  it;
- a wrong password with `.code === 3`, a 1000-pixel cap with `.code === 9`, a
  raised `Cancel` with the same, a bad scale with `.code === 100`, an index
  past the end with `.code === 4`;
- a document with no form refused at `Form.open` rather than at first use;
- **a page outliving its document**, which is the memory rule a caller is most
  likely to get wrong.

## 9. Two things the gates found

**`js-sys` is not a `-sys` crate.** `scripts/ci.nu`'s pure-Rust check matches
on the name and flagged it. The `-sys` here means the JavaScript *standard
library*, not a C one: there is nothing native to bind, the foreign side is the
host engine reached through wasm-bindgen's imports, and the published crate has
no `build.rs` at all. It is on the same false-positive list as `linux-raw-sys`,
by name and with the reason.

**`cc` reaches the graph through the test harness.** `wasm-bindgen-test`
depends unconditionally on `minicov` — a coverage helper, with no feature that
turns it off in 0.3.78 — which build-depends on `cc`. `cargo deny` bans `cc`
outright, and rightly.

Measured before the exception was written: `cargo tree --workspace --target all
-e normal,build` finds no `cc` at all, and neither does the same query for
`pdfrum-wasm` on `wasm32-unknown-unknown`. It is reachable only through `-e
dev`. So nothing that ships — no library, not `libpdfrum`, and not the `.wasm`
a browser downloads — has a C compiler anywhere in its tree, and DEPS.md's
guarantee, which is about what the library compiles and links, is untouched.

The ban is therefore **scoped, not lifted**: `wrappers = ["minicov"]`, the
shape M12c's `pkg-config` exception already set, so a second crate reaching for
`cc` still fails the build. The alternative was no test harness for the
WebAssembly binding at all, and an untested binding is the worse trade.

## 10. What is not here

- **Scripts**, for the two reasons in §5.
- **Threads**, for the reason in §4. If the threads proposal is wanted later it
  is a different module with a different memory, not a flag on this one.
- **A backend choice.** `vello-cpu`, the facade's default; choosing a
  rasterizer is a decision about which crate to compile.
- **A wall-clock limit**, for the reason in §4. `Cancel` is the answer.
- **Page-object editing.**
- **UniFFI, PyO3.** PLAN.md §M22.5, when someone asks.
