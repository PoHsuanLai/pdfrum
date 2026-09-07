//! veraPDF as the oracle for `to_pdfa`, and the properties that hold without
//! it.
//!
//! # Why the bar here is higher than the checker's
//!
//! `pdfa_oracle.rs` asserts a *direction*: no clause we report that veraPDF
//! does not see. It cannot assert agreement, because the checker is a
//! deliberate subset of ISO 19005.
//!
//! A conversion has no such excuse. Its output either passes veraPDF or it
//! does not, and that verdict is the whole deliverable — the roadmap's exit
//! criterion is "`to_pdfa(A2b)` produces files veraPDF passes for the inputs
//! it accepts". So the scored test below converts the corpus and counts what
//! passes, with the count pinned: it may go up freely, and it may not go down
//! without someone editing the floor.
//!
//! # Running it
//!
//! `$PDFRUM_VERAPDF` names the launcher, exactly as in `pdfa_oracle.rs` and
//! with the same two cases kept apart:
//!
//! - **Not set, or naming nothing**: the scored test skips with a note. The
//!   unscored tests below still convert all 44 corpus files.
//! - **Set and naming a launcher that then produces nothing usable**: fail.
//!   A broken oracle that skips silently is a false green.
//!
//! One invocation validates the whole directory, so this costs one JVM start
//! rather than 44 — which is why the scored test is a single veraPDF call.

// An integration test is its own crate, so the `allow-*-in-tests` settings in
// `clippy.toml` — which cover `#[cfg(test)]` modules — do not reach it. The
// same allow the other test files in this directory carry.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use pdfrum::{Document, PdfaConversion, PdfaLevel, PdfaPolicy};
use pdfrum_corpus::CORPUS;

/// How many corpus files veraPDF passes at A-2b after conversion.
///
/// Measured, not chosen: the run in §9 — 10 when the
/// conversion landed, 11 once the CMYK output intent joined it. The floor exists
/// so the number cannot quietly regress — a change that converts fewer files
/// fails here rather than being noticed a milestone later.
///
/// Before conversion the same corpus passes **zero**, which is the other half
/// of the claim and is asserted alongside it.
const A2B_PASS_FLOOR: usize = 11;

/// The veraPDF launcher, when one is configured and present.
fn verapdf() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("PDFRUM_VERAPDF")?);
    path.is_file().then_some(path)
}

/// How many of the PDFs in `dir` veraPDF finds compliant at `flavour`.
///
/// `None` is a broken oracle — veraPDF produced no report envelope at all —
/// and the caller fails on it rather than skipping.
fn compliant_count(tool: &Path, dir: &Path, flavour: &str) -> Option<usize> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "pdf"))
        .collect();
    files.sort();
    if files.is_empty() {
        return None;
    }

    let output = Command::new(tool)
        .arg("-f")
        .arg(flavour)
        .arg("--format")
        .arg("json")
        .args(&files)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !text.contains("\"validationResult\"") {
        return None;
    }
    // `"compliant" : true` appears once per validated file, and only in a
    // validation result. Counting the literal costs no JSON dependency, which
    // is the same call `pdfa_oracle.rs` makes for the same reason — and a file veraPDF declines produces no such key at all, so it
    // counts as not passing, which is the honest reading.
    Some(text.matches("\"compliant\" : true").count())
}

/// Convert every corpus file into `dir`, returning what each conversion said.
fn convert_corpus(
    dir: &Path,
    level: PdfaLevel,
    policy: PdfaPolicy,
) -> Vec<(&'static str, PdfaConversion)> {
    std::fs::create_dir_all(dir).expect("the scratch directory is writable");
    let mut out = Vec::new();
    for entry in CORPUS {
        let stem = entry.stem;
        let Ok(doc) = Document::open(pdfrum_corpus::path(stem)) else {
            continue;
        };
        let dest = dir.join(format!("{stem}.pdf"));
        let conversion = doc
            .to_pdfa(&dest, level, &policy)
            .unwrap_or_else(|e| panic!("{stem}: converting must not error, got {e}"));
        out.push((stem, conversion));
    }
    out
}

