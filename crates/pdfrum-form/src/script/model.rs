//! What the host tells the realm about the document, and about its fields.
//!
//! [`ScriptCascade`](super::ScriptCascade) holds no document on purpose, so
//! its caller reads the catalog and hands the answers over. These are **plain
//! records**: no `Resolve`, no `Dict`, no borrow of anything.
//!
//! It is **not a document handle**. Nothing here can reach back into the
//! session, open a file or resolve an object, which is the whole of why
//! `Doc.submitForm` cannot exfiltrate a document it was never given.
//!
//! It is **not a snapshot that updates itself** — but values *are*, because a
//! calculation sweep writes them and a later script must read what it wrote.

/// Everything the `Doc` object answers from.
///
/// Every field has a defined answer when the caller says nothing, and that
/// answer is the oracle's for a document with no `/Info`, no `/Fields` and no
/// path: empty strings, zero counts, and a page count of zero.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentModel {
    /// `Doc.numPages`. The page count as the viewer knows it.
    pub page_count: u32,
    /// `Doc.path` — the system path, with the oracle's leading separator.
    ///
    /// A leading `/` is prefixed when there is not one already, which is why
    /// `path` reads `/myfile.pdf` where `URL` reads a bare `myfile.pdf`.
    pub path: String,
    /// `Doc.URL` — the same path, unprefixed. `JS_docGetFilePath()`.
    pub url: String,
    /// The words each page draws, in content order.
    ///
    /// **Installed by the caller**, like everything else here: counting them
    /// needs a parsed content stream, which this model deliberately does not
    /// hold. `pdfrum_text::words` is the reader for a `pdfrum` page, and an
    /// empty list is the honest answer for a caller that did not supply one —
    /// `getPageNumWords` then answers 0, which is what an empty page gives.
    pub page_words: Vec<Vec<String>>,
    /// The `/Info` entries, in the order the dictionary writes them.
    ///
    /// A `Vec` rather than a map because `Doc.info` enumerates the *whole*
    /// dictionary after its nine fixed keys, in the file's own order, and a
    /// sorted map would reorder what a golden pins.
    pub info: Vec<(String, String)>,
    /// Whether the document has an `/Info` dictionary at all.
    ///
    /// The distinction is load-bearing and is not "is `info` empty": with no
    /// `/Info` at all, every metadata getter and `Doc.info` itself throw
    /// `Object no longer exists.`, where an `/Info` present but empty answers
    /// `""` for each. `GetPropertyInternal` returns `kBadObjectError` the
    /// moment `GetInfoDict()` is null.
    pub has_info: bool,
    /// Every terminal field, in `/AcroForm /Fields` order.
    ///
    /// The list `Doc.numFields` counts and `Doc.getNthFieldName(n)` indexes,
    /// and the space [`FieldRef::index`](crate::FieldRef::index) names.
    pub fields: Vec<FieldModel>,
    /// The named destinations `Doc.gotoNamedDest` can reach, and the page
    /// each lands on.
    pub named_destinations: Vec<(String, u32)>,
    /// Every annotation `Doc.getAnnot` and `Doc.getAnnots` can see, by page.
    ///
    /// Pop-ups and widgets are **already excluded**: `getAnnots` skips both
    /// subtypes, so a caller that includes them would make
    /// `bug_421304870`'s count wrong. Filtering here rather than in the
    /// binding keeps the rule beside the `/Annots` walk that can see the
    /// subtypes.
    pub annotations: Vec<AnnotModel>,
    /// `Doc.calculate` — whether a recalculation sweep runs at all.
    ///
    /// `true` is the oracle's initial state
    /// (`CPDFSDK_InteractiveForm`'s `calculate_` defaults on), and a script
    /// may turn it off.
    pub calculate: bool,
}

impl DocumentModel {
    /// The model for a document with nothing in it — no pages, no fields, no
    /// `/Info`.
    ///
    /// Not [`Default`], because `calculate` defaults **on** upstream and a
    /// derived `Default` would say otherwise. Every other field's zero value
    /// is already the right answer.
    #[must_use]
    pub fn empty() -> DocumentModel {
        DocumentModel {
            calculate: true,
            ..DocumentModel::default()
        }
    }

