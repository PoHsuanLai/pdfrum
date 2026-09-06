# Vendored colour assets

## `CGATS001Compat-v2-micro.icc`

A CMYK ICC profile, 8464 bytes, used as the `/DestOutputProfile` of the CMYK
PDF/A output intent `pdfrum`'s `to_pdfa` writes for a document that paints in
`/DeviceCMYK`. See `crates/pdfrum-page/src/color/icc.rs` and
`docs/design/pdfa.md` §11.

- **Source**: <https://github.com/saucecontrol/Compact-ICC-Profiles>, file
  `profiles/CGATS001Compat-v2-micro.icc`.
- **Licence**: CC0-1.0, a public-domain dedication. `deny.toml` already allows
  CC0-1.0 (M12c admitted it for `hexf-parse`), so this needs no policy change.
- **What it is**: a compact profile colorimetrically compatible with CGATS
  TR 001 (SWOP), the US web-coated printing condition — a real CMYK -> Lab
  A2B lookup, not a placeholder.

It is checked in as bytes rather than pulled from a crate because what is
needed is *a profile*, not a colour-management system: `moxcms` is already the
engine in this crate, and a CMYK profile is LUT data that cannot be computed
from a formula the way `ColorProfile::new_srgb()` is. Vendoring 8 KB of
public-domain data takes no dependency at all.

The bytes are used unmodified, but the profile is **re-encoded** before it is
embedded: upstream declares device class `scnr` (an input profile) and ISO
19005-2 6.2.3 requires an output intent's profile be `prtr` or `mntr`.
`cmyk_profile_bytes` re-encodes it through `moxcms` as `prtr`, which is what
the profile is in fact used as here.