/// A scratch directory under the target directory, so nothing is written into
/// the repository and a second run does not read the first one's output.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfrum-pdfa-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// **The deliverable.** Convert the corpus to A-2b and score the output.
///
/// The claim this test makes is the roadmap's exit criterion, and it is made
/// as a *difference*: the same 44 files before and after, through the same
/// veraPDF at the same flavour. A conversion that did not move the verdict
/// would pass a "the output is valid" test written carelessly and fail this
/// one.
#[test]
fn converted_files_pass_verapdf_that_did_not_before() {
    let Some(tool) = verapdf() else {
        println!(
            "skipping: $PDFRUM_VERAPDF is unset or names no file. \
             The conversion tests below still run on all {} corpus files.",
            CORPUS.len()
        );
        return;
    };

    // Before: the corpus as it stands. Not one file is PDF/A — they are
    // rendering fixtures — so this is expected to be zero, and asserting it
    // is what makes the after-number a *conversion* result rather than a
    // statement about the corpus.
    let before_dir = scratch("before");
    std::fs::create_dir_all(&before_dir).expect("the scratch directory is writable");
    for entry in CORPUS {
        let source = pdfrum_corpus::path(entry.stem);
        let dest = before_dir.join(format!("{}.pdf", entry.stem));
        std::fs::copy(&source, &dest).expect("the corpus is readable");
    }
    let before = compliant_count(&tool, &before_dir, "2b")
        .expect("$PDFRUM_VERAPDF is set, so a broken oracle is a failure, not a skip");

    let after_dir = scratch("after");
    let conversions = convert_corpus(&after_dir, PdfaLevel::A2b, PdfaPolicy::lossy());
    let after = compliant_count(&tool, &after_dir, "2b")
        .expect("$PDFRUM_VERAPDF is set, so a broken oracle is a failure, not a skip");

    println!(
        "veraPDF A-2b over {} corpus files: {before} passed before conversion, {after} after",
        conversions.len()
    );

    assert_eq!(
        before, 0,
        "no corpus file is PDF/A to begin with; if this changes the corpus changed, \
         and the after-number stops meaning what it says"
    );
    assert!(
        after >= A2B_PASS_FLOOR,
        "conversion produced {after} files veraPDF passes, below the recorded floor of \
         {A2B_PASS_FLOOR}. Raising the floor when the number improves is intended; \
         a drop is a regression."
    );

    let _ = std::fs::remove_dir_all(&before_dir);
    let _ = std::fs::remove_dir_all(&after_dir);
}