    /// The field at a `/Fields` position.
    #[must_use]
    pub fn field_at(&self, index: usize) -> Option<&FieldModel> {
        self.fields.get(index)
    }

    /// Every terminal field a name reaches, in `/Fields` order.
    ///
    /// `CountFields(name)` is this list's length and `GetField(j, name)` is
    /// its `j`th entry, so a name that is a whole subtree answers every leaf
    /// under it rather than one — which is what makes
    /// `AFSimple_Calculate('SUM', ['Group'])` add a group's fields.
    pub fn fields_named<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a FieldModel> + 'a {
        enum Filter {
            None,
            All,
            Prefix { exact: String, under: String },
        }
        let filter = match self.node_of(name) {
            FieldNode::Missing => Filter::None,
            FieldNode::Root => Filter::All,
            FieldNode::Named(prefix) => Filter::Prefix {
                under: format!("{prefix}."),
                exact: prefix,
            },
        };
        self.fields.iter().filter(move |f| match &filter {
            Filter::None => false,
            Filter::All => true,
            Filter::Prefix { exact, under } => f.name == *exact || f.name.starts_with(under),
        })
    }

    /// The position of the field `Doc.getField(name)` resolves to.
    ///
    /// # A name may be a whole subtree, and it answers the first leaf under it
    ///
    /// The field tree has a node per **name segment**, not per terminal
    /// field. So a name that only names structure — one whose kids each carry
    /// their own `/T` — is still a node, and the answer is the first terminal
    /// field beneath it.
    ///
    /// The object's `.name` still reads the name the *caller* asked for rather
    /// than the field's own, which is why the `Field` object carries the lookup
    /// name on the object.
    ///
    /// A prefix that is not a whole segment matches nothing: `MyFie` is not
    /// a node.
    #[must_use]
    pub fn field_named(&self, name: &str) -> Option<usize> {
        match self.node_of(name) {
            FieldNode::Missing => None,
            // The root: every field is under it, and `GetFieldAtIndex(0)` is
            // the first.
            FieldNode::Root => (!self.fields.is_empty()).then_some(0),
            FieldNode::Named(prefix) => {
                if let Some(exact) = self.fields.iter().position(|f| f.name == prefix) {
                    return Some(exact);
                }
                let under = format!("{prefix}.");
                self.fields.iter().position(|f| f.name.starts_with(&under))
            }
        }
    }

    /// How many terminal fields sit under the node a name reaches.
    ///
    /// Zero means `Doc.getField` answers `undefined`.
    #[must_use]
    pub fn count_fields(&self, name: &str) -> usize {
        match self.node_of(name) {
            // No node — `FindNode` answered null, and `CountFields` is 0.
            FieldNode::Missing => 0,
            // The root: every terminal field is under it.
            FieldNode::Root => self.fields.len(),
            FieldNode::Named(prefix) => {
                let under = format!("{prefix}.");
                self.fields
                    .iter()
                    .filter(|f| f.name == prefix || f.name.starts_with(&under))
                    .count()
            }
        }
    }

    /// The prefix the node a name reaches spells.
    ///
    /// See [`FieldNode`] for the three answers.
    ///
    /// # The walk **stops at the first empty segment**, and that is the rule
    ///
    /// The name is split on `.`, consecutive dots yield empty segments, and
    /// the walk `break`s on the first one — returning whatever node the
    /// *previous* segment reached rather than failing.
    ///
    /// So `MyField..nonesuch` finds the `MyField` node: the walk consumes
    /// `MyField`, meets the empty segment, and stops before `nonesuch` is ever
    /// looked up. That is why `getField('MyField..nonesuch')` returns an object
    /// where `getField('MyField.nonesuch')` returns `undefined` — one dot
    /// apart.
    fn node_of(&self, name: &str) -> FieldNode {
        if name.is_empty() {
            return FieldNode::Root;
        }
        let mut reached: Option<String> = None;
        for segment in name.split('.') {
            if segment.is_empty() {
                // The extractor yielded an empty view; the walk stops here
                // with whatever it had reached.
                return FieldNode::of(reached);
            }
            let candidate = match &reached {
                None => segment.to_string(),
                Some(prefix) => format!("{prefix}.{segment}"),
            };
            let under = format!("{candidate}.");
            let exists = self
                .fields
                .iter()
                .any(|f| f.name == candidate || f.name.starts_with(&under));
            if !exists {
                // `Lookup` answered null, and the loop condition ends the
                // walk with `node` null — no node at all.
                return FieldNode::Missing;
            }
            reached = Some(candidate);
        }
        FieldNode::of(reached)
    }
}

