//! The document information dictionary (ISO 32000-1 §14.3.3) and the date
//! strings it holds (§7.9.4).
//!
//! `/Info` hangs off the trailer rather than the catalog, which is the one
//! thing that makes it different from every other dictionary an edit
//! touches: a document without one needs a new object *and* a trailer that
//! names it, and the trailer is built by the writer from the base document's.
//! [`EditDoc`] carries that one override and the writer reads it back through
//! `EditDoc::trailer`.

use std::time::{SystemTime, UNIX_EPOCH};

use pdfrum_object::{Dict, Name, ObjRef, Object, PdfString, Resolve, encode_text, names};

use crate::doc::EditDoc;

/// Set or remove one text entry of the document's `/Info` dictionary.
///
/// `Some(text)` writes `key` as a text string — `PDFDocEncoding` when every
/// character has a byte there, otherwise UTF-16BE behind a byte-order mark —
/// and `None` or an empty string removes the key. A document without an
/// `/Info` gains one, as a new indirect object the saved trailer names; an
/// `/Info` that is not a dictionary is replaced by one.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_edit::{EditDoc, SaveOptions, save, set_info_entry};
/// use pdfrum_object::names;
/// use pdfrum_parser::{LoadOptions, load};
///
/// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// let doc = load(bytes, &LoadOptions::default())?;
/// let mut edit = EditDoc::new(&doc);
/// set_info_entry(&mut edit, names::TITLE, Some("Hello"));
///
/// let mut out = Vec::new();
/// save(&edit, &SaveOptions::default(), &mut out)?;
/// let reloaded = load(Arc::from(&out[..]), &LoadOptions::default())?;
/// let info = reloaded.trailer().dict(names::INFO, &reloaded).expect("an /Info");
/// assert_eq!(info.text(names::TITLE, &reloaded).as_deref(), Some("Hello"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn set_info_entry(dest: &mut EditDoc<'_>, key: &Name, text: Option<&str>) {
    let existing = dest.trailer().reference(names::INFO);
    let mut info = existing
        .and_then(|r| dest.fetch(r).ok())
        .as_deref()
        .and_then(Object::as_dict)
        .cloned()
        .unwrap_or_default();
    match text.filter(|t| !t.is_empty()) {
        Some(text) => info.insert(
            key.clone(),
            Object::Str(PdfString::literal(encode_text(text))),
        ),
        None => {
            info.remove(key);
        }
    }
    store_info(dest, existing, info);
}

/// Write `info` back where the trailer will find it.
fn store_info(dest: &mut EditDoc<'_>, existing: Option<ObjRef>, info: Dict) {
    let object = Object::Dict(info);
    // A reference the trailer already carries — even one that pointed at
    // something broken — is reused, so an incremental save appends one object
    // and the trailer copies through unchanged.
    if let Some(reference) = existing {
        dest.replace(reference, object);
    } else {
        let reference = dest.add(object);
        dest.set_info(reference);
    }
}

