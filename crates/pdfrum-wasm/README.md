# pdfrum (wasm)

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

`Document`, `Page`, `Form`, `Cancel` hold linear memory the JS GC cannot
see — call `free()`. A page keeps its document alive; order does not matter.

Failed calls throw `Error` with `.code`: 2 not a PDF, 3 wrong password, 5
render, 9 limit/cancel, 100 bad argument.

`Cancel` is the only stop: `Instant::now` panics on wasm32, so there is no
wall-clock limit. Compiled with `vello-cpu`, `codecs-all`, `forms`, `edit`,
`markdown`. No `system-fonts`, no `javascript`.

Types: `pdfrum.d.ts`.
