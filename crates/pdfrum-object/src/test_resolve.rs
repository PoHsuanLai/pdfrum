//! A map-backed [`Resolve`] for exercising the accessor matrix without a
//! parser. Test-only; the real store lives in `pdfrum-parser`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Error, ObjRef, Object, Resolve};

/// Objects keyed by number, as a cross-reference table would key them.
#[derive(Debug, Default)]
pub struct TestStore(HashMap<u32, Arc<Object>>);

impl TestStore {
    /// A store holding these objects under these numbers.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (u32, Object)>) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(num, obj)| (num, Arc::new(obj)))
                .collect(),
        )
    }
}

impl Resolve for TestStore {
    /// Looks up by object number alone, ignoring the generation — the
    /// contract every real store follows.
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, Error> {
        self.0
            .get(&r.num)
            .map(Arc::clone)
            .ok_or(Error::UnresolvedRef(r))
    }
}
