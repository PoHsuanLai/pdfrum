//! The benchmark corpus: which documents, and what class each one is in.
//!
//! One list, shared by the five per-crate criterion suites, the `profile`
//! binary, the ratchet checker and `scripts/bench-oracle.nu`, so that every
//! number in describes the same 44 files. The files
//! themselves are in `benches/corpus/`, copied unmodified from the oracle
//! checkout — see its `PROVENANCE.md` for where each one came from and why it
//! is here.
//!
//! # A leaf crate, deliberately
//!
//! This crate depends on nothing — not on `pdfrum`, not on criterion. It is
//! the shared half of the M12 bench split: each of `pdfrum-parser`,
//! `pdfrum-page`, `pdfrum-render`, `pdfrum-text` and `pdfrum-edit` owns its
//! own `benches/` target and dev-depends on this list, so `cargo bench -p
//! pdfrum-parser` builds a parser and a document list rather than the whole
//! workspace. See `benches/README.md` for the layout.
//!
//! # Why a class per document
//!
//! The M12 ratchet's noise band and its pass/fail rule are both per *class*,
//! not per file: the performance ring admits a dependency on ">= 10% on
//! at least one bench class or >= 5% on the geomean", so the class has to be a
//! thing the harness knows rather than something a reader infers from a file
//! name. A document belongs to exactly one class — the cost that dominates it
//! — because a file counted in two classes would let one improvement be
//! reported twice.

#![forbid(unsafe_code)]

/// What a document is in the corpus to measure.
///
/// The six classes names, one per document. `Mixed` is not a
/// leftover bin: it is the class for documents where no single cost dominates,
/// which is the shape a real-world page usually has and the one the other five
/// classes deliberately do not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Glyph-dominated: thousands of show-text operations, few paths.
    Text,
    /// Path-dominated: tens of thousands of fills and strokes, little text.
    Vector,
    /// Decode-dominated: JPEG, JPEG 2000, JBIG2 or CCITT, and resampling.
    Image,
    /// Shading-dominated: axial, radial, function, and the mesh types.
    Shading,
    /// Widget appearance streams: the annotation-render path.
    Forms,
    /// No single dominant cost — text and paths and images together.
    Mixed,
}

impl Class {
    /// The class's name as it appears in a benchmark id and in baseline.json.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Class::Text => "text",
            Class::Vector => "vector",
            Class::Image => "image",
            Class::Shading => "shading",
            Class::Forms => "forms",
            Class::Mixed => "mixed",
        }
    }

    /// Every class, in the order tables report them.
    pub const ALL: [Class; 6] = [
        Class::Text,
        Class::Vector,
        Class::Image,
        Class::Shading,
        Class::Forms,
        Class::Mixed,
    ];
}

/// One corpus document.
#[derive(Debug, Clone, Copy)]
pub struct Doc {
    /// The file stem: `corpus/<stem>.pdf`.
    pub stem: &'static str,
    /// Which cost this document is here to measure.
    pub class: Class,
    /// How many pages it has, so a multi-page group can select on it without
    /// opening every file first.
    pub pages: u32,
}

