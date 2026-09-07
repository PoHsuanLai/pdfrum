//! `conformance triage` — clusters scoreboard failures by tag.
//!
//! Each cluster is the unit of work a burn-down agent picks up:
//! one tag, a count, and a few example files to open. Clusters are ordered by
//! size, with the tag name breaking ties so the report is stable between runs
//! over the same scoreboard.

use std::fmt::Write as _;

use crate::json::Json;
use crate::scoreboard::{Scoreboard, Status};

/// How many example files each cluster shows.
pub const EXAMPLES: usize = 3;

/// One failure cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub tag: String,
    /// Files carrying this tag.
    pub count: u64,
    /// The first few, in scoreboard (path) order.
    pub examples: Vec<String>,
}

/// Clusters a scoreboard's failures, largest first.
///
/// A `tierA-mismatch` is split by *which* dump differed, because the tag
/// alone is not a unit of work: at it covered every file in the corpus
/// while the metadata and pageinfo dumps were already byte-exact everywhere
/// and the mismatches were entirely the text and annot dumps no crate
/// produces yet. One cluster per dump makes that visible, and makes a
/// burn-down agent's assignment ("the annot dump") readable off the report.
pub fn cluster(board: &Scoreboard) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    let record = |tag: String, path: &str, clusters: &mut Vec<Cluster>| match clusters
        .iter_mut()
        .find(|c| c.tag == tag)
    {
        Some(cluster) => {
            cluster.count += 1;
            if cluster.examples.len() < EXAMPLES {
                cluster.examples.push(path.to_owned());
            }
        }
        None => clusters.push(Cluster {
            tag,
            count: 1,
            examples: vec![path.to_owned()],
        }),
    };
    for result in &board.per_file {
        if result.status != Status::Fail {
            continue;
        }
        for tag in &result.tags {
            if tag == crate::scoreboard::tag::TIER_A_MISMATCH {
                let mut classes: Vec<&'static str> = result
                    .tier_a
                    .mismatched
                    .iter()
                    .map(|name| artifact_class(name))
                    .collect();
                classes.sort_unstable();
                classes.dedup();
                for class in classes {
                    record(format!("{tag}:{class}"), &result.path, &mut clusters);
                }
                continue;
            }
            record(tag.clone(), &result.path, &mut clusters);
        }
    }
    // Largest cluster first; alphabetical by tag on a tie, so equal-sized
    // clusters do not swap places between runs.
    clusters.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    clusters
}

/// Which dump an artifact name belongs to.
///
/// Deliberately a suffix test and not `Path::extension`: the names are
/// multi-part (`input.pdf.0.annot.txt`), so an extension lookup sees only
/// `txt` and cannot tell an annot dump from a text one — which is the single
/// distinction this function exists to draw.
fn artifact_class(name: &str) -> &'static str {
    use crate::generate::has_suffix;
    match name {
        "metadata.txt" => "metadata",
        "pageinfo.txt" => "pageinfo",
        "structure.txt" => "structure",
        _ if has_suffix(name, ".annot.txt") => "annot",
        _ if has_suffix(name, ".txt") => "text",
        _ if has_suffix(name, ".png") => "png",
        _ => "other",
    }
}

/// Renders clusters as the human report.
pub fn render(board: &Scoreboard, clusters: &[Cluster]) -> String {
    let totals = board.totals();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "conformance triage - {} files, {} pass, {} fail (generated {})",
        totals.files, totals.pass, totals.fail, board.generated_at
    );
    if let (Some(rate), Some(nonempty)) = (totals.text_rate(), totals.text_nonempty_rate()) {
        // Both rates, because the first one flatters a tool that extracts
        // nothing: slightly under half the corpus's text goldens are empty.
        let _ = writeln!(
            out,
            "text {}/{} ({:.1}%), nonempty {}/{} ({:.1}%)",
            totals.text.matched,
            totals.text.pages,
            rate * 100.0,
            totals.text.substantive_matched,
            totals.text.substantive,
            nonempty * 100.0,
        );
    }
    if clusters.is_empty() {
        out.push_str("\nno failures\n");
        return out;
    }
    let _ = writeln!(out, "\ntop {} failure clusters:", clusters.len());
    for cluster in clusters {
        let _ = writeln!(out, "\n  {:<20} {:>6}", cluster.tag, cluster.count);
        for example in &cluster.examples {
            let _ = writeln!(out, "      {example}");
        }
        let shown = cluster.examples.len() as u64;
        if cluster.count > shown {
            let _ = writeln!(out, "      ... and {} more", cluster.count - shown);
        }
    }
    out
}

