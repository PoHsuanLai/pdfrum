//! The font-database seam, and the matching policy over it.
//!
//! `fontdb` is used purely as a face **enumerator** — the role
//! `SystemFontInfoIface` played — while the ranking is our port of PDFium's
//! own, so a substitution is predictable rather than dependent on someone
//! else's heuristics. That resolves OQ-6(b) in the direction the brief
//! recommended: `fontdb`'s query ranking is a different function, and Tier-B
//! results would drift with its version.

use super::charset::{Charset, PitchFamily, charset_for_code_page_bit};
use super::style::{style_bits, tt_normalize};
use read_fonts::TableProvider;
use skrifa::MetadataProvider;
use std::path::PathBuf;
use std::sync::Arc;

/// A handle to one face in a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaceHandle(pub usize);

/// What a database knows about one installed face.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceInfo {
    /// The display name, joined as `GetFontNameFromFace` joins it: family,
    /// then a space and the style unless the style is empty or `Regular`.
    pub name: String,
    /// The style bits read from the face's own tables.
    pub styles: u32,
    /// The charsets the face claims, from its `OS/2` code-page ranges. ANSI is
    /// always included, matching the folder enumerator.
    pub charsets: Vec<Charset>,
}

impl FaceInfo {
    /// The similarity score for a request, out of 68.
    ///
    /// Each term rewards *agreement*, not presence: a non-bold face scores for
    /// a non-bold request exactly as a bold one scores for a bold request. The
    /// weights are 16/16/16/8/8 plus a 4-point bonus when the request's name
    /// is the same length as the face's, which is PDFium's cheap stand-in for
    /// "an exact name match".
    #[must_use]
    pub fn similarity_score(
        &self,
        weight: i32,
        italic: bool,
        pitch: PitchFamily,
        exact_match_bonus: bool,
    ) -> i32 {
        let mut score = 0;
        if (self.styles & style_bits::FORCE_BOLD != 0) == (weight > 400) {
            score += 16;
        }
        if (self.styles & style_bits::ITALIC != 0) == italic {
            score += 16;
        }
        if (self.styles & STYLE_SERIF != 0) == pitch.has(PitchFamily::ROMAN) {
            score += 16;
        }
        if (self.styles & STYLE_SCRIPT != 0) == pitch.has(PitchFamily::SCRIPT) {
            score += 8;
        }
        if (self.styles & STYLE_FIXED_PITCH != 0) == pitch.has(PitchFamily::FIXED) {
            score += 8;
        }
        if exact_match_bonus {
            score += 4;
        }
        score
    }

    /// Can this face answer a request for `charset` at all?
    ///
    /// A `Default` charset makes every face eligible, which is what lets the
    /// last rung of the ladder pick anything at all.
    #[must_use]
    pub fn is_eligible(&self, charset: Charset) -> bool {
        charset == Charset::Default || self.charsets.contains(&charset)
    }
}

/// The highest score [`FaceInfo::similarity_score`] can return.
pub const SIMILARITY_SCORE_MAX: i32 = 68;

/// The face-style bits read from a face's own tables, which are a different
/// set from the request's `/Flags`.
const STYLE_SERIF: u32 = 1 << 1;
const STYLE_SCRIPT: u32 = 1 << 3;
const STYLE_FIXED_PITCH: u32 = 1 << 0;

/// Does an installed face's name contain a family name **as a whole word**?
///
/// The rule that makes `"Univers"` match `"Univers Bold"` but not
/// `"Universal"`: the byte after the match must not be a lowercase ASCII
/// letter. Uppercase and punctuation are fine, which is why `"Oxygen"` matches
/// `"Oxygen-Sans"`.
#[must_use]
pub fn find_family_name_match(family: &str, installed: &str) -> bool {
    let Some(at) = installed.find(family) else {
        return false;
    };
    let next = at + family.len();
    !installed
        .as_bytes()
        .get(next)
        .is_some_and(u8::is_ascii_lowercase)
}