/// The corpus, ordered by class and then by name.
pub const CORPUS: &[Doc] = &[
    Doc {
        stem: "text_bug_1029",
        class: Class::Text,
        pages: 1,
    },
    Doc {
        stem: "text_cjk_functions",
        class: Class::Text,
        pages: 4,
    },
    Doc {
        stem: "text_cjk_page",
        class: Class::Text,
        pages: 7,
    },
    Doc {
        stem: "text_cjk_structure",
        class: Class::Text,
        pages: 7,
    },
    Doc {
        stem: "text_foxit_products",
        class: Class::Text,
        pages: 11,
    },
    Doc {
        stem: "text_foxittext",
        class: Class::Text,
        pages: 1,
    },
    Doc {
        stem: "text_quick_start",
        class: Class::Text,
        pages: 11,
    },
    Doc {
        stem: "text_tcpdf_055",
        class: Class::Text,
        pages: 14,
    },
    Doc {
        stem: "text_tcpdf_063",
        class: Class::Text,
        pages: 10,
    },
    Doc {
        stem: "vector_en_system",
        class: Class::Vector,
        pages: 1,
    },
    Doc {
        stem: "vector_en_tem",
        class: Class::Vector,
        pages: 6,
    },
    Doc {
        stem: "vector_font_feature",
        class: Class::Vector,
        pages: 10,
    },
    Doc {
        stem: "vector_font_size14",
        class: Class::Vector,
        pages: 9,
    },
    Doc {
        stem: "vector_paths_1751",
        class: Class::Vector,
        pages: 1,
    },
    Doc {
        stem: "vector_tcpdf_009",
        class: Class::Vector,
        pages: 1,
    },
    Doc {
        stem: "image_bug_583804",
        class: Class::Image,
        pages: 1,
    },
    Doc {
        stem: "image_bug_718762",
        class: Class::Image,
        pages: 1,
    },
    Doc {
        stem: "image_bug_898443",
        class: Class::Image,
        pages: 1,
    },
    Doc {
        stem: "image_ccitt_3bigpreview",
        class: Class::Image,
        pages: 1,
    },
    Doc {
        stem: "image_ccitt_transfer",
        class: Class::Image,
        pages: 2,
    },
    Doc {
        stem: "image_en_fqa",
        class: Class::Image,
        pages: 4,
    },
    Doc {
        stem: "image_jbig2_1478366",
        class: Class::Image,
        pages: 1,
    },
    Doc {
        stem: "image_jbig2_880920",
        class: Class::Image,
        pages: 1,
    },
    Doc {
        stem: "image_jpx_123",
        class: Class::Image,
        pages: 1,
    },
    Doc {
        stem: "shading_axial_radial",
        class: Class::Shading,
        pages: 1,
    },
    Doc {
        stem: "shading_coons",
        class: Class::Shading,
        pages: 1,
    },
    Doc {
        stem: "shading_gouraud",
        class: Class::Shading,
        pages: 1,
    },
    Doc {
        stem: "shading_tcpdf_030",
        class: Class::Shading,
        pages: 2,
    },
    Doc {
        stem: "shading_tcpdf_056",
        class: Class::Shading,
        pages: 1,
    },
    Doc {
        stem: "shading_tcpdf_058",
        class: Class::Shading,
        pages: 1,
    },
    Doc {
        stem: "shading_tensor",
        class: Class::Shading,
        pages: 1,
    },
    Doc {
        stem: "shading_type4_5",
        class: Class::Shading,
        pages: 1,
    },
    Doc {
        stem: "forms_combo_box",
        class: Class::Forms,
        pages: 2,
    },
    Doc {
        stem: "forms_list_box",
        class: Class::Forms,
        pages: 4,
    },
    Doc {
        stem: "forms_number",
        class: Class::Forms,
        pages: 1,
    },
    Doc {
        stem: "forms_push_button",
        class: Class::Forms,
        pages: 4,
    },
    Doc {
        stem: "forms_signature",
        class: Class::Forms,
        pages: 2,
    },
    Doc {
        stem: "forms_text_field",
        class: Class::Forms,
        pages: 3,
    },
    Doc {
        stem: "forms_widgets_407",
        class: Class::Forms,
        pages: 5,
    },
    Doc {
        stem: "mixed_en_uicase",
        class: Class::Mixed,
        pages: 12,
    },
    Doc {
        stem: "mixed_formfield",
        class: Class::Mixed,
        pages: 1,
    },
    Doc {
        stem: "mixed_tcpdf_006",
        class: Class::Mixed,
        pages: 6,
    },
    Doc {
        stem: "mixed_tcpdf_045",
        class: Class::Mixed,
        pages: 16,
    },
    Doc {
        stem: "mixed_tcpdf_059",
        class: Class::Mixed,
        pages: 16,
    },
];

/// The documents with enough pages for per-page parallelism to be visible.
///
/// Eight is the threshold and it is a judgement, stated so it can be argued
/// with: below it a rayon scaling curve is measuring thread startup against a
/// handful of pages, and the corpus tops out at sixteen pages because
/// **nothing in either of the oracle's two directories has more** — the
/// largest document in 1375 files is sixteen pages. The scaling numbers in
/// docs/status/M12.md are bounded by that and say so.
#[must_use]
pub fn multipage() -> Vec<&'static Doc> {
    CORPUS.iter().filter(|doc| doc.pages >= 8).collect()
}

/// Where the corpus documents live, as an absolute path.
///
/// Resolved from this crate's own `CARGO_MANIFEST_DIR` — baked in at compile
/// time — rather than from the process's working directory. That distinction
/// became load-bearing with the per-crate bench split: `cargo bench -p
/// pdfrum-render` runs the harness with `crates/pdfrum-render` as its working
/// directory, `cargo bench -p pdfrum-bench` runs it from `benches/`, and a
/// `cargo run` from the workspace root does neither. A relative `corpus/`
/// resolved correctly in exactly one of those three and panicked in the other
/// two.
///
/// The compile-time path is the fallback, not the first answer: cargo reuses
/// this crate's compiled artefact across checkouts that share a target
/// directory, so a harness built in one worktree and run in another carries
/// the first worktree's path — which may no longer exist (seen twice on
/// 2026-09-06, a `save` bench panicking on a corpus file in a deleted
/// worktree). The working directory is always inside the workspace when
/// cargo runs a bench, so the corpus is found by walking up from it first.
#[must_use]
pub fn dir() -> std::path::PathBuf {
    let marker = "text_quick_start.pdf";
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            let corpus = ancestor.join("benches").join("corpus");
            if corpus.join(marker).is_file() {
                return corpus;
            }
        }
    }
    // `benches/corpus-list` → `benches/corpus`.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(
            || std::path::PathBuf::from("corpus"),
            |benches| benches.join("corpus"),
        )
}

/// The path to one corpus document.
#[must_use]
pub fn path(stem: &str) -> std::path::PathBuf {
    dir().join(format!("{stem}.pdf"))
}

/// Read one corpus document's bytes.
///
/// # Panics
///
/// When the file is missing — a benchmark with no input measures nothing, and
/// failing loudly beats reporting a zero.
#[must_use]
pub fn bytes(stem: &str) -> std::sync::Arc<[u8]> {
    let path = path(stem);
    match std::fs::read(&path) {
        Ok(bytes) => std::sync::Arc::from(bytes),
        Err(err) => panic!("corpus file {} is missing: {err}", path.display()),
    }
}