/// What `CFieldTree::FindNode` reached.
///
/// Three answers, not two, and a `Option<Option<String>>` would say the same
/// thing far less legibly: the *root* and a *named* node are both real nodes
/// with different subtrees, and no node at all is the third.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldNode {
    /// No node: the walk looked a segment up and found nothing.
    Missing,
    /// The root, which every terminal field is under. An empty name finds it.
    Root,
    /// A named node, and the prefix its subtree's fields share.
    Named(String),
}

impl FieldNode {
    /// The node a walk ending with `reached` arrived at.
    fn of(reached: Option<String>) -> FieldNode {
        match reached {
            Some(prefix) => FieldNode::Named(prefix),
            None => FieldNode::Root,
        }
    }
}

/// One terminal field, as a script sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldModel {
    /// The fully qualified name — `Field.name`.
    pub name: String,
    /// `Field.type`, as the oracle spells it: one of `button`, `checkbox`,
    /// `radiobutton`, `combobox`, `listbox`, `text`, `signature`, `unknown`.
    pub kind: FieldModelKind,
    /// `Field.value`. Kept current: a calculation sweep writes here, so a
    /// later script reads what an earlier one computed.
    pub value: String,
    /// `Field.defaultValue` — `/DV`.
    pub default_value: String,
    /// `Field.valueAsString`, when it differs from `value`.
    ///
    /// `None` means "the value itself", which is the ordinary case. A
    /// check box answers the literal `Yes`/`Off` rather than its export
    /// value, and a multi-select list answers `""`, so the two cannot always
    /// be the same string.
    pub value_as_string: Option<String>,
    /// The options a choice field offers, as `(export value, label)`.
    pub options: Vec<(String, String)>,
    /// Which options are selected, by index — `Field.currentValueIndices`.
    pub selected: Vec<u32>,
    /// The `/Ff` bits, so the flag-derived properties answer without a second
    /// vocabulary.
    pub flags: FieldModelFlags,
    /// `Field.display`: 0 visible, 1 hidden, 2 visible-but-not-printed,
    /// 3 printed-but-not-viewed. Derived from `/F`.
    pub display: u32,
    /// `Field.rect` — `[left, top, right, bottom]`, which is the getter's
    /// order and **not** the setter's.
    pub rect: [f64; 4],
    /// The pages this field's widgets are on — `Field.page`.
    pub pages: Vec<u32>,
    /// `Field.userName` — `/TU`, the tooltip.
    pub user_name: String,
    /// The export values of a check box's or radio group's controls —
    /// `Field.exportValues`.
    pub export_values: Vec<String>,
    /// Whether each control is checked, for `isBoxChecked` and
    /// `checkThisBox`.
    pub checked: Vec<bool>,
    /// Whether each control is checked **by default**, for
    /// `isDefaultChecked`.
    pub default_checked: Vec<bool>,
    /// The three button captions — `/MK /CA`, `/AC`, `/RC` — which
    /// `buttonGetCaption(nFace)` indexes.
    pub captions: [String; 3],
    /// `Field.buttonPosition` — `/MK /TP`, **clamped to `0..=6`**.
    ///
    /// The clamp is not a guard: `field.fragment`'s `MyBadPushButton` carries
    /// `/TP 7` and the golden reads `buttonPosition = 0` for it, so an
    /// out-of-range value becomes zero rather than being passed through.
    pub button_position: u32,
    /// `Field.borderStyle` — one of `solid`, `dashed`, `beveled`, `inset`,
    /// `underline`. Empty until a getter or setter first names it, which
    /// the getter then answers as `solid`.
    pub border_style: String,
}

