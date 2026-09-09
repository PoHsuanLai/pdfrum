#!/usr/bin/env nu
# Record the CLI showcase by driving a real Kitty window and capturing
# the X11 framebuffer. That is what `preview` / `view` look like: Kitty's
# graphics protocol, not half-blocks.
#
#   ./scripts/record-cli.nu
#
# Stages in-tree fixtures under friendly names, builds pdfrum-cli, launches
# Kitty on Xvfb, sends the session over Kitty remote control, and writes
# MP4 (canonical) plus a GIF for the README. Not part of the gate.
#
# Requires kitty, Xvfb, ffmpeg. Software GL (llvmpipe) is used on a
# headless host.

const WORK = '/tmp/pdfrum-cli-demo'
const DISPLAY_NUM = 93
const SIZE = '960x1280'
const GIF_BUDGET = 8000000
const MP4_BUDGET = 15000000

def kitty-bin [] {
    let candidates = [
        ($env.HOME | path join .local kitty.app kitty.app bin kitty)
        ($env.HOME | path join .local bin kitty)
    ]
    for p in $candidates {
        if ($p | path exists) { return $p }
    }
    let on_path = (which kitty)
    if not ($on_path | is-empty) { return $on_path.0.path }
    print --stderr "error: kitty is not on PATH"
    print --stderr "       curl -fsSL https://sw.kovidgoyal.net/kitty/installer.sh | sh /dev/stdin dest ~/.local/kitty.app launch=n"
    exit 1
}

def send [kitty: string, sock: string, text: string] {
    ^$kitty @ --to $sock send-text -- $text
}

def key [kitty: string, sock: string, name: string] {
    ^$kitty @ --to $sock send-key -- $name
}

