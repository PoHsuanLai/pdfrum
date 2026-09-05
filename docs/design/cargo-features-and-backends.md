# Cargo Features & Backend Architecture: Explicit Render, Opt-in Backends, and Modular Dependencies

**Status:** Landed 2026-09-04 (`893e153`..), with the deviations noted at the end  
**Date:** 2026-09-04  
**Scope:** `pdfrum`, `pdfrum-form`, `pdfrum-tool`, `pdfrum-render`, `pdfrum-page`, `pdfrum-doc`, `pdfrum-edit`, `pdfrum-font`, `pdfrum-filters`, `benches`, and CI scripts  
**Related Documents:** [STYLE.md](STYLE.md), [DEPS.md](DEPS.md), [PLAN.md](PLAN.md), [docs/design/idiomatic-api.md](docs/design/idiomatic-api.md)

---

## 0. Executive Summary & Verdict

This document specifies a unified architecture for Cargo features, backend selection, and dependency boundaries across `pdfrum`:

1. **Feature Renaming:**
   * `walk-profile` $\rightarrow$ **`profiling`**: Replaces internal jargon ("walk") with the ecosystem-standard naming for diagnostic timers and instrumentation. Rejects the misleading name `bench`, which would corrupt Criterion benchmarks.
   * `script` $\rightarrow$ **`javascript`**: Replaces an ambiguous domain name with the ISO 32000 PDF capability name. Rejects the name `boa`, which leaks an ephemeral third-party implementation detail.

2. **Burn-Style Opt-in Backend Features on Facade:**
   * Move `pdfrum-raster-vello-cpu` from an unconditional dependency to an optional dependency gated under the default feature `vello-cpu`.
   * Add optional features for other rasterizers directly on the facade: `tinyskia`, `agg`, and `vello-gpu` (opt-in only).
   * Enable true **zero-rasterizer headless/parser-only builds** (`default-features = false`), cutting dependency weight for pure text-extraction, form-reading, or editing workflows.

3. **Explicit Rendering API (No Hidden Defaults):**
   * **Eliminate implicit backend instantiation:** Remove `Page::render(&options)` and `PreparedPage::render()` that silently allocated `VelloCpuBackend::new()`.
   * **Explicit backend everywhere:** Every render method explicitly takes `&B where B: RasterBackend`. Callers have full transparency over which raster engine runs and when caches are reused.

4. **Dependency Demotion:**
   * **Demote `rayon` to `[dev-dependencies]`:** Library code in `crates/pdfrum/src/` contains zero Rayon calls and spawns no threads; types are `Send + Sync` via standard `Arc`/`Mutex`. Demoting `rayon` removes thread-pool and crossbeam dependencies from all production consumer trees.

5. **Subsystem & Codec Modularization:**
   * **`edit`** *(default on)*: Gates `pdfrum-edit` and `subsetter` (font subsetting for PDF editing/saving). Read-only consumers compile neither.
   * **`forms`** *(default on)*: Gates `pdfrum-form` widget state machines; `javascript` implies `forms`.
   * **`codecs-all`** *(default on)*: Gates heavy/specialized image decoders: `jpx` (`hayro-jpeg2000`), `jbig2` (`hayro-jbig2`), and `ccitt` (`hayro-ccitt`).
   * **`system-fonts`** *(default on)*: Gates `fontdb`'s host filesystem font scanning and `memmap2`, enabling clean WebAssembly (`wasm32-unknown-unknown`) compilation backed by bundled Foxit base-14 blobs.
   * **`png`** *(optional utility)*: Adds ergonomic `pixmap.save_png(path)` and `pixmap.encode_png()` directly on `Pixmap`.

---

## 1. Feature Renaming

### 1.1 `walk-profile` $\rightarrow$ `profiling`

