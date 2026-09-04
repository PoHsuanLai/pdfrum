# pdfrum-markdown

Markdown, and layout-preserving text, from a PDF page on the `pdfrum`
engine — no models, no weights.

Two tiers, one output. A tagged PDF says what its content is, and the
structure tree is read as is: headings, paragraphs, lists, tables and
figures with their alternative text. An untagged PDF is read by its
typography: the body size is the most common font size, a line at 1.6× is
a `#` heading and at 1.3× a `##`, a short bold line is `###`, monospaced
lines are a code fence, bullets and numbers make lists, wrapped lines join
into paragraphs with hyphenation undone, ligatures become letters, and the
top and bottom 8% of the page lose their running headers and page numbers.

Those rules come from the non-learned half of
[MinerU](https://github.com/opendatalab/MinerU)'s text pipeline; they are
implemented here from their description.

```rust,ignore
let doc = pdfrum::Document::open("paper.pdf")?;
let page = doc.page(0)?;
println!("{}", page.markdown());      // with the facade's `markdown` feature
println!("{}", page.layout_text());   // columns kept as columns
```

From the command line: `pdfrum extract markdown paper.pdf` and
`pdfrum extract text --layout paper.pdf`.
