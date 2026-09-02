//! Write the fixture's page, imported, with and without font subsetting.
//!
//! Not part of the library: it exists so the conformance oracle can be pointed
//! at a subsetted save directly, which `save-round-trip` cannot do because
//! `pdfrum-tool` has no switch for the option.

use std::sync::Arc;

use pdfrum_common::PageIndex;
use pdfrum_edit::{EditDoc, IdSource, ImportOptions, PageRange, SaveOptions, import_pages, save};
use pdfrum_parser::{LoadOptions, load};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = args
        .next()
        .ok_or("usage: subset_dump <src.pdf> <dest.pdf> <out-prefix>")?;
    let destination = args.next().ok_or("missing destination")?;
    let prefix = args.next().ok_or("missing output prefix")?;

    let src = load(Arc::from(std::fs::read(source)?), &LoadOptions::default())?;
    let dest = load(
        Arc::from(std::fs::read(destination)?),
        &LoadOptions::default(),
    )?;

    for subset_new_fonts in [false, true] {
        let mut edit = EditDoc::new(&dest);
        import_pages(
            &mut edit,
            &src,
            &PageRange::of([0u32]),
            &ImportOptions {
                at: PageIndex::new(0),
                viewer_preferences: false,
            },
        )?;
        let mut out = Vec::new();
        save(
            &edit,
            &SaveOptions {
                subset_new_fonts,
                id_source: IdSource::Fixed([7; 16]),
                ..SaveOptions::default()
            },
            &mut out,
        )?;
        let suffix = if subset_new_fonts { "on" } else { "off" };
        let path = format!("{prefix}-{suffix}.pdf");
        std::fs::write(&path, &out)?;
        println!("{path}: {} bytes", out.len());
    }
    Ok(())
}