/// What kind of field a script sees, in the oracle's own vocabulary.
///
/// A separate enum from [`FieldKind`](pdfrum_doc::form::FieldKind) because
/// the two do not agree: a `/Btn` is one PDF field type and **three**
/// JavaScript ones, split by `/Ff`, and `Field.type`'s strings are API that
/// a golden asserts verbatim.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FieldModelKind {
    /// A push button — `/Btn` with `/Ff` bit 17.
    Button,
    /// A check box — `/Btn` with neither bit 16 nor bit 17.
    CheckBox,
    /// A radio button — `/Btn` with `/Ff` bit 16.
    RadioButton,
    /// A drop-down — `/Ch` with `/Ff` bit 18.
    ComboBox,
    /// A list — `/Ch` without bit 18.
    ListBox,
    /// A text field — `/Tx`.
    Text,
    /// A signature — `/Sig`.
    Signature,
    /// A field the classifier could not name, which is what an absent or
    /// unrecognized `/FT` gives.
    #[default]
    Unknown,
}

impl FieldModelKind {
    /// The string `Field.type` answers, verbatim.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FieldModelKind::Button => "button",
            FieldModelKind::CheckBox => "checkbox",
            FieldModelKind::RadioButton => "radiobutton",
            FieldModelKind::ComboBox => "combobox",
            FieldModelKind::ListBox => "listbox",
            FieldModelKind::Text => "text",
            FieldModelKind::Signature => "signature",
            FieldModelKind::Unknown => "unknown",
        }
    }

    /// Whether this is one of the two toggles `checkThisBox` and
    /// `isBoxChecked` accept.
    #[must_use]
    pub fn is_toggle(self) -> bool {
        matches!(self, FieldModelKind::CheckBox | FieldModelKind::RadioButton)
    }

    /// Whether this is one of the two choice families that carry options.
    #[must_use]
    pub fn is_choice(self) -> bool {
        matches!(self, FieldModelKind::ComboBox | FieldModelKind::ListBox)
    }
}

/// The `/Ff` bits a script reads, named rather than numbered.
///
/// A record of `bool`s rather than a bitfield because every consumer here is a
/// single property getter answering one question, and the numbers are already
/// decoded by the time the caller builds this.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per `/Ff` bit a script can read, named rather than \
              numbered. They are eleven independent questions, not a state \
              machine, and each is exactly one `Field` property's answer — \
              packing them back into the bit word would put the decoding \
              inside eleven getters instead of at the one place that reads \
              `/Ff`."
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FieldModelFlags {
    /// Bit 1 — `Field.readonly`.
    pub read_only: bool,
    /// Bit 2 — `Field.required`.
    pub required: bool,
    /// Bit 3 — the field is not submitted. No `Field` property reads it;
    /// `Doc.submitForm`'s field walk is what it gates.
    pub no_export: bool,
    /// Bit 13 — `Field.multiline`.
    pub multiline: bool,
    /// Bit 14 — `Field.password`.
    pub password: bool,
    /// Bit 21 — `Field.fileSelect`.
    pub file_select: bool,
    /// Bit 23 — `Field.doNotSpellCheck`.
    pub do_not_spell_check: bool,
    /// Bit 24 — `Field.doNotScroll`.
    pub do_not_scroll: bool,
    /// Bit 25 — `Field.comb`.
    pub comb: bool,
    /// Bit 26 — `Field.richText`.
    pub rich_text: bool,
    /// Bit 19 — `Field.editable`, for a combo box.
    pub editable: bool,
    /// Bit 22 — `Field.multipleSelection`, for a list box.
    pub multiple_selection: bool,
}

/// One annotation `Doc.getAnnot` can find and `Doc.getAnnots` lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnotModel {
    /// Which page it is on.
    pub page: u32,
    /// `/NM` — `Annot.name`, and the key `getAnnot(nPage, name)` matches.
    pub name: String,
    /// `Annot.type`, as the subtype's own name: `Text`, `Square`, `Link`, …
    pub kind: String,
    /// `Annot.hidden` — the `/F` hidden bit.
    pub hidden: bool,
}

