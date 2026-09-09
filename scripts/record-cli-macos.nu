#!/usr/bin/env nu
# Record the CLI showcase natively on macOS: a real Kitty window on a real
# display, captured by window id. Same beats as ./scripts/record-cli.nu,
# which drives Xvfb on a headless Linux host.
#
#   ./scripts/record-cli-macos.nu
#
# Prefer this on a Mac with a display. `screencapture -l<window-id>` grabs
# the Kitty window itself, so nothing else on screen can enter the frame and
# there is no crop geometry to keep in sync. The window is captured at 2x
# (1920x1480) and downscaled to 960x740, which supersamples the glyphs: text
# is sharper than the Xvfb recording, which rasterized at 1x under llvmpipe.
#
# The X11 script's two capture workarounds are deliberately absent. Both were
# llvmpipe damage-tracking artifacts: a priming image before the grab, and an
# extra clear once ffmpeg was up. A native compositor repaints on its own, so
# the first `preview` lands in a frame like every other beat.
#
# Requires kitty, ffmpeg, and DejaVu Sans Mono:
#   brew install --cask font-dejavu
#
# Screen Recording permission is required for whatever runs this (Terminal,
# iTerm, etc.) under System Settings > Privacy & Security > Screen Recording.

const WORK = '/tmp/pdfrum-cli-demo'
const WIN_W = 960
const WIN_H = 740
const GIF_BUDGET = 8000000
const MP4_BUDGET = 15000000

def kitty-bin [] {
    let candidates = [
        ($env.HOME | path join .local kitty.app Contents MacOS kitty)
        "/Applications/kitty.app/Contents/MacOS/kitty"
    ]
    for p in $candidates {
        if ($p | path exists) { return $p }
    }
    let on_path = (which kitty)
    if not ($on_path | is-empty) { return $on_path.0.path }
    print --stderr "error: kitty is not on PATH"
    print --stderr "       brew install --cask kitty"
    exit 1
}

def send [kitty: string, sock: string, text: string] {
    ^$kitty @ --to $sock send-text -- $text
}

def key [kitty: string, sock: string, name: string] {
    ^$kitty @ --to $sock send-key -- $name
}

# Drop Kitty images, then clear. Ctrl+L only scrolls the viewport on this
# shell, which stacks one beat's output under the next, so the shell clears
# for real. ESC[3J takes the scrollback with it.
#
# Verified, not timed: a `clear` sent while the previous command still owns
# the terminal is read as that command's input and lost, which stacks the
# next beat under this one.
def clear-screen [kitty: string, sock: string, esc: string] {
    try { ^$kitty @ --to $sock kitten icat --clear }
    mut i = 0
    while $i < 25 {
        send $kitty $sock ("clear && printf \u{27}" + $esc + "[3J\u{27}" + (char cr))
        sleep 200ms
        let txt = (^$kitty @ --to $sock get-text | complete)
        if $txt.exit_code == 0 {
            let body = ($txt.stdout | lines | where { |l| ($l | str trim) != "" })
            if ($body | length) <= 1 { return }
        }
        $i = $i + 1
    }
    print --stderr "warning: screen did not clear between beats"
}

def wait-sock [kitty: string, sock: string] {
    mut i = 0
    while $i < 50 {
        let r = (^$kitty @ --to $sock ls | complete)
        if $r.exit_code == 0 { return }
        sleep 100ms
        $i = $i + 1
    }
    print --stderr "error: kitty remote control did not come up"
    exit 1
}

# The socket answers before bash has been exec'd in the window, and a
# send-text into a missing child is dropped with only a line in kitty's log.
# Wait until a marker actually round-trips through the shell.
def wait-shell [kitty: string, sock: string] {
    mut i = 0
    while $i < 100 {
        ^$kitty @ --to $sock send-text -- ("echo pdfrum-ready" + (char cr))
        sleep 200ms
        let txt = (^$kitty @ --to $sock get-text | complete)
        if $txt.exit_code == 0 and ($txt.stdout | str contains "pdfrum-ready") {
            return
        }
        $i = $i + 1
    }
    print --stderr "error: shell in kitty never became ready"
    exit 1
}

