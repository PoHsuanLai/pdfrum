# Axial/radial shading: the colour ramp is filled at `i/256` but sampled at `s*255`, and the radial "decreasing" test truncates a distance to an integer

**Status:** drafted 2026-09-02, not yet filed.

**Confidence:** code-level, contrived trigger.

**Source read at** PDFium commit `6f2272e1f3aa` (2026-08-28).
**Host** for any observed output: Ubuntu 22.04.5 LTS, x86-64.
**Tiebreaker**: Mozilla's pdf.js, an independent implementation of the same
specification, cited at file:line where its answer differs.

Everything from the `##` heading down is issue text, ready to paste
into the Chromium tracker's template; nothing above it is meant to be
posted.

---

## axial/radial shading: the colour ramp is filled at `i/256` but sampled at `s*255`, and the radial "decreasing" test truncates a distance to an integer

**What steps will reproduce the problem?**

1. Build `pdfium_test`.
2. Render any page with a smooth axial (`/ShadingType 2`) gradient and compare
   the rendered ramp against the specification's `t` parameterisation — the
   skew is one part in 256 and accumulates across the ramp.
3. For the second defect: render a radial (`/ShadingType 3`) shading whose
   geometry has `hypot(dx, dy)` and `dr` both between the same pair of
   integers — e.g. `dx = 1.5, dy = 0, dr = -1.2`.

**What is the expected output? What do you see instead?**

Defect A (ramp skew): expected the ramp evaluated over `[t₀, t₁]` inclusive;
observed a systematic offset because the fill and the lookup disagree about the
divisor — `t_max` is never evaluated at all.

Defect B (radial): expected `bDecreasing` to be false for `dx = 1.5,
dr = -1.2` (since `1.5 < 1.2` is false); observed true, because the distance is
truncated to `1` first. That flag selects between the two roots of the
quadratic, so it changes **which** gradient is drawn, not merely its shading.

**What version of the product are you using? On what operating system?**

PDFium at `6f2272e1f3aa` (2026-08-28). Ubuntu 22.04.5 LTS, x86-64.

**Please provide any additional information below.**

Defect A, `core/fpdfapi/render/cpdf_rendershading.cpp:81` (fill):
```cpp
float input = diff * i / kShadingSteps + t_min;      // divides by 256
```
against `:160` (lookup):
```cpp
int index = static_cast<int32_t>(scale * (kShadingSteps - 1));   // times 255
```
`kShadingSteps` is 256 (`:45`). Filling with `i / 256` means the last entry
corresponds to `t = t_min + 255/256 · diff`, so `t_max` is never evaluated,
while the lookup maps `scale = 1.0` onto index 255. Either the fill should
divide by `kShadingSteps - 1` or the lookup should multiply by `kShadingSteps`
and clamp; they must agree.

Defect B, `:220`:
```cpp
bool bDecreasing = dr < 0 && static_cast<int>(hypotf(dx, dy)) < -dr;
```
The `static_cast<int>` quantises the centre distance to a whole device unit
before comparing it against a float radius delta. Dropping the cast makes the
comparison the geometric one. Neither behaviour has a basis in §8.7.4.5.4.

---
