//! Naming the resources a regenerated stream refers to (ISO 32000-1 §7.8.3).
//!
//! A content stream cannot hold a font, an image or a graphics-state
//! dictionary; it holds a *name*, and the page's `/Resources` says what the
//! name means. So regenerating a stream means allocating names, and the rules
//! for that are load-bearing rather than cosmetic: the oracle's own regenerated
//! page allocates the same names in the same order, and conformance compares
//! the two.
//!
//! # `FX` plus one letter plus a number, restarting at one every time
//!
//! A name is `FX` + the category's first letter + a number: `FXF1` for a font,
//! `FXX1` for an image *or* a form — both live in `/XObject` — and `FXE1` for
//! a graphics state. The search restarts at 1 on **every** call and takes the
//! first free slot, so it is not a counter: allocate three names and free the
//! second, and the next allocation reuses the middle number.
//!
//! # Removed entries still reserve their names
//!
//! An entry the sweep removed is parked rather than dropped, because a later
//! regeneration may need it back. A parked name is therefore *not* free — a
//! fresh allocation skips it, or restoring the parked entry would collide with
//! whatever took its name.
//!
//! # Only three categories exist here
//!
//! `/ExtGState`, `/Font` and `/XObject`. Colour spaces, patterns, shadings,
//! `/Properties` and `/ProcSet` are neither created, swept, nor preserved —
//! which is a real gap, and one we keep: a regenerated stream never refers to
//! any of them, because the emitter never writes an operator that would.

use std::collections::{BTreeMap, BTreeSet};

use pdfrum_object::{Dict, Name, ObjRef, Object};

/// The three resource categories a regenerated page maintains.
///
/// In the order the C++ sweeps them, which is the order removals are recorded
/// in and therefore the order a re-run reallocates in.
pub const CATEGORIES: [&str; 3] = ["ExtGState", "Font", "XObject"];

/// The resource dictionaries of one page, while its streams are regenerated.
///
/// Holds the sub-dictionary per category plus the entries a previous sweep
/// parked, so a name allocation can avoid both. A plain record: the four
/// functions below are the operations.
#[derive(Debug, Clone, Default)]
pub struct ResourceTable {
    /// The live entries, per category, in insertion order.
    entries: BTreeMap<String, Dict>,
    /// Entries a sweep removed, per category. They still reserve their names.
    parked: BTreeMap<String, BTreeMap<Name, Object>>,
}

impl ResourceTable {
    /// Read the three categories out of a page's `/Resources`.
    ///
    /// Every other key — colour spaces, patterns, `/ProcSet` — is left where
    /// it is and carried through untouched.
    #[must_use]
    pub fn load(resources: &Dict, r: &impl pdfrum_object::Resolve) -> Self {
        let mut entries = BTreeMap::new();
        for category in CATEGORIES {
            let key = Name::from(category);
            if let Some(dict) = resources.dict(&key, r) {
                entries.insert(category.to_owned(), dict);
            }
        }
        Self {
            entries,
            parked: BTreeMap::new(),
        }
    }

    /// The name `object` is known by in `category`, allocating one if it has
    /// none yet.
    ///
    /// An object already in the category keeps the name it has — reusing an
    /// existing `/F1` rather than minting `FXF1` beside it — which is what
    /// keeps a page that regenerates twice from growing a resource dictionary
    /// without bound.
    pub fn realize(&mut self, category: &str, object: ObjRef) -> Name {
        let dict = self.entries.entry(category.to_owned()).or_default();
        for (name, held) in dict.iter() {
            if held.as_ref_id() == Some(object) {
                return name.clone();
            }
        }
        let name = self.free_name(category);
        let dict = self.entries.entry(category.to_owned()).or_default();
        dict.push(name.clone(), Object::Ref(object));
        name
    }

