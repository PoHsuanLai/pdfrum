//! The file header (ISO 32000-1 §7.5.2).
//!
//! Nine bytes of `%PDF-1.N`, then a comment line of four bytes above 127.
//! That comment is not decoration: §7.5.2 says a file containing binary data
//! should carry one so that transfer programs sniffing the first lines
//! classify the file as binary rather than text and stop mangling its line
//! endings.

/// The version to fall back on when the document declares none.
///
/// A header-less or version-less document saves as 1.7 rather than as
/// something older, because the writer may emit constructs — xref streams,
/// object streams — that an older version does not admit.
const DEFAULT_VERSION: u8 = 7;

/// Whether a version the caller asked for is one we will honour.
///
/// `10..=17` is 1.0 through 1.7. Anything else — 0, 18, a negative that
/// arrived as a large unsigned — means "use the document's own", which is
/// what makes an out-of-range request produce the input's version rather than
/// an error.
const fn is_honoured(version: u8) -> bool {
    version >= 10 && version <= 17
}

/// Write `%PDF-1.N` and the binary comment.
///
/// `requested` is what the caller asked for and `document` what the file
/// declared; the first wins when it is in range, then the second, then 1.7.
pub fn write_header(out: &mut Vec<u8>, requested: u8, document: u8) {
    let version = if is_honoured(requested) {
        requested
    } else if document > 0 {
        document
    } else {
        DEFAULT_VERSION
    };

    out.extend_from_slice(b"%PDF-1.");
    // The units digit: 17 is 1.7, 14 is 1.4. A two-digit version reduced this
    // way is what the C++ writes and what every reader expects.
    out.push(b'0' + (version % 10));
    out.extend_from_slice(b"\r\n%\xA1\xB3\xC5\xD7\r\n");
}

#[cfg(test)]
mod tests {
    use super::write_header;

    fn header(requested: u8, document: u8) -> String {
        let mut out = Vec::new();
        write_header(&mut out, requested, document);
        String::from_utf8_lossy(out.get(..9).unwrap_or_default()).into_owned()
    }

    // SaveSimpleDoc: a 1.7 input saves as 1.7.
    #[test]
    fn the_documents_own_version_is_kept() {
        assert_eq!(header(0, 17), "%PDF-1.7\r");
        assert_eq!(header(0, 14), "%PDF-1.4\r");
    }

    // SaveSimpleDocWithVersion: an in-range request wins.
    #[test]
    fn an_in_range_request_wins() {
        assert_eq!(header(14, 17), "%PDF-1.4\r");
        assert_eq!(header(10, 17), "%PDF-1.0\r");
        assert_eq!(header(17, 14), "%PDF-1.7\r");
    }

    // SaveSimpleDocWithBadVersion: -1, 0 and 18 all fall through to the
    // document's own, which for a 1.7 input is 1.7.
    #[test]
    fn an_out_of_range_request_falls_through() {
        for requested in [0u8, 9, 18, 100, 255] {
            assert_eq!(header(requested, 17), "%PDF-1.7\r", "asked for {requested}");
        }
    }

    // A document with no version of its own saves as 1.7.
    #[test]
    fn a_version_less_document_saves_as_one_seven() {
        assert_eq!(header(0, 0), "%PDF-1.7\r");
    }

    // The binary comment is what stops transfer programs treating the file as
    // text; all four bytes must be above 127.
    #[test]
    fn the_binary_comment_follows_the_version() {
        let mut out = Vec::new();
        write_header(&mut out, 0, 17);
        assert_eq!(out, b"%PDF-1.7\r\n%\xA1\xB3\xC5\xD7\r\n");
        assert!(out.get(11..15).is_some_and(|c| c.iter().all(|b| *b > 127)));
    }
}
