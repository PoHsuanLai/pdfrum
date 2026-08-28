//! `conformance/scoreboard.json` — the fitness function every burn-down loop
//! optimizes (PLAN.md §5).
//!
//! Two properties matter more than anything else here:
//!
//! - **Stable ordering.** Files are sorted by id, tags are sorted, and object
//!   keys are emitted in a fixed order, so two runs over unchanged code
//!   produce byte-identical files and a real change shows up as a small diff.
//! - **Round-trippable.** `run --check-regressions <old>` reads a previous
//!   scoreboard and compares, so writing and reading must agree exactly.

use std::collections::BTreeMap;

use crate::json::Json;

/// Whether a file met its criteria.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Pass,
    Fail,
}

impl Status {
    /// The spelling used in the JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
        }
    }

    /// Parses the JSON spelling; anything unrecognized reads as a failure,
    /// which is the safe direction for the regression gate.
    pub fn from_str(text: &str) -> Status {
        if text == "pass" {
            Status::Pass
        } else {
            Status::Fail
        }
    }
}

/// Failure tags, the clustering key `triage` groups by (PLAN.md §5.3).
pub mod tag {
    /// The `pdfrum-tool` binary is missing, or lacks the subcommand asked of
    /// it. The whole-corpus M0 baseline.
    pub const UNSUPPORTED_TOOL: &str = "unsupported-tool";
    /// The tool exited by panic or signal.
    pub const CRASH: &str = "crash";
    /// The tool ran but exited nonzero.
    pub const TOOL_ERROR: &str = "tool-error";
    /// No golden exists for this file yet.
    pub const MISSING_GOLDEN: &str = "missing-golden";
    /// A Tier A dump differed byte-for-byte.
    pub const TIER_A_MISMATCH: &str = "tierA-mismatch";
    /// Page count disagreed with the oracle.
    pub const PAGE_COUNT: &str = "page-count";
    /// A rendered page scored below its SSIM floor.
    pub const PIXEL_FAIL: &str = "pixel-fail";
    /// A rendered page came out at the wrong size.
    pub const SIZE_MISMATCH: &str = "size-mismatch";
    /// A golden or candidate PNG would not decode.
    pub const BAD_PNG: &str = "bad-png";
}

/// Tier A (byte-exact) outcome for one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierA {
    /// Dumps compared, e.g. `text`, `annot`, `metadata`.
    pub compared: Vec<String>,
    /// Dumps that differed.
    pub mismatched: Vec<String>,
    /// Page count the oracle reported.
    pub golden_pages: Option<u32>,
    /// Page count our tool reported.
    pub actual_pages: Option<u32>,
}

impl TierA {
    /// True when every compared dump matched and page counts agree.
    pub fn is_clean(&self) -> bool {
        self.mismatched.is_empty() && self.golden_pages == self.actual_pages
    }
}

/// Tier B (perceptual) outcome for one file: the worst page decides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierB {
    /// Lowest mean SSIM across the file's pages.
    pub ssim: f64,
    /// True when every page matched byte-for-byte.
    pub exact: bool,
    /// Largest per-channel difference across the file's pages.
    pub max_channel_diff: u8,
    /// Pages actually compared.
    pub pages: u32,
}

impl Default for TierB {
    fn default() -> Self {
        TierB {
            ssim: 1.0,
            exact: true,
            max_channel_diff: 0,
            pages: 0,
        }
    }
}

/// One corpus file's row in the scoreboard.
#[derive(Debug, Clone, PartialEq)]
pub struct FileResult {
    /// Corpus-relative id, e.g. `corpus/fx/text/hello.pdf`.
    pub path: String,
    pub status: Status,
    /// Sorted, deduplicated failure tags; empty on a pass.
    pub tags: Vec<String>,
    pub tier_a: TierA,
    pub tier_b: Option<TierB>,
    /// Human-readable detail, one short line.
    pub notes: String,
}

/// A whole run.
#[derive(Debug, Clone, PartialEq)]
pub struct Scoreboard {
    /// RFC-3339-ish UTC stamp; informational only, never compared.
    pub generated_at: String,
    /// Rows sorted by `path`.
    pub per_file: Vec<FileResult>,
}