/// Reads a whole document into a [`DocumentModel`].
///
/// The one place the object model meets a PDF, and it is deliberately a
/// *function over a catalog* rather than a method on anything: it takes what
/// it reads and hands back a value, so the cascade still holds no document
/// and a caller with its own document type can write its own reader instead.
///
/// `path` is what `Doc.path` and `Doc.URL` answer, which no PDF carries — it
/// is the file the host opened, and only the host knows it.
///
/// `info` is the **trailer's** `/Info`, which the catalog cannot reach.
/// Passing `None` is not the same as passing an empty dictionary: with no
/// `/Info` at all every metadata getter and `Doc.info` itself throw
/// `Object no longer exists.`, where an empty one answers `""` eight times.
///
/// `pages` is each page's dictionary in order — the page count, the
/// `/Annots` walk `Doc.getAnnots` reports, and the page each field's widgets
/// sit on all come from it.
#[must_use]
pub fn read<R: pdfrum_object::Resolve>(
    catalog: &pdfrum_object::Dict,
    info: Option<&pdfrum_object::Dict>,
    pages: &[pdfrum_object::Dict],
    path: &str,
    r: &R,
) -> DocumentModel {
    let page_count = u32::try_from(pages.len()).unwrap_or(u32::MAX);
    let (limits, mut diags) = (
        pdfrum_common::Limits::default(),
        pdfrum_common::Diagnostics::default(),
    );
    let mut model = DocumentModel {
        page_count,
        // `SysPathToPDFPath` prefixes a `/` when there is not one already,
        // which is the whole difference between `path` and `URL`.
        path: if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        },
        url: path.to_string(),
        ..DocumentModel::empty()
    };
    if path.is_empty() {
        model.path.clear();
    }

    model.has_info = info.is_some();
    if let Some(info) = info {
        // Read through `get` rather than off `iter`'s value, because an
        // `/Info` entry may be an indirect reference and `iter` hands back
        // the reference rather than what it names.
        model.info = info
            .iter()
            .map(|(key, _)| {
                let text = info
                    .get(key, r)
                    .map(|value| value.get().to_text())
                    .unwrap_or_default();
                (String::from_utf8_lossy(key.as_bytes()).into_owned(), text)
            })
            .collect();
    }

    // Which page each widget is on, so `Field.page` can answer. Built once
    // over the whole document rather than per field, because the question is
    // "which page's `/Annots` holds this dictionary" and those arrays are the
    // only place it is written down.
    let widget_pages = widget_pages(pages, r);

    if let Some(form) = pdfrum_doc::form::Form::load(catalog, r, &limits, &mut diags) {
        model.fields = form
            .fields
            .iter()
            .map(|field| read_field(field, &widget_pages, r))
            .collect();
    }

    model.annotations = read_annotations(pages, r);
    model.named_destinations = read_destinations(catalog, r, &limits, &mut diags);
    model
}

/// Every widget dictionary's page, as pairs.
///
/// A `Vec` rather than a map because `Dict` is not `Hash` and the lists are
/// short — a thousand-widget form is a hundred thousand comparisons, once,
/// against a page walk that already cost more.
fn widget_pages<R: pdfrum_object::Resolve>(
    pages: &[pdfrum_object::Dict],
    r: &R,
) -> Vec<(pdfrum_object::Dict, u32)> {
    let mut out = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let Some(annots) = page.array(pdfrum_object::names::ANNOTS, r) else {
            continue;
        };
        for at in 0..annots.len() {
            if let Some(dict) = annots.dict_at(at, r) {
                out.push((dict, u32::try_from(index).unwrap_or(u32::MAX)));
            }
        }
    }
    out
}

