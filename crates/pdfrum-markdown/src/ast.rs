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
    /// Running text, wrapped lines already joined. A paragraph that opens
    /// with a bold lead-in — `**Redaction** - Lets you …` — carries it
    /// wrapped in the Markdown marks, the one piece of markup a block holds;
    /// [`lead_in`] takes it apart, and everything after it is plain text.
    Paragraph(String),
    /// A list; each item is one line of text.
    List {
        /// How the items were marked, which decides how they are written.
        marker: ListMarker,
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
        /// Which of the page's images it is, by index in drawing order — the
        /// order the facade's `Page::images` lists them in — or `None` for a
        /// figure that drew no image, which is its alternative text alone.
        index: Option<usize>,
    },
}

/// How a list's items are marked.
///
/// A document that carries its own numbers keeps them: `2.1 Background`
/// and `5.4 Virus Scan` are section numbers a cross-reference points at,
/// and a renderer that renumbered them from one would write `1.` and
/// `4.` — the wrong section. A list the producer marked with bullets, or
/// numbered without the numbers surviving into the text, is renumbered by
/// the Markdown renderer as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListMarker {
    /// Bullets: written `- `.
    Bullet,
    /// Numbered by the renderer, from one: written `1. `, `2. `, …
    Ordered,
    /// Numbered by the document, one label per item, in the same order —
    /// `1.`, `2.1`, `5.4` — written as the page had them.
    Labelled(Vec<String>),
}

impl Block {
    /// The text a block carries, for callers that only want words.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Paragraph(text) => {
                lead_in(text).map_or_else(|| text.clone(), |(lead, rest)| format!("{lead}{rest}"))
            }
            Self::Heading { text, .. } | Self::Code(text) => text.clone(),
            Self::List { items, .. } => items.join("\n"),
            Self::Table(rows) => rows
                .iter()
                .map(|row| row.join("\t"))
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Image { alt, .. } => alt.clone(),
        }
    }
}

/// A paragraph's bold lead-in and what follows it, when it opens with one:
/// `**Redaction** - Lets you` is `("Redaction", " - Lets you")`.
#[must_use]
pub fn lead_in(text: &str) -> Option<(&str, &str)> {
    let (lead, rest) = text.strip_prefix("**")?.split_once("**")?;
    (!lead.is_empty()).then_some((lead, rest))
}

#[cfg(test)]
mod tests {
    use super::{Block, lead_in};

    #[test]
    fn a_lead_in_is_taken_apart_and_words_come_without_the_marks() {
        assert_eq!(
            lead_in("**Redaction** - Lets you"),
            Some(("Redaction", " - Lets you"))
        );
        assert_eq!(lead_in("Redaction - Lets you"), None);
        assert_eq!(lead_in("****"), None);
        assert_eq!(
            Block::Paragraph("**Redaction** - Lets you".into()).text(),
            "Redaction - Lets you"
        );
    }
}
