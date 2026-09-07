# Security

pdfrum parses untrusted PDFs. A crash, a panic in library code, an unbounded
allocation, or a hang on a crafted file is a security bug.

**Do not** open a public GitHub issue. Use [GitHub private vulnerability
reporting](https://github.com/PoHsuanLai/pdfrum/security/advisories/new).
Include a minimal PDF, what you saw, and the crate version or commit.

In scope: panics in a published crate; unbounded memory or CPU outside
`Limits`; a document script reaching the network, filesystem, or a process
(even with `javascript` on); C ABI unsoundness.

Not in scope: rendering differently from Adobe or PDFium (that's a
conformance bug). Enabling `javascript` runs the document's own scripts;
that is the feature.