def main [] {
    let root = ($env.FILE_PWD | path dirname)
    cd $root

    if (which ffmpeg | is-empty) {
        print --stderr "error: ffmpeg is not on PATH"
        exit 1
    }
    let kitty = (kitty-bin)

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
    let corpus = ($root | path join "benches" "corpus")
    rm -rf $WORK
    mkdir $WORK
    cp ($fx | path join "bookmarks.pdf") ($WORK | path join "report.pdf")
    cp ($fx | path join "parser_rebuildxref_correct.pdf") ($WORK | path join "damaged.pdf")
    cp ($bfx | path join "foxittext.pdf") ($WORK | path join "paper.pdf")
    cp ($corpus | path join "shading_tcpdf_030.pdf") ($WORK | path join "gradients.pdf")

    hide-env -i NO_COLOR

    let sock = "unix:/tmp/pdfrum-cli-kitty"
    let conf = ($root | path join "docs" "assets" "cli" "kitty.conf")
    let mov = "/tmp/pdfrum-cli-raw.mov"
    let mp4 = "docs/assets/cli/pdfrum-cli.mp4"
    let gif = "docs/assets/cli/pdfrum-cli.gif"
    rm -f /tmp/pdfrum-cli-kitty $mov

    print $"==> kitty ($WIN_W)x($WIN_H)"
    # Launched from a bash wrapper, not with nushell's `&`: on macOS that
    # blocks until kitty exits, so the socket never comes up.
    let bang = '$' + '!'
    let launch = "/tmp/pdfrum-cli-launch.sh"
    [
        "#!/bin/bash"
        "set -euo pipefail"
        "export BASH_SILENCE_DEPRECATION_WARNING=1"
        $"($kitty) --config ($conf) --listen-on ($sock) -o initial_window_width=($WIN_W) -o initial_window_height=($WIN_H) bash --noprofile --norc >/tmp/pdfrum-kitty.log 2>&1 &"
        $"echo ($bang) > /tmp/pdfrum-kitty.pid"
    ] | str join (char nl) | save -f $launch
    ^chmod +x $launch
    ^bash $launch
    wait-sock $kitty $sock
    wait-shell $kitty $sock
    sleep 300ms

    let ls_json = (^$kitty @ --to $sock ls | from json)
    let wid = ($ls_json | get 0.platform_window_id)
    let cols = ($ls_json | get 0.tabs.0.windows.0.columns)
    let lines = ($ls_json | get 0.tabs.0.windows.0.lines)
    print $"    window id ($wid), grid ($cols)x($lines)"
    # The widest beat (`extract markdown`) is 69 columns.
    if $cols < 80 {
        print --stderr $"error: only ($cols) columns; text beats will wrap"
        exit 1
    }

    # Setup, not recorded. `$` is built outside the interpolation: in an
    # interpolated string nushell would eat `$ ` as interpolation syntax and
    # bash would get a malformed PS1.
    let dollar = '$'
    let esc = "\u{1b}"
    let bindir = ($bin | path dirname)
    send $kitty $sock $"export PATH=($bindir):($dollar)PATH\r"
    send $kitty $sock $"export PS1='($dollar) '\r"
    send $kitty $sock $"cd ($WORK)\r"
    sleep 400ms
    clear-screen $kitty $sock $esc
    # Let the window settle on the cleared state before the capture opens:
    # screencapture takes the window as it finds it.
    sleep 1sec

    print "==> screencapture"
    let cap = "/tmp/pdfrum-cli-capture.sh"
    [
        "#!/bin/bash"
        $"screencapture -v -x -o -l($wid) ($mov) >/tmp/pdfrum-capture.log 2>&1 &"
        $"echo ($bang) > /tmp/pdfrum-capture.pid"
    ] | str join (char nl) | save -f $cap
    ^chmod +x $cap
    ^bash $cap
    sleep 1200ms
    # The setup lines are still on screen: the clear above happened before the
    # capture existed. Reset again now that it is running, scrollback included,
    # so the recording opens on a bare prompt.
    clear-screen $kitty $sock $esc
    sleep 400ms

    # Visible session. Graphics beats stay before `view`, matching the X11
    # script: after the pager's alt-screen a later image can be dropped.
    # Pauses are reading time. A page renders in ~40ms and the window follows
    # within a frame.
    send $kitty $sock "pdfrum preview gradients.pdf\r"
    sleep 3.2sec
    clear-screen $kitty $sock $esc
    send $kitty $sock "pdfrum stamp text gradients.pdf DRAFT --angle 30 --opacity 0.4 --size 72 -o stamped.pdf\r"
    sleep 1.2sec
    clear-screen $kitty $sock $esc
    send $kitty $sock "pdfrum preview stamped.pdf\r"
    sleep 2.6sec
    clear-screen $kitty $sock $esc
    send $kitty $sock "pdfrum view stamped.pdf\r"
    # `view` emits its first placement in ~5ms; this is reading time for the
    # status bar, not a workaround. `0` (zoom 100%) is kept from the X11
    # script so the opening frame is a known zoom level.
    sleep 700ms
    key $kitty $sock "0"
    sleep 1.5sec
    key $kitty $sock "j"
    sleep 1.4sec
    key $kitty $sock "k"
    sleep 1.2sec
    # send-key never reports failure and kitty logged "+ has unknown modifier";
    # the pager reads a literal keypress, so send the character itself.
    send $kitty $sock "+"
    sleep 1.4sec
    key $kitty $sock "q"
    # The pager places its page on the alt-screen; quitting restores the
    # primary screen with that image still placed, and the next command then
    # prints underneath it. Let the alt-screen unwind, then clear twice: once
    # to drop the placement, once for the text.
    sleep 900ms
    clear-screen $kitty $sock $esc
    clear-screen $kitty $sock $esc
    send $kitty $sock "pdfrum doctor damaged.pdf\r"
    sleep 1.8sec
    clear-screen $kitty $sock $esc
    send $kitty $sock "pdfrum doctor damaged.pdf --json\r"
    sleep 2.2sec
    clear-screen $kitty $sock $esc
    send $kitty $sock "pdfrum search ISO paper.pdf\r"
    sleep 1.6sec
    clear-screen $kitty $sock $esc
    send $kitty $sock "pdfrum extract markdown paper.pdf --pages 1\r"
    sleep 2sec

    print "==> stop"
    sleep 500ms
    # screencapture writes the movie on SIGINT.
    let cap_pid = (open /tmp/pdfrum-capture.pid | into string | str trim)
    try { ^kill -INT $cap_pid }
    sleep 1500ms
    # close-window leaves the process; kill the pid too.
    try { ^$kitty @ --to $sock close-window }
    sleep 300ms
    let k_pid = (open /tmp/pdfrum-kitty.pid | into string | str trim)
    try { ^kill -9 $k_pid }

    if not ($mov | path exists) {
        print --stderr "error: screencapture wrote no movie"
        print --stderr "       check System Settings > Privacy & Security > Screen Recording"
        exit 1
    }

    print "==> encode"
    rm -f $mp4
    # Captured at 2x; downscale to logical size so glyphs are supersampled.
    let enc = (^ffmpeg -y -i $mov -vf $"scale=($WIN_W):($WIN_H):flags=lanczos" -r 24 -c:v libx264 -pix_fmt yuv420p -crf 18 -preset fast -movflags +faststart $mp4 | complete)
    if $enc.exit_code != 0 {
        print --stderr "error: mp4 encode failed"
        print --stderr $enc.stderr
        exit 1
    }

    print "==> gif"
    let gif_r = (^ffmpeg -y -i $mp4 -vf "fps=12,scale=720:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=256[p];[s1][p]paletteuse=dither=bayer" $gif | complete)
    if $gif_r.exit_code != 0 {
        print --stderr "error: gif encode failed"
        print --stderr $gif_r.stderr
        exit 1
    }

    for out in [$mp4 $gif] {
        let bytes = (ls $out | get size | first | into int)
        print $"wrote ($out) ($bytes) bytes"
        let budget = (if ($out | str ends-with ".gif") { $GIF_BUDGET } else { $MP4_BUDGET })
        if $bytes > $budget {
            print --stderr $"warning: ($out) is ($bytes) bytes, over ($budget)"
        }
    }
}
