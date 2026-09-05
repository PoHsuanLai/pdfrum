# pdfrum

A pure-Rust PDF engine for the web. Open a PDF, render a page to a canvas,
extract text, fill a form.

```js
import init, { Document } from "pdfrum";

await init();

const doc = Document.open(new Uint8Array(await file.arrayBuffer()));
const page = doc.page(0);
const image = page.render(2); // 2x, so 144 dpi

const canvas = document.querySelector("canvas");
canvas.width = image.width;
canvas.height = image.height;
canvas
  .getContext("2d")
  .putImageData(new ImageData(image.data, image.width, image.height), 0, 0);

console.log(page.text());

page.free();
doc.free();
```

**Free your handles.** `Document`, `Page`, `Form` and `Cancel` hold memory
inside the WebAssembly module that the JavaScript garbage collector cannot see.
Call `free()` when you are done with one. A page keeps its document alive, so
the order does not matter.

**Errors are thrown.** A failed call throws a real `Error` with a `.code`: 2 not
a PDF, 3 wrong password, 5 render failed, 9 a limit or a cancellation, 100 a bad
argument.

TypeScript types are generated from the Rust source and ship in `pdfrum.d.ts`.