    /// The name `object` already has in `category`, without allocating one.
    ///
    /// What an object in a stream this run is *not* rewriting needs: the
    /// stream refers to the resource by the name it already spells, so the
    /// name has to be found rather than minted — and if there is none, there
    /// is nothing to keep alive.
    #[must_use]
    pub fn name_of(&self, category: &str, object: ObjRef) -> Option<Name> {
        self.entries
            .get(category)?
            .iter()
            .find(|(_, held)| held.as_ref_id() == Some(object))
            .map(|(name, _)| name.clone())
    }

    /// The name an equal direct dictionary already has in `category`, without
    /// allocating one.
    #[must_use]
    pub fn name_of_dict(&self, category: &str, value: &Dict) -> Option<Name> {
        self.entries
            .get(category)?
            .iter()
            .find(|(_, held)| matches!(held, Object::Dict(d) if d == value))
            .map(|(name, _)| name.clone())
    }

    /// Add a direct dictionary — an `/ExtGState` the emitter built rather than
    /// found — under a fresh name, or return the name an equal one already has.
    pub fn realize_dict(&mut self, category: &str, value: &Dict) -> Name {
        let existing = self
            .entries
            .get(category)
            .and_then(|dict| {
                dict.iter().find(|(_, held)| match held {
                    Object::Dict(d) => d == value,
                    _ => false,
                })
            })
            .map(|(name, _)| name.clone());
        existing.unwrap_or_else(|| {
            let name = self.free_name(category);
            let dict = self.entries.entry(category.to_owned()).or_default();
            dict.push(name.clone(), Object::Dict(value.clone()));
            name
        })
    }

