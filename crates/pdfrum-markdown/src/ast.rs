//! The blocks a page reduces to, whichever tier produced them.

/// One block of a page's content, in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A heading at `level` 1 through 6.
    Heading {
        /// 1 is the largest.
        level: u8,
        /// The heading's text, on one line.
        text: String,
    },
    /// Running text, wrapped lines already joined.
    Paragraph(String),
    /// A list; each item is one line of text.
    List {
        /// Whether the items were numbered.
        ordered: bool,
        /// The items, bullets and numbers stripped.
        items: Vec<String>,
    },
    /// Monospaced text, line breaks kept.
    Code(String),
    /// Rows of cells; the first row is the header.
    Table(Vec<Vec<String>>),
    /// A picture, with whatever the document said it shows.
    Image {
        /// The alternative text, possibly empty.
        alt: String,
    },
}

impl Block {
    /// The text a block carries, for callers that only want words.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Heading { text, .. } | Self::Paragraph(text) | Self::Code(text) => text.clone(),
            Self::List { items, .. } => items.join("\n"),
            Self::Table(rows) => rows
                .iter()
                .map(|row| row.join("\t"))
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Image { alt } => alt.clone(),
        }
    }
}
