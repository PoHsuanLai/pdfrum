> **Reviewer:** Grok (xAI), cross-vendor review, 2026-09-01. Verbatim below the
> rule; this header is the only text added.
>
> **Fixed.** Finding 1 (the leak before the real-GPU check), finding 2
> (`Error::TargetTooLarge` documented and never constructed) and finding 3
> (wgpu's uncaptured-error hook panicking, against STYLE §3) in `7af8b95` —
> one commit because all three land in `lib.rs` and a path-scoped commit cannot
> split a file. Nit 6 (`block.rs` busy-spinning on `PollType::Poll`) in
> `5ebd4f9`. Nit 4 (`check-no-wgpu.sh` walking only `crates/*/`) in `5dc92f6`.
> Nit 5 (no GPU test for a nested mask or an empty masked layer) in `b9a03b3`.
>
> **Declined:** nothing. One correction to the report rather than a decline:
> finding 2's fix is a fallible size check *plus* a clamp in the infallible
> constructor, not the "tiny blank" the finding offers as an alternative —
> `RasterBackend` has no failure channel and three CPU backends share it, so
> the trait method must still return a `Pixmap` of a size its caller can
> compose with.

---

# GPU backend review — pdfrum-raster-vello-gpu (REPORT ONLY)

## Findings

1. **should-fix** `adapter.rs:92-93,115-121`. `try_real_gpu` leaks Device+Queue then discards on llvmpipe; `tests/gpu.rs` does this 14 times. `request_adapter` does not refuse software adapters. Fix: `Error::NoAdapter` before leak; `OnceLock` in tests.

2. **should-fix** `lib.rs:131,235` / `error.rs:32`. Target > `max_texture_dimension_2d` (~9k-px; worst 65535²). Docs claim `Error::TargetTooLarge`; never constructed. `rasterize` does `Pixmap::new(w,h)` → `w*h*4` zeros (~17 GiB). Fix: tiny blank or clamp; drop the variant or add a fallible size check.

3. **should-fix** `lib.rs:183`. Device loss during `finish`/`snapshot`: wgpu's default uncaptured-error hook panics (STYLE). Own `Result`s fail open; wgpu's do not. Fix: recording `on_uncaptured_error` in `new`.

4. **nit** `scripts/check-no-wgpu.sh:60` — `crates/*/` only; `conformance/` and `benches/` unchecked. Core ring is clean.

5. **nit** `tests/gpu.rs:133` — no GPU test for nested masks or a masked layer popped with no draws. Logic matches M12c §4.2.

6. **nit** `block.rs:76` — `PollType::Poll` + `yield_now` busy-spins up to 30s. Prefer a bounded Wait.

## Contract

1. **ISOLATION — satisfied.** Facade and `pdfrum-tool` have no gpu/wgpu/vello dep. `check-no-wgpu.sh` uses `-e normal` and asserts this crate still depends on `vello` (L70–79), so a broken query cannot pass vacuously. Facade/corpus/exact/vello-cpu edges are `[dev-dependencies]` only.

2. **DEVICE INJECTION — satisfied, with finding 1.** `VelloGpuBackend::new` borrows `&Device`/`&Queue`. Leak is one device+queue per `request_adapter` call, documented. Accidental loop: every GPU test, and `try_real_gpu` on llvmpipe.

3. **LAYER/MASK — satisfied.** `push_layer` carries `Frame::Masked`; `pop` emits `push_luminance_mask_layer` + grey `(m,m,m,255)` + two pops (`lib.rs:430-502`). Nested LIFO stays aligned (luma pair not on `frames`). Empty masked pop → transparent. All 16 PDF modes map (`convert.rs:36-56`). `AntiAlias::Off`/`FullCover` ignored (`lib.rs:363`; README, M12c §5.1).

4. **READBACK — satisfied.** `unpadded=width*4`, `padded=next_multiple_of(256)`; copy `unpadded` bytes/row (`readback.rs:144-204`). Width 100 → 400/512. `Rgba8Unorm` + `Pixmap::from_vec` = premul RGBA8. GPU test `gpu.rs:240`.

5. **NO PANICS / NO UNSAFE — mostly satisfied.** `#![forbid(unsafe_code)]`. No `unwrap`/`expect` in lib paths; over-pop is a no-op; `NoAdapter` is distinct. llvmpipe refused on `try_real_gpu`, not on `request_adapter` (finding 1). Device-loss panic is finding 3.

6. **DEV-DEPS / SKIP — satisfied.** Those lockfile edges are dev-only. `tests/gpu.rs` returns after a skip print; src unit tests need no GPU. They do not fail or hang; they report as passed, not ignored.

## Verdict

**MERGEABLE-WITH-NITS**

Isolation, deferred mask, readback un-pad, and no-GPU skip hold. Fix 1–2 before relying on the skip path or large pages; 3 is STYLE.
