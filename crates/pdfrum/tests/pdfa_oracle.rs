//! veraPDF as the oracle for `check_pdfa`, exactly as `pdfium_test` is the
//! conformance board's for rendering.
//!
//! # Why this file exists
//!
//! A PDF/A checker is a claim about a specification with dozens of
//! independent requirements, and the failure mode is claiming conformance you
//! do not have. The only thing that turns the claim into a result is scoring
//! it against an implementation nobody here wrote. veraPDF is the reference
//! implementation of ISO 19005, it is the one archives actually run, and it
//! disagreeing with us is information either way.
//!
//! The deliverable is therefore **the divergence list**, not agreement. The
//! checker has not been tuned to agree — a checker that agrees with its
//! oracle by construction proves nothing — and `docs/design/pdfa.md` carries
//! every disagreement adjudicated as ours right, theirs right, or not
//! determined.
//!
//! # Running it
//!
//! veraPDF is a **Java** tool and is not installed by any of this
//! repository's scripts. `$PDFRUM_VERAPDF` names the `verapdf` launcher;
//! DEPS.md's tools table says how to install it and why it is the only
//! dependency in this workspace that needs a JVM.
//!
//! Absence is handled as a missing *input*, the way `$PDFRUM_GOLDENS` is in
//! `pdfrum-svg`'s round trip:
//!
//! - **Not set, or naming nothing**: the oracle half skips with a note. The
//!   self-consistency half below still runs over all 44 corpus files.
//! - **Set and naming a launcher that then produces nothing usable**: fail.
//!   That is a broken oracle, and a broken oracle that skips silently is a
//!   false green — which is the trap this repository has hit before and the
//!   reason the two cases are kept apart.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use pdfrum::{Document, PdfaClause, PdfaLevel};
use pdfrum_corpus::CORPUS;

/// The veraPDF launcher, when one is configured and present.
///
/// `$PDFRUM_VERAPDF` only. There is no in-repo default the way there is for
/// the goldens, because veraPDF installs outside the tree by design (DEPS.md)
/// and guessing at `~/verapdf/verapdf` would make one developer's layout a
/// silent requirement for everyone else.
fn verapdf() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("PDFRUM_VERAPDF")?);
    path.is_file().then_some(path)
}

/// The ISO clause numbers veraPDF failed a file on, at one flavour.
///
/// The full report is a large JSON document, and the only field this
/// comparison needs is the set of `"clause"` values on the rules that failed.
/// Scanning for them costs no dependency; a JSON crate to read one string
/// field would be exactly the case STYLE.md §5 says to write instead.
///
/// [`Verdict::Unparsable`] is veraPDF declining the file, and `None` is
/// veraPDF producing nothing at all — a broken oracle. The two are separate
/// because only the second is a defect in the setup.
enum Verdict {
    /// The clauses veraPDF failed the file on.
    Clauses(BTreeSet<String>),
    /// veraPDF could not parse the file, so it has no opinion to compare
    /// against. Our parser opens all three such corpus files; see
    /// `docs/design/pdfa.md` §6.
    Unparsable,
}

fn oracle_clauses(tool: &PathBuf, file: &PathBuf, flavour: &str) -> Option<Verdict> {
    let output = Command::new(tool)
        .arg("-f")
        .arg(flavour)
        .arg("--format")
        .arg("json")
        .arg(file)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // veraPDF ran and declined the file. The warning goes to stderr while the
    // report envelope still comes out on stdout, so both streams are consulted
    // — and this stays distinguishable from the tool not running at all.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let declined = |s: &str| s.contains("doesn't appear to be a valid PDF");
    if declined(&text) || declined(&stderr) {
        return Some(Verdict::Unparsable);
    }
    if !text.contains("\"validationResult\"") {
        return None;
    }

    // A failed rule is a `"ruleStatus" : "FAILED"` object carrying a
    // `"clause"`. The two keys are in the same object and `clause` precedes
    // `ruleStatus` in veraPDF's output, so pair them by scanning forward from
    // each clause to the next status.
    let mut clauses = BTreeSet::new();
    for (index, _) in text.match_indices("\"clause\"") {
        let Some(rest) = text.get(index..) else {
            continue;
        };
        let Some(value) = between(rest, "\"clause\" : \"", "\"") else {
            continue;
        };
        // The rule's status is the next `"ruleStatus"` *before* the following
        // clause, so a passed rule's clause is not counted.
        let next_clause = rest
            .get("\"clause\"".len()..)
            .and_then(|r| r.find("\"clause\""));
        let status = rest.find("\"status\" : \"failed\"");
        let counts = match (status, next_clause) {
            (Some(s), Some(n)) => s < n + "\"clause\"".len(),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if counts {
            clauses.insert(value.to_owned());
        }
    }
    Some(Verdict::Clauses(clauses))
}

/// The text between two delimiters, starting at the first.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let rest = text.get(start..)?;
    rest.get(..rest.find(close)?)
}

