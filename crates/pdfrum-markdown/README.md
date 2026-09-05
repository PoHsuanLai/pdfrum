# pdfrum-markdown

Markdown, and layout-preserving text, from a PDF page on the `pdfrum`
engine — no models, no weights.

Two tiers, one output. A tagged PDF says what its content is, and the
structure tree is read as is: headings, paragraphs, lists, tables and
figures with their alternative text. An untagged PDF is read by its
typography: the body size is the most common font size, a line at 1.6× is
a `#` heading and at 1.3× a `##`, a short bold line is `###`, monospaced
lines are a code fence, bullets and numbers make lists, wrapped lines join
into paragraphs with hyphenation undone — a line that stops short of its
column ends one, and a bold or dash-closed lead-in begins one, written
`**Lead-in** - rest` — ligatures become letters, a dot
leader becomes ` ... `, and the top and bottom 10% of the page lose their
running headers, page numbers and URLs. In either tier a line's text is
the text page's text for that line — its spacing is the contract — and a
run the producer drew twice, for a faux bold or a shadow, is read once.
A figure is the image drawn under its marked-content id, and an untagged
page's pictures — anything drawn 4 pt or more on each side — take their
place among the lines; each image block carries its index into the page's
images, and `render_with_images` links it to a file the caller wrote.

Read as a document rather than a page at a time (`document_blocks`), a
line repeated in the top or bottom 12% of a majority of the pages, three
at least, is a running header or footer — `Page 3 of 11` and `Page 4 of
11` are one line — and goes from every page.

Those rules come from the non-learned half of
[MinerU](https://github.com/opendatalab/MinerU)'s text pipeline; they are
implemented here from their description.

```rust,ignore
let doc = pdfrum::Document::open("paper.pdf")?;
let page = doc.page(0)?;
println!("{}", page.markdown());      // with the facade's `markdown` feature
println!("{}", page.layout_text());   // columns kept as columns
println!("{}", doc.markdown());       // every page, running headers dropped
```

From the command line: `pdfrum extract markdown paper.pdf`, with `-o DIR`
to write and link the images, and `pdfrum extract text --layout paper.pdf`.
