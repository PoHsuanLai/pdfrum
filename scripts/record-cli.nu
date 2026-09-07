#!/usr/bin/env nu
# Record the CLI showcase GIF from docs/assets/cli/pdfrum-cli.tape.
#
#   ./scripts/record-cli.nu
#
# Stages a few in-tree fixtures under friendly names, puts the release
# `pdfrum` on PATH, and runs vhs. On a headless host (no DISPLAY), wraps
# the run in xvfb-run. Not part of the gate: re-run when the CLI's human
# output changes.
#
# Requires vhs, ttyd, ffmpeg (and xvfb-run when DISPLAY is unset).

const WORK = '/tmp/pdfrum-cli-demo'
const BUDGET = 2000000

def main [] {
    let root = ($env.FILE_PWD | path dirname)
    cd $root

    for tool in [vhs ttyd ffmpeg] {
        if (which $tool | is-empty) {
            print --stderr $"error: ($tool) is not on PATH"
            print --stderr "       vhs: https://github.com/charmbracelet/vhs/releases"
            print --stderr "       ttyd and ffmpeg: the host package"
            exit 1
        }
    }

    print "==> cargo build -p pdfrum-cli --release"
    ^cargo build -p pdfrum-cli --release

    let target = (
        if ($env.CARGO_TARGET_DIR? | is-empty) {
            $root | path join "target"
        } else {
            $env.CARGO_TARGET_DIR
        }
    )
    let bin = ($target | path join "release" "pdfrum")
    if not ($bin | path exists) {
        print --stderr $"error: no binary at ($bin)"
        exit 1
    }

    let fx = ($root | path join "crates" "pdfrum-cli" "tests" "fixtures")
    let bfx = ($root | path join "benches" "fixtures")
    rm -rf $WORK
    mkdir $WORK
    cp ($fx | path join "bookmarks.pdf") ($WORK | path join "report.pdf")
    cp ($fx | path join "parser_rebuildxref_correct.pdf") ($WORK | path join "damaged.pdf")
    cp ($bfx | path join "foxittext.pdf") ($WORK | path join "paper.pdf")

    # A parent NO_COLOR would mute the recording; vhs's PTY is a terminal.
    hide-env -i NO_COLOR

    let tape = "docs/assets/cli/pdfrum-cli.tape"
    let gif = "docs/assets/cli/pdfrum-cli.gif"
    # `$env.PATH` is a list; a colon-string here would hide xvfb-run and ttyd.
    let vars = {
        PATH: ($env.PATH | prepend ($bin | path dirname))
        TERM: "xterm-256color"
        COLORTERM: "truecolor"
    }

    print "==> vhs"
    let display = ($env.DISPLAY? | default "")
    if ($display | is-empty) {
        if (which xvfb-run | is-empty) {
            print --stderr "error: DISPLAY is unset and xvfb-run is not on PATH"
            exit 1
        }
        with-env $vars { ^xvfb-run -a vhs $tape }
    } else {
        with-env $vars { ^vhs $tape }
    }

    if not ($gif | path exists) {
        print --stderr $"error: vhs did not write ($gif)"
        exit 1
    }
    let bytes = (ls $gif | get size | first | into int)
    print $"wrote ($gif) ($bytes) bytes"
    if $bytes > $BUDGET {
        print --stderr $"warning: GIF is ($bytes) bytes, over ($BUDGET); tighten Sleep, Framerate, or drop a beat"
    }
}
