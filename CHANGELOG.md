# Changelog

[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Semver from the
first crates.io release.

## [Unreleased]

## [0.3.0] - 2026-09-16

### Breaking

This release breaks source compatibility. The crate is young enough that the
churn is worth more than the pin, but a `cargo update` from 0.2 will not
compile untouched.

- `pdfrum_doc::form::apply` takes a `&Dict` catalog and an
  `Option<&ap::FormFonts>` before its resolver. Callers that have neither can
  pass `&Dict::new()` and `None` to keep the old chrome-only behaviour; a
  caller that wants filled fields to be *visible* should pass the real catalog
  and `FormFonts::load`'s answer, which is what `Document::save_form` now does.
- `AnnotMeta` no longer derives `Eq`. It carries an `f32` opacity now, which
  has no total equality; `PartialEq` is unchanged. The API baseline does not
  track trait impls, so this is called out here rather than showing in that
  diff.
- `AnnotBorder` is `#[non_exhaustive]`, so its new `dash` field — and any
  later one — is additive rather than breaking. Build one with
  `AnnotBorder::solid(..).with_style(..)` instead of a struct literal.
- The `AnnotSpec::*` constructors and the per-variant `with_*` setters are
  gone. The typed builders replace them, and now carry everything the enum
  could say: `spec_builder!` emits `author`, `name`, `modified`, `flags` and
  `meta` alongside `contents`, so a builder no longer has to convert to
  `AnnotSpec` to name an author. `FreeTextSpec` fills the one subtype that had
  no builder.

  Eight of the removed setters — `with_interior`, `with_quads`, `with_border`,
  `with_icon`, `with_open`, `with_color`, `with_highlight`,
  `with_line_endings` — applied to some variants and not others, and on the
  wrong one they tripped a debug assertion and then **silently did nothing in
  release**. That is the failure `annot_spec.rs` was written to prevent, and
  the reason the old path could not simply stay.

  Migration is mechanical: `AnnotSpec::square(r, c).with_border(b)` becomes
  `SquareSpec::new(r, c).border(b)`, and the result converts with `.into()`
  wherever a spec is taken. `AnnotWrite` gained bare-verb spellings of its own
  metadata setters so a chain reads the same after the first one.
- Four options types gained `#[non_exhaustive]`: `pdfrum_edit::SaveOptions`,
  `Encryption`, `ImportOptions` and `NUpOptions`. Each has a `builder()` now,
  so the migration is mechanical — `SaveOptions { mode, ..Default::default() }`
  becomes `SaveOptions::builder().mode(mode).build()`, and a caller who only
  assigns fields can keep doing that from `default()`. The attribute is the
  point: without it, every option added later is a major break. The facade's
  own `pdfrum::SaveOptions` already carried it and is unchanged.
- `pdfrum::Metadata` gained `trapped` and `custom`. It is already
  `#[non_exhaustive]`, so a struct literal outside this crate never compiled;
  what breaks is an exhaustive destructuring, and `Metadata::builder()` is the
  construction path. Reading a `Metadata` is unaffected.

### Fixed

- A filled form renders its values. `save_form` and `write_form_to` laid out a
  widget's chrome and stopped, so a text field stored `/V` and drew an empty
  box in any reader that does not rebuild appearances itself — which is most of
  them, absent `/NeedAppearances`. Both now lay the value out through the same
  `pdfrum_doc::vt` engine the page renderer and `flatten` already used.
- A choice field's `/I` is rewritten to agree with the `/V` a fill writes.
  `/I` indexes `/Opt`, and a reader that finds the two disagreeing discards
  `/I` wholesale and matches the text instead — so a fill that rewrote `/V`
  alone left behind exactly the stale pair the reader defends against. A value
  naming no option writes an empty array rather than guessing an index.
- A field body reads its value from the dictionary it was handed when that
  dictionary carries `/V`, rather than resolving the name through
  `/AcroForm /Fields`. Two `/Fields` entries sharing a `/T` are still one
  field with two controls — that lookup is unchanged for the widgets it was
  written for — but an *edited* dictionary that is not yet in the file now
  wins over the stale one the file still points at.
- Adding a second attachment no longer inlines the first. Every attachment
  write rewrites the whole `/Names /EmbeddedFiles` tree, and it rewrote it from
  the name tree's *resolved* entries — so a file specification written as a
  reference came back as a dictionary and was written out inline, losing its
  object identity and duplicating the dictionary on the next save. The write
  paths now read the tree's raw entries and keep whatever spelling it had.
  `/AF` is what made this visible, since an associated file has to be the same
  object as the attachment rather than a copy of it.


### Added
- `WidgetAppearance` and `set_widget_appearance` write a widget's `/MK`
  appearance characteristics — `/BG` background, `/BC` border colour, `/R`
  rotation, and `/CA` caption. The generator reads `/MK` on every
  regeneration, so a written one changes how the field is drawn with no
  companion redraw; a characteristic the builder leaves unset keeps whatever
  the widget already had, so one key can be edited without flattening the
  rest.
- `flatten_document` flattens every page, which is what a caller who wants no
  annotations left usually means. The per-page `flatten` made that a loop
  that also re-pruned `/AcroForm` once per page. Answers `Done` when any page
  had something to draw.
- `add_form_field` **creates** a form field — a text field, a check box, or a
  radio group — where before the write side could only fill fields a file
  already declared. It writes the widget annotation, the field dictionary, the
  `/AcroForm /Fields` entry, and the appearance stream, so the field is
  addressable by name, listed on its page, and drawn without the reader
  rebuilding it. A field and its widget are one object when the field has a
  single control; a radio group gets a parent field with one widget kid per
  button, and only the chosen button's `/AS` names its export value.
  `FieldSpec` carries the attributes: `/V`, `/DV`, `/TU`, `/DA`, `/MaxLen`,
  and the `/Ff` bits worth naming (`read_only`, `required`, `multiline`,
  `password`, `comb`) plus a `flags` escape hatch for the rest. A name the
  document already uses is refused rather than silently merged into the
  existing field, since the reader treats a shared name as one field with two
  controls.
- `add_form_font` registers a face in `/AcroForm /DR /Font` so a field's `/DA`
  can name it. A created field gives the form a `Helv` if it has none, because
  the default `/DA` names that face and a `/DR` without it lays every value out
  in a substituted one that measures differently.
- `DEFAULT_FIELD_DA`, the appearance string a created field takes by default:
  black `Helv` at the auto-size sentinel, so the generator picks a size that
  fits the widget rather than clipping a fixed one.
- `FieldKind::is_choice`, the `/Opt`-backed kinds — the two that carry `/I`.
- `set_need_appearances` sets or clears `/AcroForm /NeedAppearances`, asking a
  reader to build every field's appearance from its value and `/DA`. Clearing
  removes the key rather than writing `false`, which is the same thing to a
  reader. A document with no `/AcroForm` gains one, since a flag with no form
  to hang on would be dropped by the next reader that rewrites the catalog.
- `/CA` constant opacity on every annotation subtype, through
  `AnnotSpec::with_opacity` / `AnnotMeta::with_opacity`. The generator already
  folded `/CA` into each appearance's `/ExtGState`; the writer never emitted
  it, so every annotation pdfrum wrote was forced opaque — including
  highlights, where translucency is the norm. Out-of-range values are clamped
  rather than refused.
- `/IC` interior fill on Square and Circle, through `SquareSpec::interior` /
  `CircleSpec::interior`. The appearance generator has always filled these
  shapes from `/IC`; the old `AnnotSpec` setter was restricted to Line, so a
  filled callout box could not be written.
- `/BS /D` dash patterns, through `AnnotBorder::with_dash`. A dashed border
  previously wrote `/S /D` with no pattern, leaving a reader to fall back to
  its own `[3 0 0]`. Written only for a dashed style, since it means nothing
  to the others.
- `/Q` text alignment on `FreeText`, through `FreeTextSpec::align`, reusing
  the read side's `pdfrum_doc::vt::Alignment` rather than a second enum over
  the same three values. `Alignment::to_quadding` is its inverse. Every
  free-text annotation pdfrum wrote was flush left.
- `set_outline` writes the document outline — its bookmarks. The reader hands
  back a flat list carrying a depth, and writing takes the same shape: a
  caller describes the tree as depth-tagged items and this builds the
  doubly-linked `/First` `/Last` `/Next` `/Prev` `/Parent` structure a PDF
  actually holds. `/Count` follows the convention a reader expects — positive
  on an open item, negated on a closed one, absent on a leaf. A depth that
  jumps by more than one is clamped to one deeper, since no tree has that
  shape. Nothing in the writer touched `/Outlines` before this, so a merge or
  a split dropped every bookmark in the document silently.
- `set_xmp_metadata` writes the `/Metadata` XMP packet. The generator existed
  but was reachable only through PDF/A conversion; the packet is now written
  verbatim into the `/Type /Metadata /Subtype /XML` stream the writer already
  knew never to compress, so a reader scanning the raw bytes can still find
  it. `None` removes the stream.
- `associate_file_with_document` and `associate_file_with_page` write `/AF`,
  the array that says an embedded file *belongs to* the document or to one
  page rather than merely riding along in it, with `Relationship`
  (`/AFRelationship`) saying how. The distinction is what PDF/A-3 and the
  electronic-invoice profiles built on it are about: the invoice XML has to be
  declared the alternative representation of the rendered page, not an
  attachment that happens to sit beside it. `document_associated_files` and
  `page_associated_files` read the associations back as attachment indices.
  Neither call embeds anything — the file is one the document already carries,
  which is why every entry names an attachment by index.
- `set_page_labels` writes `/PageLabels`, the number tree that decides the
  page numbers a reader shows. `PageLabelRange` names a starting page, a
  `PageLabelStyle`, an optional `/P` prefix and an optional `/St` first
  number; ranges may be given in any order and are sorted, since a number tree
  whose keys do not ascend gives the reader's lower-bound scan the wrong rule.
  An empty slice removes the tree. The reader (`pdfrum_doc::page_label`) has
  been complete since 0.2 with no writer at all, and this pairs with
  `delete_pages` / `import_pages`, which invalidate labels.
- `set_viewer_preferences` writes `/ViewerPreferences` from a
  `ViewerPreferences` builder — the six booleans, `/Direction`,
  `/PrintScaling`, `/NumCopies` and `/Duplex`. A preference the builder leaves
  unset keeps whatever the document had, so one can be changed without reading
  the rest. `set_open_action` and `clear_open_action` write and remove the
  catalog's `/OpenAction` as a destination on a page, which is what "open at
  this page" means; the action-dictionary form is not offered.
- `set_info_name` sets an `/Info` entry whose value is a **name**. `/Trapped`
  is the one such key the specification defines, and writing `Unknown` as a
  *string* is a different object than the name a PDF/X validator reads.
- `pdfrum::Metadata` carries `/Trapped` (as a three-state `Trapped`, because
  `Unknown` is a real answer a prepress workflow distinguishes from an absent
  key) and every other `/Info` key the document holds, in `custom` — so
  reading a `Metadata`, changing one field and writing it back no longer drops
  the producer's own keys. `Metadata::builder()` constructs one from outside
  the crate.
- `set_attachment_file_with` replaces an attachment's bytes **keeping** the
  MIME type and date the options name. `set_attachment_file` drops both, which
  is documented but rarely meant: the `/Subtype` a viewer picks an application
  by and the file's own `/Params /ModDate` both vanish on a plain replace.
- `set_attachment_name` renames an attachment. The name is the tree key and is
  written to the specification's `/F` and `/UF` too, so the name a viewer
  shows and the name it looks the attachment up by stay the same string; a
  name another attachment already has is refused, since the tree is keyed by
  name.
- `builder()` on `SaveOptions`, `Encryption`, `ImportOptions` and
  `NUpOptions`. `NUpOptionsBuilder` takes a `kurbo::Size` for the sheet and
  `.grid(columns, rows)` for the grid, where the bare `(f32, f32)` and
  `(u32, u32)` tuples say nothing about which number is which.
- `reorder_pages` arranges the pages into a new order given as old indices.
  The CLI faked this three times over by building a throwaway document,
  importing into it and deleting the template page. A list that is not a
  permutation of every page is refused rather than guessed at: naming fewer
  pages is a deletion and naming one twice is a duplication, and both have
  their own call. The page tree flattens in the process, with inherited
  `/Resources`, `/MediaBox`, `/CropBox` and `/Rotate` resolved onto each page
  first so nothing is lost.
- `set_page_rotation_to` takes the read side's own `Rotation` where
  `set_page_rotation` takes degrees and rounds. The CLI had been reading a
  `Rotation` and hand-converting it back to `i32` to feed the writer — a round
  trip through a type that can only lose information.
- `delete_named_destination` and `named_destinations` finish the name-tree
  CRUD. Delete answers `Ok(false)` for a name that was not there, matching
  `delete_attachment`; a `/Kids` hierarchy is edited in its leaves rather than
  flattened. A link still naming a removed destination is left alone — this
  answers what the tree holds, not what points at it.
- Per-subtype annotation builders — `MarkupSpec` (with `MarkupKind`),
  `TextSpec`, `SquareSpec`, `CircleSpec`, `InkSpec`, `LineSpec`,
  `LinkSpec`, and `CaretSpec` — each carrying only the options its
  subtype has, and converting with `Into<AnnotSpec>` so they are accepted
  wherever a spec is. `LineSpec::interior` exists, `TextSpec::interior`
  does not compile, where `AnnotSpec::with_interior` on a `Text` was
  accepted and dropped.
- `AnnotSpec::link_action` builds a Link from an `AnnotLinkAction`
  already in hand; the other `link_*` constructors are this one with the
  action spelled out.
- `DocEdit::add_annotation` / `pdfrum_edit::add_annotation` with typed
  `AnnotSpec` variants — enough for Rotero to drop lopdf when writing
  annotations. Text markup (Highlight, Underline, `StrikeOut`, Squiggly)
  takes `/QuadPoints` in tl/tr/bl/br order; Text (note), Square, Circle,
  Line, Ink, `FreeText`, Link, and Caret cover the rest.
- Appearance streams are generated for the shapes that have a geometry to
  draw (Square, Circle, Ink, Line, Link, Caret) through the same
  `pdfrum_doc::ap` pipeline the reader already uses, so a written
  annotation renders without a viewer regenerating `/AP`.
- `AnnotMeta` carries `/T` author, `/NM` name, `/M` date, and `/F` flags;
  flags default to Print.
- Per-subtype appearance controls: `AnnotBorder` / `AnnotBorderStyle`
  (`/BS` width and style) on Square, Circle, Ink, and Link;
  `LineEndingStyle` (`/LE`) plus `/IC` interior fill on Line, with the
  ending sized from the stroke width; `/Name` icon and `/Open` on Text;
  `/C` and `AnnotLinkHighlight` (`/H`) on Link.
- Link actions via `AnnotLinkAction`: `Uri`, `GoTo` (local, with
  `AnnotGoToView` covering Fit, `XYZ`, `FitH`, `FitV`, `FitR`, `FitB`,
  `FitBH`, and `FitBV`), `GoToR` (remote, page or named destination,
  `/F` written as a `/Type /Filespec` with `/F` and `/UF`), `Launch`,
  and `Named` / `NamedExisting`.
- Named destinations: `set_named_destination` and
  `ensure_named_destination` upsert into the catalog's `/Names /Dests`,
  merging into an existing `/Kids` hierarchy in place rather than
  flattening it, so a named `GoTo` resolves after reopen.
- `update_annotation` / `delete_annotation` by `ObjRef`, and
  `update_annotation_at` / `delete_annotation_at` by page and 0-based
  `/Annots` index. Update preserves the object number and regenerates
  `/AP`; an inline `/Annots` dictionary is promoted to an indirect object
  before update and removed from the array on delete.
- `TextPage::rects_loose` unions each character's em-box (advance ×
  ascent/descent) instead of its ink box, so markup `/QuadPoints` sit
  where Acrobat puts them.

