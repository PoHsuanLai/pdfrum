//! Which documents, and what class each is in. One list so every number
//! describes the same 44 files in `benches/corpus/`.

#![forbid(unsafe_code)]

/// What a document is in the corpus to measure.
///
/// One class per document. `Mixed` is documents where no single cost dominates.
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

/// Documents with enough pages for per-page parallelism to be visible.
/// Eight pages: below that, a scaling curve is mostly thread startup.
#[must_use]
pub fn multipage() -> Vec<&'static Doc> {
    CORPUS.iter().filter(|doc| doc.pages >= 8).collect()
}

/// Absolute path to `benches/corpus/`.
///
/// Walks up from cwd first: `cargo bench -p` sets cwd to the crate, not
/// the workspace. `CARGO_MANIFEST_DIR` is the fallback — cargo reuses this
/// crate's artefact across checkouts that share a target dir, so a baked-in
/// path can point at a deleted tree.
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
