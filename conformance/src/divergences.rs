//! `conformance/divergences.toml` — the files where pdfrum deliberately
//! disagrees with the oracle.
//!
//! PDFium is the oracle, not the specification. Where the two part company and
//! ISO 32000-1 backs pdfrum, matching the oracle would mean reproducing a known
//! defect, so the file is scored [`Status::Diverged`](crate::scoreboard::Status)
//! rather than failed: it leaves the pass/fail denominator and is reported as
//! its own count.
//!
//! The ratchet is the same one-directional discipline
//! [`thresholds`](crate::thresholds) enforces, moved from a number to a
//! justification: an entry is admitted only with evidence. Both fields are
//! required and neither may be blank, so a row cannot be added as a bare path
//! to turn a regression green — writing one down means naming the tracker
//! issue or the `[oracle-bug]` source location that reasons it.
//!
//! The format read here is the small subset the file uses — a `[divergences]`
//! section of `"path" = { why = "…", cite = "…" }` with `#` comments — parsed
//! in-crate for the same reason the thresholds ratchet is: neither gate may
//! move under a dependency update.

use std::collections::BTreeMap;

/// Why one file is allowed to disagree with the oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    /// One sentence: what the oracle does and why the specification is
    /// against it.
    pub why: String,
    /// The evidence — a tracker issue, the `[oracle-bug]` source location
    /// that reasons it, and/or the write-up under `docs/upstream/`.
    pub cite: String,
}

/// Parsed `divergences.toml`, keyed by corpus-relative path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Divergences {
    entries: BTreeMap<String, Divergence>,
}

impl Divergences {
    /// The justification for a path, or `None` when it has none.
    pub fn get(&self, path: &str) -> Option<&Divergence> {
        self.entries.get(path)
    }

    /// Every entry, in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Divergence)> {
        self.entries.iter()
    }

    /// How many files are excused.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the file excuses nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// What went wrong reading the divergences file.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DivergenceError {
    #[error("line {line}: expected `\"path\" = {{ why = \"…\", cite = \"…\" }}`, found {text:?}")]
    Malformed { line: usize, text: String },
    #[error(
        "line {line}: {path} claims a divergence without a {field} — an entry \
         is admitted only with evidence"
    )]
    MissingEvidence {
        line: usize,
        path: String,
        field: &'static str,
    },
    #[error("line {line}: {path} appears twice")]
    Duplicate { line: usize, path: String },
    #[error("line {line}: unknown key {key:?} in the entry for {path}")]
    UnknownKey {
        line: usize,
        path: String,
        key: String,
    },
    #[error("line {line}: unknown section {section:?}")]
    UnknownSection { line: usize, section: String },
}

/// Parses `divergences.toml`, enforcing the evidence rule as it goes.
///
/// Recognized shape:
///
/// ```toml
/// [divergences]
/// "resources/whitespace.pdf" = { why = "…", cite = "crbug.com/40643656" }
/// ```
pub fn parse(text: &str) -> Result<Divergences, DivergenceError> {
    let mut entries: BTreeMap<String, Divergence> = BTreeMap::new();
    let mut inside = false;
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = section_name(line) {
            if name != "divergences" {
                return Err(DivergenceError::UnknownSection {
                    line: line_no,
                    section: name.to_owned(),
                });
            }
            inside = true;
            continue;
        }
        if !inside {
            return Err(DivergenceError::Malformed {
                line: line_no,
                text: line.to_owned(),
            });
        }
        let (path, entry) = parse_entry(line, line_no)?;
        if entries.insert(path.clone(), entry).is_some() {
            return Err(DivergenceError::Duplicate {
                line: line_no,
                path,
            });
        }
    }
    Ok(Divergences { entries })
}

/// Splits one `"path" = { … }` line into its key and its justification.
fn parse_entry(line: &str, line_no: usize) -> Result<(String, Divergence), DivergenceError> {
    let malformed = || DivergenceError::Malformed {
        line: line_no,
        text: line.to_owned(),
    };
    let (key, rest) = line.split_once('=').ok_or_else(malformed)?;
    let path = unquote(key.trim());
    if path.is_empty() {
        return Err(malformed());
    }
    let body = rest
        .trim()
        .strip_prefix('{')
        .and_then(|inner| inner.strip_suffix('}'))
        .ok_or_else(malformed)?;

    let (mut why, mut cite) = (None, None);
    for field in split_fields(body) {
        let (name, value) = field.split_once('=').ok_or_else(malformed)?;
        let slot = match name.trim() {
            "why" => &mut why,
            "cite" => &mut cite,
            other => {
                return Err(DivergenceError::UnknownKey {
                    line: line_no,
                    path,
                    key: other.to_owned(),
                });
            }
        };
        *slot = Some(unquote(value.trim()));
    }

    // The evidence rule. A blank field is the same omission as a missing one:
    // both name nothing, and a row that names nothing excuses nothing.
    let required = |slot: Option<String>, field: &'static str| {
        slot.filter(|value| !value.trim().is_empty())
            .ok_or(DivergenceError::MissingEvidence {
                line: line_no,
                path: path.clone(),
                field,
            })
    };
    Ok((
        path.clone(),
        Divergence {
            why: required(why, "why")?,
            cite: required(cite, "cite")?,
        },
    ))
}

/// Splits an inline table's body on the commas that separate its fields.
///
/// A plain `split(',')` would cut a justification in half, and the `why`
/// sentences here routinely hold commas, so a comma inside quotes is passed
/// over.
fn split_fields(body: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (offset, ch) in body.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(body[start..offset].trim());
                start = offset + 1;
            }
            _ => {}
        }
    }
    let last = body[start..].trim();
    if !last.is_empty() {
        fields.push(last);
    }
    fields.retain(|field| !field.is_empty());
    fields
}