### Changed

- `AnnotBorderStyle` is an alias of `pdfrum_doc::ap::BorderStyle` rather
  than a second enum over the same five `/BS /S` values. The read side,
  the form layer, and the annotation writer now name one type. The two
  sides still differ in how they reach a value — reading takes the first
  byte of `/S`, so `/Dotted` reads as `Dash`, while writing goes through
  the new `BorderStyleName::as_bytes` and emits only the five legal
  names. `AnnotBorderStyle::Dashed` is spelled `Dash`, and `as_bytes`
  needs `BorderStyleName` in scope. Both shipped unreleased, so no
  published API changes.
- Every `AnnotSpec` variant is `#[non_exhaustive]`, so a later subtype
  option is additive. Build through the typed `*Spec` builders rather than
  a struct literal.

## [0.2.0] - 2026-09-11

### Added

- GPU viewers that already hold a `wgpu` device can record with
  `render_page_to_device` and present with `VelloBackend::render_to_view`
  / `render_to_texture` — no `map_async`, no host pixmap. Isolated groups
  stay on the parent as native layers; a texture/buffer pool (cap 8)
  reuses GPU resources across pages.

### Changed

- `Page::render` takes the rasterizer by value and uses default options, so
  `page.render(VelloCpuBackend)` names the backend without constructing a
  handle. `render_with` is the explicit-options form; `render_on` still
  threads a session. A backend that holds a device is passed by reference
  (`page.render(&gpu)`) through a blanket `RasterBackend` impl for `&T`.
  The same split applies to `to_svg` / `to_svg_with`.
