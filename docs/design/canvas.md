# The canvas: drawing on an existing page

`Canvas` is the entry point M23 adds: a caller puts a watermark, a header
rule or a line of branded text onto a page they did not author, without
writing a single content-stream operator. It lives in the facade
(`crates/pdfrum/src/canvas.rs`) and is reached through
`DocEdit::draw_page` and `DocEdit::draw_pages`.

This document states the two things the milestone's exit requires in
writing: the coordinate space, and the resource-merging rule. Everything
else is on the rustdoc.

## Why the facade and not `pdfrum-edit`

The machinery — the object overlay, `ContentsShape`, the number spellings,
font embedding, image embedding — is all in `pdfrum-edit`, and that is where
a reader would first look for the canvas. It is not there, for one reason
that decides it: **the contract fixes the vocabulary as `kurbo` for geometry
and `peniko` for colour, and forbids a new dependency.** `pdfrum-edit`
depends on `kurbo` (through `pdfrum-common`) but *not* on `peniko`. The
facade already depends on both, and already re-exports `Color`, `Affine`,
`Rect` and `Point` as the types a caller of this library holds.

So the canvas is a facade type built out of `pdfrum-edit`'s public surface —
`write_float`, `write_matrix`, `write_point`, `write_rect`, `ContentsShape`,
`EmbeddedFont`, `EmbeddedImage`, `EditDoc` — and adds no PDF knowledge that
`pdfrum-edit` does not already have, which is the rule M23 states. The one
thing that did move *down* into `pdfrum-edit` is
`EmbeddedFont::encode_checked`, because only the encoder knows whether a
character has a glyph; see "Missing glyphs" below.

## The coordinate space

Canvas coordinates are **the page as displayed**, in PDF points:

- the **origin** is the lower-left corner of the page a reader sees — the
  crop box's lower-left corner after `/Rotate` has been applied;
- **x runs right and y runs up**, as PDF page space does and as a raster
  does not;
- the **extent** is `Canvas::size`, which is the crop box's width and
  height with the two **swapped** on a quarter or three-quarter turn.

The composition is one line:

```rust,ignore
(rotate.display_matrix(crop).inverse(), size)
```

`Rotation::display_matrix` is the matrix `pdfrum-render` itself uses to map
a page onto its device box, and its output is y-**up** — the renderer
applies its own y flip afterwards, in `page_matrix`. Its inverse is
therefore exactly the map from displayed space back to page space, with the
crop box's offset and the quarter turn falling out together. There is no
second derivation to keep in step with the renderer's, which is the point:
what a caller places and what a viewer shows cannot drift apart, because
both go through the same matrix.

The composed transform is a **proper rotation**, determinant +1. Nothing is
mirrored, so text drawn along +x reads upright on a rotated page rather than
backwards. `tests/canvas.rs` pins this two ways: the canvas rectangle maps
onto the crop box, *and* composing the canvas transform with the display
matrix gives the identity. The second assertion is the load-bearing one —
a canvas turned a quarter turn the wrong way still fills the same bounding
box, and that was a real bug during this milestone's development, found by
rendering the four rotations rather than by the bounding-box test.

The rotation and crop box are read from the **session's** page dictionary,
through the overlay, not from the base document. A `set_rotation` or
`set_page_box` earlier in the same session is therefore what the canvas is
built on. Reading the base instead was the other half of that same bug.

### The frame

The emitted stream is:

```text
q
<canvas-to-page matrix> cm
… the drawing …
Q
```

The outer `q`/`Q` is what keeps the page's graphics state out of the
drawing and the drawing's out of the page. Inside it, every shape and every
text run is wrapped in its own `q`/`Q` as well, so each states its colour and
width from scratch and none of them can leak into the next.

`Canvas::saved(|c| …)` is the **only** spelling of `q`/`Q` a caller has.
There is no bare `save` to leave unmatched and no `restore` to over-pop:
nesting is closure nesting, so an unbalanced stream is not expressible.
That was a deliberate choice over a matched pair — the pair is one line
shorter to write and one whole class of malformed output to permit, and a
canvas is exactly the place a caller writes an early `return` in the middle
of a drawing.

## The resource-merging rule

The canvas emits **one** content stream and appends it to the page's
`/Contents`; it never rewrites what the page already draws. That is why
drawing on a page costs none of the regeneration losses `pdfrum-edit`'s
crate documentation lists — the page's own streams are not touched, and a
page drawn on with the canvas keeps its CMYK fills, its shadings, its
patterns and its character spacing.

