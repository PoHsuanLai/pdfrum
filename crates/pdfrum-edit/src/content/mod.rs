//! Turning a page-object graph back into content-stream operators
//! (ISO 32000-1 §8, §9).
//!
//! A page whose objects were modified is rewritten from the graph rather than
//! patched, so these bytes *are* the round trip: any divergence here shows up
//! as pixels.
//!
//! # This is lossy, and matching the loss is the requirement
//!
//! The emitter reproduces the C++'s exactly, including what it drops. That
//! looks perverse until you notice what conformance compares: a regenerated
//! page of ours against a regenerated page of the oracle's. Emitting *more*
//! than the oracle does would fail that comparison as surely as emitting
//! less. The losses, in full:
//!
//! - **Colour.** Only `rg` and `RG` are ever written, and only when the
//!   colour space is stock `DeviceRGB` or `DeviceGray`. CMYK, `ICCBased`, Indexed,
//!   Separation, `DeviceN`, Lab, `CalRGB` **and every pattern** emit nothing and
//!   inherit the black the per-stream prologue set.
//! - **Shadings.** A shading page object emits nothing at all.
//! - **Text.** Only `Tm`, `Tf`, `Tr` and `TJ`. All positioning collapses into
//!   `Tm`; `Td`, `TD`, `T*`, `Tj`, `'`, `"`, `Tc`, `Tw`, `Tz`, `TL` and `Ts`
//!   are never emitted, so character and word spacing are lost. A Type 3 font
//!   drops its whole text object.
//! - **Graphics state.** Only `ca`, `CA` and `BM` reach an `/ExtGState`. The
//!   miter limit and soft masks do not.
//! - **Clips.** Path clips only: text clips, clip-path soft masks and shading
//!   clips are never written.
//!
//! # No state diffing, ever
//!
//! Each object is wrapped in its own `q`/`Q` and states everything it needs
//! from scratch, comparing each value against the *hardcoded PDF default* —
//! never against the object emitted before it. That makes the output longer
//! than a hand-written stream and makes every object independently
//! relocatable, which is what lets objects from several source streams
//! interleave into new ones without a fixup pass.
//!
//! # An unsupported object contributes nothing
//!
//! Two C++ paths write a prefix and then bail, leaving a `q ` — and for text
//! a `BT ` — unclosed. We build each object's bytes into a scratch buffer and
//! commit them only on success, so an object we cannot express contributes
//! *nothing* rather than an unbalanced fragment (divergence D6). The rendered
//! result is the same for well-formed input, because the stream-level `Q`
//! closes the C++'s stray `q` anyway; the difference is that our output stays
//! parseable.

pub mod emit;
mod marks;
pub(crate) mod num;
mod path;
mod text;

pub use emit::{DEFAULT_GRAPHICS, GraphicsKey, emit_object, emit_page_objects};
pub use marks::{PropertyNamer, emit_mark_diff, finish_marks};
pub use num::{write_float, write_matrix, write_point, write_rect};
pub use path::{emit_path_points, paint_operator};
pub use text::emit_text_body;