- `scanline::FillRule` is `pdfrum_render::FillRule`: `Winding` / `EvenOdd`,
  the same type the backend trait uses. AGG no longer maps names.
- `ColorScheme::new` / `ColorScheme::all` and `Argb::new` / `Argb::WHITE`.
- `FormSession::hover_for_page` returns `Option<AnnotId>`.
- `DocEdit::n_up` takes a `Size` for the sheet.
- `TextPage::find(needle)` uses default options; `find_with` takes flags.
- `Annotation::quad_points` yields an iterator.
- `Array::of` / `Dict::from_pairs` take `Into<Object>` / `Into<Name>`.
- `Dict::text` follows `byte_string` and reads through `as_direct()`.
- `PdfString` bytes and syntax are methods; fields are private.
- `EditDoc::page_state` / `apply_page` return `edit::Error`.
- Remaining edit page-index arguments take `impl Into<PageIndex>`.
- `Pixmap::to_straight_bgra` / `to_opaque_bgra` replace the `opaque: bool`.
- `PathBuilder::fill_rule` is `Fill` rather than `even_odd: bool`.
- `ObjectIndex` has `new` / `get` / `From` / `Display`.
- `pdfrum_text::content_words` is the content-stream word split, distinct
  from `TextPage::words`.
- `LoadOptions::with_password` takes the same spellings as the facade.
- `ChoiceState::new` takes any iterator of options.
- Collection APIs that were a `map`/`filter` over data already in memory
  now return an iterator or a slice instead of a `Vec`: `Document::revisions`,
  `Form::fields`, `Page::annotations`, `OwnedPage::annotations`,
  `page_links`, `Face::charmaps`, `GlyphSource::charmaps`, and
  `ContentsShape::elements`. `Array::to_numbers` is removed; callers already
  use `number_at_or_zero` at a known index.
