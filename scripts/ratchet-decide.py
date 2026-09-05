#!/usr/bin/env python3
"""The ratchet re-baseline decision, encoded before the numbers were in.

WHY THIS EXISTS AS A SCRIPT AND NOT AS A PARAGRAPH

M12b P3 had to decide whether to re-baseline `benches/baseline.json`, and one
baseline entry was expected to come back above its band for a reason that has
nothing to do with the code. Raising an
entry for that reason can be legitimate; raising one *because it is the last
thing standing between you and a successful `ratchet update`* never is, and the
two are indistinguishable in a status document written afterwards.

So the rules were written down, as runnable code, **before `cargo bench`
finished** — its commit precedes the finished numbers, which is the only
durable answer to a future reader's obvious suspicion that a pre-argued
exemption was retrofitted. If you are reading this while considering an
exemption of your own: add it here, commit it, *then* run the bench. That order
is the whole mechanism.

THE RULES (from the M12b P3 review, restated verbatim as behaviour)

  R1. Never raise a baseline entry to make `update` succeed. If that is the only
      reason, the debt stands. An unpaid debt is recoverable; a loosened ratchet
      is not, because the evidence that it was loosened wrongly is gone.
  R2. Raising is legitimate ONLY on an independent showing that the entry
      measures something the change structurally cannot reach, AND that the new
      number reflects the measurement moving rather than the code.
  R3. If the pre-argued entry lands inside its band, raise NOTHING.
  R4. Any other regression is judged on its own evidence or the whole update
      goes unwritten. Never swept in alongside a justified one.
  R5. Contamination — an implausible delta on a document the change cannot have
      moved — means discard the run and record the debt. Writing no baseline is
      an acceptable outcome; writing a bad one is not.

WHAT WAS PRE-ARGUED FOR M12b P3, AND ON WHAT EVIDENCE

Only `render-cold-*/vector_vector_en_tem`, because:
  - `render-cold` builds the page graph *inside* the timed closure
    (`crates/pdfrum-render/benches/render.rs` module docs);
  - on that document the build is 93-101 ms against a 6.1 ms render, so the
    benchmark is ~94% page build and ~6% render;
  - `pdfrum-page` has no dependency on `pdfrum-render`, so P3's commits are
    structurally unable to touch the dominant term;
  - two byte-identical copies of one binary disagreed by 32 ms on that
    measurement, which `profile` takes once with no warmup;
  - the committed baseline's own loop-measured `build/` entry for it is
    91.78 ms, which the post-change binaries straddle.

`shading_type4_5` came back above band too and was NOT exempted, although five
independent lines of evidence said its render was unchanged (§9.3). It had no
pre-argued case, and constructing one after it blocked is exactly what R1
forbids. That refusal is the point of this file.

USAGE

    python3 scripts/ratchet-decide.py [criterion-log]

Reads a `cargo bench -p pdfrum-render` log (default: the path M12b P3 used) and
`benches/baseline.json`. Exit 0 = raise nothing, run `update` as-is; 1 = only
pre-argued entries regressed, re-verify their case before raising; 2 = leave the
update unwritten.
"""

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = os.path.join(ROOT, "benches", "baseline.json")
# M12b P3's own run, kept in the tree so its verdict is reproducible from the
# repository alone rather than from a scratch directory that no longer exists.
# Pass a path to check a fresh run instead.
DEFAULT_LOG = os.path.join(ROOT, "docs", "status", "data", "M12b-P3-bench.txt")

# The only entries with a pre-argued structural case. Adding to this set is a
# decision someone has to make deliberately, in a commit that precedes the run.
PRE_ARGUED = {
    "render-cold-exact/vector_vector_en_tem",
    "render-cold-tinyskia/vector_vector_en_tem",
    "render-cold-vello/vector_vector_en_tem",
}

# R5: a delta this large on a document the change cannot plausibly have moved is
# the contamination signature (M12b P2 saw +66.8% on one), not a result.
CONTAMINATION_PCT = 40.0

# criterion prints microseconds with the MICRO SIGN (U+00B5), not "us", and a
# fast benchmark is the only place it does — which is why the warm groups were
# the first to hit it. Both spellings are accepted so neither can be missed.
UNIT = {"ns": 1.0, "us": 1e3, "µs": 1e3, "ms": 1e6, "s": 1e9}

