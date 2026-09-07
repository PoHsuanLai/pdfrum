//! Tier B thresholds and the ratchet rule.
//!
//! `conformance/thresholds.toml` carries a global default SSIM floor and
//! per-file overrides. makes the ratchet one-directional: **a
//! passing test's threshold may never loosen**. So an override is only honored
//! when it is at least as strict as the global floor; a looser one is rejected
//! rather than silently applied, because a loosened threshold is how a
//! regression hides.
//!
//! The format read here is the small subset the file actually uses — a
//! `[section] key = value` shape with `#` comments — parsed in-crate for the
//! same reason SSIM is hand-rolled: the ratchet must not move
//! under a dependency update.

use std::collections::BTreeMap;

/// The default SSIM floor when a file has no override.
pub const DEFAULT_SSIM: f64 = 0.99;

/// Parsed `thresholds.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Thresholds {
    /// Applies to every file without an override.
    pub global_ssim: f64,
    /// Corpus-relative path to its (tighter) floor.
    pub per_file: BTreeMap<String, f64>,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            global_ssim: DEFAULT_SSIM,
            per_file: BTreeMap::new(),
        }
    }
}

/// What went wrong reading the thresholds file.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ThresholdError {
    #[error("line {line}: expected `key = value`, found {text:?}")]
    Malformed { line: usize, text: String },
    #[error("line {line}: {value:?} is not a number")]
    NotANumber { line: usize, value: String },
    #[error("line {line}: ssim {value} is outside [0, 1]")]
    OutOfRange { line: usize, value: f64 },
    #[error(
        "line {line}: {path} sets ssim {value}, looser than the global floor \
         {global} — the ratchet only tightens"
    )]
    Loosened {
        line: usize,
        path: String,
        value: f64,
        global: f64,
    },
    #[error("line {line}: unknown section {section:?}")]
    UnknownSection { line: usize, section: String },
}

impl Thresholds {
    /// The SSIM floor a given corpus-relative path must clear.
    pub fn ssim_for(&self, path: &str) -> f64 {
        self.per_file.get(path).copied().unwrap_or(self.global_ssim)
    }
}

/// Parses `thresholds.toml`, enforcing the ratchet as it goes.
///
/// Recognized shape:
///
/// ```toml
/// [global]
/// ssim = 0.99
///
/// [per_file]
/// "corpus/fx/text/hello.pdf" = 0.995
/// ```
pub fn parse(text: &str) -> Result<Thresholds, ThresholdError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Section {
        None,
        Global,
        PerFile,
    }

    // Two passes: the global floor must be known before any override can be
    // checked against it, and TOML does not require [global] to come first.
    let mut global_ssim = DEFAULT_SSIM;
    let mut section = Section::None;
    for (index, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = section_name(line) {
            section = match name {
                "global" => Section::Global,
                "per_file" => Section::PerFile,
                other => {
                    return Err(ThresholdError::UnknownSection {
                        line: index + 1,
                        section: other.to_owned(),
                    });
                }
            };
            continue;
        }
        if section == Section::Global {
            let (key, value) = split_pair(line, index + 1)?;
            if key == "ssim" {
                global_ssim = number(&value, index + 1)?;
                check_range(global_ssim, index + 1)?;
            }
        }
    }

    let mut per_file = BTreeMap::new();
    let mut section = Section::None;
    for (index, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = section_name(line) {
            section = if name == "global" {
                Section::Global
            } else {
                Section::PerFile
            };
            continue;
        }
        if section != Section::PerFile {
            continue;
        }
        let (key, value) = split_pair(line, index + 1)?;
        let ssim = number(&value, index + 1)?;
        check_range(ssim, index + 1)?;
        if ssim < global_ssim {
            return Err(ThresholdError::Loosened {
                line: index + 1,
                path: key.clone(),
                value: ssim,
                global: global_ssim,
            });
        }
        per_file.insert(key, ssim);
    }

    Ok(Thresholds {
        global_ssim,
        per_file,
    })
}

fn strip_comment(raw: &str) -> &str {
    // Comments only start a line or follow a value; no `#` appears inside the
    // quoted paths this file holds.
    raw.split('#').next().unwrap_or("").trim()
}

fn section_name(line: &str) -> Option<&str> {
    line.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

fn split_pair(line: &str, line_no: usize) -> Result<(String, String), ThresholdError> {
    let (key, value) = line.ok_or_malformed(line_no)?;
    Ok((unquote(key.trim()), value.trim().to_owned()))
}

/// Small extension so `split_pair` reads as one expression.
trait SplitEq {
    fn ok_or_malformed(&self, line: usize) -> Result<(&str, &str), ThresholdError>;
}

impl SplitEq for str {
    fn ok_or_malformed(&self, line: usize) -> Result<(&str, &str), ThresholdError> {
        self.split_once('=')
            .ok_or_else(|| ThresholdError::Malformed {
                line,
                text: self.to_owned(),
            })
    }
}

fn unquote(text: &str) -> String {
    text.strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(text)
        .to_owned()
}

fn number(text: &str, line: usize) -> Result<f64, ThresholdError> {
    text.parse::<f64>().map_err(|_| ThresholdError::NotANumber {
        line,
        value: text.to_owned(),
    })
}

fn check_range(value: f64, line: usize) -> Result<(), ThresholdError> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(ThresholdError::OutOfRange { line, value })
    }
}

