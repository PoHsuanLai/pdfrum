//! Blocks to GitHub-flavoured Markdown.

use std::fmt::Write;

use crate::ast::Block;

/// The blocks as Markdown, blank-line separated, ending in one newline.
#[must_use]
pub fn render(blocks: &[Block]) -> String {
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
                out.push_str(&escape(text.trim()));
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
            Block::Image { alt } => {
                let _ = writeln!(out, "![{}](image)", escape(alt.trim()));
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
    let mut out = String::with_capacity(text.len());
    let mut at_line_start = true;
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
    use super::render;

    use crate::ast::Block;

    #[test]
    fn every_block_kind_renders_as_expected() {
        let md = render(&[
            Block::Heading {
                level: 2,
                text: "Intro".into(),
            },
            Block::Paragraph("Some *text* here.".into()),
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
            },
        ]);
        assert_eq!(
            md,
            "## Intro\n\nSome \\*text\\* here.\n\n1. one\n2. two\n\n``\nlet a = `b`;\n``\n\n| h1 | h2 |\n| --- | --- |\n| a\\|b | c |\n\n![A chart](image)\n"
                .replace("``\nlet", "```\nlet")
                .replace("`;\n``\n", "`;\n```\n")
        );
    }
}