/// Our clauses, spelled as the ISO numbers veraPDF uses, so the two sets are
/// directly comparable.
///
/// Not every clause maps: veraPDF enforces requirements this checker does not
/// implement (CMap embedding, the `PDPage` group rules), and those show up as
/// "theirs only" in the divergence table rather than being quietly dropped.
fn our_clauses(report: &pdfrum::PdfaReport, level: PdfaLevel) -> BTreeSet<String> {
    report
        .clauses()
        .into_iter()
        .filter_map(|clause| {
            // `Clause::iso` returns "ISO 19005-1:2005, 6.3.4"; veraPDF's
            // field is the bare "6.3.4".
            clause.iso(level).split(", ").nth(1).map(str::to_owned)
        })
        .collect()
}

/// Clause numbers that name the *same requirement* in veraPDF's rule set
/// under a different number than `Clause::iso` cites.
///
/// These are attribution differences, not disagreements about the file. The
/// one that shows up on this corpus:
///
/// - **`6.5.1` vs `6.4.1` (A-2).** A JavaScript action on a form field breaks
///   both "no JavaScript action" (6.5.1) and "a Field dictionary shall not
///   contain the A or AA keys" (6.4.1). veraPDF's A-2 profile reports it under
///   6.4.1; we report the action, under 6.5.1. `forms_number` genuinely
///   carries 174 `/S/JavaScript` actions, so both sides are right about the
///   file and differ only on which clause to cite.
///
/// Listing them here rather than renumbering the clause is deliberate: the
/// citation `Clause::iso` returns should be the one that describes what we
/// actually found, and an alias table is honest about the mapping in a way
/// that silently adopting veraPDF's numbering would not be.
fn aliases(clause: &str) -> &'static [&'static str] {
    match clause {
        "6.5.1" => &["6.4.1"],
        _ => &[],
    }
}