#[cfg(test)]
// Exact float equality is deliberate here: these assertions pin values that
// are exact by construction (SSIM of identical input is 1.0; a threshold
// parsed from text round-trips bit-for-bit). An epsilon would weaken them.
#[allow(clippy::float_cmp, reason = "asserting exactly-representable values")]
mod tests {
    use super::*;

    #[test]
    fn an_absent_file_uses_the_documented_default() {
        let thresholds = Thresholds::default();
        assert_eq!(thresholds.global_ssim, 0.99);
        assert_eq!(thresholds.ssim_for("anything.pdf"), 0.99);
    }

    #[test]
    fn reads_a_global_floor_and_overrides() {
        let text = "[global]\nssim = 0.99\n\n[per_file]\n\"corpus/a.pdf\" = 0.995\n";
        let thresholds = parse(text).unwrap();
        assert_eq!(thresholds.global_ssim, 0.99);
        assert_eq!(thresholds.ssim_for("corpus/a.pdf"), 0.995);
        assert_eq!(thresholds.ssim_for("corpus/b.pdf"), 0.99);
    }

    #[test]
    fn an_override_may_tighten_to_exactness() {
        let text = "[global]\nssim = 0.99\n[per_file]\n\"a.pdf\" = 1.0\n";
        assert_eq!(parse(text).unwrap().ssim_for("a.pdf"), 1.0);
    }

    #[test]
    fn an_override_equal_to_the_global_floor_is_allowed() {
        let text = "[global]\nssim = 0.99\n[per_file]\n\"a.pdf\" = 0.99\n";
        assert_eq!(parse(text).unwrap().ssim_for("a.pdf"), 0.99);
    }

    #[test]
    fn a_loosened_override_is_rejected() {
        let text = "[global]\nssim = 0.99\n[per_file]\n\"a.pdf\" = 0.90\n";
        assert!(matches!(
            parse(text),
            Err(ThresholdError::Loosened { ref path, .. }) if path == "a.pdf"
        ));
    }

    #[test]
    fn the_ratchet_holds_when_global_is_declared_after_the_overrides() {
        // Section order must not decide whether a loosened override slips by.
        let text = "[per_file]\n\"a.pdf\" = 0.90\n\n[global]\nssim = 0.99\n";
        assert!(matches!(parse(text), Err(ThresholdError::Loosened { .. })));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "# thresholds\n\n[global]\nssim = 0.99  # the floor\n\n\
                    [per_file]\n# tightened 2026-08-29\n\"a.pdf\" = 0.999\n";
        let thresholds = parse(text).unwrap();
        assert_eq!(thresholds.global_ssim, 0.99);
        assert_eq!(thresholds.ssim_for("a.pdf"), 0.999);
    }

    #[test]
    fn unquoted_keys_work_too() {
        let text = "[global]\nssim = 0.5\n[per_file]\nplain.pdf = 0.75\n";
        assert_eq!(parse(text).unwrap().ssim_for("plain.pdf"), 0.75);
    }

    #[test]
    fn out_of_range_and_non_numeric_values_are_errors() {
        assert!(matches!(
            parse("[global]\nssim = 1.5\n"),
            Err(ThresholdError::OutOfRange { .. })
        ));
        assert!(matches!(
            parse("[global]\nssim = -0.1\n"),
            Err(ThresholdError::OutOfRange { .. })
        ));
        assert!(matches!(
            parse("[global]\nssim = tight\n"),
            Err(ThresholdError::NotANumber { .. })
        ));
    }

    #[test]
    fn a_line_without_an_equals_sign_is_an_error() {
        assert!(matches!(
            parse("[global]\nssim\n"),
            Err(ThresholdError::Malformed { line: 2, .. })
        ));
    }

    #[test]
    fn an_unknown_section_is_an_error() {
        assert!(matches!(
            parse("[tier_c]\nssim = 0.99\n"),
            Err(ThresholdError::UnknownSection { .. })
        ));
    }

    #[test]
    fn an_empty_file_is_the_default() {
        assert_eq!(parse("").unwrap(), Thresholds::default());
    }

    #[test]
    fn the_shipped_file_parses_and_ratchets() {
        let text = include_str!("../thresholds.toml");
        let thresholds = parse(text).unwrap();
        assert_eq!(thresholds.global_ssim, DEFAULT_SSIM);
        for (path, ssim) in &thresholds.per_file {
            assert!(
                *ssim >= thresholds.global_ssim,
                "{path} loosens the global floor"
            );
        }
    }
}