# Drop Kitty images, then clear the screen. Ctrl+L alone leaves graphics
# placed, and a later `preview` will not show in the X11 grab.
def clear-screen [kitty: string, sock: string] {
    try { ^$kitty @ --to $sock kitten icat --clear }
    key $kitty $sock "ctrl+l"
    # The screen really is empty between dropping the image and the next
    # command's output, so the grab catches one blank frame per transition —
    # the blink between beats. It is a single 24fps frame either way, so this
    # wait is only about letting the clear land, not about hiding it.
    sleep 200ms
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

def kill-pidfile [path: string] {
    if not ($path | path exists) { return }
    let pid = (open $path | into string | str trim)
    if ($pid | is-empty) { return }
    try { ^kill -INT $pid }
    sleep 400ms
    try { ^kill -9 $pid }
    rm -f $path
}

def main [] {
    let root = ($env.FILE_PWD | path dirname)
    cd $root

    for tool in [Xvfb ffmpeg] {
        if (which $tool | is-empty) {
            print --stderr $"error: ($tool) is not on PATH"
            exit 1
        }
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
    let mp4 = "docs/assets/cli/pdfrum-cli.mp4"
    let gif = "docs/assets/cli/pdfrum-cli.gif"
    let display = $":($DISPLAY_NUM)"
    let path = ($env.PATH | prepend ($bin | path dirname) | prepend ($kitty | path dirname))

    let xvfb_pidf = "/tmp/pdfrum-xvfb.pid"
    let kitty_pidf = "/tmp/pdfrum-kitty.pid"
    let ff_pidf = "/tmp/pdfrum-ffmpeg.pid"
    rm -f $xvfb_pidf $kitty_pidf $ff_pidf /tmp/pdfrum-cli-kitty

    let path_colon = ($path | str join ":")
    let launch = "/tmp/pdfrum-cli-launch.sh"
    let bang = '$' + '!'
    let dollar = '$'
    [
        "#!/bin/bash"
        "set -euo pipefail"
        $"Xvfb ($display) -screen 0 ($SIZE)x24 -ac +extension GLX +render -noreset >/tmp/pdfrum-xvfb.log 2>&1 &"
        $"echo ($bang) > ($xvfb_pidf)"
        "sleep 0.4"
        $"export DISPLAY=($display)"
        "export LIBGL_ALWAYS_SOFTWARE=1"
        "export GALLIUM_DRIVER=llvmpipe"
        "export KITTY_DISABLE_WAYLAND=1"
        $"export PATH=($path_colon)"
        $"export PS1='($dollar) '"
        $"($kitty) --config ($conf) --listen-on ($sock) bash --noprofile --norc >/tmp/pdfrum-kitty.log 2>&1 &"
        $"echo ($bang) > ($kitty_pidf)"
    ] | str join (char nl) | save -f $launch
    ^chmod +x $launch

    print $"==> Xvfb ($display) ($SIZE) + kitty"
    ^bash $launch
    wait-sock $kitty $sock
    sleep 300ms

    # Setup, not recorded.
    send $kitty $sock "cd /tmp/pdfrum-cli-demo\r"
    sleep 200ms
    # Prime the graphics path before the capture starts. x11grab under llvmpipe
    # does not pick up Xvfb damage until something forces a full repaint, and
    # the first image of a session has nothing before it to force one: the
    # shell runs the command 18ms after the send, but the grab showed the
    # prompt and the page arriving together ~2s later, which read as a slow
    # first `preview`. One throwaway image here pays that off camera.
    #
    # It has to stay on screen: a `clear` after it puts the grab back to
    # square one and the first beat goes to ~4.7s. `--width 12` keeps the
    # leftover thumbnail small, which costs a little (first beat ~0.4s rather
    # than the 42ms of the rest) but does not park a full page in frame one.
    send $kitty $sock "pdfrum preview gradients.pdf --width 12\r"
    sleep 1.5sec
    clear-screen $kitty $sock

    print "==> ffmpeg"
    rm -f $mp4
    [
        "#!/bin/bash"
        $"export DISPLAY=($display)"
        $"ffmpeg -nostdin -y -f x11grab -draw_mouse 0 -video_size ($SIZE) -framerate 24 -i ($display) -c:v libx264 -pix_fmt yuv420p -crf 18 -preset fast ($mp4) >/tmp/pdfrum-ffmpeg.log 2>&1 &"
        $"echo ($bang) > ($ff_pidf)"
    ] | str join (char nl) | save -f /tmp/pdfrum-cli-ffmpeg.sh
    ^bash /tmp/pdfrum-cli-ffmpeg.sh
    sleep 1sec
    # The setup lines and the priming image are still on screen: the pre-grab
    # clear does not reach the capture. Clear once more now that x11grab is
    # running, so the session opens on a bare prompt.
    clear-screen $kitty $sock

    # Visible session. Graphics commands stay before `view`: a later
    # `preview` in the same Kitty after the pager's alt-screen does not
    # composite into the X11 grab.
    # Pauses are reading time, not work: a page renders in about 40ms and the
    # framebuffer follows within a frame. Each hold is just long enough to
    # take the frame in.
    send $kitty $sock "pdfrum preview gradients.pdf\r"
    sleep 3.2sec
    clear-screen $kitty $sock
    send $kitty $sock "pdfrum stamp text gradients.pdf DRAFT --angle 30 --opacity 0.4 --size 72 -o stamped.pdf\r"
    sleep 1.2sec
    clear-screen $kitty $sock
    send $kitty $sock "pdfrum preview stamped.pdf\r"
    sleep 2.6sec
    clear-screen $kitty $sock
    send $kitty $sock "pdfrum view stamped.pdf\r"
    sleep 250ms
    # The first placement after the alt-screen switch is dropped by the grab,
    # and `0` (zoom 100%) forces the redraw that makes page 1 appear. So the
    # wait above is the whole visible startup gap: the status bar is up and
    # the page is not until this key lands, which reads as a slow open. Keep
    # it short — `view` emits its first placement in ~5ms. A page turn needs
    # no such guard: neighbours render and transmit while we wait for a key,
    # so j/k land in a single frame.
    key $kitty $sock "0"
    sleep 1.5sec
    key $kitty $sock "j"
    sleep 1.4sec
    key $kitty $sock "k"
    sleep 1.2sec
    key $kitty $sock "+"
    sleep 1.4sec
    key $kitty $sock "q"
    sleep 600ms
    clear-screen $kitty $sock
    send $kitty $sock "pdfrum doctor damaged.pdf\r"
    sleep 1.8sec
    clear-screen $kitty $sock
    send $kitty $sock "pdfrum doctor damaged.pdf --json\r"
    sleep 2.2sec
    clear-screen $kitty $sock
    send $kitty $sock "pdfrum search ISO paper.pdf\r"
    sleep 1.6sec
    clear-screen $kitty $sock
    send $kitty $sock "pdfrum extract markdown paper.pdf --pages 1\r"
    sleep 2sec

    print "==> stop"
    sleep 500ms
    kill-pidfile $ff_pidf
    sleep 800ms
    kill-pidfile $kitty_pidf
    kill-pidfile $xvfb_pidf

    if not ($mp4 | path exists) {
        print --stderr "error: ffmpeg did not write the MP4; see /tmp/pdfrum-ffmpeg.log"
        exit 1
    }

    print "==> gif"
    # Derived from the MP4 for GitHub README; the MP4 is the recording.
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
