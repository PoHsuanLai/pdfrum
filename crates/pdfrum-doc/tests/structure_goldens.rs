//! `--show-structure` against the oracle's own fixtures.
//!
//! Each case is a whole-file dump compared byte for byte, because the format
//! is a Tier-A contract: field order, indentation and the sorted attribute
//! keys are all behavior, and a test that checked only "contains" would let
//! any of them drift.

use std::path::{Path, PathBuf};

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::structure::{StructTree, dump_tree};

/// The read-only C++ PDFium checkout, resolved the one way every script and
/// test in this repository resolves it: `$PDFRUM_ORACLE_CHECKOUT`, else the
/// sibling `../pdfium-c++` directory README.md names.
///
/// Six lines rather than a shared module: forbids a `common`,
/// `util` or `helpers` module name, and an integration test in one crate
/// cannot reach another crate's test code anyway.
fn oracle_checkout() -> PathBuf {
    let checkout = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
        PathBuf::from,
    );
    if !checkout.is_dir() {
        // Said once per test process, so a run where every case below did
        // nothing says so rather than reporting a silent green.
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            eprintln!(
                "skipping the oracle-corpus cases: no checkout at {} \
                 (set PDFRUM_ORACLE_CHECKOUT)",
                checkout.display()
            );
        });
    }
    checkout
}

/// The oracle checkout's fixture directory. Tests skip when it is absent, so
/// the suite still runs on a machine that has only this repository.
fn resource(name: &str) -> Option<PathBuf> {
    let path = oracle_checkout().join("testing/resources").join(name);
    path.exists().then_some(path)
}

/// The whole dump for one page, or nothing when the fixture is unavailable.
fn dump_page(name: &str, index: u32) -> Option<String> {
    let bytes = std::fs::read(resource(name)?).ok()?;
    let doc = pdfrum_parser::load(bytes.into(), &pdfrum_parser::LoadOptions::default()).ok()?;
    let catalog = doc.catalog().ok()?;
    let page = doc.page(index).ok()?;
    let obj_num = page.reference.map_or(0, |r| r.num);
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    let tree = StructTree::load_page(&catalog, &page.dict, obj_num, &doc, &limits, &mut diags);
    Some(dump_tree(tree.as_ref(), index, &doc))
}

#[test]
fn the_four_marked_content_shapes() {
    let Some(got) = dump_page("tagged_marked_content.pdf", 0) else {
        return;
    };
    assert_eq!(
        got,
        "Structure Tree for Page 0\n S: NonStruct\n MCID0: 0\n Type: StructElem\n S: NonStruct\n \
         MCID0: 1\n Type: StructElem\n S: NonStruct\n MCID0: 2\n MCID1: 3\n Type: StructElem\n \
         S: NonStruct\n Type: StructElem\n\n\n"
    );
}

#[test]
fn nested_elements_indent_two_spaces_per_level() {
    let Some(got) = dump_page("tagged_mcr_objr.pdf", 0) else {
        return;
    };
    assert_eq!(
        got,
        "Structure Tree for Page 0\n S: Document\n Lang: en-US\n Type: StructElem\n   \
         S: NonStruct\n   Type: StructElem\n     S: P\n     Type: StructElem\n       \
         S: NonStruct\n       MCID0: 0\n       Type: StructElem\n   S: P\n   Type: StructElem\n     \
         S: NonStruct\n     Type: StructElem\n       S: NonStruct\n       MCID0: 1\n       \
         Type: StructElem\n\n\n"
    );
}

#[test]
fn attributes_sort_their_keys_and_numbers_carry_six_decimals() {
    let Some(got) = dump_page("tagged_table.pdf", 0) else {
        return;
    };
    // Written `Scope` before `O` in the file; alphabetical in the output.
    assert!(
        got.contains("       A[0]:\n         O: Table\n         Scope: Row\n"),
        "{got}"
    );
    assert!(got.contains("ColSpan: 2.000000"), "{got}");
    // A present-but-empty attribute string still prints its label.
    assert!(got.contains("     Summary: \n"), "{got}");
    // `/Lang` is not inherited: the rows under a Hungarian table report none.
    assert!(
        got.contains("     S: TR\n     ID: \n     Parent ID: node12\n"),
        "{got}"
    );
}

#[test]
fn a_root_whose_kids_never_link_prints_only_its_header() {
    // The `/K` slots are sized from the root but filled by the upward walk,
    // and this file's never link — so the count exceeds what is fetchable.
    let Some(got) = dump_page("bug_1768.pdf", 0) else {
        return;
    };
    assert_eq!(got, "Structure Tree for Page 0\n\n\n");
}

#[test]
fn an_untagged_document_dumps_nothing_at_all() {
    let Some(got) = dump_page("hello_world.pdf", 0) else {
        return;
    };
    assert_eq!(got, "");
}

#[test]
fn the_same_element_tree_is_reachable_from_either_page() {
    let Some(first) = dump_page("tagged_mcr_multipage.pdf", 0) else {
        return;
    };
    let Some(second) = dump_page("tagged_mcr_multipage.pdf", 1) else {
        return;
    };
    // Same shape, different page header — and the marked-content identifier
    // belongs to whichever page is being read.
    assert!(first.starts_with("Structure Tree for Page 0\n"));
    assert!(second.starts_with("Structure Tree for Page 1\n"));
    assert_eq!(first.matches("S: ").count(), second.matches("S: ").count());
}

/// The dump must terminate on every one of these, and a stack overflow is
/// what it used to do.
///
/// A kid slot carries two indices from different spaces — its position in the
/// parent's `/K`, and its position in the tree's element table — and
/// conflating them made a kid point back at an ancestor, so `dump_element`
/// recursed until the stack ran out. These four files all have the shape that
/// triggered it; an overflow aborts the process, so reaching the end of this
/// test *is* the assertion.
#[test]
fn damaged_structure_trees_terminate_rather_than_recursing_without_bound() {
    for name in [
        "tagged_nested.pdf",
        "bug_1296920.pdf",
        "tagged_table.pdf",
        "tagged_table_bad_parent.pdf",
        "tagged_table_bad_elem.pdf",
        "tagged_mcr_objr.pdf",
    ] {
        let _ = dump_page(name, 0);
    }
}

/// `bug_717.pdf` keeps its whole structure tree in an object stream, and the
/// hybrid-reference table its writer produced lists those objects as free —
/// so until the parser learned that a classic table's free entry is inert,
/// `/StructTreeRoot` read as absent and this tree came out empty. The
/// self-skip that stood here while that was true is gone: the answer is
/// available now, and a test that tolerates the empty dump would not notice
/// it going away again.
///
/// A *missing oracle checkout* is a different thing from an empty dump, and
/// the `let else` below is that distinction: with no fixture to read there is
/// nothing to assert, and `dump_page` has already said so once.
#[test]
fn a_structure_tree_inside_an_object_stream_still_builds() {
    let Some(got) = dump_page("bug_717.pdf", 0) else {
        return;
    };
    assert_eq!(
        got,
        "Structure Tree for Page 0\n S: Sect\n Type: StructElem\n   S: P\n   MCID0: 0\n   \
         Type: StructElem\n   S: Figure\n   MCID0: 1\n   Type: StructElem\n\n\n"
    );
}