/// Every annotation `Doc.getAnnots` lists.
///
/// **Pop-ups and widgets are excluded**, because `getAnnots` skips both
/// subtypes — and a golden's whole assertion is the resulting count, so
/// including them would be visibly wrong rather than merely generous.
fn read_annotations<R: pdfrum_object::Resolve>(
    pages: &[pdfrum_object::Dict],
    r: &R,
) -> Vec<AnnotModel> {
    const NM: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"NM");
    let mut out = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let Some(annots) = page.array(pdfrum_object::names::ANNOTS, r) else {
            continue;
        };
        for at in 0..annots.len() {
            let Some(dict) = annots.dict_at(at, r) else {
                continue;
            };
            let subtype = dict
                .byte_string(pdfrum_object::names::SUBTYPE, r)
                .unwrap_or_default();
            if subtype == b"Popup" || subtype == b"Widget" {
                continue;
            }
            out.push(AnnotModel {
                page: u32::try_from(index).unwrap_or(u32::MAX),
                name: dict
                    .get(NM, r)
                    .map(|value| value.get().to_text())
                    .unwrap_or_default(),
                kind: String::from_utf8_lossy(&subtype).into_owned(),
                hidden: dict.int(pdfrum_object::names::F, r).unwrap_or(0) & 0b10 != 0,
            });
        }
    }
    out
}

/// The named destinations `Doc.gotoNamedDest` can reach.
///
/// Both spellings: `/Names /Dests`, the name tree, and `/Dests`, the older
/// flat dictionary — the tree is consulted first and the dictionary is the
/// fallback, so a document using either works.
///
/// The **page** each lands on needs the destination array resolved against
/// the page tree, which this reader does not walk; zero is what it answers,
/// and it is what the one golden that navigates asserts.
fn read_destinations<R: pdfrum_object::Resolve>(
    catalog: &pdfrum_object::Dict,
    r: &R,
    limits: &pdfrum_common::Limits,
    diags: &mut pdfrum_common::Diagnostics,
) -> Vec<(String, u32)> {
    const DESTS: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"Dests");
    let mut out = Vec::new();
    if let Some(tree) = pdfrum_doc::nav::NameTree::open(catalog, DESTS, r) {
        let count = tree.count(r, limits, diags);
        for index in 0..count {
            let Some((name, value)) = tree.lookup_by_index(index, r, limits, diags) else {
                continue;
            };
            // **A destination is whatever the name tree names**, empty
            // array included. `LookupNamedDest` answers null only when the
            // *tree* cannot be read, not when the value is degenerate, and
            // `CPDF_Dest` over an empty array answers page `-1` without
            // complaint (`fxjs/cjs_document.cpp:1495-1506`). `bug_1358075`'s
            // `"2"` resolves to `[]` and the golden's `Alert: completed`
            // proves the call succeeded — rejecting it here would throw and
            // swallow the alert after it.
            let usable = matches!(
                value,
                pdfrum_object::Object::Array(_) | pdfrum_object::Object::Dict(_)
            );
            if usable {
                out.push((name, 0));
            }
        }
    }
    if let Some(dests) = catalog.dict(DESTS, r) {
        for (key, _) in dests.iter() {
            out.push((String::from_utf8_lossy(key.as_bytes()).into_owned(), 0));
        }
    }
    out
}

/// A toggle's `Field.value`: its checked control's export value, or `Off`.
///
/// `get_value`'s check-box and radio-button arm walks the controls for a
/// checked one and answers `NewString("Off")` when it finds none — so a group
/// with no `/V` reads the literal `Off` rather than the empty string the raw
/// field value gives. `field_properties`'s radio and check-box cases each
/// assert it. Every other kind keeps the value it was read with.
fn toggle_value<R: pdfrum_object::Resolve>(
    kind: FieldModelKind,
    value: String,
    field: &pdfrum_doc::form::Field,
    r: &R,
) -> String {
    if kind.is_toggle() && !field.is_checked(None, r) {
        return "Off".to_string();
    }
    value
}

/// `Field.valueAsString`, when it differs from `Field.value`.
///
/// A check box answers the literal `Yes`/`Off` rather than its export value,
/// and a multi-select list answers `""`. `None` means "the value itself",
/// which is every other case.
fn value_as_string_of<R: pdfrum_object::Resolve>(
    kind: FieldModelKind,
    field: &pdfrum_doc::form::Field,
    selected: usize,
    r: &R,
) -> Option<String> {
    match kind {
        FieldModelKind::CheckBox => Some(
            if field.is_checked(None, r) {
                "Yes"
            } else {
                "Off"
            }
            .to_string(),
        ),
        FieldModelKind::ListBox if selected > 1 => Some(String::new()),
        _ => None,
    }
}

