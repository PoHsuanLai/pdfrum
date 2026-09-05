# Rust Style Constitution — pdfrum

Binding for every agent and every crate. `PLAN.md` says *what* and *when*; this
file says *how code must look*. SPEC.md holds the concrete type contracts.
Violations are review-blockers even when tests pass.

## 1. Data + functions, not objects

- Model the domain as **plain data types** (`struct`s, `enum`s) transformed by
  **free functions and inherent methods that read like functions** (take input,
  return output). No "manager", "handler", "controller", "context-object with
  15 responsibilities" designs. The C++'s `CFX_GEModule`, `CPDF_ModuleMgr`
  singletons and observer webs are *anti-patterns to erase*, not port.
- **Small structs; data and logic separated.** A struct is a record of facts —
  a handful of fields with an obvious invariant, close to POD. Behavior lives
  in functions (free or thin inherent methods) that *operate on* the data;
  a struct that accumulates fields and methods until it is "the thing that
  does X" must be split into the record and the operations. The C++'s
  1000-line god classes (`CPDF_Parser`, `CPDF_RenderStatus`, `CFX_FontMapper`)
  each decompose into several small records plus function modules — never
  arrive as one struct. Rule of thumb: if you can't state a struct's invariant
  in one sentence, or a method doesn't need most of its fields, split it.
- **Enums over class hierarchies.** Every closed set is an enum with exhaustive
  `match`: PDF objects, filters, colorspaces, functions, shadings, page
  objects, content operators, blend modes, security handler revisions,
  annotation types. The C++ simulates this with virtual dispatch + `AsDict()`
  down-casts; in Rust the compiler enforces the case analysis. Avoid `_ =>`
  arms on our own enums — when a variant is added, every match site must fail
  to compile.
- **Traits only at genuine seams**, and few: `RenderDevice` (swappable raster
  backends), `Resolve` (indirect-object lookup), and little else. A trait with
  one implementation is a design smell. No trait-object soups; prefer
  `impl Trait` / generics at boundaries, `&mut dyn` only where object safety
  is the point (RenderDevice).
- **No global state. None.** No `static mut`, no `lazy_static` registries, no
  process-wide caches. Everything a function needs arrives via parameters.
  Caches live inside the owning value (`Document`, `GlyphCache`) and are
  passed down.

## 2. Functional discipline

- Prefer **pure transformations**: content bytes → `Vec<Op>` → page-object
  graph → device calls. Each stage is a function testable in isolation on
  values.
- Iterators and combinators over index loops where they read better; but
  clarity beats cleverness — a plain `for` beats a five-combinator chain
  nobody can debug.
- Immutable by default. Mutation is local and obvious: builders during
  construction, `&mut` sinks for output accumulation. Interior mutability
  (`OnceLock`, `RwLock`) only for lazy caches, each one documented with *why*.
- **No `Rc<RefCell<…>>` object graphs.** Cross-references between PDF objects
  stay as *ids* (`ObjRef`), resolved through the store — the graph is data,
  not pointers. Shared immutable payloads use `Arc`.
- State machines (lexer modes, progressive render, xref recovery) are enums
  driven by `match`, not boolean-flag clusters.
- **New code is type-driven** (user rule, 2026-09-05): an impossible state
  does not compile rather than being caught at runtime. Newtypes for ids,
  indices and units (`ObjRef`, `Gid`, `CharCode`, `PageIndex`, `Dpi`);
  enums over booleans and stringly options; a distinct type for a value
  that only exists after a step (a decoded font, a converted row, an owned
  page) instead of an `Option` field checked at every use; `#[repr(C)]`
  structs and enums as a foreign contract, never magic integers. Existing
  code is not rewritten for this alone, and typestate or trait-level
  metaprogramming is still not used for its own sake — the test is whether
  the type removes a check or a comment.

## 2b. Traits, generics, macros — the complete map

