#!/usr/bin/env bash
# The dev loop: three representative documents per class, low sample counts.
#
# Usage: scripts/bench-quick.sh [group ...]
#
#   scripts/bench-quick.sh                 # every group
#   scripts/bench-quick.sh render-warm     # just the warm render groups
#   scripts/bench-quick.sh open build      # the two cheap ones
#
# The full suite is `scripts/ci.sh`'s big sibling: 44 documents x eleven groups,
# about an hour, and it is what `benches/baseline.json` and every table in
# docs/status/M12.md are taken from. It is not a thing to run between two edits.
# This is: **eighteen documents, three per class**, at criterion's floor rather
# than the suite's window, which lands in three to four minutes.
#
# What that buys and what it does not. It buys the direction of a change on
# every class — enough to tell an optimization from a pessimization before
# committing to a real measurement. It does **not** buy a number anyone should
# write down: at these sample counts the confidence interval is several times
# the ratchet's noise band, and the ratchet is deliberately not run here. A
# result from this script is a reason to run the full suite, never a substitute
# for having run it.
#
# The eighteen are chosen for *spread inside* the class rather than for weight:
# one document at each end of the class's cost range and one in the middle, so a
# change that helps large inputs and hurts small ones shows up as two rows
# moving in opposite directions instead of one average that moved a little. See
# benches/corpus/PROVENANCE.md for what each file is.
set -euo pipefail
cd "$(dirname "$0")/.."

# Three per class: cheap, middling, heavy. The names are criterion filters —
# it matches on a substring of the benchmark id, and every id is
# `<group>/<class>/<stem>`, so a bare stem selects that document in every group.
FILTER='text_bug_1029|text_cjk_page|text_tcpdf_055'
FILTER+='|vector_tcpdf_009|vector_en_tem|vector_paths_1751'
FILTER+='|image_ccitt_transfer|image_jpx_123|image_jbig2_880920'
FILTER+='|shading_type4_5|shading_tcpdf_058|shading_axial_radial'
FILTER+='|forms_number|forms_text_field|forms_widgets_407'
FILTER+='|mixed_formfield|mixed_tcpdf_059|mixed_en_uicase'

# Which crate owns which group, since M12's per-crate split. A group named on
# the command line runs only its own crate's bench binary; with none named,
# all five run.
declare -A OWNER=(
    [open]=pdfrum-parser
    [build]=pdfrum-page
    [render-cold]=pdfrum-render
    [render-warm]=pdfrum-render
    [text]=pdfrum-text
    [save]=pdfrum-edit
)

groups=("$@")
if [ ${#groups[@]} -eq 0 ]; then
    crates=(pdfrum-parser pdfrum-page pdfrum-render pdfrum-text pdfrum-edit)
    group_filter=''
else
    # De-duplicate the owners: `render-cold render-warm` is one crate, not two
    # runs of it, and the group name goes into the filter instead.
    declare -A seen=()
    crates=()
    for g in "${groups[@]}"; do
        owner=${OWNER[$g]:-}
        if [ -z "$owner" ]; then
            echo "error: unknown group '$g'" >&2
            echo "known: ${!OWNER[*]}" >&2
            exit 2
        fi
        if [ -z "${seen[$owner]:-}" ]; then
            seen[$owner]=1
            crates+=("$owner")
        fi
    done
    group_filter=$(IFS='|'; echo "${groups[*]}")
fi

# Criterion's own floor. `--sample-size 10` is the minimum it accepts and
# `--warm-up-time 1` still lets a warm group's cache fill; below either, the
# numbers stop being repeatable even to the one significant figure this script
# is for. `--measurement-time 1` is what makes it three minutes instead of forty.
CRITERION_ARGS=(--sample-size 10 --warm-up-time 1 --measurement-time 1 --noplot)

# Criterion takes one regex. When a group was named, the id has to match both
# the group and one of the eighteen documents, which a single regex expresses
# only as a product — so build it that way rather than passing two filters
# criterion would AND incorrectly.
if [ -n "$group_filter" ]; then
    PATTERN="^($group_filter)[^/]*/[^/]*/($FILTER)$"
else
    PATTERN="/($FILTER)$"
fi

echo "==> bench-quick: 18 documents, ${#crates[@]} crate(s), criterion floor"
echo "    pattern: $PATTERN"
echo "    NOT a ratchet run — see the header of this script before quoting a number."
echo

for crate in "${crates[@]}"; do
    echo "==> $crate"
    cargo bench -p "$crate" -- "${CRITERION_ARGS[@]}" "$PATTERN"
    echo
done

echo "bench-quick done. For a number that counts:"
echo "  cargo bench --workspace"
echo "  cargo run --release -p pdfrum-bench --bin ratchet -- check"