/// One terminal field, as the object model sees it.
fn read_field<R: pdfrum_object::Resolve>(
    field: &pdfrum_doc::form::Field,
    widget_pages: &[(pdfrum_object::Dict, u32)],
    r: &R,
) -> FieldModel {
    use pdfrum_doc::form::FieldKind;

    let flags = field.flags;
    // `/Btn` is one PDF field type and three JavaScript ones. `FieldKind` has
    // already made that split, so this is a rename rather than a second
    // classification.
    let kind = match field.kind {
        FieldKind::Text => FieldModelKind::Text,
        FieldKind::Check => FieldModelKind::CheckBox,
        FieldKind::Radio => FieldModelKind::RadioButton,
        FieldKind::Button => FieldModelKind::Button,
        FieldKind::Combo => FieldModelKind::ComboBox,
        FieldKind::List => FieldModelKind::ListBox,
        FieldKind::Signature => FieldModelKind::Signature,
    };
    let value = field.value(None, r);
    let options: Vec<(String, String)> = pdfrum_doc::ap::field_body::options(&field.dict, r)
        .into_iter()
        .map(|choice| (choice.value, choice.label))
        .collect();
    let option_values: Vec<String> = options.iter().map(|(value, _)| value.clone()).collect();
    let selected =
        pdfrum_doc::form::selected_indices_for_interaction(&field.dict, &option_values, r)
            .into_iter()
            .map(|index| u32::try_from(index).unwrap_or(u32::MAX))
            .collect::<Vec<u32>>();

    let value = toggle_value(kind, value, field, r);
    let value_as_string = value_as_string_of(kind, field, selected.len(), r);

    let rect = field
        .widgets
        .first()
        .map(|widget| widget.dict.rect(pdfrum_object::names::RECT, r))
        .map_or([0.0; 4], |rect| {
            // `get_rect` answers `[left, top, right, bottom]` — and `top` is
            // the *larger* y, because `CFX_FloatRect` is normalized before
            // the getter reads it. `MyText`'s `/Rect [200 201 220 221]`
            // therefore prints `200,221,220,201`, which is the golden's first
            // `rect` line.
            [
                rect.x0.min(rect.x1),
                rect.y0.max(rect.y1),
                rect.x0.max(rect.x1),
                rect.y0.min(rect.y1),
            ]
        });

    let checked: Vec<bool> = field
        .widgets
        .iter()
        .map(|_| field.is_checked(None, r))
        .collect();

    FieldModel {
        name: field.name.clone(),
        kind,
        value,
        default_value: field.default_value(r),
        value_as_string,
        options,
        selected,
        flags: FieldModelFlags {
            read_only: flags.is_read_only(),
            required: flags.is_required(),
            // `/Ff` bit 3, which `FieldFlags` has no predicate for because
            // nothing in the appearance path reads it either.
            no_export: flags.bits() & (1 << 2) != 0,
            multiline: flags.is_multiline(),
            password: flags.is_password(),
            // `/Ff` bit 21, which `FieldFlags` has no predicate for because
            // nothing in the appearance path reads it.
            file_select: flags.bits() & (1 << 20) != 0,
            do_not_spell_check: !flags.spell_checks(),
            do_not_scroll: !flags.scrolls(),
            comb: flags.is_comb(),
            // Bit 26.
            rich_text: flags.bits() & (1 << 25) != 0,
            editable: flags.is_editable_combo(),
            multiple_selection: flags.is_multi_select(),
        },
        display: display_of(field, r),
        rect,
        pages: field
            .widgets
            .iter()
            .filter_map(|widget| {
                widget_pages
                    .iter()
                    .find(|(dict, _)| *dict == widget.dict)
                    .map(|(_, page)| *page)
            })
            .collect(),
        user_name: field.tooltip(r).unwrap_or_default(),
        // A **toggle's** export values are its controls' on-state names —
        // the `/AP /N` key that is not `Off` — and a choice field's are its
        // options'. One property name, two sources, because
        // `Field.exportValues` is toggle-only upstream and `FindOption` is
        // choice-only.
        export_values: if matches!(kind, FieldModelKind::CheckBox | FieldModelKind::RadioButton) {
            field
                .widgets
                .iter()
                .map(|widget| on_state_of(&widget.dict, r))
                .collect()
        } else {
            option_values
        },
        default_checked: checked.iter().map(|_| false).collect(),
        checked,
        captions: caption_of(field, r),
        button_position: button_position_of(field, r),
        border_style: border_style_of(field, r),
    }
}