/// A source of installed faces.
///
/// A genuine seam by STYLE.md §2b's bar: there are two real implementations —
/// [`SystemFontDb`] over the operating system's fonts, and [`TestFontDb`] over
/// an explicit list, which the substitution tests and the hermetic
/// `--font-dir` runs both need. Threaded as `&impl FontDb`, never `dyn`.
pub trait FontDb {
    /// Every installed face, in a stable order.
    fn faces(&self) -> &[FaceInfo];

    /// The bytes and collection index of one face.
    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)>;

    /// Find the best face for a request (`CFX_FolderFontInfo::FindFont`).
    ///
    /// The default implementation is the port; an implementor overrides it
    /// only to delegate somewhere else entirely.
    ///
    /// Two passes, and the first exists purely to seed the score: a face whose
    /// name is exactly the requested family establishes a baseline the general
    /// scan must beat, and returns immediately on a perfect 68.
    fn find_font(
        &self,
        weight: i32,
        italic: bool,
        charset: Charset,
        pitch: PitchFamily,
        family: &str,
        must_match_name: bool,
    ) -> Option<FaceHandle> {
        let faces = self.faces();
        let mut best: Option<usize> = None;
        let mut best_score = 0;

        if must_match_name
            && let Some((i, face)) = faces
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == family && f.is_eligible(charset))
        {
            best_score = face.similarity_score(weight, italic, pitch, true);
            best = Some(i);
            if best_score == SIMILARITY_SCORE_MAX {
                return Some(FaceHandle(i));
            }
        }

        for (i, face) in faces.iter().enumerate() {
            if !face.is_eligible(charset) {
                continue;
            }
            // The "exact match" bonus is granted on a *length* comparison, not
            // a content one — PDFium's own shortcut, and cheaper than it looks
            // because a differing length rules out equality anyway.
            let bonus = must_match_name && family.len() == face.name.len();
            let score = face.similarity_score(weight, italic, pitch, bonus);
            if score <= best_score {
                continue;
            }
            if must_match_name && !find_family_name_match(family, &face.name) {
                continue;
            }
            best_score = score;
            best = Some(i);
        }

        if let Some(i) = best {
            return Some(FaceHandle(i));
        }
        // The one hard-coded consolation prize: a fixed-pitch ANSI request
        // that matched nothing gets Courier New if it exists.
        if charset == Charset::Ansi && pitch.has(PitchFamily::FIXED) {
            return self.font_by_name("Courier New");
        }
        None
    }

    /// An exact-name lookup, which two rungs of the ladder use directly.
    fn font_by_name(&self, name: &str) -> Option<FaceHandle> {
        self.faces()
            .iter()
            .position(|f| f.name == name)
            .map(FaceHandle)
    }

    /// Find an installed face whose *normalized* name equals `normalized`,
    /// scanning in **reverse** insertion order (`MatchInstalledFonts`).
    ///
    /// The reverse scan means a later-registered face shadows an earlier one
    /// with the same normalized name.
    fn match_installed(&self, normalized: &str) -> Option<String> {
        self.faces()
            .iter()
            .rev()
            .find(|f| tt_normalize(&f.name) == normalized)
            .map(|f| f.name.clone())
    }
}

/// A database whose *by-name* lookups see Croscore family names instead of the
/// ones the ladder computed.
///
/// The `test_fonts` directory carries no Arial, Times or Courier — it carries
/// the metric-compatible Arimo, Tinos and Cousine — so a hermetic run rewrites
/// the family a lookup asks for. The rewrite belongs **here**, on the two
/// by-name queries, and not on the `/BaseFont` name the ladder starts from: the
/// ladder's base-14 recognition, style parsing and charset decision all run on
/// the document's own spelling, and only the family that finally reaches the
/// database is renamed. Rewriting earlier would hide `Arial,Bold` from the
/// base-14 table and lose the bold with it.
///
/// [`faces`](FontDb::faces) and [`face_bytes`](FontDb::face_bytes) pass
/// straight through: the *installed* names are what the enumeration produced,
/// and the name-normalizing rung matches against those unrenamed.
#[derive(Debug, Clone, Copy)]
pub struct CroscoreDb<'a, D> {
    inner: &'a D,
}