    /// The first `FX*n` name free in `category`, counting from one.
    ///
    /// Free means absent from both the live entries and the parked ones — a
    /// parked entry may be restored, and would then collide.
    fn free_name(&self, category: &str) -> Name {
        let letter = category.chars().next().unwrap_or('X');
        let live = self.entries.get(category);
        let parked = self.parked.get(category);
        for id in 1u32.. {
            let candidate = Name::from(format!("FX{letter}{id}").as_str());
            let taken = live.is_some_and(|d| d.contains_key(&candidate))
                || parked.is_some_and(|p| p.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
        // `1u32..` is unbounded, so the loop always returns; this satisfies
        // the type checker without a panic.
        Name::from("FXX1")
    }

    /// Drop every entry no object used, parking it in case a later
    /// regeneration wants it back, and restore any parked entry that is wanted
    /// now.
    ///
    /// `used` names, per category, exactly what the regenerated streams
    /// referred to — every name [`Self::realize`] handed out and nothing else.
    /// A name the caller allocated and then forgot to list is swept away, so
    /// the caller records at the point of allocation.
    pub fn sweep(&mut self, used: &BTreeMap<String, BTreeSet<Name>>) {
        for category in CATEGORIES {
            let wanted: BTreeSet<Name> =
                used.get(category).into_iter().flatten().cloned().collect();

            if let Some(dict) = self.entries.get_mut(category) {
                let mut kept = Dict::new();
                let parked = self.parked.entry(category.to_owned()).or_default();
                for (name, value) in dict.iter() {
                    if wanted.contains(name) {
                        kept.push(name.clone(), value.clone());
                    } else {
                        parked.insert(name.clone(), value.clone());
                    }
                }
                *dict = kept;
            }

            // Anything wanted that is parked comes back.
            let restorable: Vec<(Name, Object)> = self
                .parked
                .get(category)
                .into_iter()
                .flatten()
                .filter(|(name, _)| wanted.contains(*name))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            if restorable.is_empty() {
                continue;
            }
            let dict = self.entries.entry(category.to_owned()).or_default();
            let parked = self.parked.entry(category.to_owned()).or_default();
            for (name, value) in restorable {
                if !dict.contains_key(&name) {
                    dict.push(name.clone(), value);
                }
                parked.remove(&name);
            }
        }
    }

    /// The `/Resources` dictionary this table describes, built onto `base` so
    /// that categories the table does not maintain survive untouched.
    #[must_use]
    pub fn to_dict(&self, base: &Dict) -> Dict {
        let mut out = Dict::new();
        for (key, value) in base.iter() {
            if CATEGORIES.iter().any(|c| key.as_bytes() == c.as_bytes()) {
                continue;
            }
            out.push(key.clone(), value.clone());
        }
        for category in CATEGORIES {
            let Some(dict) = self.entries.get(category) else {
                continue;
            };
            // An empty category is dropped rather than written as `<< >>`:
            // the sweep emptied it, and the key means nothing without entries.
            if dict.is_empty() {
                continue;
            }
            out.push(Name::from(category), Object::Dict(dict.clone()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{CATEGORIES, ResourceTable};
    use pdfrum_object::{Dict, Name, NoResolve, ObjRef, Object};
    use std::collections::{BTreeMap, BTreeSet};

    fn used(pairs: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<Name>> {
        pairs
            .iter()
            .map(|(category, names)| {
                (
                    (*category).to_owned(),
                    names.iter().map(|n| Name::from(*n)).collect(),
                )
            })
            .collect()
    }

    fn names_of(table: &ResourceTable, category: &str) -> Vec<String> {
        let dict = table.to_dict(&Dict::new());
        let Some(Object::Dict(sub)) = dict.raw(&Name::from(category)) else {
            return Vec::new();
        };
        sub.keys()
            .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
            .collect()
    }

    // The three categories, and only those three.
    #[test]
    fn exactly_three_categories_are_maintained() {
        assert_eq!(CATEGORIES, ["ExtGState", "Font", "XObject"]);
    }

    // `RealizeResource` (:513-552): `FX` + the category's first letter + a
    // number counting from one.
    #[test]
    fn a_fresh_name_is_fx_plus_the_categorys_letter_plus_one() {
        let mut table = ResourceTable::default();
        assert_eq!(table.realize("Font", ObjRef::new(4, 0)), Name::from("FXF1"));
        assert_eq!(
            table.realize("XObject", ObjRef::new(5, 0)),
            Name::from("FXX1")
        );
        assert_eq!(
            table.realize("ExtGState", ObjRef::new(6, 0)),
            Name::from("FXE1")
        );
    }

    #[test]
    fn successive_objects_take_successive_numbers() {
        let mut table = ResourceTable::default();
        let first = table.realize("XObject", ObjRef::new(1, 0));
        let second = table.realize("XObject", ObjRef::new(2, 0));
        assert_eq!(first, Name::from("FXX1"));
        assert_eq!(second, Name::from("FXX2"));
    }

    // An object already named keeps its name rather than gaining a second.
    #[test]
    fn an_object_already_in_the_dictionary_keeps_its_name() {
        let existing = Dict::from_pairs([(Name::from("F1"), Object::Ref(ObjRef::new(9, 0)))]);
        let resources = Dict::from_pairs([(Name::from("Font"), Object::Dict(existing))]);
        let mut table = ResourceTable::load(&resources, &NoResolve);
        assert_eq!(table.realize("Font", ObjRef::new(9, 0)), Name::from("F1"));
        assert_eq!(names_of(&table, "Font"), vec!["F1".to_owned()]);
    }

    // A name the dictionary already holds is skipped, whatever it names.
    #[test]
    fn an_occupied_name_is_skipped() {
        let existing = Dict::from_pairs([(Name::from("FXF1"), Object::Int(0))]);
        let resources = Dict::from_pairs([(Name::from("Font"), Object::Dict(existing))]);
        let mut table = ResourceTable::load(&resources, &NoResolve);
        assert_eq!(table.realize("Font", ObjRef::new(3, 0)), Name::from("FXF2"));
    }

    // `DoubleGenerating` (fpdf_edit_embeddertest.cpp:3638): the entry nothing
    // uses is dropped, and the next allocation does **not** reuse its number,
    // because the dropped entry is parked and still reserves it.
    #[test]
    fn a_swept_name_is_parked_and_still_reserved() {
        let mut table = ResourceTable::default();
        let first = table.realize("ExtGState", ObjRef::new(1, 0));
        let second = table.realize("ExtGState", ObjRef::new(2, 0));
        assert_eq!(
            (first.clone(), second.clone()),
            (Name::from("FXE1"), Name::from("FXE2"))
        );

        // Only the first is used; the second is swept away.
        table.sweep(&used(&[("ExtGState", &["FXE1"])]));
        assert_eq!(names_of(&table, "ExtGState"), vec!["FXE1".to_owned()]);

        // The next allocation skips the parked FXE2.
        let third = table.realize("ExtGState", ObjRef::new(3, 0));
        assert_eq!(third, Name::from("FXE3"));
    }

    // The other half of the parking rule: an entry that is wanted again comes
    // back rather than being minted afresh under a new name.
    #[test]
    fn a_parked_entry_wanted_again_is_restored() {
        let existing = Dict::from_pairs([
            (Name::from("A"), Object::Int(1)),
            (Name::from("B"), Object::Int(2)),
        ]);
        let resources = Dict::from_pairs([(Name::from("Font"), Object::Dict(existing))]);
        let mut table = ResourceTable::load(&resources, &NoResolve);

        table.sweep(&used(&[("Font", &["A"])]));
        assert_eq!(names_of(&table, "Font"), vec!["A".to_owned()]);

        table.sweep(&used(&[("Font", &["A", "B"])]));
        assert_eq!(
            names_of(&table, "Font"),
            vec!["A".to_owned(), "B".to_owned()]
        );
    }

    // A category the sweep empties loses its key rather than becoming `<< >>`.
    #[test]
    fn an_emptied_category_loses_its_key() {
        let existing = Dict::from_pairs([(Name::from("F1"), Object::Int(1))]);
        let resources = Dict::from_pairs([(Name::from("Font"), Object::Dict(existing))]);
        let mut table = ResourceTable::load(&resources, &NoResolve);
        table.sweep(&BTreeMap::new());
        let out = table.to_dict(&Dict::new());
        assert!(!out.contains_key(&Name::from("Font")));
    }

    // Categories nobody maintains survive the round trip untouched — the sweep
    // must not eat a page's colour spaces.
    #[test]
    fn an_unmaintained_category_is_carried_through() {
        let base = Dict::from_pairs([
            (Name::from("ColorSpace"), Object::Int(7)),
            (Name::from("Font"), Object::Int(0)),
        ]);
        let table = ResourceTable::default();
        let out = table.to_dict(&base);
        assert_eq!(out.raw(&Name::from("ColorSpace")), Some(&Object::Int(7)));
        // The maintained category is replaced by the table's own view, which
        // here is empty.
        assert!(!out.contains_key(&Name::from("Font")));
    }

    // Two objects wanting the same graphics state share one entry.
    #[test]
    fn an_equal_direct_dictionary_is_reused() {
        let mut table = ResourceTable::default();
        let gs = Dict::from_pairs([(Name::from("ca"), Object::Real(0.5))]);
        let first = table.realize_dict("ExtGState", &gs);
        let second = table.realize_dict("ExtGState", &gs);
        assert_eq!(first, second);
        assert_eq!(names_of(&table, "ExtGState"), vec!["FXE1".to_owned()]);
    }

    // The sweep is driven by the `used` set alone, so a name allocated and
    // then not listed is swept — which is why the caller records each name at
    // the point it allocates it rather than afterwards.
    #[test]
    fn a_name_the_caller_did_not_list_is_swept() {
        let mut table = ResourceTable::default();
        let _ = table.realize("XObject", ObjRef::new(1, 0));
        table.sweep(&BTreeMap::new());
        assert!(names_of(&table, "XObject").is_empty());
    }
}