**Traits.** Two kinds exist in this codebase, and the seam list is closed:
- *Seams (polymorphism on purpose):* `RenderDevice`/`RasterBackend`
  (swappable rasterizers), `Resolve` (indirect-object lookup; `&impl Resolve`
  bounds, real second impls: test resolvers, the editor's flattened view), and
  — added by `[spec]` 2026-09-01 for M14 — `Cascade` in `pdfrum-form` (the
  form-commit script hooks; second impl is M15's `boa` engine, the first is
  the no-script default whose method defaults *are* the V8-off behaviour).
  `dyn` exists in exactly two *kinds* of place: `RenderDevice`, and `&mut dyn
  Cascade` on `pdfrum-form`'s commit path. *Corrected 2026-09-02 (M15 step
  1):* that second one used to be described as "one `&mut dyn Cascade` in
  `pdfrum-form::commit`", which was true and was the problem — the seam was
  at one call site because nothing called it. It is now a parameter on
  `apply`, `choose` and `kill_focus` and is threaded to `commit::run` and the
  typing path (SPEC §15.1). One seam, more call sites; **the count that
  matters is the seams, and it is still three.** Adding a fourth is a
  `[spec]` change.

  *When a fourth is proposed, the test is what the library needs from the
  other side.* Invert — take a trait — only when the library must **ask a
  question it cannot answer** and cannot proceed until it hears back:
  `RenderDevice` (rasterize this), `Cascade` (run this script, give me the
  result), `Resolve` (fetch this object). Do **not** invert to let a host draw
  something the library merely **knows about**: expose the state and let the
  host pull it on its own schedule. Ruled 2026-09-01 on M14's viewer chrome —
  PDFium's `fpdfsdk/pwl` layer is a *closed* list of five pieces (caret,
  selection band, focus rectangle, scroll bar, combo dropdown), of which only
  the last two are host UI, so `PopupView`/`ScrollView` value getters plus
  intent methods (`choose`, `close_popup`) say everything a `FormChrome` trait
  would, without a trait object threaded through the session, a synchronous
  mid-dispatch callback contract, or a lifetime on a public type. The same
  rule explains why the chrome the oracle bakes *inside* a widget's `/Rect`
  (caret, selection band, the 12-unit scroll-bar reservation) belongs in the
  appearance stream, while chrome *outside* it is the host's to draw.
  *Clarified 2026-09-02 for M15:* `ScriptCascade` (the `boa`-backed
  implementor) is `Cascade`'s **second implementation**, which is what the
  seam was admitted for — not a fourth seam. The alert transcript a script
  produces comes back as a value the host reads, not as a `ScriptHost` trait
  the host implements; the list stays closed at three.
- *Vocabulary impls (Rust idiom, not OOP):* implement std/ecosystem traits
  liberally — `Iterator` (lexer, `Font::decode`, pages, outlines), `Deref`
  (`ByteSpan`, `Resolved`), `TryFrom`, `Default`, `Debug`/`Display`,
  thiserror's `Error`.
- (Withdrawn 2026-08-29 during pdfrum-object implementation: the once-sanctioned
  internal `FromObj` generic accessor is deliberately NOT part of the design —
  the accessor resolution matrix's rows differ in resolution, type filtering,
  and fallback shape, which a generic parameter would erase. Explicit accessors
  are the design; do not reintroduce it.)

**Generics.** Plumbing, never architecture: `&impl Resolve` threading,
`io::Write` in the serializer, `impl Iterator` return types. Lifetimes stay in
the zero-copy layer (`Lexer<'a>`, `Token<'a>`, `Resolved<'a>`) and never creep
into `Page`/`Document`-level types. No type-level programming, no generics
over pixel formats, no const-generic cleverness. The facade uses concrete
types only.

**Macros.** Exactly two of our own, both `macro_rules!`, both table-shaped
(one source of truth for lists that would otherwise desync):
- `ops!` — the content-operator table: generates the `Op` enum, operand
  arity/type checking, parse dispatch, and debug names from one declaration.
- `names!` — the PDF name constants.

Banned: our own proc-macros (derives only from deps: thiserror, insta); any
`#[derive(FromDict)]`-style dict-to-struct DSL — PDF dicts have inherited
attributes, resolver-dependent refs, and damage tolerance, so extraction is
*explicit accessor code by design*, not boilerplate to macro away;
macro-generated `#[test]`s per corpus file (the conformance harness is a
data-driven binary; unit tests are hand-written); macros that hide control
flow.

## 3. Errors

- Every library crate defines one `Error` enum with `thiserror`, variants named
  by *what went wrong in the domain* (`XrefBroken`, `CipherKeyLength`,
  `JbxSegmentTruncated`…), carrying the data a caller needs. `anyhow` is
  allowed **only** in `pdfrum-tool` and the conformance harness.
- **No panics in library code.** Workspace lints:
  `clippy::unwrap_used = deny`, `clippy::expect_used = deny`,
  `clippy::panic = deny`, `clippy::indexing_slicing = warn` in parser-facing
  crates. All slicing of untrusted input via `get()`. Arithmetic on untrusted
  sizes via `checked_*`/`saturating_*` — fuzzers enforce this.
- **Damage tolerance is a first-class channel, not an error.** PDFium's value
  is opening broken files. Functions that can proceed past damage take a
  `&mut Diagnostics` sink (plain struct wrapping `Vec<Diagnostic>` + limits)
  and return the best-effort value; `Err` is reserved for "cannot continue".
  Never silently swallow a recovery — record it.
- Fallible conversions are `TryFrom`; `From` never lies.

## 4. API surface

- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/):
  no `get_` prefixes, `iter()`/`into_iter()` conventions, `#[must_use]` on
  pure functions, `Debug` on everything public, `Clone` where cheap or
  obviously wanted.