- `Table::row` takes any iterator of string-like cells, so a CLI table can
  be filled from string slices. `Response::one` is the one-update constructor
  every caller was spelling as `with(vec![…])`. `Face::single` takes
  `impl Into<Vec<u8>>` for both the alias and the bytes.
- Lossless wrappers grow the `From`/`AsRef` impls they already were:
  `Name` from owned bytes and strings, `GlyphName` as bytes, `Array` from
  and into `Vec<Object>`, `PdfString` as bytes, `Gid` and `FieldId` from
  their integers. `Document::open_with_password` takes the same password
  spellings as the options builder.

### Fixed

- Hint-reliant CJK faces (DynaLab stroke-assembled, FreeType's "tricky"
  list) run the TrueType interpreter at `Target::Mono`, keyed off the
  face rather than whether one composite carries bytecode.
- Glyph width is capped in LCD subpixels, matching FreeType's tripled
  `FT_PIXEL_MODE_LCD` columns, so a wide glyph the oracle skips is
  skipped here too.
- A substituted face that is not the slant or the weight the document
  asked for is sheared and dilated the way the oracle synthesises italic
  and bold.
- Style tokens an aborted suffix parse already applied (`Bold` in
  `Foo,Bold,Italic`) stay on the substitution instead of being discarded.
