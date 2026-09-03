//! The oracle's searchex tables, ported to the typed indices: the character
//! stream against the text buffer, on a page with nothing stripped and on one
//! with a leading control character. The oracle's negative-index cases are
//! unrepresentable in typed indices.

use pdfrum::{CharIndex, Document, TextIndex};

#[test]
fn a_text_index_is_its_char_index_when_nothing_is_stripped() {
    const PAIRS: &[(usize, usize)] = &[(0, 0), (1, 1), (2, 2), (5, 5), (10, 10), (29, 29)];
    let doc = Document::open("tests/fixtures/hello_world.pdf").unwrap();
    let text = doc.page(0).unwrap().text();
    let map = &text.runs;
    for &(t, c) in PAIRS {
        assert_eq!(
            map.char_index(TextIndex::new(t)),
            Some(CharIndex::new(c)),
            "text index {t}"
        );
    }
    assert!(
        map.char_index(TextIndex::new(30)).is_none(),
        "text index 30"
    );
}

#[test]
fn a_char_index_is_its_text_index_when_nothing_is_stripped() {
    const PAIRS: &[(usize, usize)] = &[(0, 0), (1, 1), (2, 2), (5, 5), (10, 10), (29, 29)];
    let doc = Document::open("tests/fixtures/hello_world.pdf").unwrap();
    let text = doc.page(0).unwrap().text();
    let map = &text.runs;
    for &(c, t) in PAIRS {
        assert_eq!(
            map.text_index(CharIndex::new(c)),
            Some(TextIndex::new(t)),
            "char index {c}"
        );
    }
    assert!(
        map.text_index(CharIndex::new(30)).is_none(),
        "char index 30"
    );
}

#[test]
fn a_leading_control_character_shifts_every_text_index_by_one() {
    const PAIRS: &[(usize, usize)] = &[(0, 1), (1, 2), (2, 3), (5, 6), (10, 11), (29, 30)];
    let doc = Document::open("tests/fixtures/bug_1139.pdf").unwrap();
    let text = doc.page(0).unwrap().text();
    let map = &text.runs;
    for &(t, c) in PAIRS {
        assert_eq!(
            map.char_index(TextIndex::new(t)),
            Some(CharIndex::new(c)),
            "text index {t}"
        );
    }
    assert!(
        map.char_index(TextIndex::new(30)).is_none(),
        "text index 30"
    );
}

#[test]
fn a_leading_control_character_has_no_text_index_and_shifts_the_rest() {
    const PAIRS: &[(usize, usize)] = &[(1, 0), (2, 1), (5, 4), (10, 9), (29, 28), (30, 29)];
    let doc = Document::open("tests/fixtures/bug_1139.pdf").unwrap();
    let text = doc.page(0).unwrap().text();
    let map = &text.runs;
    assert!(map.text_index(CharIndex::new(0)).is_none(), "char index 0");
    for &(c, t) in PAIRS {
        assert_eq!(
            map.text_index(CharIndex::new(c)),
            Some(TextIndex::new(t)),
            "char index {c}"
        );
    }
    assert!(
        map.text_index(CharIndex::new(31)).is_none(),
        "char index 31"
    );
}
