# Public constants whose value is sentinel-*shaped* but is not a sentinel

WP13's `scripts/check-pub-consts.nu` fails any `pub const` in
`docs/status/api-baseline/*.txt` whose defining value is `-1`, `u16::MAX` /
`0xFFFF`, `u32::MAX` / `0xFFFF_FFFF`, `usize::MAX`, `i32::MIN`, an inverted
rect, or `NaN` — unless the constant is listed here.

A row here is a **limit, identity, or table value**, not absence standing for
a value (`docs/design/idiomatic-api.md` §C). One line per constant: the
snapshot path, an em dash, the reason. A constant that leaves the surface
must leave this file; a new exception is a review.

pdfrum_render::MAX_TARGET_DIMENSION — limit: vello_cpu sizes every target with u16, so 65535 (`u16::MAX`) is a ceiling a backend cannot allocate past, not an absence
pdfrum_object::INT_RANGE — ISO 32000 integer domain; the lower bound is i32::MIN because that is the format's own range (`-2^31 ..= 2^32-1`), not a sentinel
