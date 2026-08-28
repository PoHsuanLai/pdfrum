//! Reading `testing/SUPPRESSIONS` from the oracle checkout.
//!
//! The file is five space-separated columns: file name, platform, v8 support,
//! xfa support, rendering backend. Every column must match for a line to
//! suppress its file, and each column (bar the name) may hold a comma list or
//! `*`. A file name may repeat on later lines to suppress more cases.
//!
//! We build the oracle without V8 and without XFA and render with AGG, so our
//! selector is `linux / nov8 / noxfa / agg` — the same one
//! `testing/tools/suppressor.py` computes for that configuration.

use std::collections::BTreeSet;

/// The build configuration a suppression line is matched against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub platform: String,
    pub v8: String,
    pub xfa: String,
    pub renderer: String,
}

impl Default for Selector {
    /// Our oracle: Linux, `pdf_enable_v8=false`, `pdf_enable_xfa=false`, AGG.
    fn default() -> Self {
        Selector {
            platform: "linux".to_owned(),
            v8: "nov8".to_owned(),
            xfa: "noxfa".to_owned(),
            renderer: "agg".to_owned(),
        }
    }
}

/// Something wrong with a suppressions line.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SuppressionError {
    #[error("line {line}: expected 5 columns, found {found}")]
    ColumnCount { line: usize, found: usize },
}

/// Parses a SUPPRESSIONS file, returning the file names suppressed under
/// `selector`.
///
/// Comments (`#` to end of line) and blank lines are dropped, matching the
/// upstream Python reader.
pub fn parse(text: &str, selector: &Selector) -> Result<BTreeSet<String>, SuppressionError> {
    let mut suppressed = BTreeSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let columns: Vec<&str> = line.split_whitespace().collect();
        let [name, platform, v8, xfa, renderer] = columns[..] else {
            return Err(SuppressionError::ColumnCount {
                line: index + 1,
                found: columns.len(),
            });
        };
        if column_matches(platform, &selector.platform)
            && column_matches(v8, &selector.v8)
            && column_matches(xfa, &selector.xfa)
            && column_matches(renderer, &selector.renderer)
        {
            suppressed.insert(name.to_owned());
        }
    }
    Ok(suppressed)
}

/// A column matches when it is `*` or its comma list contains the value.
fn column_matches(column: &str, value: &str) -> bool {
    column == "*" || column.split(',').any(|item| item == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        parse(text, &Selector::default())
            .unwrap()
            .into_iter()
            .collect()
    }

    #[test]
    fn wildcards_match_our_configuration() {
        assert_eq!(names("everywhere.pdf * * * *\n"), ["everywhere.pdf"]);
    }

    #[test]
    fn a_platform_we_are_not_does_not_match() {
        assert!(names("mac_only.pdf mac * * agg\n").is_empty());
        assert!(names("win_only.pdf win * * *\n").is_empty());
    }

    #[test]
    fn v8_and_xfa_columns_select_our_build() {
        // We are nov8/noxfa, so lines keyed to those apply to us; lines keyed
        // to a v8 or xfa build do not.
        assert_eq!(names("needs_nov8.pdf * nov8 * *\n"), ["needs_nov8.pdf"]);
        assert_eq!(names("needs_noxfa.pdf * * noxfa *\n"), ["needs_noxfa.pdf"]);
        assert!(names("v8_only.pdf * v8 * *\n").is_empty());
        assert!(names("xfa_only.pdf * * xfa *\n").is_empty());
    }

    #[test]
    fn the_renderer_column_selects_agg() {
        assert_eq!(names("agg.pdf * * * agg\n"), ["agg.pdf"]);
        assert!(names("skia.pdf * * * skia\n").is_empty());
    }

    #[test]
    fn comma_lists_match_any_member() {
        assert_eq!(names("multi.pdf linux,win * * agg,skia\n"), ["multi.pdf"]);
        assert!(names("other.pdf mac,win * * *\n").is_empty());
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "# a header comment\n\
                    \n\
                    real.pdf * * * *\n\
                    trailing.pdf * * * *  # why it is suppressed\n\
                    # 12.pdf mac * * agg\n";
        assert_eq!(names(text), ["real.pdf", "trailing.pdf"]);
    }

    #[test]
    fn a_repeated_name_is_recorded_once() {
        let text = "dup.pdf mac * * agg\n\
                    dup.pdf linux * * agg\n";
        assert_eq!(names(text), ["dup.pdf"]);
    }

    #[test]
    fn a_short_line_is_an_error() {
        assert_eq!(
            parse("bad.pdf * *\n", &Selector::default()),
            Err(SuppressionError::ColumnCount { line: 1, found: 3 })
        );
    }

    #[test]
    fn selector_default_is_our_oracle_build() {
        let selector = Selector::default();
        assert_eq!(selector.platform, "linux");
        assert_eq!(selector.v8, "nov8");
        assert_eq!(selector.xfa, "noxfa");
        assert_eq!(selector.renderer, "agg");
    }

    #[test]
    fn parses_a_slice_of_the_real_file() {
        // Verbatim lines from testing/SUPPRESSIONS.
        let text = "12.pdf mac * * agg\n\
                    1_1_textbox.pdf * * * *\n\
                    1_matrix.pdf mac * * agg\n\
                    2_6_textbox.pdf * * * *\n";
        assert_eq!(names(text), ["1_1_textbox.pdf", "2_6_textbox.pdf"]);
    }
}