The `/Contents` reshaping follows `apply_rewrite`'s own rule: the array
object the page points at is reused when the page owns it outright, and a
fresh array is added when the page shares it or wrote it inline.

Resources merge under these rules:

- **Three categories only**: `/Font`, `/XObject` and `/ExtGState`. Those
  are the only ones the canvas emits an operator for. Every other key of
  `/Resources` — colour spaces, patterns, shadings, `/ProcSet` — is carried
  through untouched.
- **Fresh names cannot collide.** A merged entry is named `PdfrumC<n>`,
  counting from one, and `n` is chosen against the set of names the page's
  `/Resources` already holds in that category *plus* every name this
  drawing has already allocated. A page that already has a `PdfrumC1` gets
  `PdfrumC2`; the existing entry is kept and still names what it named.
  The prefix is this crate's own and is minted nowhere else in the
  workspace — `pdfrum-edit`'s regeneration uses `FX*` — but the collision
  is *checked* rather than assumed away, because a producer may spell a
  name any way it likes.
- **An equal resource is named once.** Drawing the same font or the same
  image twice in one drawing allocates one name.
- **A shared `/Resources` is copied before it is written to.** When the
  page reaches its resources through an object it does not share, the merge
  is written through that object and the page's own key is left alone. When
  the dictionary is shared with another page, written inline, or absent,
  the page gets its own copy — so drawing on one page can never change
  another. A sub-dictionary reached indirectly is inlined into the merged
  copy for the same reason.

## Missing glyphs are an error

`EmbeddedFont::encode` writes `.notdef` — code 0 — for a character the font
cannot draw, which is right for a bulk conversion and wrong for placing a
watermark: a degree sign that silently became a blank is worse than one that
refused to be written, because nothing downstream can tell the two apart.

`EmbeddedFont::encode_checked` is the same encoding with that fallback
replaced by a `MissingGlyph` error naming the character and its byte offset,
and `Canvas::text` encodes through it. The refusal is recorded on the canvas
and returned from `draw_page`; **nothing of the drawing is written** when it
fails, so a refused string leaves no half-drawn page behind.

The check is per-character rather than per-string, and a composite font's
GID 0 counts as missing exactly as a simple font's code 0 does.

## What is not here

**No layout.** No line breaking, no wrapping, no paragraph model. The only
measurement is `Canvas::text_width`, a single string's advance, which is
what centring one string needs. A caller who needs layout is holding a
typesetting problem and brings their own layout to `text`. This is the same
line SPEC.md draws against authoring, and it is what keeps this from
becoming a document compiler.

## Stamp, and whether it sits on top of this

It does not, today, and this milestone did not move it.

`DocEdit::stamp_text` and `stamp_image` reach the page a different way: they
build a `PageObject`, push it onto a `PageEdit`, and let `regenerate` +
`apply_rewrite` turn the whole graph back into operators. That path appends
a stream too, but it gets there by **regenerating** — which is why the stamp
tests have to assert that the page's original objects survived the trip, and
why the stamp inherits the regeneration losses on any page it touches.

Expressing stamp on the canvas would be a real simplification:
`StampPosition` and `Placement::of` compute a point on the page as
displayed and then map it back into page space by hand, four rotations
spelled out — which is precisely the inverse display matrix the canvas
already has, derived a second time. `Placement::rotation` would become
`Canvas::transform`, `with_opacity` would become `Canvas::opacity`, and the
`SessionPage`/`text_extent` machinery would become `Canvas::text_width`.

It did not fall out for free, so per the milestone's own instruction it was
left alone. Two things stand in the way, both small and neither free:

1. `stamp_image` sets `fill_alpha` on the image object directly; the canvas
   spells that as `Canvas::opacity`, an `/ExtGState` before the `Do`. The
   two produce the same rendering but not the same bytes, and the stamp
   tests compare bytes in places.
2. The stamp's text is positioned from an ascent/descent box, and the
   canvas's `text` takes a baseline. The conversion is three lines but it
   changes where a corner-placed stamp lands by a fraction of a point,
   which the stamp tests assert to within two points.

Both are a follow-up worth doing — the duplicated four-rotation mapping is
the kind of second derivation this document argues against everywhere else
— but doing it inside M23 would have meant changing stamp's observable
output, which the milestone explicitly does not ask for.
