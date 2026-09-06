//! `Page::words` on real files: the words, their boxes, their fonts and
//! sizes, and that each one slices back out of the text page it came from.
//!
//! The pinned numbers come from reading the fixtures' content streams:
//! `hello_world_2_pages.pdf` draws `Hello, world!` in `/F1 12 Tf`
//! (Times-Roman) and `Goodbye, world!` in `/F2 16 Tf` (Helvetica) on both of
//! its pages, from one shared stream.

use pdfrum::{Document, Word};

const HELLO_2: &str = "tests/fixtures/hello_world_2_pages.pdf";
const QUICK_START: &str = "../../benches/corpus/text_quick_start.pdf";

fn texts(words: &[Word]) -> Vec<&str> {
    words.iter().map(|w| w.text.as_str()).collect()
}

#[test]
fn both_pages_of_the_shared_stream_give_the_same_four_words() {
    let doc = Document::open(HELLO_2).expect("open");
    for index in 0..2 {
        let page = doc.page(index).expect("page");
        let words = page.words();
        assert_eq!(
            texts(&words),
            ["Hello,", "world!", "Goodbye,", "world!"],
            "page {index}"
        );
    }
}

#[test]
fn each_word_slices_back_to_itself_from_the_text_page() {
    let doc = Document::open(HELLO_2).expect("open");
    let page = doc.page(0).expect("page");
    let text = page.text();
    for word in page.words() {
        assert_eq!(text.slice(word.range.clone()), word.text, "{word:?}");
    }
}

#[test]
fn the_words_of_one_line_sit_left_to_right_without_overlapping() {
    let doc = Document::open(HELLO_2).expect("open");
    let words = doc.page(0).expect("page").words();
    let (hello, world) = (&words[0], &words[1]);
    assert!(
        hello.rect.width() > 0.0 && hello.rect.height() > 0.0,
        "{hello:?}"
    );
    assert!(
        world.rect.width() > 0.0 && world.rect.height() > 0.0,
        "{world:?}"
    );
    assert!(hello.rect.x1 <= world.rect.x0, "{hello:?} then {world:?}");
    // The line was set at `20 50 Td`: the boxes are in the page's own
    // (y-up, unrotated) space, so the first word starts at x = 20 and sits
    // on the baseline y = 50 rather than 150 rows down from the top.
    assert!((hello.rect.x0 - 20.0).abs() < 1.0, "{hello:?}");
    assert!(hello.rect.y0 <= 50.0 && 50.0 < hello.rect.y1, "{hello:?}");
    let (goodbye, world) = (&words[2], &words[3]);
    assert!(
        goodbye.rect.x1 <= world.rect.x0,
        "{goodbye:?} then {world:?}"
    );
    // The second line is 50 points higher.
    assert!(
        goodbye.rect.y0 > hello.rect.y1,
        "{goodbye:?} above {hello:?}"
    );
}

#[test]
fn the_font_and_size_are_the_first_characters() {
    let doc = Document::open(HELLO_2).expect("open");
    let words = doc.page(0).expect("page").words();
    assert_eq!(words[0].font.as_deref(), Some("Times-Roman"));
    assert_eq!(words[1].font.as_deref(), Some("Times-Roman"));
    assert_eq!(words[2].font.as_deref(), Some("Helvetica"));
    assert_eq!(words[3].font.as_deref(), Some("Helvetica"));
    assert!((words[0].size - 12.0).abs() < 1e-9, "{:?}", words[0]);
    assert!((words[1].size - 12.0).abs() < 1e-9, "{:?}", words[1]);
    assert!((words[2].size - 16.0).abs() < 1e-9, "{:?}", words[2]);
}

#[test]
fn the_corpus_guide_gives_a_stable_count_of_real_words() {
    let doc = Document::open(QUICK_START).expect("open");
    let page = doc.page(0).expect("page");
    let words = page.words();
    // 33 is the oracle's own count: `pdfium_test --txt` on this page splits
    // into exactly 33 whitespace-separated words, and our page text has been
    // byte-exact against it since M28 dropped the spurious generated spaces.
    // The former 45 counted those spurious spaces as word separators.
    assert_eq!(words.len(), 33, "{:?}", texts(&words));
    for word in &words {
        assert!(!word.text.is_empty(), "{word:?}");
        assert!(word.text.trim() == word.text, "{word:?}");
        assert!(
            word.rect.width() > 0.0 && word.rect.height() > 0.0,
            "{word:?}"
        );
        assert!(word.size > 0.0, "{word:?}");
    }
    assert!(words[0].font.is_some(), "{:?}", words[0]);
    let text = page.text();
    for word in &words {
        assert_eq!(text.slice(word.range.clone()), word.text, "{word:?}");
    }
}