#### Why not `bench`?
* In `pdfrum`, this feature injects thread-local `Instant::now()` pairs and allocation counters directly into the inner rendering and interpretation loops (`pdfrum-render`, `pdfrum-page`, and `pdfrum-doc`).
* When active, the timers add a **25%–35% measurement overhead**.
* In Criterion benchmarks (`cargo bench`), this feature **must remain OFF** so that committed figures measure the engine rather than the timing instrument. Naming it `bench` would encourage developers to run `cargo bench --features bench`, invalidating benchmarks.
* `bench` also collides with the standard cargo subcommand and bench targets.

#### Why `profiling`?
* Standard Rust ecosystem convention (e.g. `profiling`, `tracing`).
* The instrument measures not just the display list walk (`pdfrum-render`), but the whole page pipeline (content parse, interpretation, raster) and annotation appearance passes. `profiling` accurately captures this entire scope.

### 1.2 `script` $\rightarrow$ `javascript`

#### Why not `boa`?
* Cargo features must describe **capabilities**, not vendor packages.
* In ISO 32000-1 (§12.6.4.16, §14.8.4), PDF defines **JavaScript Actions** (`/Action /S /JavaScript`, `/AA` additional actions for calculate, format, validate, keystroke).
* An embedder cares whether the PDF engine executes JavaScript, not whether the interpreter is Boa, QuickJS, or a WASM sandbox. Naming it `boa` violates abstraction and forces a breaking change if the engine is ever replaced or augmented.

#### Why not `script`?
* "Script" is ambiguous in a publishing/document processing library: it easily conflates with writing scripts (Latin, Arabic, CJK scripts in font handling) or automation shell scripts.
* `javascript` (or `js`) is immediately recognized across the PDF industry.

---

## 2. Dependency Demotion: `rayon` $\rightarrow$ `[dev-dependencies]`

