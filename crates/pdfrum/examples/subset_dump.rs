//! Write the fixture's page, imported, with and without font subsetting.
//!
//! Not part of the library: it exists so the conformance oracle can be pointed
//! at a subsetted save directly, which `save-round-trip` cannot do because
//! `pdfrum-tool` has no switch for the option.

use pdfrum::{Document, IdSource, SaveOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = args
        .next()
        .ok_or("usage: subset_dump <src.pdf> <dest.pdf> <out-prefix>")?;
    let destination = args.next().ok_or("missing destination")?;
    let prefix = args.next().ok_or("missing output prefix")?;

    let src = Document::open(source)?;
    let dest = Document::open(destination)?;

    for subset_new_fonts in [false, true] {
        let mut edit = dest.edit();
        edit.import_pages(&src, [0u32], 0u32)?;
        let options = SaveOptions::builder()
            .subset_new_fonts(subset_new_fonts)
            .id_source(IdSource::Fixed([7; 16]))
            .build();
        let mut out = Vec::new();
        edit.write_to(&mut out, &options)?;
        let suffix = if subset_new_fonts { "on" } else { "off" };
        let path = format!("{prefix}-{suffix}.pdf");
        std::fs::write(&path, &out)?;
        println!("{path}: {} bytes", out.len());
    }
    Ok(())
}