/// The checker runs on every corpus file and never panics.
///
/// The half that does not need the oracle. It is deliberately separate: a
/// missing veraPDF must not take this with it, or the whole file becomes a
/// silent no-op on a machine without a JVM.
#[test]
fn every_corpus_file_is_checked_at_both_levels() {
    let mut checked = 0;
    for doc_entry in CORPUS {
        let stem = doc_entry.stem;
        let Ok(doc) = Document::open(pdfrum_corpus::path(stem)) else {
            continue;
        };
        for level in [PdfaLevel::A1b, PdfaLevel::A2b] {
            let report = doc.check_pdfa(level);
            assert_eq!(report.level, level, "{stem}: the report names its level");
            // Not one corpus document is a PDF/A file — they are rendering
            // fixtures — so every one must fail something.
            assert!(
                !report.conforms(),
                "{stem} at {level}: no corpus file is PDF/A, so none may pass"
            );
            // No corpus file carries an XMP identification schema, and that
            // requirement is unconditional at both levels — unlike the output
            // intent, whose absence only matters once device colour is found
            // (see `check_output_intent`). So this is the clause every file
            // must fail, and a file passing it would mean the checks did not
            // run.
            assert!(
                report
                    .clauses()
                    .contains(&PdfaClause::XmpIdentificationMissing)
                    || report.clauses().contains(&PdfaClause::XmpMissing),
                "{stem} at {level}: no corpus file identifies itself as PDF/A"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, CORPUS.len(), "every corpus file was checked");
}

/// A-1 is strictly stricter than A-2 on the requirements where they differ.
///
/// Transparency, optional content, embedded files and JPEG 2000 are forbidden
/// under A-1 and permitted under A-2, and nothing is forbidden under A-2 that
/// A-1 allows. So for every file, A-2's clause set is a subset of A-1's on
/// those four — a property that holds without an oracle and that catches a
/// level plumbed through to the wrong check.
#[test]
fn a1_is_stricter_than_a2_where_they_differ() {
    let stricter = [
        PdfaClause::Transparency,
        PdfaClause::OptionalContent,
        PdfaClause::EmbeddedFile,
    ];
    for doc_entry in CORPUS {
        let stem = doc_entry.stem;
        let Ok(doc) = Document::open(pdfrum_corpus::path(stem)) else {
            continue;
        };
        let a1 = doc.check_pdfa(PdfaLevel::A1b);
        let a2 = doc.check_pdfa(PdfaLevel::A2b);
        for clause in stricter {
            assert!(
                a1.by_clause(clause).count() >= a2.by_clause(clause).count(),
                "{stem}: {clause:?} must not fire more often under A-2 than A-1"
            );
        }
    }
}

/// Score every check against veraPDF over the corpus, and print the
/// divergence table.
///
/// This does **not** assert agreement. The checker is deliberately a subset
/// of ISO 19005 (`docs/design/pdfa.md` lists what it does not cover), so
/// veraPDF legitimately reports clauses we do not, and asserting equality
/// would either be false or would force the checker to claim coverage it does
/// not have.
///
/// What it does assert is the one direction that would be a *defect*: a
/// clause we report that veraPDF does not see on the same file at the same
/// flavour. That is us inventing a violation, and it is the failure that
/// matters, because a checker that over-reports makes conforming files look
/// broken.
#[test]
fn scored_against_verapdf() {
    let Some(tool) = verapdf() else {
        println!(
            "\nNOTE: $PDFRUM_VERAPDF is unset or names no file, so the oracle \
             half did not run. The other two tests in this file still checked \
             all {} corpus files at both levels. veraPDF is a Java tool; \
             DEPS.md says how to install it.",
            CORPUS.len()
        );
        return;
    };

    let mut scored = 0;
    let mut agreed = 0;
    let mut ours_only: Vec<String> = Vec::new();
    let mut theirs_only: BTreeSet<String> = BTreeSet::new();
    let mut broken: Vec<String> = Vec::new();
    let mut unparsable: Vec<String> = Vec::new();

    for doc_entry in CORPUS {
        let stem = doc_entry.stem;
        let path = pdfrum_corpus::path(stem);
        let Ok(doc) = Document::open(&path) else {
            continue;
        };
        for (level, flavour) in [(PdfaLevel::A1b, "1b"), (PdfaLevel::A2b, "2b")] {
            let theirs = match oracle_clauses(&tool, &path, flavour) {
                None => {
                    broken.push(format!("{stem} at {flavour}"));
                    continue;
                }
                // veraPDF declined the file, so there is no verdict to score
                // against. Counted and named rather than passed over.
                Some(Verdict::Unparsable) => {
                    unparsable.push(format!("{stem} at {flavour}"));
                    continue;
                }
                Some(Verdict::Clauses(clauses)) => clauses,
            };
            scored += 1;
            let ours = our_clauses(&doc.check_pdfa(level), level);

            // A clause of ours is covered when veraPDF reports it, or reports
            // one of the aliases that names the same requirement.
            let covered = |clause: &String| {
                theirs.contains(clause) || aliases(clause).iter().any(|a| theirs.contains(*a))
            };
            let uncovered: Vec<&String> = ours.iter().filter(|c| !covered(c)).collect();
            for clause in &uncovered {
                ours_only.push(format!("{stem} {flavour}: {clause}"));
            }
            for clause in theirs.difference(&ours) {
                theirs_only.insert(format!("{flavour} {clause}"));
            }
            if uncovered.is_empty() {
                agreed += 1;
            }
        }
    }

    println!("\nveraPDF divergence, {scored} file-level pairs scored");
    if !unparsable.is_empty() {
        println!(
            "  {} pairs veraPDF declined to parse (our parser opens all of \
             them): {unparsable:?}",
            unparsable.len()
        );
    }
    println!("  {agreed} where every clause we report, veraPDF also reports");
    println!(
        "  {} clauses reported by us and not by veraPDF",
        ours_only.len()
    );
    println!(
        "  {} distinct clauses veraPDF reports that we do not implement",
        theirs_only.len()
    );
    for clause in &theirs_only {
        println!("    theirs only: {clause}");
    }
    for entry in &ours_only {
        println!("    OURS ONLY:   {entry}");
    }

    // A broken oracle is a failure, never a skip: `$PDFRUM_VERAPDF` was set,
    // so the operator asked for this half to run, and letting it pass
    // vacuously is exactly the false green this file's docs warn about.
    assert!(
        broken.is_empty(),
        "$PDFRUM_VERAPDF is set but produced no usable report for: {broken:?}"
    );
    assert!(
        scored > 0,
        "$PDFRUM_VERAPDF is set, so the oracle must have scored something"
    );

    // The one direction that is a defect, minus the cases adjudicated in
    // `docs/design/pdfa.md` §6. Everything else on our side alone still fails,
    // and the adjudicated total is pinned so it cannot grow silently.
    //
    // - **6.3.4 / 6.2.11.4.1, font embedding** — *theirs right*. Both parts
    //   scope the rule to fonts "used within" the file. We have no
    //   interpreter and report every unembedded font in `/Resources`, so a
    //   listed-but-unused one is an over-report on our side.
    // - **6.1.3, JPEG 2000 under A-1** — *not determined*. veraPDF's A-1
    //   profile has no JPXDecode rule at all, so it cannot confirm or deny.
    //   A-1's PDF 1.4 base does not define JPEG 2000, which is why the check
    //   stays; without an oracle opinion it is unadjudicated rather than
    //   wrong.
    // - **6.6.1, JavaScript under A-1** — *ours right*. `forms_number`
    //   carries 174 `/S/JavaScript` actions, which 6.6.1 forbids by name.
    //   veraPDF's A-1 profile does not report them.
    let adjudicated = ["6.3.4", "6.2.11.4.1", "6.1.3", "6.6.1"];
    let (known, other): (Vec<&String>, Vec<&String>) = ours_only
        .iter()
        .partition(|entry| adjudicated.iter().any(|c| entry.ends_with(c)));

    assert!(
        other.is_empty(),
        "we report clauses veraPDF does not see on the same file, which means \
         we are inventing violations: {other:#?}"
    );
    assert_eq!(
        known.len(),
        8,
        "the adjudicated divergences are the eight `docs/design/pdfa.md` §6 \
         records; the set changed: {known:#?}"
    );
}
