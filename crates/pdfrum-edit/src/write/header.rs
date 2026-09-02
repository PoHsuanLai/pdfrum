//! The file header (ISO 32000-1 §7.5.2).
//!
//! Nine bytes of `%PDF-1.N`, then a comment line of four bytes above 127.
//! That comment is not decoration: §7.5.2 says a file containing binary data
//! should carry one so that transfer programs sniffing the first lines
//! classify the file as binary rather than text and stop mangling its line
//! endings.

use pdfrum_common::PdfVersion;

/// The version to fall back on when the document declares none.
///
/// A header-less or version-less document saves as 1.7 rather than as
/// something older, because the writer may emit constructs — xref streams,
/// object streams — that an older version does not admit.
const DEFAULT_VERSION: PdfVersion = PdfVersion::PDF_1_7;

/// Whether a version the caller asked for is one we will honour.
///
/// 1.0 through 1.7. Anything else — 1.8, 2.0, a major other than 1 — means
/// "use the document's own", which is what makes an out-of-range request
/// produce the input's version rather than an error.
///
/// Was `10..=17` over the packed `u8` this function used to take. The bound is
/// the same one; it is now spelled in digits (`docs/design/idiomatic-api.md`
/// §WP1).
const fn is_honoured(version: PdfVersion) -> bool {
    version.major == 1 && version.minor <= 7
}

/// Write `%PDF-1.N` and the binary comment.
///
/// `requested` is what the caller asked for and `document` what the file
/// declared; the first wins when it is in range, then the second, then 1.7.
/// `None` for either means it named nothing.
pub fn write_header(
    out: &mut Vec<u8>,
    requested: Option<PdfVersion>,
    document: Option<PdfVersion>,
) {
    let version = match (requested, document) {
        (Some(r), _) if is_honoured(r) => r,
        (_, Some(d)) => d,
        _ => DEFAULT_VERSION,
    };

    out.extend_from_slice(b"%PDF-1.");
    // The units digit: 1.7 writes `7`, 1.4 writes `4`. A version reduced this
    // way is what the C++ writes and what every reader expects — and it is why
    // a document declaring 2.0 still writes `%PDF-1.0`, which is the oracle's
    // behaviour rather than an oversight here.
    out.push(b'0' + (version.minor % 10));
    out.extend_from_slice(b"\r\n%\xA1\xB3\xC5\xD7\r\n");
}

#[cfg(test)]
mod tests {
    use pdfrum_common::PdfVersion;

    use super::write_header;

    fn header(requested: Option<PdfVersion>, document: Option<PdfVersion>) -> String {
        let mut out = Vec::new();
        write_header(&mut out, requested, document);
        String::from_utf8_lossy(out.get(..9).unwrap_or_default()).into_owned()
    }

    // SaveSimpleDoc: a 1.7 input saves as 1.7.
    #[test]
    fn the_documents_own_version_is_kept() {
        assert_eq!(header(None, Some(PdfVersion::PDF_1_7)), "%PDF-1.7\r");
        assert_eq!(header(None, Some(PdfVersion::PDF_1_4)), "%PDF-1.4\r");
    }

    // SaveSimpleDocWithVersion: an in-range request wins.
    #[test]
    fn an_in_range_request_wins() {
        assert_eq!(
            header(Some(PdfVersion::PDF_1_4), Some(PdfVersion::PDF_1_7)),
            "%PDF-1.4\r"
        );
        assert_eq!(
            header(Some(PdfVersion::PDF_1_0), Some(PdfVersion::PDF_1_7)),
            "%PDF-1.0\r"
        );
        assert_eq!(
            header(Some(PdfVersion::PDF_1_7), Some(PdfVersion::PDF_1_4)),
            "%PDF-1.7\r"
        );
    }

    // SaveSimpleDocWithBadVersion: -1, 0 and 18 all fall through to the
    // document's own, which for a 1.7 input is 1.7. The sentinel spellings
    // are gone — `None` is the "asked for nothing" case — but every value
    // that used to fall through still does, now as an out-of-range pair.
    #[test]
    fn an_out_of_range_request_falls_through() {
        let requests = [
            None,
            Some(PdfVersion::new(0, 9)),
            Some(PdfVersion::new(1, 8)),
            Some(PdfVersion::PDF_2_0),
            Some(PdfVersion::new(25, 5)),
        ];
        for requested in requests {
            assert_eq!(
                header(requested, Some(PdfVersion::PDF_1_7)),
                "%PDF-1.7\r",
                "asked for {requested:?}"
            );
        }
    }

    // A document with no version of its own saves as 1.7.
    #[test]
    fn a_version_less_document_saves_as_one_seven() {
        assert_eq!(header(None, None), "%PDF-1.7\r");
    }

    // The binary comment is what stops transfer programs treating the file as
    // text; all four bytes must be above 127.
    #[test]
    fn the_binary_comment_follows_the_version() {
        let mut out = Vec::new();
        write_header(&mut out, None, Some(PdfVersion::PDF_1_7));
        assert_eq!(out, b"%PDF-1.7\r\n%\xA1\xB3\xC5\xD7\r\n");
        assert!(out.get(11..15).is_some_and(|c| c.iter().all(|b| *b > 127)));
    }
}