- Options via plain config structs with `Default` + struct-update syntax, not
  builder ladders, unless construction is genuinely staged.
- Public API of every crate fits in one `lib.rs` re-export block a reviewer
  can read in one screen. Internal modules named by domain (`xref`, `lexer`,
  `shading`), never `util`, `helpers`, `common`, `misc`.
- The committed `docs/status/api-baseline/` snapshots **are** the public API.
  A drift against them is the review: `./scripts/api-snapshot.nu check` is
  the gate, and an intended change is `./scripts/api-snapshot.nu update`
  run deliberately — a change to the baseline is a change to what
  `cargo add pdfrum` sees.
- Unexported types in public signatures are a build failure. rustdoc
  `broken-intra-doc-links` is denied with the rest of `-D warnings`, and
  `crates/pdfrum/tests/reexports.rs` plus the enum-constructibility test
  require every named type — payload types included — to be reachable from
  `pdfrum::*`.
- A new public item in a member crate is a review question: facade-facing,
  sibling-crate plumbing, or should it have been private?
- All public types are `Send + Sync` unless a documented reason exists.
  Rendering multiple pages in parallel with `rayon` must Just Work.
- **Library code carries no `#[allow(dead_code)]`.** `#[expect(dead_code)]` is
  the same deferral spelled differently and is not an escape either. When the
  lint finds an item nothing calls, there are three honest answers and the
  attribute is none of them:
  - **Only tests call it** — put it under `#[cfg(test)]`, in the module's own
    `mod tests` where only that module reads it, or on the item where a
    sibling test module needs it. That says to the compiler what the `allow`
    was asking it to overlook.
  - **Nothing calls it** — delete it. If it recorded something worth keeping,
    the record is one sentence in the module doc citing the oracle line, not a
    function nobody runs. A `len`/`is_empty` pair with no production reader is
    dead however complete it makes the type look; let the test count another
    way.
  - **It ports oracle behaviour we reach no other way** — then it is a *missed
    wire*, not dead code. File it in `docs/status/unwired-oracle-ports.md`
    with both citations and name that file in the attribute's `reason`. This
    is the only shape of suppression the tree keeps; an attribute whose
    reason does not cite the registry is a decision not yet taken.

- **An optimization or refactoring pass ends with a dead-code sweep** (user
  rule, 2026-09-05). A rewritten stage leaves its old sampler behind, a new
  type leaves the old flag, a keyed cache leaves the lookup it replaced;
  the lint does not see `pub` items, so read the callers. The sweep covers
  what the pass touched — helpers, fields, variants, constants, feature
  gates and the tests that only they had — and lands in the same series,
  as its own commit that lists what went.

  Deciding which of the three applies means reading the oracle, not the port's
  own doc comment — a comment claiming a caller is not evidence one exists.
  Test harnesses under `examples/` and `benches/`, where each binary uses a
  different subset of a shared file, are outside the rule and outside the
  check.

## 5. Dependencies

- The dependency set is **closed** — exactly the table in PLAN.md §3. Adding a
  crate is a spec change (see SPEC.md §0), not a convenience. When tempted to
  pull a helper crate for 30 lines of code, write the 30 lines.
- `unsafe_code = "forbid"` in every crate. If SIMD ever justifies an
  exception, it gets its own tiny audited crate; that decision is the user's.

## 6. Tests & docs

- `cargo nextest run` is the runner; doctests additionally via
  `cargo test --doc` (nextest silently skips them).
- Unit tests port *assertions* from the C++ unittests, restated over our
  types. Snapshot tests (`insta`) for dump-shaped outputs. Fuzz targets for
  every byte-consuming entry point.
- Every public item has a doc comment saying what it does in PDF terms
  (spec section references — ISO 32000 §x.y — welcome). Module-level docs
  explain the design, especially where we deliberately diverge from the C++
  structure. Comments never narrate C++ provenance ("this ports foo.cpp") —
  that mapping lives in the design brief.
- Public rustdoc is for a caller of the crate, not for the next agent. First
  sentence, then the invariant they can get wrong, then `# Errors`, then at
  most one example per type. Design history, C++ paths and class names,
  internal milestone and work-package numbers, internal document names, and
  rejected alternatives belong in `docs/design/` and in `//` on the
  implementation — a reader on docs.rs has none of those and can follow none
  of them. Caps and the sibling rule: `docs/design/rustdoc.md`.

## 7. The transliteration test

Before finishing any file, ask: *would this code look the same if the author
had never seen the C++?* If a function is a line-by-line shadow of a C++
method — same locals, same control flow, out-params turned into `&mut` —
rewrite it. What must survive from the C++ is **behavior** (including recovery
quirks and limits), captured in the design brief and pinned by tests, never
its shape.