/// Renders clusters as JSON for machine consumers.
pub fn to_json(board: &Scoreboard, clusters: &[Cluster]) -> Json {
    let totals = board.totals();
    Json::Obj(vec![
        ("generated_at".to_owned(), Json::str(&board.generated_at)),
        (
            "totals".to_owned(),
            Json::Obj(vec![
                ("files".to_owned(), Json::int(totals.files)),
                ("pass".to_owned(), Json::int(totals.pass)),
                ("fail".to_owned(), Json::int(totals.fail)),
            ]),
        ),
        (
            "clusters".to_owned(),
            Json::Arr(
                clusters
                    .iter()
                    .map(|cluster| {
                        Json::Obj(vec![
                            ("tag".to_owned(), Json::str(&cluster.tag)),
                            ("count".to_owned(), Json::int(cluster.count)),
                            (
                                "examples".to_owned(),
                                Json::Arr(cluster.examples.iter().map(Json::str).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoreboard::{FileResult, TierA, tag};

    fn fail(path: &str, tags: &[&str]) -> FileResult {
        FileResult {
            path: path.to_owned(),
            status: Status::Fail,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            tier_a: TierA::default(),
            tier_b: None,
            notes: String::new(),
        }
    }

    fn pass(path: &str) -> FileResult {
        FileResult {
            status: Status::Pass,
            tags: vec![],
            ..fail(path, &[])
        }
    }

    #[test]
    fn clusters_are_ordered_largest_first() {
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![
                fail("a.pdf", &[tag::CRASH]),
                fail("b.pdf", &[tag::PIXEL_FAIL]),
                fail("c.pdf", &[tag::PIXEL_FAIL]),
                fail("d.pdf", &[tag::PIXEL_FAIL]),
            ],
        );
        let clusters = cluster(&board);
        assert_eq!(clusters[0].tag, tag::PIXEL_FAIL);
        assert_eq!(clusters[0].count, 3);
        assert_eq!(clusters[1].tag, tag::CRASH);
        assert_eq!(clusters[1].count, 1);
    }

    #[test]
    fn form_events_failures_cluster_under_their_own_tag() {
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![
                fail("a.pdf#form-events", &[tag::FORM_EVENTS]),
                fail("b.pdf#form-events", &[tag::FORM_EVENTS]),
                fail("c.pdf", &[tag::PIXEL_FAIL]),
            ],
        );
        let clusters = cluster(&board);
        let form = clusters.iter().find(|c| c.tag == tag::FORM_EVENTS).unwrap();
        assert_eq!(form.count, 2);
        assert_eq!(
            form.examples,
            [
                "a.pdf#form-events".to_owned(),
                "b.pdf#form-events".to_owned()
            ]
        );
    }

    #[test]
    fn equal_sized_clusters_break_ties_alphabetically() {
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![fail("a.pdf", &["zeta"]), fail("b.pdf", &["alpha"])],
        );
        let clusters = cluster(&board);
        assert_eq!(clusters[0].tag, "alpha");
        assert_eq!(clusters[1].tag, "zeta");
    }

    #[test]
    fn a_file_with_several_tags_counts_in_each_cluster() {
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![fail("a.pdf", &[tag::CRASH, tag::PAGE_COUNT])],
        );
        let clusters = cluster(&board);
        assert_eq!(clusters.len(), 2);
        assert!(clusters.iter().all(|c| c.count == 1));
    }

    #[test]
    fn at_most_three_examples_are_kept_in_path_order() {
        let board = Scoreboard::new(
            "t".to_owned(),
            (0..10)
                .map(|i| fail(&format!("f{i}.pdf"), &[tag::UNSUPPORTED_TOOL]))
                .collect(),
        );
        let clusters = cluster(&board);
        assert_eq!(clusters[0].count, 10);
        assert_eq!(clusters[0].examples, ["f0.pdf", "f1.pdf", "f2.pdf"]);
    }

    #[test]
    fn passing_files_are_not_clustered() {
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![pass("a.pdf"), pass("b.pdf"), fail("c.pdf", &[tag::CRASH])],
        );
        let clusters = cluster(&board);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].examples, ["c.pdf"]);
    }

    #[test]
    fn an_all_green_board_clusters_to_nothing() {
        let board = Scoreboard::new("t".to_owned(), vec![pass("a.pdf")]);
        assert!(cluster(&board).is_empty());
        assert!(render(&board, &[]).contains("no failures"));
    }

    #[test]
    fn the_m0_baseline_reports_one_cluster_holding_everything() {
        let board = Scoreboard::new(
            "t".to_owned(),
            (0..1400)
                .map(|i| FileResult::unsupported_tool(format!("corpus/{i:04}.pdf"), "stub".into()))
                .collect(),
        );
        let clusters = cluster(&board);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].tag, tag::UNSUPPORTED_TOOL);
        assert_eq!(clusters[0].count, 1400);
        let report = render(&board, &clusters);
        assert!(report.contains("1400"));
        assert!(report.contains("and 1397 more"));
    }

    #[test]
    fn a_tier_a_mismatch_becomes_one_cluster_per_dump_that_differed() {
        let mismatching = |path: &str, dumps: &[&str]| FileResult {
            tier_a: TierA {
                mismatched: dumps.iter().map(|d| (*d).to_owned()).collect(),
                ..TierA::default()
            },
            ..fail(path, &[tag::TIER_A_MISMATCH])
        };
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![
                mismatching("a.pdf", &["input.pdf.0.txt", "input.pdf.0.annot.txt"]),
                mismatching("b.pdf", &["input.pdf.0.annot.txt"]),
                mismatching("c.pdf", &["metadata.txt"]),
            ],
        );
        let clusters = cluster(&board);
        let by_tag: Vec<(&str, u64)> = clusters.iter().map(|c| (c.tag.as_str(), c.count)).collect();
        assert_eq!(
            by_tag,
            [
                ("tierA-mismatch:annot", 2),
                ("tierA-mismatch:metadata", 1),
                ("tierA-mismatch:text", 1),
            ]
        );
        // The undifferentiated tag no longer appears: it was never a unit of
        // work, only a count of everything at once.
        assert!(!clusters.iter().any(|c| c.tag == tag::TIER_A_MISMATCH));
    }

    #[test]
    fn several_pages_of_one_dump_still_count_their_file_once() {
        let board = Scoreboard::new(
            "t".to_owned(),
            vec![FileResult {
                tier_a: TierA {
                    mismatched: vec![
                        "input.pdf.0.txt".to_owned(),
                        "input.pdf.1.txt".to_owned(),
                        "input.pdf.2.txt".to_owned(),
                    ],
                    ..TierA::default()
                },
                ..fail("a.pdf", &[tag::TIER_A_MISMATCH])
            }],
        );
        let clusters = cluster(&board);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].tag, "tierA-mismatch:text");
        assert_eq!(clusters[0].count, 1);
    }

    #[test]
    fn artifact_names_are_classified_by_their_full_suffix() {
        assert_eq!(artifact_class("input.pdf.0.txt"), "text");
        assert_eq!(artifact_class("input.pdf.0.annot.txt"), "annot");
        assert_eq!(artifact_class("metadata.txt"), "metadata");
        assert_eq!(artifact_class("pageinfo.txt"), "pageinfo");
        assert_eq!(artifact_class("structure.txt"), "structure");
        assert_eq!(artifact_class("input.pdf.0.png"), "png");
        assert_eq!(artifact_class("input.pdf.attachment.x"), "other");
    }

    #[test]
    fn the_json_report_carries_counts_and_examples() {
        let board = Scoreboard::new(
            "when".to_owned(),
            vec![fail("a.pdf", &[tag::CRASH]), fail("b.pdf", &[tag::CRASH])],
        );
        let value = to_json(&board, &cluster(&board));
        assert_eq!(value.get("generated_at").unwrap().as_str(), Some("when"));
        let clusters = value.get("clusters").unwrap().as_arr().unwrap();
        assert_eq!(clusters[0].get("count").unwrap().as_f64(), Some(2.0));
        assert_eq!(
            clusters[0].get("examples").unwrap().as_arr().unwrap().len(),
            2
        );
        // The report must itself round-trip as JSON.
        assert!(Json::parse(&value.to_pretty()).is_ok());
    }
}