# `render-warm` hoists the RenderSession out of the timed closure and is the
# group that isolates the render; M12's exit target is judged on it.
# `render-cold` rebuilds the page graph per iteration, so it is substantially a
# measurement of `pdfrum-page`. Reported apart so a cold figure is never read as
# a statement about the walk.
FAMILIES = ("render-warm", "render-cold")


def parse_log(path):
    """Median times by baseline id, from criterion's own stdout."""
    with open(path) as handle:
        text = handle.read()
    pat = re.compile(
        r"^(render-[^\s]+)\n\s+time:\s+\[[\d.]+ \w+ ([\d.]+) (\w+) [\d.]+ \w+\]",
        re.M,
    )
    out = {}
    for match in pat.finditer(text):
        gid, mid, unit = match.group(1), float(match.group(2)), match.group(3)
        parts = gid.split("/")
        # criterion writes `<group>/<a>/<b>` to disk as `<group>/<a>_<b>`, which
        # is what the baseline keys on.
        out[parts[0] + "/" + "_".join(parts[1:])] = mid * UNIT[unit]
    return out


def classify(entries, bands, measured):
    improved, regressed, fresh, unchanged = [], [], [], 0
    for key, now in sorted(measured.items()):
        old = entries.get(key)
        if old is None:
            fresh.append(key)
            continue
        if old <= 0:
            continue
        band = bands.get(key.split("/")[0], 0.05)
        delta = (now - old) / old
        if delta > band:
            regressed.append((key, old, now, delta * 100, band * 100))
        elif delta < -band:
            improved.append((key, old, now, delta * 100))
        else:
            unchanged += 1
    return improved, regressed, fresh, unchanged


def main():
    log = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_LOG
    with open(BASE) as handle:
        base = json.load(handle)
    bands, entries = base["bands"], base["benchmarks"]
    measured = parse_log(log)
    improved, regressed, fresh, unchanged = classify(entries, bands, measured)

    print(f"measured {len(measured)} of {len(entries)} baseline entries")
    print(f"  {unchanged} unchanged, {len(improved)} improved, "
          f"{len(regressed)} regressed, {len(fresh)} new\n")

    print("by group family (warm isolates the render; cold rebuilds the page):")
    for fam in FAMILIES:
        got = [k for k in measured if k.startswith(fam)]
        reg = [r for r in regressed if r[0].startswith(fam)]
        imp = [r for r in improved if r[0].startswith(fam)]
        print(f"  {fam:12s} measured={len(got):3d} improved={len(imp):3d} "
              f"regressed={len(reg):3d}")

    print("\nREGRESSIONS (above band):")
    if not regressed:
        print("  none")
    blocking = []
    for key, old, now, delta, band in sorted(regressed, key=lambda r: -r[3]):
        tag = "pre-argued (R2)" if key in PRE_ARGUED else "*** BLOCKING (R4) ***"
        print(f"  {delta:+7.1f}%  {key}  {old / 1e6:.3f} -> {now / 1e6:.3f} ms "
              f"(band {band:.0f}%)  {tag}")
        if key not in PRE_ARGUED:
            blocking.append(key)

    suspect = [(k, d) for k, _, _, d in improved if abs(d) > CONTAMINATION_PCT]
    print("\nlargest improvements (top 8):")
    for key, old, now, delta in sorted(improved, key=lambda r: r[3])[:8]:
        print(f"  {delta:+7.1f}%  {key}  {old / 1e6:.3f} -> {now / 1e6:.3f} ms")
    if suspect:
        print(f"\n  note: {len(suspect)} entries moved more than "
              f"{CONTAMINATION_PCT:.0f}%. Confirm these are real prior work "
              f"landing in the baseline (M12b P1's image commits were) and not "
              f"R5 contamination, before writing anything.")

    print("\n--- DECISION ---")
    if blocking:
        print("LEAVE UNWRITTEN (R4): regressions with no independent case:")
        for key in blocking:
            print(f"    {key}")
        return 2
    pre = [r for r in regressed if r[0] in PRE_ARGUED]
    if not pre:
        print("R3: no pre-argued entry regressed -> RAISE NOTHING.")
        print("    `ratchet update` writes improvements only; run it as-is.")
        return 0
    print("R2 candidate(s); re-verify the structural case before raising:")
    for key, old, now, delta, _band in pre:
        print(f"    {key}  {delta:+.1f}%  {old / 1e6:.3f} -> {now / 1e6:.3f} ms")
    return 1


if __name__ == "__main__":
    sys.exit(main())