/// `time` as a PDF date string (ISO 32000-1 §7.9.4): `D:YYYYMMDDHHmmSSZ00'00'`,
/// always in UTC, which is the one zone every reader agrees on.
///
/// A time before 1970 reads as the epoch.
///
/// ```
/// use std::time::{Duration, UNIX_EPOCH};
/// use pdfrum_edit::pdf_date;
///
/// assert_eq!(pdf_date(UNIX_EPOCH), "D:19700101000000Z00'00'");
/// assert_eq!(
///     pdf_date(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
///     "D:20231114221320Z00'00'"
/// );
/// ```
#[must_use]
pub fn pdf_date(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z00'00'",
        second_of_day / 3600,
        second_of_day % 3600 / 60,
        second_of_day % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01, as (year, month,
/// day) — the era arithmetic of Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days.saturating_add(719_468);
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, UNIX_EPOCH};

    use pdfrum_object::{Name, ObjRef, Object, Resolve, names};
    use pdfrum_parser::{Document, LoadOptions, load};

    use super::{civil_from_days, pdf_date, set_info_entry};
    use crate::doc::EditDoc;

    const PAGE_TREE: &[u8] = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n";

    fn doc(with_info: bool) -> Document {
        let mut file = PAGE_TREE.to_vec();
        file.extend_from_slice(b"4 0 obj\n<< /Title (Old) /Author (Ann) >>\nendobj\n");
        file.extend_from_slice(if with_info {
            b"trailer\n<< /Root 1 0 R /Info 4 0 R /Size 5 >>\n"
        } else {
            b"trailer\n<< /Root 1 0 R /Size 5 >>\n"
        });
        load(Arc::from(file), &LoadOptions::default()).expect("opens")
    }

    fn info_of(edit: &EditDoc<'_>) -> pdfrum_object::Dict {
        edit.trailer()
            .dict(names::INFO, edit)
            .expect("an /Info the trailer names")
    }

    #[test]
    fn an_existing_info_is_edited_in_place() {
        let base = doc(true);
        let mut edit = EditDoc::new(&base);
        set_info_entry(&mut edit, names::TITLE, Some("New"));
        set_info_entry(&mut edit, names::AUTHOR, None);
        set_info_entry(&mut edit, names::SUBJECT, Some("\u{7f51}\u{9875}"));
        assert_eq!(
            edit.trailer().reference(names::INFO),
            Some(ObjRef::new(4, 0))
        );
        let info = info_of(&edit);
        assert_eq!(info.text(names::TITLE, &edit).as_deref(), Some("New"));
        assert!(!info.contains_key(names::AUTHOR));
        assert_eq!(
            info.string(names::SUBJECT).map(|s| s.bytes.to_vec()),
            Some(b"\xFE\xFF\x7F\x51\x98\x75".to_vec()),
            "outside PDFDocEncoding goes out as UTF-16BE with a mark"
        );
    }

    #[test]
    fn a_document_without_an_info_gains_one_the_trailer_names() {
        let base = doc(false);
        let mut edit = EditDoc::new(&base);
        assert!(!edit.trailer().contains_key(names::INFO));
        set_info_entry(&mut edit, names::TITLE, Some("T"));
        set_info_entry(&mut edit, names::CREATOR, Some("C"));
        let reference = edit.trailer().reference(names::INFO).expect("named");
        assert!(reference.num > 4, "a fresh object, not a reused number");
        let info = info_of(&edit);
        assert_eq!(info.text(names::TITLE, &edit).as_deref(), Some("T"));
        assert_eq!(info.text(names::CREATOR, &edit).as_deref(), Some("C"));
        // The second entry landed in the same object as the first.
        assert_eq!(edit.edited().count(), 1);
    }

    #[test]
    fn an_empty_value_removes_and_a_missing_key_removes_nothing() {
        let base = doc(true);
        let mut edit = EditDoc::new(&base);
        set_info_entry(&mut edit, names::TITLE, Some(""));
        set_info_entry(&mut edit, names::KEYWORDS, None);
        let info = info_of(&edit);
        assert!(!info.contains_key(names::TITLE));
        assert!(!info.contains_key(names::KEYWORDS));
        assert_eq!(info.text(names::AUTHOR, &edit).as_deref(), Some("Ann"));
    }

    #[test]
    fn an_info_that_is_not_a_dictionary_is_replaced() {
        let mut file = PAGE_TREE.to_vec();
        file.extend_from_slice(
            b"4 0 obj\n42\nendobj\ntrailer\n<< /Root 1 0 R /Info 4 0 R /Size 5 >>\n",
        );
        let base = load(Arc::from(file), &LoadOptions::default()).expect("opens");
        let mut edit = EditDoc::new(&base);
        set_info_entry(&mut edit, &Name::from("Title"), Some("T"));
        let object = edit.fetch(ObjRef::new(4, 0)).expect("replaced");
        assert!(matches!(&*object, Object::Dict(d) if d.contains_key(names::TITLE)));
    }

    #[test]
    fn dates_are_utc_and_civil() {
        assert_eq!(pdf_date(UNIX_EPOCH), "D:19700101000000Z00'00'");
        assert_eq!(
            pdf_date(UNIX_EPOCH + Duration::from_hours(264_384)),
            "D:20000229000000Z00'00'",
            "a century leap day"
        );
        assert_eq!(
            pdf_date(UNIX_EPOCH + Duration::from_hours(474_780)),
            "D:20240229120000Z00'00'"
        );
        assert_eq!(
            pdf_date(UNIX_EPOCH + Duration::from_secs(4_102_444_799)),
            "D:20991231235959Z00'00'"
        );
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(-719_468), (0, 3, 1));
    }
}
