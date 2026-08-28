//! Page content semantics (ISO 32000 §8): content-stream operators parsed to
//! a typed `Op` list, the interpreter folding ops into a typed page-object
//! graph, graphics state, colorspaces (device/ICC/Indexed/Separation/Lab),
//! PDF functions (types 0/2/3/4), patterns and shadings (types 1–7), and the
//! transparency model — groups, soft masks, blend modes (SPEC.md §7).

#![forbid(unsafe_code)]