/// Aggregate counts, derived rather than stored so they can never drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub pass: u64,
    pub fail: u64,
    /// Failure tag to the number of files carrying it.
    pub by_tag: BTreeMap<String, u64>,
}

impl Scoreboard {
    /// Builds a scoreboard, sorting rows and tags into their canonical order.
    pub fn new(generated_at: String, mut per_file: Vec<FileResult>) -> Scoreboard {
        for result in &mut per_file {
            result.tags.sort();
            result.tags.dedup();
        }
        per_file.sort_by(|a, b| a.path.cmp(&b.path));
        Scoreboard {
            generated_at,
            per_file,
        }
    }

    /// Counts, recomputed from the rows.
    pub fn totals(&self) -> Totals {
        let mut totals = Totals {
            files: self.per_file.len() as u64,
            ..Totals::default()
        };
        for result in &self.per_file {
            match result.status {
                Status::Pass => totals.pass += 1,
                Status::Fail => totals.fail += 1,
            }
            for tag in &result.tags {
                *totals.by_tag.entry(tag.clone()).or_default() += 1;
            }
        }
        totals
    }

    /// Files that passed before and fail now — the monotone rule of PLAN.md §7.
    ///
    /// A file that vanished from the new run is *not* a regression: the corpus
    /// or the suppression list may legitimately shrink.
    pub fn regressions(previous: &Scoreboard, current: &Scoreboard) -> Vec<String> {
        let now: BTreeMap<&str, Status> = current
            .per_file
            .iter()
            .map(|r| (r.path.as_str(), r.status))
            .collect();
        previous
            .per_file
            .iter()
            .filter(|before| before.status == Status::Pass)
            .filter(|before| now.get(before.path.as_str()) == Some(&Status::Fail))
            .map(|before| before.path.clone())
            .collect()
    }

    /// Renders the scoreboard as stable JSON text.
    pub fn to_json(&self) -> Json {
        let totals = self.totals();
        Json::Obj(vec![
            ("generated_at".to_owned(), Json::str(&self.generated_at)),
            (
                "totals".to_owned(),
                Json::Obj(vec![
                    ("files".to_owned(), Json::int(totals.files)),
                    ("pass".to_owned(), Json::int(totals.pass)),
                    ("fail".to_owned(), Json::int(totals.fail)),
                    (
                        "by_tag".to_owned(),
                        Json::Obj(
                            totals
                                .by_tag
                                .iter()
                                .map(|(tag, count)| (tag.clone(), Json::int(*count)))
                                .collect(),
                        ),
                    ),
                ]),
            ),
            (
                "per_file".to_owned(),
                Json::Arr(self.per_file.iter().map(FileResult::to_json).collect()),
            ),
        ])
    }

    /// Renders to the exact text written to `scoreboard.json`.
    pub fn to_text(&self) -> String {
        self.to_json().to_pretty()
    }

    /// Parses a scoreboard back from its JSON text.
    pub fn from_text(text: &str) -> Result<Scoreboard, crate::json::JsonError> {
        let value = Json::parse(text)?;
        let per_file = value
            .get("per_file")
            .and_then(Json::as_arr)
            .map(|rows| rows.iter().filter_map(FileResult::from_json).collect())
            .unwrap_or_default();
        Ok(Scoreboard {
            generated_at: value
                .get("generated_at")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
            per_file,
        })
    }
}