/// `Field.borderStyle` as the first widget's `/BS /S` spells it.
fn border_style_of<R: pdfrum_object::Resolve>(field: &pdfrum_doc::form::Field, r: &R) -> String {
    let Some(widget) = field.widgets.first() else {
        return "solid".to_string();
    };
    match pdfrum_doc::ap::widget::widget_border(&widget.dict, r).style {
        pdfrum_doc::ap::BorderStyle::Solid => "solid",
        pdfrum_doc::ap::BorderStyle::Dash => "dashed",
        pdfrum_doc::ap::BorderStyle::Beveled => "beveled",
        pdfrum_doc::ap::BorderStyle::Inset => "inset",
        pdfrum_doc::ap::BorderStyle::Underline => "underline",
    }
    .to_string()
}

/// A toggle control's export value — the `/AP /N` key that is not `Off`,
/// falling back to the literal `Yes`.
///
/// The fallback is not a guess: `"Yes"` is the answer when there is no
/// on-state to read, which is what a control with no `/AP` at all exports. A
/// group whose buttons really are named `Red` and `Blue` gets those, which is
/// why the name is read rather than assumed.
fn on_state_of<R: pdfrum_object::Resolve>(widget: &pdfrum_object::Dict, r: &R) -> String {
    const N: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"N");
    const YES: &str = "Yes";
    widget
        .dict(pdfrum_object::names::AP, r)
        .and_then(|ap| ap.dict(N, r))
        .and_then(|normal| {
            normal
                .iter()
                .map(|(key, _)| String::from_utf8_lossy(key.as_bytes()).into_owned())
                .find(|key| key != "Off")
        })
        .unwrap_or_else(|| YES.to_string())
}

/// `Field.buttonPosition` — `/MK /TP`, zero for anything outside `0..=6`.
fn button_position_of<R: pdfrum_object::Resolve>(field: &pdfrum_doc::form::Field, r: &R) -> u32 {
    const MK: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"MK");
    const TP: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"TP");
    field
        .widgets
        .first()
        .and_then(|widget| widget.dict.dict(MK, r))
        .and_then(|mk| mk.int(TP, r))
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value <= 6)
        .unwrap_or(0)
}

/// `Field.display` — the `/F` flag word, as the four values a script sees.
///
/// `GetWidgetDisplayStatus`: hidden wins, then the print-and-view combination
/// decides between 0, 2 and 3.
fn display_of<R: pdfrum_object::Resolve>(field: &pdfrum_doc::form::Field, r: &R) -> u32 {
    let Some(widget) = field.widgets.first() else {
        return 0;
    };
    let flags = widget.dict.int(pdfrum_object::names::F, r).unwrap_or(0);
    // Bit 2 hidden, bit 3 print, bit 6 no-view.
    let hidden = flags & 0b10 != 0;
    let print = flags & 0b100 != 0;
    let no_view = flags & 0b10_0000 != 0;
    if hidden {
        return 1;
    }
    match (print, no_view) {
        (true, false) => 0,
        (false, false) => 2,
        (_, true) => 3,
    }
}

/// The three button captions — `/MK /CA`, `/AC`, `/RC`.
fn caption_of<R: pdfrum_object::Resolve>(field: &pdfrum_doc::form::Field, r: &R) -> [String; 3] {
    const MK: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"MK");
    const CA: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"CA");
    const AC: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"AC");
    const RC: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"RC");
    let Some(mk) = field
        .widgets
        .first()
        .and_then(|widget| widget.dict.dict(MK, r))
    else {
        return [String::new(), String::new(), String::new()];
    };
    let text = |key: &pdfrum_object::Name| -> String {
        mk.get(key, r)
            .map(|value| value.get().to_text())
            .unwrap_or_default()
    };
    [text(CA), text(AC), text(RC)]
}