impl<'a, D: FontDb> CroscoreDb<'a, D> {
    /// Wrap a database so its by-name lookups take Croscore family names.
    #[must_use]
    pub fn new(inner: &'a D) -> Self {
        Self { inner }
    }
}

impl<D: FontDb> FontDb for CroscoreDb<'_, D> {
    fn faces(&self) -> &[FaceInfo] {
        self.inner.faces()
    }

    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
        self.inner.face_bytes(h)
    }

    fn find_font(
        &self,
        weight: i32,
        italic: bool,
        charset: Charset,
        pitch: PitchFamily,
        family: &str,
        must_match_name: bool,
    ) -> Option<FaceHandle> {
        self.inner.find_font(
            weight,
            italic,
            charset,
            pitch,
            &super::croscore_name(family),
            must_match_name,
        )
    }

    fn font_by_name(&self, name: &str) -> Option<FaceHandle> {
        self.inner.font_by_name(&super::croscore_name(name))
    }
}

/// A database backed by an explicit list of faces — the test seam, and what a
/// hermetic `--font-dir` run uses.
#[derive(Debug, Clone, Default)]
pub struct TestFontDb {
    faces: Vec<FaceInfo>,
    bytes: Vec<Option<(Arc<[u8]>, u32)>>,
}

impl TestFontDb {
    /// An empty database, in which every lookup fails and the ladder falls all
    /// the way through to the built-in faces.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a face with no bytes, for testing the ladder's decisions
    /// without needing a real font program.
    pub fn push(&mut self, name: &str, styles: u32, charsets: Vec<Charset>) {
        self.faces.push(FaceInfo {
            name: name.to_owned(),
            styles,
            charsets,
        });
        self.bytes.push(None);
    }

    /// Register a face with real bytes.
    pub fn push_with_bytes(
        &mut self,
        name: &str,
        styles: u32,
        charsets: Vec<Charset>,
        bytes: Arc<[u8]>,
    ) {
        self.faces.push(FaceInfo {
            name: name.to_owned(),
            styles,
            charsets,
        });
        self.bytes.push(Some((bytes, 0)));
    }
}

impl FontDb for TestFontDb {
    fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }

    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
        self.bytes.get(h.0).cloned().flatten()
    }
}

/// A database over the operating system's installed fonts, via `fontdb`.
///
/// Scanning is eager and can be slow on a machine with thousands of fonts, so
/// a caller builds one per document at most — which is why it lives behind
/// [`SubstitutionOptions`](super::SubstitutionOptions) rather than being
/// created implicitly.
#[derive(Debug)]
pub struct SystemFontDb {
    faces: Vec<FaceInfo>,
    sources: Vec<(Arc<[u8]>, u32)>,
}

