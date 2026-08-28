//! `conformance triage` — clusters scoreboard failures by tag.
//!
//! Each cluster is the unit of work a burn-down agent picks up (PLAN.md §7):
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
pub fn cluster(board: &Scoreboard) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    for result in &board.per_file {
        if result.status != Status::Fail {
            continue;
        }
        for tag in &result.tags {
            match clusters.iter_mut().find(|c| &c.tag == tag) {
                Some(cluster) => {
                    cluster.count += 1;
                    if cluster.examples.len() < EXAMPLES {
                        cluster.examples.push(result.path.clone());
                    }
                }
                None => clusters.push(Cluster {
                    tag: tag.clone(),
                    count: 1,
                    examples: vec![result.path.clone()],
                }),
            }
        }
    }
    // Largest cluster first; alphabetical by tag on a tie, so equal-sized
    // clusters do not swap places between runs.
    clusters.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    clusters
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
