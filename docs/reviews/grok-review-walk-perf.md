> **Reviewer:** Grok (xAI), cross-vendor review, 2026-09-01. Verbatim below the
> rule; this header is the only text added.
>
> **Fixed.** Nit 1 (the NaN hole in `path_cull_bounds`) and nit 3 (the bracket
> test never using a zero-size clip or a non-finite point) in `8bb811c` — one
> commit, because the test nit 3 asks for is the test that proves nit 1's fix.
> Nit 2 (`Phase::PathXform`'s doc still claiming two `BezPath` builds after
> `d51ef60` fused them) in `71b6e1f`.
>
> **Declined:** nothing. One correction to the report: nit 1's *direction* is
> the other way round. The review reasons that a finite outer box could cull a
> path `path_bbox` keeps, conditional on "if kurbo's extrema box stays NaN".
> It does not — kurbo's extrema solve drops a NaN exactly as `f64::min`/`max`
> do — so a NaN *control* point leaves both boxes finite and both answers
> equal, and that predicted failure cannot occur. The real hole is the *inner*
> box and a NaN *endpoint*: the NaN lands there unfolded, every `outside`
> comparison against it is false, and the bracket answers **keep** where the
> exact test culls. A missed cull rather than a wrong one, so nothing was ever
> drawn incorrectly. Measured over a sliding clip; the fix nit 1 prescribes —
> `None` on any non-finite coordinate — closes it either way, and is what
> landed.
>
> **Conformance:** byte-identical over all 1675 files, 1624 pass, no
> regressions. No corpus document carries a non-finite path coordinate, which
> is what keeps this a review finding rather than a bug report.

---

# Walk-perf review (b7fe49e d51ef60 f9387fd fad749e 1790568)

## Findings

1. **nit** — `walk.rs:436-441` (`add`). Rust `f64::min`/`max` drop NaN, so a mixed NaN/finite path's outer is the finite AABB; `outside` on a NaN rect is always false (keep). If kurbo's extrema box stays NaN, `outside(outer)` can cull while `path_bbox` would keep. Scenario: `CurveTo` with one NaN control, other points finite and off-clip. Fix: return `None` from the bracket when `!p.x.is_finite() || !p.y.is_finite()` (same as `cull_rect`, `walk.rs:367-369`).

2. **nit** — `walkprofile.rs:90-92`. `Phase::PathXform` still says "Two BezPath builds per fill" after `d51ef60` fused them. Fix: one reserved buffer via `transform_hard_clip`.

3. **nit** — `walk.rs:2114-2201`. Bracket test never uses a zero-size clip or a non-finite point. `outside` is unchanged, so a degenerate clip is identical; the gap is (1). Fix: `Rect::new(1,1,1,1)` plus a NaN point asserting the bracket returns `None` or agrees with exact.

## Contract

1. **Cull — satisfied for finite affine cubics/quads.** Affine maps preserve convex combinations, so the AABB of transformed controls (`matrix * p` then min/max) ⊇ the transformed curve. Inner holds only LineTo/QuadTo/CurveTo ends (MoveTo is outer-only), so inner ⊆ exact: KEEP via inner cannot false-keep; CULL is only via outer. Declines: no leading MoveTo (`walk.rs:431-433`); no drawn segment (`inner?` at 474). Empty / bare MoveTo / Move+Close go to `path_bbox` (`Rect::default()` at origin). No panic. Test slides a 4×4 clip at 0.5 on [-12,14)² under identity, translate, and shear+scale, including the bulge (peak y=7.5, controls y=10) and two-MoveTo.

2. **Points overflow — satisfied.** `path.rs:245-248` `push` is `get_mut(self.len)?`; past CAP returns `None`. `f9387fd` (`path.rs:565-623`): 100-seg polyline → None; 33 on-path points → None via `> 32`; 32-point closed rect with repeats appends the 33rd, collapses to 5, `path_rect` returns `Some(0,0,10,5)`; `const { assert!(Points::CAP == 33) }`. No panic, no silent truncate of a legal rect.

3. **to_vec → borrow — satisfied.** `nudge_degenerate_subpaths` (`path.rs:139-158`) borrows `elements()`; `should_nudge` only reads; output is a new `BezPath`. `transform_hard_clip` is one pass over `path.elements()`, source not mutated. Order unchanged.

4. **Session scratch — satisfied.** `scan_into_inner` (`zero_area.rs:109-110`) clears `found` and `points` first. First MoveTo runs `zero_area_path` on empty (`len < 2` → None). Early returns in `draw_path_inner` happen *before* the scan; leftover buffers are private. `paint.rs:169` clones owned `zero.path`; `&mut Scratch` is not written during the loop. P3's pinned invariant is the CAP/guard pair (`M12b-P3.md` §10), not a take/restore; no `mem::take` here.

5. **STYLE — satisfied.** No `unwrap`/`expect`/`panic!` in library paths (`as_slice` uses `unwrap_or`). No `unsafe`. `Points`/`Scratch` are records; logic is free functions.

## Verdict

**MERGEABLE-WITH-NITS**