impl SystemFontDb {
    /// Scan the system font directories, plus any extra directories given.
    ///
    /// An empty `extra_dirs` list means "the system's own directories"; a
    /// non-empty one means **only** those, which is how a hermetic run pins
    /// the font set.
    #[must_use]
    pub fn scan(extra_dirs: &[PathBuf]) -> Self {
        let mut db = fontdb::Database::new();
        if extra_dirs.is_empty() {
            db.load_system_fonts();
        } else {
            for dir in extra_dirs {
                db.load_fonts_dir(dir);
            }
        }

        let mut faces = Vec::new();
        let mut sources = Vec::new();
        for face in db.faces() {
            let Some(bytes) = face_bytes_of(&db, face) else {
                continue;
            };
            let Some(info) = describe(&face.families, face.index, &bytes) else {
                continue;
            };
            faces.push(info);
            sources.push((bytes, face.index));
        }

        // Sorted by face name, because the C++'s font list is a
        // `std::map<ByteString, ...>` (`cfx_folderfontinfo.h:98`) and two of
        // the ladder's decisions read it *in order*: a face can only displace
        // the incumbent by scoring strictly higher, so a tie goes to whichever
        // came first, and rung 5 takes the first face claiming the charset
        // outright. Leaving these in `fontdb`'s enumeration order — which is
        // directory order, so it varies by filesystem — would make both
        // answers depend on something the oracle's does not.
        let mut order: Vec<usize> = (0..faces.len()).collect();
        order.sort_by(|&l, &r| match (faces.get(l), faces.get(r)) {
            (Some(l), Some(r)) => l.name.cmp(&r.name),
            _ => std::cmp::Ordering::Equal,
        });
        let sorted_faces = order
            .iter()
            .filter_map(|&i| faces.get(i).cloned())
            .collect();
        let sorted_sources = order
            .iter()
            .filter_map(|&i| sources.get(i).cloned())
            .collect();

        Self {
            faces: sorted_faces,
            sources: sorted_sources,
        }
    }
}

impl FontDb for SystemFontDb {
    fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }

    fn face_bytes(&self, h: FaceHandle) -> Option<(Arc<[u8]>, u32)> {
        self.sources.get(h.0).cloned()
    }
}

fn face_bytes_of(_db: &fontdb::Database, face: &fontdb::FaceInfo) -> Option<Arc<[u8]>> {
    match &face.source {
        fontdb::Source::Binary(data) | fontdb::Source::SharedFile(_, data) => {
            Some(Arc::from(data.as_ref().as_ref()))
        }
        fontdb::Source::File(path) => std::fs::read(path).ok().map(Arc::from),
    }
}

/// Read a face's display name, style bits and charsets from its own tables.
///
/// The charsets come from `OS/2`'s code-page ranges, which `fontdb` does not
/// expose — the reason this crate reads the face itself rather than trusting
/// the database's metadata. ANSI is added unconditionally, matching the folder
/// enumerator, so a face with no code-page claims at all is still usable for
/// Latin text.
fn describe(
    families: &[(String, fontdb::Language)],
    index: u32,
    bytes: &Arc<[u8]>,
) -> Option<FaceInfo> {
    let font = skrifa::FontRef::from_index(bytes, index).ok()?;

    let family = families.first().map(|(n, _)| n.clone()).or_else(|| {
        font.localized_strings(skrifa::string::StringId::FAMILY_NAME)
            .english_or_first()
            .map(|s| s.chars().collect())
    })?;
    let style: String = font
        .localized_strings(skrifa::string::StringId::SUBFAMILY_NAME)
        .english_or_first()
        .map(|s| s.chars().collect())
        .unwrap_or_default();
    let name = if style.is_empty() || style == "Regular" {
        family
    } else {
        format!("{family} {style}")
    };

    let mut styles = 0u32;
    let mut charsets = vec![Charset::Ansi];
    if let Ok(os2) = font.os2() {
        let selection = os2.fs_selection().bits();
        // `fsSelection` bit 0 is italic, bit 5 is bold.
        if selection & 0x01 != 0 {
            styles |= style_bits::ITALIC;
        }
        if selection & 0x20 != 0 {
            styles |= style_bits::FORCE_BOLD;
        }
        // PANOSE byte 0 is the family kind: 2 is Latin text (serif decided by
        // byte 1), 3 is scripts, 4 is decorative.
        let panose = os2.panose_10();
        if panose.first() == Some(&3) {
            styles |= STYLE_SCRIPT;
        }
        if matches!(panose.first(), Some(2)) && !matches!(panose.get(1), Some(0 | 1 | 11..=15)) {
            styles |= STYLE_SERIF;
        }

        let ranges = u64::from(os2.ul_code_page_range_1().unwrap_or(0))
            | (u64::from(os2.ul_code_page_range_2().unwrap_or(0)) << 32);
        for bit in 0..64u32 {
            if ranges & (1u64 << bit) == 0 {
                continue;
            }
            if let Some(c) = charset_for_code_page_bit(bit)
                && !charsets.contains(&c)
            {
                charsets.push(c);
            }
        }
    }
    if font.post().is_ok_and(|p| p.is_fixed_pitch() != 0) {
        styles |= STYLE_FIXED_PITCH;
    }

    Some(FaceInfo {
        name,
        styles,
        charsets,
    })
}

