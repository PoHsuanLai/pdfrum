//! Ported `cffl_combobox_embeddertest.cpp` assertions.
//!
//! Two rows, and they are about the **payload a field hands its scripts**
//! rather than about the scripts themselves — which is why they port here
//! with no engine present. `CFFL_FieldAction` gathers the change, the value
//! before it and the selection it replaces; `Keystroke` is that record.
//!
//! `GetActionData` fills different subsets for different triggers, and the
//! difference is the interesting part: a **keystroke** carries the export
//! text of the selected option in `sChangeEx`, while **validate** and
//! **focus** leave it empty. Reproducing that asymmetry is what stops a
//! script seeing a change where none was offered.

use pdfrum_doc::vt::{Config, Metrics};
use pdfrum_form::cascade::FieldRef;
use pdfrum_form::cascade::{Cascade, Keystroke, KeystrokeOutcome, NoScripts};
use pdfrum_form::edit::ops::{self, TextEdit};

fn metrics() -> Metrics<'static> {
    Metrics {
        width: &|_| 1000,
        ascent: 800,
        descent: -200,
    }
}

fn config() -> Config {
    Config {
        plate: kurbo::Rect::new(0.0, 0.0, 1000.0, 20.0),
        font_size: 1.0,
        ..Config::default()
    }
}

fn edit(text: &str) -> TextEdit {
    TextEdit::new(text, &config(), &metrics(), true)
}

/// `GetActionData`: a keystroke payload reads the field's current value and
/// the selection the change would replace.
#[test]
fn a_keystroke_payload_reads_the_value_and_the_selection() {
    let mut field = edit("Banana");
    field.set_selection(2, 4);

    let keystroke = Keystroke::of(&field, "Hamster");
    assert_eq!(keystroke.value, "Banana");
    assert_eq!(keystroke.change, "Hamster");
    assert_eq!(keystroke.selection_start, 2);
    assert_eq!(keystroke.selection_end, 4);
}

/// `SetActionData`: applying that payload replaces the selection with the
/// change. `"Banana"` with rows 2..4 replaced by `"Hamster"` is
/// `"BaHamsterna"` — the upstream row's exact expectation.
#[test]
fn applying_a_keystroke_replaces_its_selection_with_its_change() {
    let mut field = edit("Banana");
    field.set_selection(2, 4);

    let keystroke = Keystroke::of(&field, "Hamster");
    assert_eq!(keystroke.applied(), "BaHamsterna");
}

/// With **nothing** selected the payload's two indices are the caret twice
/// over, and applying it inserts rather than replaces.
#[test]
fn an_empty_selection_makes_the_keystroke_an_insertion() {
    let mut field = edit("Banana");
    field.set_selection(3, 3);

    let keystroke = Keystroke::of(&field, "XY");
    assert_eq!(keystroke.selection_start, keystroke.selection_end);
    assert_eq!(keystroke.applied(), "BanXYana");
}

/// A hook that **rewrites** the change is answered by what it returned, not
/// by what it was offered. That is the whole reason the payload goes back and
/// forth rather than being a read-only notification.
#[test]
fn a_rewritten_change_is_what_gets_applied() {
    let mut field = edit("Banana");
    field.set_selection(0, 6);

    let offered = Keystroke::of(&field, "lower");
    let rewritten = Keystroke {
        change: "UPPER".to_string(),
        ..offered
    };
    assert_eq!(rewritten.applied(), "UPPER");
}

/// A deletion is a keystroke whose change is empty.
#[test]
fn a_deletion_is_a_keystroke_with_an_empty_change() {
    let mut field = edit("Banana");
    field.set_selection(2, 4);

    let keystroke = Keystroke::of(&field, "");
    assert_eq!(keystroke.applied(), "Bana");
}

/// Indices past the text clamp rather than panicking — a script can set them,
/// so they are untrusted input.
#[test]
fn out_of_range_indices_clamp_rather_than_panic() {
    let field = edit("Banana");
    for (start, end) in [
        (0, u32::MAX),
        (u32::MAX, 0),
        (100, 200),
        (u32::MAX, u32::MAX),
    ] {
        let keystroke = Keystroke {
            change: "X".to_string(),
            value: field.text.clone(),
            selection_start: start,
            selection_end: end,
        };
        // The only requirement is an answer, not a particular one.
        let _ = keystroke.applied();
    }
}

/// The script-free cascade accepts every keystroke unchanged, which is what
/// makes a V8-off build behave as though no hook existed at all.
#[test]
fn the_script_free_cascade_accepts_every_keystroke_unchanged() {
    let mut cascade = NoScripts;
    let field = FieldRef {
        name: "Combo1".to_string(),
        index: 0,
    };
    let offered = Keystroke {
        change: "Hamster".to_string(),
        value: "Banana".to_string(),
        selection_start: 2,
        selection_end: 4,
    };

    match cascade.keystroke(&field, offered.clone()) {
        KeystrokeOutcome::Accept(back) => assert_eq!(back, offered),
        KeystrokeOutcome::Reject => panic!("the script-free cascade rejects nothing"),
    }
    assert!(cascade.keystroke_commit(&field, "Banana"));
    assert!(cascade.validate(&field, "Banana"));
    assert_eq!(cascade.format(&field, "1234"), None);
}

/// The payload agrees with what the editing operation actually does, which is
/// the property that makes offering it to a script meaningful.
#[test]
fn the_payload_predicts_what_the_edit_performs() {
    let mut field = edit("Banana");
    field.set_selection(2, 4);
    let predicted = Keystroke::of(&field, "Hamster").applied();

    ops::replace_selection(&mut field, &config(), &metrics(), "Hamster", None);
    assert_eq!(field.text, predicted);
}
