# pdfrum (wasm)

The WebAssembly binding over the [`pdfrum`](https://crates.io/crates/pdfrum)
facade: open, render, extract and fill PDFs in a browser or in Node. The same
shape as the C ABI in JavaScript terms — opaque handles, an explicit
`free()`, a code on every error — and it holds no logic of its own.

```js
import init, { Document } from "pdfrum";
await init();

const doc = Document.open(new Uint8Array(await file.arrayBuffer()));
const page = doc.page(0);
const image = page.render(2);

canvas.width = image.width;
canvas.height = image.height;
canvas.getContext("2d")
  .putImageData(new ImageData(image.data, image.width, image.height), 0, 0);

page.free();
doc.free();
```

**`free()` is not optional.** `Document`, `Page`, `Form` and `Cancel` hold
WebAssembly linear memory the JavaScript GC cannot see, so dropping the last
reference to one leaks the document until the module is torn down. A page
keeps its document alive, so the order you free them in does not matter.

Failed calls throw an `Error` carrying a `.code`: 2 not a PDF, 3 wrong
password, 5 render, 9 limit or cancel, 100 bad argument.

**`Cancel` is the only stop.** `Instant::now` panics on `wasm32`, so there is
no wall-clock limit here and a long render cannot time itself out — hold a
`Cancel` and call it from the host if you need to interrupt one.

## Build

```sh
./scripts/wasm-package.nu     # builds the npm package into pkg/
```

Compiled with `vello-cpu`, `codecs-all`, `forms`, `edit` and `markdown`.
Not `system-fonts` — a browser has no host font directory, and the bundled
base-14 faces carry the text. Not `javascript`.

TypeScript declarations are generated as `pdfrum.d.ts`.

`publish = false` on crates.io — nothing `cargo add`s a cdylib. The published
surface is the npm package.

Part of [pdfrum](https://crates.io/crates/pdfrum).

MIT OR Apache-2.0