#[cfg(test)]
mod tests {
    // Test fixtures are fixed-size arrays with known contents.
    #![allow(clippy::indexing_slicing)]
    use super::*;

    fn db() -> TestFontDb {
        let mut db = TestFontDb::new();
        db.push("Arial", 0, vec![Charset::Ansi]);
        db.push("Arial Bold", style_bits::FORCE_BOLD, vec![Charset::Ansi]);
        db.push("Arial Italic", style_bits::ITALIC, vec![Charset::Ansi]);
        db.push("Times New Roman", STYLE_SERIF, vec![Charset::Ansi]);
        db.push("Courier New", STYLE_FIXED_PITCH, vec![Charset::Ansi]);
        db.push("MS Gothic", 0, vec![Charset::ShiftJis]);
        db
    }

    /// `cfx_folderfontinfo_unittest.cpp`'s `TestFindFont` name rule, which is
    /// the most consequential row of that test.
    #[test]
    fn a_family_name_must_match_at_a_word_boundary() {
        // The C++'s own worked example.
        assert!(find_family_name_match("Univers", "Univers Bold"));
        assert!(!find_family_name_match("Univers", "Universal"));
        // The oracle test's other rows.
        assert!(!find_family_name_match("Book", "Bookshelf Symbol 7"));
        assert!(find_family_name_match("Bookshelf", "Bookshelf Symbol 7"));
        assert!(find_family_name_match("Tofu", "Tofu"));
        assert!(find_family_name_match("Lato", "Lato"));
        assert!(find_family_name_match("Oxygen", "Oxygen"));
        // A hyphen is not a lowercase letter, so this matches.
        assert!(find_family_name_match("Oxygen", "Oxygen-Sans"));
        // An absent name matches nothing.
        assert!(!find_family_name_match("Helvetica", "Arial"));
    }

    #[test]
    fn an_uppercase_next_character_still_matches() {
        assert!(find_family_name_match("Foo", "FooBar"));
        assert!(!find_family_name_match("Foo", "Foobar"));
    }

    #[test]
    fn the_similarity_score_rewards_agreement_in_both_directions() {
        let plain = FaceInfo {
            name: "Plain".to_owned(),
            styles: 0,
            charsets: vec![Charset::Ansi],
        };
        let bold = FaceInfo {
            name: "Bold".to_owned(),
            styles: style_bits::FORCE_BOLD,
            charsets: vec![Charset::Ansi],
        };
        let p = PitchFamily::default();
        // A plain face scores for a plain request...
        assert!(
            plain.similarity_score(400, false, p, false)
                > bold.similarity_score(400, false, p, false)
        );
        // ...and a bold face for a bold one.
        assert!(
            bold.similarity_score(700, false, p, false)
                > plain.similarity_score(700, false, p, false)
        );
    }

    #[test]
    fn a_perfect_score_is_sixty_eight() {
        let face = FaceInfo {
            name: "F".to_owned(),
            styles: style_bits::FORCE_BOLD | style_bits::ITALIC,
            charsets: vec![Charset::Ansi],
        };
        assert_eq!(
            face.similarity_score(700, true, PitchFamily::default(), true),
            SIMILARITY_SCORE_MAX
        );
    }