/// Every corpus file converts, and the output re-opens and re-checks cleanly.
///
/// The half that needs no oracle. A conversion that produced a file our own
/// parser could not open would be a defect no amount of veraPDF passing would
/// excuse, and this is the test that would catch it.
#[test]
fn every_corpus_file_converts_and_the_output_reopens() {
    let dir = scratch("reopen");
    let conversions = convert_corpus(&dir, PdfaLevel::A2b, PdfaPolicy::lossy());
    assert_eq!(
        conversions.len(),
        CORPUS.len(),
        "every corpus file was converted"
    );

    for (stem, conversion) in &conversions {
        assert!(
            conversion.converted(),
            "{stem}: the lossy policy authorizes every compromise, so nothing may refuse: {conversion}"
        );
        assert_eq!(conversion.level, PdfaLevel::A2b);
        // A-2b permits transparency, so no page may be rasterized at this
        // level even once that repair exists.
        assert!(
            conversion.rasterized_pages().is_empty(),
            "{stem}: A-2b permits transparency, so nothing needs rasterizing"
        );

        let path = dir.join(format!("{stem}.pdf"));
        let converted =
            Document::open(&path).unwrap_or_else(|e| panic!("{stem}: the output reopens, got {e}"));

        // The two clauses the conversion is *unconditionally* responsible for.
        // Every corpus file fails both before conversion (`pdfa_oracle.rs`
        // asserts the first), so a converted file still failing one means the
        // repair did not reach the file at all.
        let report = converted.check_pdfa(PdfaLevel::A2b);
        let clauses = report.clauses();
        for unwanted in [
            pdfrum::PdfaClause::XmpMissing,
            pdfrum::PdfaClause::XmpMalformed,
            pdfrum::PdfaClause::XmpIdentificationMissing,
            pdfrum::PdfaClause::XmpIdentificationMismatch,
            pdfrum::PdfaClause::DeviceColorWithoutOutputIntent,
            pdfrum::PdfaClause::OutputIntentMissing,
            pdfrum::PdfaClause::OutputIntentProfileMissing,
        ] {
            assert!(
                !clauses.contains(&unwanted),
                "{stem}: the conversion writes the XMP packet and the output intent, \
                 so {unwanted:?} must not survive it. Remaining: {clauses:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A CMYK document gets a CMYK output intent, and an RGB one does not.
///
/// The half of the CMYK intent that needs no oracle. An output intent carries
/// **one** destination profile (ISO 19005-2 6.2.4.2), so writing a CMYK
/// profile is not an addition to the sRGB one but a choice against it — and a
/// choice made wrongly is worse than no CMYK support at all, because it would
/// take an RGB file that passes today and fail it on 6.2.4.3-2.
///
/// So this asserts the choice in both directions over the corpus rather than
/// only that a CMYK profile can be written: `image_bug_718762` paints in
/// `/DeviceCMYK` and nothing else, and the remaining files must keep sRGB.
#[test]
fn only_a_cmyk_document_is_given_a_cmyk_output_intent() {
    /// The `/OutputConditionIdentifier` each profile is written under.
    const CMYK_CONDITION: &[u8] = b"CGATS TR 001";
    const SRGB_CONDITION: &[u8] = b"sRGB IEC61966-2.1";

    let dir = scratch("intent");
    let conversions = convert_corpus(&dir, PdfaLevel::A2b, PdfaPolicy::lossy());

    let mut cmyk = Vec::new();
    for (stem, _) in &conversions {
        let bytes = std::fs::read(dir.join(format!("{stem}.pdf")))
            .unwrap_or_else(|e| panic!("{stem}: the output is readable, got {e}"));
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
        // Exactly one intent reaches the file, whichever it is.
        assert!(
            has(CMYK_CONDITION) != has(SRGB_CONDITION),
            "{stem}: a file carries one output intent, not both and not neither"
        );
        if has(CMYK_CONDITION) {
            cmyk.push(*stem);
        }
    }

    assert!(
        cmyk.contains(&"image_bug_718762"),
        "image_bug_718762 paints only in /DeviceCMYK, so it must get the CMYK intent; \
         got {cmyk:?}"
    );
    // The corpus is overwhelmingly RGB, so a scan that had gone wrong in the
    // permissive direction would show up as most of it turning CMYK.
    assert!(
        cmyk.len() * 4 < conversions.len(),
        "only a document painting in CMYK alone takes the CMYK intent, and the corpus \
         is mostly RGB; {} of {} is too many",
        cmyk.len(),
        conversions.len()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The strict policy refuses rather than silently dropping a forbidden
/// feature.
///
/// This is the property M26's fourth item is about, and it is asserted as a
/// *difference between two policies over the same file*: the corpus files that
/// carry JavaScript or a widget `/AA` convert under `lossy` and refuse under
/// `strict`. A conversion that ignored the policy would pass a test that only
/// ran one of them.
#[test]
fn the_strict_policy_refuses_what_the_lossy_policy_compromises() {
    let strict_dir = scratch("strict");
    let strict = convert_corpus(&strict_dir, PdfaLevel::A2b, PdfaPolicy::strict());

    let mut refused = 0;
    for (stem, conversion) in &strict {
        if conversion.converted() {
            // A file with nothing forbidden in it converts under either
            // policy, and must do so with no compromises at all.
            assert!(
                conversion.compromises.is_empty(),
                "{stem}: the strict policy authorizes no compromise, so a conversion \
                 under it must make none: {:?}",
                conversion.compromises
            );
            continue;
        }
        refused += 1;
        assert!(
            !conversion.refusals.is_empty(),
            "{stem}: a conversion that did not convert must say why"
        );
        // Nothing is written on a refusal. That is the interlock which stops a
        // caller mistaking a refusal for a conversion by looking at the file.
        assert!(
            !strict_dir.join(format!("{stem}.pdf")).exists(),
            "{stem}: a refused conversion must write no file"
        );
    }

    assert!(
        refused > 0,
        "the corpus carries JavaScript, widget /AA entries and forbidden annotations, \
         so the strict policy must refuse something — otherwise the policy is not \
         being consulted at all"
    );
    let _ = std::fs::remove_dir_all(&strict_dir);
}

/// Every compromise the conversion makes is one the policy authorized.
///
/// The converse of the test above, and the one that would catch a repair added
/// later that changes the document without asking. It works because
/// `Compromise` and the `Policy` fields are the same vocabulary: every variant
/// belongs to exactly one concession.
#[test]
fn no_compromise_is_made_that_the_policy_did_not_authorize() {
    let dir = scratch("authorized");
    // A policy that permits the forbidden-feature removals and nothing else.
    // Every compromise the corpus produces must fall under that one field; a
    // compromise of any other kind means a repair reached the file through a
    // concession nobody granted.
    let policy = PdfaPolicy::strict().forbidden_feature(pdfrum::PdfaConcession::Accept);
    for (stem, conversion) in convert_corpus(&dir, PdfaLevel::A2b, policy) {
        for compromise in &conversion.compromises {
            let authorized = matches!(
                compromise,
                pdfrum::PdfaCompromise::ActionRemoved { .. }
                    | pdfrum::PdfaCompromise::AnnotationRemoved { .. }
                    | pdfrum::PdfaCompromise::AnnotationFlagsChanged { .. }
                    | pdfrum::PdfaCompromise::EmbeddedFileRemoved { .. }
                    | pdfrum::PdfaCompromise::OptionalContentRemoved { .. }
                    | pdfrum::PdfaCompromise::ObjectMetadataRemoved { .. }
            );
            assert!(
                authorized,
                "{stem}: {compromise:?} was made under a policy that authorized only \
                 forbidden-feature removal"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A refusal names the concession that would have permitted it.
///
/// The report is meant to be actionable: a caller reads the refusals, widens
/// exactly the fields they name, and runs again. This asserts that loop closes
/// — and it derives the second policy *from the first run's refusals* rather
/// than from a guess about what the corpus contains, so it tests the mapping
/// from `Refusal` variant to `Policy` field rather than restating it.
#[test]
fn widening_the_policy_a_refusal_names_makes_the_conversion_succeed() {
    let strict_dir = scratch("loop-strict");
    let refused: Vec<(&'static str, PdfaPolicy)> =
        convert_corpus(&strict_dir, PdfaLevel::A2b, PdfaPolicy::strict())
            .into_iter()
            .filter(|(_, c)| !c.converted())
            .map(|(stem, conversion)| {
                // Grant exactly what this file's own refusals ask for, and
                // nothing else. A refusal that named the wrong field yields a
                // policy that still refuses, which is the failure below.
                let mut policy = PdfaPolicy::strict();
                for refusal in &conversion.refusals {
                    policy = match refusal {
                        pdfrum::PdfaRefusal::UnembeddableFont { .. } => {
                            policy.unembeddable_font(pdfrum::PdfaConcession::Accept)
                        }
                        pdfrum::PdfaRefusal::UnrepresentableContent { .. } => {
                            policy.unrepresentable_content(pdfrum::PdfaConcession::Accept)
                        }
                        pdfrum::PdfaRefusal::ForbiddenFeature { .. } => {
                            policy.forbidden_feature(pdfrum::PdfaConcession::Accept)
                        }
                        _ => policy,
                    };
                }
                (stem, policy)
            })
            .collect();
    let _ = std::fs::remove_dir_all(&strict_dir);
    assert!(!refused.is_empty(), "the corpus must refuse under strict");

    let retry_dir = scratch("loop-retry");
    std::fs::create_dir_all(&retry_dir).expect("the scratch directory is writable");
    for (stem, policy) in &refused {
        let doc = Document::open(pdfrum_corpus::path(stem)).expect("the corpus reopens");
        let dest = retry_dir.join(format!("{stem}.pdf"));
        let conversion = doc
            .to_pdfa(&dest, PdfaLevel::A2b, policy)
            .expect("converting must not error");
        assert!(
            conversion.converted(),
            "{stem} refused under the strict policy; granting exactly the concessions \
             its own refusals named must convert it, or a refusal named the wrong \
             field. Still refusing: {:?}",
            conversion.refusals
        );
    }
    let _ = std::fs::remove_dir_all(&retry_dir);
}