/// Strips a trailing comment.
///
/// Only a `#` outside quotes starts one: a `cite` naming a fragment row such
/// as `foo.in#js-transcript` carries one inside its quotes, and cutting there
/// would silently truncate the evidence.
fn strip_comment(raw: &str) -> &str {
    let mut quoted = false;
    for (offset, ch) in raw.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '#' if !quoted => return raw[..offset].trim(),
            _ => {}
        }
    }
    raw.trim()
}

fn section_name(line: &str) -> Option<&str> {
    line.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

fn unquote(text: &str) -> String {
    text.strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(text)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Divergences {
        parse(text).unwrap()
    }

    #[test]
    fn an_entry_carries_its_reason_and_its_citation() {
        let text = "[divergences]\n\"a.pdf\" = { why = \"the oracle drops it\", \
                    cite = \"crbug.com/1\" }\n";
        let parsed = one(text);
        assert_eq!(parsed.len(), 1);
        let entry = parsed.get("a.pdf").unwrap();
        assert_eq!(entry.why, "the oracle drops it");
        assert_eq!(entry.cite, "crbug.com/1");
    }

    #[test]
    fn a_path_with_no_entry_is_not_excused() {
        let parsed = one("[divergences]\n\"a.pdf\" = { why = \"w\", cite = \"c\" }\n");
        assert!(parsed.get("a.pdf").is_some());
        assert_eq!(parsed.get("b.pdf"), None);
    }

    #[test]
    fn an_entry_without_a_citation_is_rejected() {
        let text = "[divergences]\n\"a.pdf\" = { why = \"the oracle drops it\" }\n";
        assert_eq!(
            parse(text),
            Err(DivergenceError::MissingEvidence {
                line: 2,
                path: "a.pdf".to_owned(),
                field: "cite",
            })
        );
    }

    #[test]
    fn an_entry_without_a_reason_is_rejected() {
        let text = "[divergences]\n\"a.pdf\" = { cite = \"crbug.com/1\" }\n";
        assert!(matches!(
            parse(text),
            Err(DivergenceError::MissingEvidence { field: "why", .. })
        ));
    }

    #[test]
    fn a_blank_citation_is_the_same_omission_as_a_missing_one() {
        let text = "[divergences]\n\"a.pdf\" = { why = \"w\", cite = \"   \" }\n";
        assert!(matches!(
            parse(text),
            Err(DivergenceError::MissingEvidence { field: "cite", .. })
        ));
    }

    #[test]
    fn a_bare_path_excuses_nothing() {
        // The shape a regression would be swept under if the file took one.
        assert!(matches!(
            parse("[divergences]\n\"a.pdf\"\n"),
            Err(DivergenceError::Malformed { line: 2, .. })
        ));
    }

    #[test]
    fn a_reason_may_hold_commas() {
        let text = "[divergences]\n\"a.pdf\" = { why = \"first, second, third\", \
                    cite = \"crbug.com/1\" }\n";
        assert_eq!(one(text).get("a.pdf").unwrap().why, "first, second, third");
    }

    #[test]
    fn a_fragment_row_keeps_its_hash() {
        let text = "[divergences]\n\"r/a.in#js-transcript\" = { why = \"w\", \
                    cite = \"c\" }  # trailing\n";
        let parsed = one(text);
        assert!(parsed.get("r/a.in#js-transcript").is_some());
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn the_same_path_may_not_be_listed_twice() {
        let text = "[divergences]\n\"a.pdf\" = { why = \"w\", cite = \"c\" }\n\
                    \"a.pdf\" = { why = \"w2\", cite = \"c2\" }\n";
        assert!(matches!(
            parse(text),
            Err(DivergenceError::Duplicate { line: 3, .. })
        ));
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let text = "[divergences]\n\"a.pdf\" = { why = \"w\", cite = \"c\", ok = \"yes\" }\n";
        assert!(matches!(
            parse(text),
            Err(DivergenceError::UnknownKey { ref key, .. }) if key == "ok"
        ));
    }

    #[test]
    fn an_unknown_section_is_an_error() {
        assert!(matches!(
            parse("[per_file]\n\"a.pdf\" = { why = \"w\", cite = \"c\" }\n"),
            Err(DivergenceError::UnknownSection { .. })
        ));
    }

    #[test]
    fn an_entry_outside_the_section_is_an_error() {
        assert!(matches!(
            parse("\"a.pdf\" = { why = \"w\", cite = \"c\" }\n"),
            Err(DivergenceError::Malformed { line: 1, .. })
        ));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "# header\n\n[divergences]\n# the empty-box gate\n\
                    \"a.pdf\" = { why = \"w\", cite = \"c\" }\n";
        assert_eq!(one(text).len(), 1);
    }

    #[test]
    fn an_empty_file_excuses_nothing() {
        assert_eq!(parse("").unwrap(), Divergences::default());
        assert_eq!(parse("[divergences]\n").unwrap().len(), 0);
    }

    #[test]
    fn the_shipped_file_parses_and_every_entry_cites_evidence() {
        let parsed = parse(include_str!("../divergences.toml")).unwrap();
        assert!(parsed.len() >= 10);
        for (path, entry) in parsed.iter() {
            assert!(!entry.why.trim().is_empty(), "{path} states no reason");
            assert!(!entry.cite.trim().is_empty(), "{path} cites nothing");
        }
    }
}