    #[test]
    fn the_exact_match_bonus_is_worth_four() {
        let face = FaceInfo {
            name: "F".to_owned(),
            styles: 0,
            charsets: vec![Charset::Ansi],
        };
        let p = PitchFamily::default();
        assert_eq!(
            face.similarity_score(400, false, p, true)
                - face.similarity_score(400, false, p, false),
            4
        );
    }

    #[test]
    fn eligibility_gates_on_charset_unless_the_request_is_default() {
        let face = FaceInfo {
            name: "MS Gothic".to_owned(),
            styles: 0,
            charsets: vec![Charset::ShiftJis],
        };
        assert!(face.is_eligible(Charset::ShiftJis));
        assert!(!face.is_eligible(Charset::Ansi));
        // A Default request makes everything eligible.
        assert!(face.is_eligible(Charset::Default));
    }

    #[test]
    fn find_font_prefers_the_matching_style() {
        let db = db();
        let bold = db
            .find_font(
                700,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Arial",
                true,
            )
            .expect("a face is found");
        assert_eq!(db.faces()[bold.0].name, "Arial Bold");

        let italic = db
            .find_font(
                400,
                true,
                Charset::Ansi,
                PitchFamily::default(),
                "Arial",
                true,
            )
            .expect("a face is found");
        assert_eq!(db.faces()[italic.0].name, "Arial Italic");
    }

    #[test]
    fn find_font_respects_the_charset_gate() {
        let db = db();
        let jp = db
            .find_font(
                400,
                false,
                Charset::ShiftJis,
                PitchFamily::default(),
                "MS Gothic",
                true,
            )
            .expect("the Japanese face is found");
        assert_eq!(db.faces()[jp.0].name, "MS Gothic");
        // A charset nothing claims finds nothing.
        assert!(
            db.find_font(
                400,
                false,
                Charset::Hebrew,
                PitchFamily::default(),
                "Arial",
                true
            )
            .is_none()
        );
    }

    #[test]
    fn find_font_falls_back_to_courier_new_for_a_fixed_ansi_request() {
        let mut db = TestFontDb::new();
        // Nothing whose name matches, so the scan finds nothing...
        db.push("Courier New", STYLE_FIXED_PITCH, vec![Charset::Ansi]);
        let h = db.find_font(
            400,
            false,
            Charset::Ansi,
            PitchFamily(PitchFamily::FIXED),
            "NoSuchFamily",
            true,
        );
        // ...and the hard-coded consolation prize applies.
        assert_eq!(
            h.map(|h| db.faces()[h.0].name.as_str()),
            Some("Courier New")
        );
    }

    #[test]
    fn the_name_gate_is_lifted_when_must_match_name_is_false() {
        let db = db();
        // With the gate on, a family nothing carries finds nothing...
        assert!(
            db.find_font(
                400,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Zzz",
                true
            )
            .is_none()
        );
        // ...and with it off, the best-scoring eligible face wins.
        assert!(
            db.find_font(
                400,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Zzz",
                false
            )
            .is_some()
        );
    }

    #[test]
    fn match_installed_scans_in_reverse_so_a_later_face_shadows_an_earlier() {
        let mut db = TestFontDb::new();
        db.push("Foo Bar", 0, vec![Charset::Ansi]);
        db.push("FooBar", 0, vec![Charset::Ansi]);
        // Both normalize to "foobar"; the later one wins.
        assert_eq!(db.match_installed("foobar").as_deref(), Some("FooBar"));
        assert_eq!(db.match_installed("nothing"), None);
    }

    #[test]
    fn an_empty_database_answers_nothing() {
        let db = TestFontDb::new();
        assert!(db.faces().is_empty());
        assert!(
            db.find_font(
                400,
                false,
                Charset::Ansi,
                PitchFamily::default(),
                "Arial",
                true
            )
            .is_none()
        );
        assert!(db.font_by_name("Arial").is_none());
        assert!(db.face_bytes(FaceHandle(0)).is_none());
    }
}