### 2.1 The Audit
A codebase-wide audit reveals:
* In [`crates/pdfrum/src/`](crates/pdfrum/src), **there is not a single call to `rayon`**.
* The library spawns no threads and builds no thread pools.
* [`Document`](crates/pdfrum/src/document.rs#L38) and [`Page`](crates/pdfrum/src/page.rs#L12) are `Send + Sync` because their internal fields use standard library sync primitives (`std::sync::Mutex`, `std::sync::Arc`).
* `rayon` is only used in:
  1. [`crates/pdfrum/tests/facade.rs`](crates/pdfrum/tests/facade.rs#L946) to verify that `pages.par_iter()` renders identical pixels to serial iteration.
  2. [`crates/pdfrum/examples/parallel-render.rs`](crates/pdfrum/examples/parallel-render.rs#L27) to demonstrate multi-threaded batch rendering.

### 2.2 The Action
* Move `rayon.workspace = true` from `[dependencies]` to `[dev-dependencies]` in `crates/pdfrum/Cargo.toml`.
* Embedders in async Tokio runtimes, single-threaded CLI tools, or WASM workers are no longer forced to compile `rayon`, `crossbeam`, and thread-pool machinery.
* The promise in [DEPS.md](DEPS.md#L178) ("data-parallel fits; engine itself stays single-threaded per page") is realized with zero cost to consumers.

---

## 3. Burn-Style Opt-in Backends & Modular Features

### 3.1 Facade Feature Matrix

In [`crates/pdfrum/Cargo.toml`](crates/pdfrum/Cargo.toml):

```toml
[features]
default = [
    "vello-cpu",
    "edit",
    "forms",
    "system-fonts",
    "codecs-all",
]

# Rasterizer backends (opt-in)
vello-cpu = ["dep:pdfrum-raster-vello-cpu"]
tinyskia  = ["dep:pdfrum-raster-tinyskia"]
agg       = ["dep:pdfrum-raster-agg"]
vello-gpu = ["dep:pdfrum-raster-vello"]

# Subsystems
edit         = ["dep:pdfrum-edit"]
forms        = ["dep:pdfrum-form"]
javascript   = ["forms", "pdfrum-form/javascript"]
system-fonts = ["pdfrum-font/system-fonts"]

# Specialized Image Codecs
codecs-all = ["jpx", "jbig2", "ccitt"]
jpx        = ["pdfrum-page/jpx"]
jbig2      = ["pdfrum-page/jbig2"]
ccitt      = ["pdfrum-filters/ccitt"]

# Convenience Utilities
png = ["dep:png"]

# Internal Diagnostics
profiling = ["pdfrum-page/profiling", "pdfrum-doc/profiling"]

[dependencies]
# Internal crates — modularized
pdfrum-common.workspace  = true
pdfrum-object.workspace  = true
pdfrum-parser.workspace  = true
pdfrum-crypt.workspace   = true
pdfrum-font.workspace    = true
pdfrum-page.workspace    = true
pdfrum-filters.workspace = true
pdfrum-render.workspace  = true
pdfrum-text.workspace    = true
pdfrum-doc.workspace     = true

# Optional subsystems
pdfrum-form.workspace = { workspace = true, optional = true }
pdfrum-edit.workspace = { workspace = true, optional = true }

# Optional rasterizers
pdfrum-raster-vello-cpu = { workspace = true, optional = true }
pdfrum-raster-tinyskia  = { workspace = true, optional = true }
pdfrum-raster-agg       = { workspace = true, optional = true }
pdfrum-raster-vello     = { workspace = true, optional = true }

# Optional utilities
png = { workspace = true, optional = true }

# Vocabulary & error handling
kurbo.workspace     = true
peniko.workspace    = true
thiserror.workspace = true

[dev-dependencies]
rayon.workspace                 = true
png.workspace                   = true
pdfrum-raster-tinyskia.workspace = true
pdfrum-raster-agg.workspace     = true
pdfrum-render.workspace         = true
```

### 3.2 Re-exports in `crates/pdfrum/src/lib.rs`

The trait seam is unconditionally available; backend implementations and optional subsystems are re-exported when their features are activated:

```rust
// Unconditional trait seam:
pub use pdfrum_render::{RasterBackend, RenderDevice};

// Opt-in backend structs:
#[cfg(feature = "vello-cpu")]
pub use pdfrum_raster_vello_cpu::VelloCpuBackend;

#[cfg(feature = "tinyskia")]
pub use pdfrum_raster_tinyskia::TinySkiaBackend;

#[cfg(feature = "agg")]
pub use pdfrum_raster_agg::AggBackend;

#[cfg(feature = "vello-gpu")]
pub use pdfrum_raster_vello::VelloBackend as VelloGpuBackend;

// Opt-in subsystems:
#[cfg(feature = "forms")]
pub use form::{Field, FieldFlags, FieldKind, Form, UnknownField};
#[cfg(feature = "forms")]
pub use form_session::{
    AppearanceUpdate, Button, Cascade, FieldRef, FieldWrites, FormSession, Key, Keystroke,
    KeystrokeOutcome, Modifiers, NoScripts, Response, SessionConfig, UpdateKind,
};

#[cfg(feature = "javascript")]
pub use pdfrum_form::script::{
    BuildError as ScriptBuildError, FieldActions, ScriptFailure, ScriptStop,
};
#[cfg(feature = "javascript")]
pub use pdfrum_form::{ScriptCascade, ScriptConfig, TranscriptLine};

#[cfg(feature = "edit")]
pub use edit::{ImageBuilder, PageEdit, PathBuilder, TextBuilder};
#[cfg(feature = "edit")]
pub use pdfrum_edit::{EmbeddedFont, FontEncoding, StandardFont};
```

### 3.3 Consumer Profiles

| Profile | Configuration | What It Compiles | Typical Use Case |
| :--- | :--- | :--- | :--- |
| **Full Desktop** | `default` | Everything (Vello CPU, Edit, Forms, Codecs, System Fonts) | Native PDF reader/editor apps, desktop viewers. |
| **Headless Parser** | `default-features = false` | Core parser, text extraction, outline, encryption | High-throughput search indexers, metadata scrapers, document text extraction. **Zero rasterizers, zero font subsetter, zero image codecs.** |
| **Read-Only Viewer** | `default-features = false, features = ["vello-cpu", "codecs-all", "system-fonts"]` | Viewer pipeline without editing or form state machines | High-performance document display services. |
| **Alternative Engine** | `default-features = false, features = ["tinyskia", "codecs-all"]` | TinySkia CPU renderer | Deterministic rendering matching Resvg/Skia; no Vello CPU in tree. |
| **WASM Worker** | `default-features = false, features = ["tinyskia"]` | TinySkia + base-14 fonts | Web browser rendering via WebAssembly. **No filesystem scanning, no threadpools, no wgpu.** |

### 3.4 Preserving the GPU Isolation Guarantee

[DEPS.md](DEPS.md#L90) enforces that default builds contain zero `wgpu` or platform graphics driver dependencies.
* `vello-gpu` is **never in `default`**.
* [`scripts/check-no-wgpu.nu`](scripts/check-no-wgpu.nu) inspects default dependency closures and will continue to pass unconditionally.

---

## 4. Subsystem & Codec Deep Dive

### 4.1 Subsystems: `edit` and `forms`
* **`edit`:** Gates [`pdfrum-edit`](crates/pdfrum-edit/Cargo.toml) and [`subsetter`](Cargo.toml#L118). Gating this removes the entire font-subsetting engine, CFF/TrueType table writers, and document serialization routines from read-only binaries.
* **`forms`:** Gates [`pdfrum-form`](crates/pdfrum-form/Cargo.toml). Disabling this removes widget layout calculations, focus rings, keystroke undo stacks, and popup controllers. Note: `javascript = ["forms", "pdfrum-form/javascript"]` ensures JavaScript cascades always have form widgets to operate on.

### 4.2 Specialized Codecs (`codecs-all`, `jpx`, `jbig2`, `ccitt`)
In [`pdfrum-page`](crates/pdfrum-page/Cargo.toml#L39-L43) and [`pdfrum-filters`](crates/pdfrum-filters/Cargo.toml#L19-L22):
* `miniz_oxide` (Flate) and `zune-jpeg` (DCT/JPEG) remain unconditional dependencies because over 99% of PDF files rely on them for text streams and photos.
* `hayro-jpeg2000` (`jpx`), `hayro-jbig2` (`jbig2`), and `hayro-ccitt` (`ccitt`) are gated under individual features and bundled under `codecs-all`.
* If a disabled format is encountered in a PDF stream, the engine records a non-fatal diagnostic (`Diagnostic::UnsupportedFilter` / `UnsupportedCodec`) and renders the remainder of the page gracefully, consistent with `pdfrum`'s damage-recovery philosophy.

### 4.3 Platform: `system-fonts`
* In [`pdfrum-font/Cargo.toml`](crates/pdfrum-font/Cargo.toml#L32), `fontdb` is configured with `default-features = false`.
* Feature `system-fonts = ["fontdb/fs", "fontdb/memmap2"]` (default on).
* In WASM environments where filesystem scanning and memory-mapped files are unsupported, turning off `system-fonts` falls back to the embedded Foxit Base-14 font data shipped directly inside `pdfrum-font`.

### 4.4 Ergonomic Utility: `png`
* Adds optional dependency `png = { workspace = true, optional = true }`.
* Exposes methods on [`Pixmap`](crates/pdfrum-render/src/pixmap.rs):
  ```rust
  #[cfg(feature = "png")]
  impl Pixmap {
      /// Saves the pixmap as an RGBA PNG file to the given path.
      pub fn save_png(&self, path: impl AsRef<std::path::Path>) -> Result<(), crate::Error>;

      /// Encodes the pixmap as an in-memory PNG byte buffer.
      pub fn encode_png(&self) -> Result<Vec<u8>, crate::Error>;
  }
  ```

---

## 5. Explicit Render Surface

### 5.1 Eliminating the Invisible Default

Currently, `Page::render` hides backend choice and cache management:
```rust
// CURRENT (HIDDEN ALLOCATIONS & BACKEND COUPLING)
pub fn render(&self, options: &RenderOptions) -> Result<Pixmap> {
    self.render_on(
        &VelloCpuBackend::new(),
        options,
        &mut RenderSession::default(),
    )
}
```

This presents two architectural problems:
1. **Broken by `default-features = false`:** If `vello-cpu` is optional, `Page::render` cannot compile when default features are disabled.
2. **Hidden allocations:** It silently instantiates a backend and fresh caches per page, hiding cache lifecycle from callers.

### 5.2 The New Explicit Method Signatures

We structure the rendering surface around explicit backend passing:

#### On [`Page`](crates/pdfrum/src/page.rs#L107):

```rust
impl<'a> Page<'a> {
    /// Renders the page using an explicit rasterizer backend with fresh caches.
    pub fn render<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
    ) -> Result<Pixmap> {
        self.render_on(backend, options, &mut RenderSession::default())
    }

    /// Renders the page using an explicit rasterizer backend, reusing a caller-owned session.
    pub fn render_on<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
        session: &mut RenderSession,
    ) -> Result<Pixmap> {
        self.prepare(options, session).render_on(backend, session)
    }
}
```

#### On [`PreparedPage`](crates/pdfrum/src/page.rs#L520):

```rust
impl<'a> PreparedPage<'a> {
    /// Draws the prepared page on an explicit rasterizer backend with fresh caches.
    pub fn render<B: RasterBackend>(
        &self,
        backend: &B,
    ) -> Result<Pixmap> {
        self.render_on(backend, &mut RenderSession::default())
    }

    /// Draws the prepared page on an explicit rasterizer backend, reusing a caller-owned session.
    pub fn render_on<B: RasterBackend>(
        &self,
        backend: &B,
        session: &mut RenderSession,
    ) -> Result<Pixmap> {
        let mut target = backend.create_target(self.width(), self.height())?;
        // ... drawing walk ...
        target.finish()
    }
}
```

#### Signature Comparison Matrix

| Type | Old Signature | New Signature | Rationale |
| :--- | :--- | :--- | :--- |
| `Page::render` | `(&self, &RenderOptions) -> Result<Pixmap>` | `(&self, &B, &RenderOptions) -> Result<Pixmap>` | Explicit backend; fresh session |
| `Page::render_on` | `(&self, &B, &RenderOptions, &mut RenderSession) -> Result<Pixmap>` | *(Unchanged)* | Explicit backend; reused session |
| `PreparedPage::render` | `(&self) -> Result<Pixmap>` | `(&self, &B) -> Result<Pixmap>` | Explicit backend; fresh session |
| `PreparedPage::render_on` | `(&self, &B, &mut RenderSession) -> Result<Pixmap>` | *(Unchanged)* | Explicit backend; reused session |

---

## 6. Comprehensive Audit of Required Codebase Changes

### 6.1 Manifests (`Cargo.toml`)

| Manifest | Changes |
| :--- | :--- |
| [`crates/pdfrum/Cargo.toml`](crates/pdfrum/Cargo.toml) | 1. Add `default = ["vello-cpu", "edit", "forms", "system-fonts", "codecs-all"]`<br>2. Add `vello-cpu`, `tinyskia`, `agg`, `vello-gpu`<br>3. Add `edit`, `forms`, `system-fonts`, `codecs-all`, `jpx`, `jbig2`, `ccitt`, `png`<br>4. Rename `script` $\rightarrow$ `javascript = ["forms", "pdfrum-form/javascript"]`<br>5. Rename `walk-profile` $\rightarrow$ `profiling = ["pdfrum-page/profiling", "pdfrum-doc/profiling"]`<br>6. Move `rayon` from `[dependencies]` to `[dev-dependencies]`<br>7. Mark `pdfrum-raster-*`, `pdfrum-form`, `pdfrum-edit`, and `png` optional |
| [`crates/pdfrum-form/Cargo.toml`](crates/pdfrum-form/Cargo.toml) | Rename `script` $\rightarrow$ `javascript = ["dep:boa_engine", "dep:pdfrum-script"]` |
| [`crates/pdfrum-tool/Cargo.toml`](crates/pdfrum-tool/Cargo.toml) | Rename `script` $\rightarrow$ `javascript = ["dep:pdfrum-form", "pdfrum-form/javascript", "pdfrum/javascript"]` |
| [`crates/pdfrum-render/Cargo.toml`](crates/pdfrum-render/Cargo.toml) | Rename `walk-profile` $\rightarrow$ `profiling = ["pdfrum-page/profiling"]` |
| [`crates/pdfrum-page/Cargo.toml`](crates/pdfrum-page/Cargo.toml) | 1. Rename `walk-profile` $\rightarrow$ `profiling = []`<br>2. Add optional dependencies: `hayro-jpeg2000` (`jpx`), `hayro-jbig2` (`jbig2`) |
| [`crates/pdfrum-filters/Cargo.toml`](crates/pdfrum-filters/Cargo.toml) | Add optional dependency: `hayro-ccitt` (`ccitt`) |
| [`crates/pdfrum-font/Cargo.toml`](crates/pdfrum-font/Cargo.toml) | Add `system-fonts = ["fontdb/fs", "fontdb/memmap2"]` |
| [`crates/pdfrum-doc/Cargo.toml`](crates/pdfrum-doc/Cargo.toml) | Rename `walk-profile` $\rightarrow$ `profiling = ["pdfrum-page/profiling"]` |
| [`benches/Cargo.toml`](benches/Cargo.toml) | Rename `walk-profile` $\rightarrow$ `profiling = ["pdfrum-render/profiling", "pdfrum/profiling"]` |

---

### 6.2 Source Code File Audit

#### A. Feature `script` $\rightarrow$ `javascript` (42 sites)

* **`crates/pdfrum/src/lib.rs`**:
  * Lines 99, 103: `#[cfg(feature = "javascript")]` for re-exporting `ScriptCascade`, `ScriptConfig`, etc.
  * Crate-level doc comment at line 39.
* **`crates/pdfrum/src/form_session.rs`**:
  * Lines 130, 139, 152: `Cascades::Scripted` gating.
  * Lines 278, 323, 363: `with_scripts`, `install_document_model`, etc.
  * Lines 825, 841, 877, 890: `scripts`, `scripts_mut`, `advance_time`, `page_action`.
  * Lines 927, 953, 983, 995: focus and page event hooks.
  * Line 1158 (`#[cfg(feature = "javascript")]`) and Line 1239 (`#[cfg(not(feature = "javascript"))]`).
* **`crates/pdfrum-form/src/lib.rs`**:
  * Lines 55, 78: Module and re-export gates.
* **`crates/pdfrum-tool/`**:
  * `src/main.rs`: Line 53 (`mod jstranscript`).
  * `src/run.rs`: Lines 99, 1098, 1117, 1131, 1152, 1167 (`options.js_transcript` runner and clock tests).
  * `src/options.rs`: Lines 257, 559 (`cfg!(feature = "javascript")`).
  * `src/text.rs`: Line 90.

#### B. Feature `walk-profile` $\rightarrow$ `profiling` (36 sites)

* **`crates/pdfrum/src/profile.rs`**:
  * Lines 14, 18, 30: `#[cfg(feature = "profiling")]` and `#[cfg(not(feature = "profiling"))]`.
* **`crates/pdfrum-page/src/lib.rs`**:
  * Lines 69, 71: `pub mod renderprofile`.
* **`crates/pdfrum-page/src/renderprofile.rs`**:
  * Lines 36, 47, 88, 104, 167, 221, 234: `Stage` and accumulator guards.
* **`crates/pdfrum-render/src/lib.rs`**:
  * Lines 115, 117: `pub mod walkprofile`.
* **`crates/pdfrum-render/src/walkprofile.rs`**:
  * Lines 25, 112, 129, 224, 310, 354: Accumulator and timing guards.
* **`crates/pdfrum-doc/src/annot_render.rs`**:
  * Lines 63, 69: Stage timers.
* **`benches/src/bin/profile.rs`**:
  * Lines 422, 554, 577, 688, 700, 710, 872.

#### C. Explicit `Page::render(&backend, &options)` Call Sites (54 sites)

Every site calling `page.render(&options)` must be updated to pass `&backend`:

```rust
// Pattern:
let backend = VelloCpuBackend::new();
let pixmap = page.render(&backend, &options)?;
```

* **Doc examples:**
  * [`crates/pdfrum/src/lib.rs`](crates/pdfrum/src/lib.rs#L9-L14) (Lines 9, 292)
  * [`crates/pdfrum/src/page.rs`](crates/pdfrum/src/page.rs#L132) (Lines 132, 136, 504)
  * [`crates/pdfrum/src/render.rs`](crates/pdfrum/src/render.rs#L99) (Line 99)
  * [`crates/pdfrum/src/session.rs`](crates/pdfrum/src/session.rs#L40) (Line 40)
* **Examples:**
  * [`crates/pdfrum/examples/render-to-png.rs`](crates/pdfrum/examples/render-to-png.rs#L53) (Line 53)
* **Integration Tests:**
  * [`crates/pdfrum/tests/facade.rs`](crates/pdfrum/tests/facade.rs): 26 call sites.
  * [`crates/pdfrum/tests/flatten.rs`](crates/pdfrum/tests/flatten.rs): Lines 34, 163, 173.
  * [`crates/pdfrum/tests/prepared_page.rs`](crates/pdfrum/tests/prepared_page.rs): Lines 64, 65 (`one_to_one.render(&backend)`).
  * [`crates/pdfrum/tests/reexports.rs`](crates/pdfrum/tests/reexports.rs): Tests for backend exports.

---

### 6.3 Scripts & Baselines

1. **[`scripts/check-no-boa.nu`](scripts/check-no-boa.nu)**:
   * Update lines 4, 31, 33, 121, 126, 127, 129: change `--features script` $\rightarrow$ `--features javascript`.
2. **[`scripts/profile.nu`](scripts/profile.nu)**:
   * Update line 203: change `--features walk-profile` $\rightarrow$ `--features profiling`.
3. **[`scripts/api-snapshot.nu`](scripts/api-snapshot.nu)**:
   * Line 117: Update `FEATURED` entry:
     ```nu
     const FEATURED = [
         {file: 'pdfrum+javascript', crate: 'pdfrum', features: 'javascript'}
     ]
     ```
4. **Baseline Snapshots**:
   * Move `docs/api-baseline/pdfrum+script.txt` $\rightarrow$ `docs/api-baseline/pdfrum+javascript.txt`.
   * Update `docs/api-baseline/pdfrum.txt` to reflect the updated signature of `Page::render`.

---

## 7. Phased Implementation & Migration Plan

To maintain git bisectability and keep CI green at every intermediate commit, the rollout is partitioned into five distinct work packages:

### Work Package 1: Feature Renaming (`walk-profile` $\rightarrow$ `profiling`)
1. Update manifests in `pdfrum-page`, `pdfrum-render`, `pdfrum-doc`, `benches`, and `pdfrum`.
2. Replace `cfg(feature = "walk-profile")` with `cfg(feature = "profiling")` in all source files.
3. Update `scripts/profile.nu`.
4. **Verification:** `cargo check --workspace --features profiling` and `cargo test --workspace`.

### Work Package 2: Feature Renaming (`script` $\rightarrow$ `javascript`)
1. Update manifests in `pdfrum-form`, `pdfrum`, and `pdfrum-tool`.
2. Replace `cfg(feature = "script")` with `cfg(feature = "javascript")` in library sources, tests, and tool CLI.
3. Update `scripts/check-no-boa.nu` and `scripts/api-snapshot.nu`.
4. Rename `docs/api-baseline/pdfrum+script.txt` $\rightarrow$ `pdfrum+javascript.txt`.
5. **Verification:** `nu scripts/check-no-boa.nu` and `cargo test --workspace --features javascript`.

### Work Package 3: Demote `rayon` to `[dev-dependencies]`
1. Move `rayon.workspace = true` to `[dev-dependencies]` in `crates/pdfrum/Cargo.toml`.
2. Update `DEPS.md` to reflect `rayon` as a test/example dependency only.
3. **Verification:** `cargo tree -p pdfrum -e normal` contains zero `rayon` crates; `cargo test -p pdfrum` still passes.

### Work Package 4: Backend & Subsystem Feature Gating
1. Update [`crates/pdfrum/Cargo.toml`](crates/pdfrum/Cargo.toml):
   * Add optional backend dependencies (`vello-cpu`, `tinyskia`, `agg`, `vello-gpu`).
   * Add optional subsystem dependencies (`edit`, `forms`, `system-fonts`, `codecs-all`, `png`).
2. Update [`crates/pdfrum/src/lib.rs`](crates/pdfrum/src/lib.rs) with `#[cfg(feature = "...")]` re-exports for backend structs and subsystem types.
3. Update child crate manifests (`pdfrum-font`, `pdfrum-page`, `pdfrum-filters`) with optional codec/font flags.
4. Add tests in `crates/pdfrum/tests/reexports.rs` confirming that each backend is available when its feature is enabled.
5. **Verification:**
   * Default build: `cargo check -p pdfrum` (resolves default set).
   * Headless build: `cargo check -p pdfrum --no-default-features` (pure parser/text).
   * Alternative build: `cargo check -p pdfrum --no-default-features --features tinyskia`.

### Work Package 5: Explicit Render API & Call Site Migration
1. Update `Page::render` and `PreparedPage::render` signatures in `crates/pdfrum/src/page.rs` to take `&backend`.
2. Update all call sites across `crates/pdfrum/src/lib.rs`, `render.rs`, `session.rs`, and examples.
3. Update all 31 integration test call sites in `tests/facade.rs`, `tests/flatten.rs`, `tests/prepared_page.rs`.
4. Re-generate public API baseline: `nu scripts/api-snapshot.nu update`.
5. **Verification:** Run complete CI gate `nu scripts/ci.nu`.

---

## 8. Landed — what differs from the blueprint

- **`fontconfig` is kept on native targets.** The blueprint's `fontdb` with
  `fs` and `memmap` alone would change which directories the host scan
  reads on Linux, and with it which substitute a missing font resolves to;
  the native target table keeps `fontconfig` so a substitute resolves as it
  did, and the wasm32 table carries none of the three.
- **Codec and font features are off at the crate level.** `pdfrum-page`,
  `pdfrum-filters` and `pdfrum-font` default to nothing; the facade's default
  set turns them on. Cargo unifies features across a build, so a crate
  default that was *on* would have been switched back on by every workspace
  dependent of that crate, and `default-features = false` on the facade
  would have dropped nothing. Standalone test runs of those crates run
  without the codecs; the tests that need them are gated, and the fuzz
  workspace asks for them.
- **`required-features`.** The facade's integration tests and examples
  declare the features they need, so a headless `cargo clippy --all-targets`
  is honest rather than red. Three tests — signatures, searchex, thumbnails —
  run on any set.
- **`Error::Save` exists only with `edit`**, as does `SaveError`; saving a
  filled form needs `edit` and `forms` both.
- **The png errors carry the encoder's message.** The render error is
  comparable and cloneable and `png::EncodingError` is neither.
- **API baselines.** `api-snapshot.nu` records seven featured surfaces now:
  the facade with `javascript`, with `png`, with `tinyskia,agg`;
  `pdfrum-page` with the codecs; `pdfrum-filters` with `ccitt`; `pdfrum-font`
  with `system-fonts`; `pdfrum-render` with `png`.
- **`Page::render` doc links.** A feature-gated item is named in a bare code
  span, not a link, so the default and the featured docs both build.