- Every CID character box grows its top edge by a sixty-fourth, matching
  `CFX_Face::GetCharBBox`.
- An indirect `/CIDToGIDMap /Identity` is resolved before its type is
  read, and a CID-keyed CFF wrapped as OTTO maps CIDs through the charset
  the way a bare CFF already did.
- Non-embedded CJK fonts pick a face from the platform preference lists
  before the generic scorer, and drop the name filter when none of those
  names is installed.
- Remaining structural misses vs the oracle: Arial fallback when a
  simple or CID font has no glyph, bilinear stretch on a sheared image,
  Darken overprint on a subtractive family, Type 3 uncoloured stroke and
  colour sole-images as luminance masks, `Field.borderStyle` writing the
  widget, and the list-box scrollbar chrome in `pdfrum-tool`.

## [0.1.1] - 2026-09-10

### Added

- `pdfrum-edit` gains `svg-import` and `svg-text` features, an
  `Error::Svg` variant, and the SVG ingestion behind them.
- Facade re-exports for the write-side surface its owning crates now hold:
  `PdfaPolicy`, `flatten`, and the attachment operations `add_attachment`,
  `delete_attachment`, `remove_attachment`, `set_attachment_file`,
  `set_attachment_description` and `set_attachment_param`.
- `tt_composite_instructions.ttf`, a fixture carrying a simple glyph, an
  uninstructed composite and an instructed one.