impl FileResult {
    /// A row for a file the tool could not be asked about at all.
    pub fn unsupported_tool(path: String, notes: String) -> FileResult {
        FileResult {
            path,
            status: Status::Fail,
            tags: vec![tag::UNSUPPORTED_TOOL.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes,
        }
    }

    fn to_json(&self) -> Json {
        let mut fields = vec![
            ("path".to_owned(), Json::str(&self.path)),
            ("status".to_owned(), Json::str(self.status.as_str())),
            (
                "tags".to_owned(),
                Json::Arr(self.tags.iter().map(Json::str).collect()),
            ),
            (
                "tierA".to_owned(),
                Json::Obj(vec![
                    (
                        "compared".to_owned(),
                        Json::Arr(self.tier_a.compared.iter().map(Json::str).collect()),
                    ),
                    (
                        "mismatched".to_owned(),
                        Json::Arr(self.tier_a.mismatched.iter().map(Json::str).collect()),
                    ),
                    (
                        "golden_pages".to_owned(),
                        self.tier_a
                            .golden_pages
                            .map_or(Json::Null, |n| Json::int(u64::from(n))),
                    ),
                    (
                        "actual_pages".to_owned(),
                        self.tier_a
                            .actual_pages
                            .map_or(Json::Null, |n| Json::int(u64::from(n))),
                    ),
                ]),
            ),
        ];
        fields.push((
            "tierB".to_owned(),
            match self.tier_b {
                None => Json::Null,
                Some(tier_b) => Json::Obj(vec![
                    ("ssim".to_owned(), Json::Num(tier_b.ssim)),
                    ("exact".to_owned(), Json::Bool(tier_b.exact)),
                    (
                        "max_channel_diff".to_owned(),
                        Json::int(u64::from(tier_b.max_channel_diff)),
                    ),
                    ("pages".to_owned(), Json::int(u64::from(tier_b.pages))),
                ]),
            },
        ));
        fields.push(("notes".to_owned(), Json::str(&self.notes)));
        Json::Obj(fields)
    }

    fn from_json(value: &Json) -> Option<FileResult> {
        let strings = |parent: &Json, field: &str| -> Vec<String> {
            parent
                .get(field)
                .and_then(Json::as_arr)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let tier_a_json = value.get("tierA").cloned().unwrap_or(Json::Null);
        let tier_a = TierA {
            compared: strings(&tier_a_json, "compared"),
            mismatched: strings(&tier_a_json, "mismatched"),
            golden_pages: tier_a_json
                .get("golden_pages")
                .and_then(Json::as_f64)
                .and_then(crate::json::as_u32),
            actual_pages: tier_a_json
                .get("actual_pages")
                .and_then(Json::as_f64)
                .and_then(crate::json::as_u32),
        };
        let tier_b = value.get("tierB").and_then(|b| {
            Some(TierB {
                ssim: b.get("ssim")?.as_f64()?,
                exact: b.get("exact")?.as_bool()?,
                max_channel_diff: crate::json::as_u8(b.get("max_channel_diff")?.as_f64()?)?,
                pages: crate::json::as_u32(b.get("pages")?.as_f64()?)?,
            })
        });
        Some(FileResult {
            path: value.get("path")?.as_str()?.to_owned(),
            status: Status::from_str(value.get("status").and_then(Json::as_str).unwrap_or("fail")),
            tags: strings(value, "tags"),
            tier_a,
            tier_b,
            notes: value
                .get("notes")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(path: &str) -> FileResult {
        FileResult {
            path: path.to_owned(),
            status: Status::Pass,
            tags: vec![],
            tier_a: TierA {
                compared: vec!["text".to_owned(), "annot".to_owned()],
                mismatched: vec![],
                golden_pages: Some(2),
                actual_pages: Some(2),
            },
            tier_b: Some(TierB {
                ssim: 0.998_5,
                exact: false,
                max_channel_diff: 3,
                pages: 2,
            }),
            notes: String::new(),
        }
    }

    fn fail(path: &str, tags: &[&str]) -> FileResult {
        FileResult {
            path: path.to_owned(),
            status: Status::Fail,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            tier_a: TierA::default(),
            tier_b: None,
            notes: "something went wrong".to_owned(),
        }
    }

    #[test]
    fn rows_and_tags_are_sorted_into_canonical_order() {
        let board = Scoreboard::new(
            "2026-08-29T00:00:00Z".to_owned(),
            vec![
                fail("z.pdf", &["pixel-fail", "crash", "crash"]),
                pass("a.pdf"),
            ],
        );
        assert_eq!(board.per_file[0].path, "a.pdf");
        assert_eq!(board.per_file[1].tags, ["crash", "pixel-fail"]);
    }

    #[test]
    fn output_is_byte_stable_across_equal_inputs() {
        let rows = vec![fail("b.pdf", &["crash"]), pass("a.pdf")];
        let first = Scoreboard::new("t".to_owned(), rows.clone()).to_text();
        let second = Scoreboard::new("t".to_owned(), rows).to_text();
        assert_eq!(first, second);
    }

    #[test]
    fn scoreboard_round_trips_through_json() {
        let board = Scoreboard::new(
            "2026-08-29T12:00:00Z".to_owned(),
            vec![
                pass("corpus/a.pdf"),
                fail("corpus/b.pdf", &[tag::UNSUPPORTED_TOOL]),
                FileResult {
                    tier_b: Some(TierB {
                        ssim: 0.5,
                        exact: false,
                        max_channel_diff: 255,
                        pages: 1,
                    }),
                    ..fail("corpus/c.pdf", &[tag::PIXEL_FAIL])
                },
            ],
        );
        let back = Scoreboard::from_text(&board.to_text()).unwrap();
        assert_eq!(back, board);
    }

    #[test]
    fn totals_are_derived_from_the_rows() {
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![
                pass("a.pdf"),
                fail("b.pdf", &[tag::UNSUPPORTED_TOOL]),
                fail("c.pdf", &[tag::UNSUPPORTED_TOOL, tag::CRASH]),
            ],
        );
        let totals = board.totals();
        assert_eq!(totals.files, 3);
        assert_eq!(totals.pass, 1);
        assert_eq!(totals.fail, 2);
        assert_eq!(totals.by_tag[tag::UNSUPPORTED_TOOL], 2);
        assert_eq!(totals.by_tag[tag::CRASH], 1);
    }

    #[test]
    fn an_empty_scoreboard_round_trips() {
        let board = Scoreboard::new("t".to_owned(), vec![]);
        let back = Scoreboard::from_text(&board.to_text()).unwrap();
        assert_eq!(back, board);
        assert_eq!(back.totals().files, 0);
    }

    #[test]
    fn a_pass_that_now_fails_is_a_regression() {
        let before = Scoreboard::new("t0".to_owned(), vec![pass("a.pdf"), pass("b.pdf")]);
        let after = Scoreboard::new(
            "t1".to_owned(),
            vec![pass("a.pdf"), fail("b.pdf", &[tag::PIXEL_FAIL])],
        );
        assert_eq!(Scoreboard::regressions(&before, &after), ["b.pdf"]);
    }

    #[test]
    fn a_fail_that_now_passes_is_not_a_regression() {
        let before = Scoreboard::new("t0".to_owned(), vec![fail("a.pdf", &[tag::CRASH])]);
        let after = Scoreboard::new("t1".to_owned(), vec![pass("a.pdf")]);
        assert!(Scoreboard::regressions(&before, &after).is_empty());
    }

    #[test]
    fn a_newly_absent_file_is_not_a_regression() {
        // The corpus or suppression list may legitimately shrink.
        let before = Scoreboard::new("t0".to_owned(), vec![pass("gone.pdf")]);
        let after = Scoreboard::new("t1".to_owned(), vec![]);
        assert!(Scoreboard::regressions(&before, &after).is_empty());
    }

    #[test]
    fn a_newly_added_failing_file_is_not_a_regression() {
        let before = Scoreboard::new("t0".to_owned(), vec![]);
        let after = Scoreboard::new("t1".to_owned(), vec![fail("new.pdf", &[tag::CRASH])]);
        assert!(Scoreboard::regressions(&before, &after).is_empty());
    }

    #[test]
    fn every_failure_in_the_m0_baseline_is_a_regression_free_starting_point() {
        // M0: everything fails as `unsupported-tool`. Comparing that board to
        // itself must report nothing.
        let board = Scoreboard::new(
            "t".to_owned(),
            (0..5)
                .map(|i| FileResult::unsupported_tool(format!("corpus/{i}.pdf"), "no tool".into()))
                .collect(),
        );
        assert!(Scoreboard::regressions(&board, &board).is_empty());
        assert_eq!(board.totals().fail, 5);
    }

    #[test]
    fn tier_a_is_clean_only_when_dumps_match_and_pages_agree() {
        assert!(TierA::default().is_clean());
        assert!(
            !TierA {
                mismatched: vec!["text".to_owned()],
                ..TierA::default()
            }
            .is_clean()
        );
        assert!(
            !TierA {
                golden_pages: Some(2),
                actual_pages: Some(3),
                ..TierA::default()
            }
            .is_clean()
        );
    }

    #[test]
    fn an_unknown_status_string_reads_as_a_failure() {
        assert_eq!(Status::from_str("pass"), Status::Pass);
        assert_eq!(Status::from_str("fail"), Status::Fail);
        assert_eq!(Status::from_str("wat"), Status::Fail);
    }
}
