//! Blocks to GitHub-flavoured Markdown.

use std::fmt::Write;

use crate::ast::{Block, lead_in};

/// The blocks as Markdown, blank-line separated, ending in one newline;
/// every image is `![alt](image)`.
#[must_use]
pub fn render(blocks: &[Block]) -> String {
    render_with_images(blocks, |_| None)
}

/// [`render`] with each image linked where `url` says: called with the
/// image's index in the page's drawing order, it returns the link's
/// destination, or `None` to keep the `image` placeholder. A destination
/// with a space or a bracket in it is written in angle brackets, as the
/// `CommonMark` spec has it.
#[must_use]
pub fn render_with_images(blocks: &[Block], url: impl Fn(usize) -> Option<String>) -> String {
    let mut out = String::new();
    for block in blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        match block {
            Block::Heading { level, text } => {
                out.push_str(&"#".repeat(usize::from(*level).clamp(1, 6)));
                out.push(' ');
                out.push_str(text.trim());
                out.push('\n');
            }
            Block::Paragraph(text) => {
                // The lead-in's marks are the crate's own and stay; the
                // text on either side is the page's and is escaped.
                if let Some((lead, rest)) = lead_in(text.trim()) {
                    let _ = write!(out, "**{}**{}", escape(lead), escape_inline(rest));
                } else {
                    out.push_str(&escape(text.trim()));
                }
                out.push('\n');
            }
            Block::List { ordered, items } => {
                for (i, item) in items.iter().enumerate() {
                    if *ordered {
                        let _ = write!(out, "{}. ", i + 1);
                    } else {
                        out.push_str("- ");
                    }
                    out.push_str(&escape(item.trim()));
                    out.push('\n');
                }
            }
            Block::Code(text) => {
                // A fence longer than any run of backticks inside.
                let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                let fence = "`".repeat(longest.max(2) + 1);
                out.push_str(&fence);
                out.push('\n');
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&fence);
                out.push('\n');
            }
            Block::Table(rows) => {
                let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
                if columns == 0 {
                    continue;
                }
                for (i, row) in rows.iter().enumerate() {
                    out.push('|');
                    for c in 0..columns {
                        out.push(' ');
                        out.push_str(&cell(row.get(c).map_or("", String::as_str)));
                        out.push_str(" |");
                    }
                    out.push('\n');
                    if i == 0 {
                        out.push('|');
                        for _ in 0..columns {
                            out.push_str(" --- |");
                        }
                        out.push('\n');
                    }
                }
            }
            Block::Image { alt, index } => {
                let destination = index.and_then(&url).map_or_else(
                    || "image".to_owned(),
                    |url| {
                        if url.contains(|c: char| c.is_whitespace() || matches!(c, '(' | ')')) {
                            format!("<{url}>")
                        } else {
                            url
                        }
                    },
                );
                let _ = writeln!(out, "![{}]({destination})", escape(alt.trim()));
            }
        }
    }
    out
}

/// A table cell: pipes and line breaks cannot appear inside one.
fn cell(text: &str) -> String {
    escape(text.trim())
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
}

/// The few characters that would otherwise be read as markup at the start
/// of a line or around a word.
fn escape(text: &str) -> String {
    escape_from(text, true)
}

/// [`escape`] for text that continues a line, where a leading `-` or `#`
/// is not at the line's start.
fn escape_inline(text: &str) -> String {
    escape_from(text, false)
}

fn escape_from(text: &str, at_line_start: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_line_start = at_line_start;
    for c in text.chars() {
        if at_line_start && matches!(c, '#' | '>' | '-' | '+' | '*') {
            out.push('\\');
        }
        match c {
            '*' | '_' | '`' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            other => out.push(other),
        }
        at_line_start = c == '\n';
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{render, render_with_images};

    use crate::ast::Block;

    #[test]
    fn every_block_kind_renders_as_expected() {
        let md = render(&[
            Block::Heading {
                level: 2,
                text: "Intro".into(),
            },
            Block::Paragraph("Some *text* here.".into()),
            Block::Paragraph("**Redaction** - Lets you *remove* text.".into()),
            Block::List {
                ordered: true,
                items: vec!["one".into(), "two".into()],
            },
            Block::Code("let a = `b`;".into()),
            Block::Table(vec![
                vec!["h1".into(), "h2".into()],
                vec!["a|b".into(), "c".into()],
            ]),
            Block::Image {
                alt: "A chart".into(),
                index: None,
            },
        ]);
        assert_eq!(
            md,
            "## Intro\n\nSome \\*text\\* here.\n\n**Redaction** - Lets you \\*remove\\* text.\n\n1. one\n2. two\n\n``\nlet a = `b`;\n``\n\n| h1 | h2 |\n| --- | --- |\n| a\\|b | c |\n\n![A chart](image)\n"
                .replace("``\nlet", "```\nlet")
                .replace("`;\n``\n", "`;\n```\n")
        );
    }

    #[test]
    fn an_image_is_linked_where_the_caller_says_and_a_figure_without_one_is_not() {
        let blocks = [
            Block::Image {
                alt: "A chart".into(),
                index: Some(3),
            },
            Block::Image {
                alt: String::new(),
                index: Some(4),
            },
            Block::Image {
                alt: "Words only".into(),
                index: None,
            },
        ];
        let md = render_with_images(&blocks, |i| match i {
            3 => Some("out/guide-p2-4.jpg".to_owned()),
            4 => Some("my images/guide-p2-5.png".to_owned()),
            _ => None,
        });
        assert_eq!(
            md,
            "![A chart](out/guide-p2-4.jpg)\n\n![](<my images/guide-p2-5.png>)\n\n![Words only](image)\n"
        );
        assert_eq!(
            render(&blocks),
            "![A chart](image)\n\n![](image)\n\n![Words only](image)\n"
        );
    }
}