- CLI README documents terminal support: where `preview` and `view` draw,
  what each `--graphics` mode maps to, the tmux caveat and the pager keys.
- `scripts/record-cli-macos.nu` records the CLI demo natively on macOS by
  capturing a real Kitty window, which the Xvfb recorder cannot do.

### Changed

- Write-side operations move out of the `pdfrum` facade into the crates
  that own them: `PageEdit` and `transform_object` to `pdfrum-page`;
  `content_segments` and `revision_end` to `pdfrum-parser`; `Canvas`,
  `stamp`, `attach`, `flatten`, SVG import, object builders, `build_graph`,
  `apply_page`, `page_state` and PDF/A `convert` to `pdfrum-edit`; PDF/A
  `policy`, `xmp_write` and `signature` to `pdfrum-doc`. Modules take
  explicit `limits` and `diags` parameters rather than a `&Document`
  bundle, and `string_width` becomes a free function over `Resolve +
  Limits`. The facade drops from 16,296 to 8,533 lines. **Every public
  path is unchanged**; the API baseline diff is re-export re-spelling.
- The CLI demo is retimed and re-recorded. Hardcoded `sleep`s of up to
  3.5s stood in for work that had already finished, so paging now reads
  as instant. MP4 1.0 MB -> 512 KB, GIF 2.5 MB -> 907 KB.

### Fixed

- A TrueType composite that carries its own instruction stream is run
  through the interpreter. The component offsets are only half the
  placement; the bytecode moves the components into their final
  positions, and stroke-assembled CJK faces scattered without it. Only
  instructed composites take the grid-fitted outline, and outlines are
  memoized per glyph.
- `stamp` called `note()` before `build_graph`, so graph-building
  diagnostics never reached the session.
- `scripts/publish-order.nu` parsed a line-broken `or` as an external
  command, so the script printed nothing, the release job published zero
  crates, and still exited 0. The predicate stays on one line and an
  empty order now fails.
- Replace `doc_auto_cfg`, removed in 1.92, so docs.rs builds on nightly.
  The 0.1.0 docs build failed with E0557; local `cargo doc` never sets
  `docsrs`, so the gate did not see it.

## [0.1.0] - 2026-09-09

### Added

- Parse, recover, render, extract text, forms, JavaScript, edit and save.
- Four raster backends: `vello_cpu` (default), `tiny-skia`, AGG, GPU `wgpu`
  (never in a default build).
- CLI: [`pdfrum-cli`](crates/pdfrum-cli/README.md).
- C ABI: `libpdfrum` + `pdfrum.h`.
- WebAssembly binding.
- Conformance harness vs PDFium (`conformance/scoreboard.json`).
- Benchmarks vs pdfium-render, mupdf, hayro, pdf-rs, pdf-extract
  (`docs/benchmarks/`).
- Fuzz targets under `fuzz/` (compiled by the gate, not run by it).

### Known limitations

- Warm median render is slower than pdfium-render and mupdf.
- No public-key (`Adobe.PubSec`) encryption.
- Text extraction keeps one space PDFium drops. A page whose only text
  object draws nothing but spaces comes back empty from the oracle, because
  its bounding-box gate discards the object; we keep the space. Written up in
  [`docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md`](docs/upstream/pdfium/text-object-bbox-gate-drops-spaces.md),
  which records it as drafted and not yet filed.
- Signatures are parsed and reported as written. Nothing verifies the
  cryptography, the certificate chain, or the byte ranges they cover.
- OTTO subsetting is skipped: an OpenType/CFF program is embedded whole as
  `/FontFile3` and passed over by the subsetter.
- PDF/A is the two *basic* levels, A-1b and A-2b; the `a` levels need a
  logical structure model this engine does not have. The checker reads the
  object graph and not the content streams, so a colour set by `1 0 0 rg`
  rather than through a named `/ColorSpace`, and a `gs`-less inline
  transparency, are invisible to it. An empty report means the checks it runs
  passed, which is weaker than ISO 19005 conformance.
- `full` is a host feature set: it includes `system-fonts`, which is never
  compiled for `wasm32`.
- JavaScript is boa, not a viewer's engine. Some documents' scripts diverge
  from what Acrobat or PDFium produce.
- An image whose declared `/Width` and `/Height` are both above 65536 is
  built at full size rather than reduced, which for the dimensions
  `/Width` and `/Height` still admit is an allocation no machine will
  satisfy. Bound untrusted input with `Limits` and a `Deadline`; a
  pixel-count limit is not among the knobs yet.
- Weakest rendering: vertical text, uncoloured tiling patterns, and image
  transformers.

[unreleased]: https://github.com/PoHsuanLai/pdfrum/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/PoHsuanLai/pdfrum/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/PoHsuanLai/pdfrum/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/PoHsuanLai/pdfrum/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/PoHsuanLai/pdfrum/releases/tag/v0.1.0
