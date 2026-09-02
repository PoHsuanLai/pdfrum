#!/usr/bin/env nu
# STYLE.md §4 / docs/design/idiomatic-api.md §C: no sentinel in a public constant.
#
# A sentinel is a magic value standing for absence or failure in a channel
# whose type says it is an ordinary value: `-1`, `u16::MAX`/`0xFFFF`,
# `u32::MAX`/`0xFFFF_FFFF`, `usize::MAX`, `i32::MIN`, an inverted rect, `NaN`.
# The name-driven search missed `WIDTH_UNSET` and `EMPTY_CLIP_RECT` (§C.2);
# this check is the value-agnostic pass WP13 exists to install.
#
# What it asserts:
#
#   1. Every `pub const` in docs/status/api-baseline/*.txt (not `pub const fn`)
#      is opened at its defining source line and its value is not a sentinel,
#      unless the constant is listed in docs/status/pub-const-allowlist.md
#      with a reason — a real limit, identity, or table value, not absence
#      standing for a value.
#   2. The converse: a planted `pub const PLANTED_SENTINEL: i32 = -1` is found
#      and reported, so the scan cannot pass by having stopped looking.
#
# Run standalone, or via scripts/ci.nu which calls it.

const BASELINE = 'docs/status/api-baseline'
const ALLOWLIST = 'docs/status/pub-const-allowlist.md'

# Snapshot files that are not a crate's public API.
const SKIP_SNAPSHOTS = ['README.md']

# `pdfrum_object::names::A85` lives in `crates/pdfrum-object`. The snapshot
# filename is the fallback for a path that does not start with `pdfrum`.
def crate-dir [path: string, snapshot: string]: nothing -> string {
    let stem = ($path | split row '::' | first)
    let crate = (if ($stem | str starts-with 'pdfrum') {
        $stem | str replace --all '_' '-'
    } else {
        let file = ($snapshot | path basename | str replace '.txt' '')
        if ($file | str contains '+') { $file | split row '+' | first } else { $file }
    })
    $'crates/($crate)/src'
}

def const-ident [path: string]: nothing -> string {
    $path | split row '::' | last
}

# `pub const PATH: TYPE` — not `pub const fn`.
def snapshot-consts [file: string]: nothing -> list<record> {
    open --raw $file | lines | enumerate | each {|r|
        let l = $r.item
        if ($l | str starts-with 'pub const fn ') or not ($l | str starts-with 'pub const ') {
            return null
        }
        # `pub const pdfrum_font::FontFlags::SERIF: Self`
        let rest = ($l | str replace 'pub const ' '')
        let parts = ($rest | split row ': ')
        if ($parts | length) < 2 { return null }
        {
            snapshot: ($file | path basename)
            path: ($parts | first | str trim)
            type: ($parts | skip 1 | str join ': ' | str trim)
            line: ($r.index + 1)
        }
    } | compact
}

def find-definition [c: record]: nothing -> record {
    let dir = (crate-dir $c.path $c.snapshot)
    if not ($dir | path exists) {
        return {ok: false, why: $"no source dir ($dir)", file: '', line: 0, value: ''}
    }
    let ident = (const-ident $c.path)
    # `const IDENT` on its own line or after `pub`. rustfmt may wrap the
    # type, so the `=` can be several lines below.
    mut hits = []
    for file in (glob $"($dir)/**/*.rs" | sort) {
        let lines = (open --raw $file | decode utf-8 | lines)
        for hit in ($lines | enumerate | where {|r|
            # A `const IDENT` in source, or the `names!` / table input
            # `IDENT = "spelling";` that expands to one.
            ($r.item =~ $'\bconst ($ident)\b') or ($r.item =~ $'^\s*($ident)\s*=\s*"')
        }) {
            mut end = $hit.index
            mut blob = ($lines | get $end)
            while ($end < (($lines | length) - 1)) and (not ($blob | str contains ';')) {
                $end = $end + 1
                $blob = $blob + ' ' + ($lines | get $end)
            }
            let value = (if ($blob | str contains '=') {
                ($blob | str replace -r '^[^=]*=' '' | str replace -r ';.*$' '' | str replace -r '//.*$' '' | str trim)
            } else {
                ''
            })
            $hits = ($hits | append {
                file: ($file | path relative-to (pwd))
                line: ($hit.index + 1)
                value: $value
                pub: (($blob | str contains 'pub const') or ($blob | str contains 'names!'))
            })
        }
    }
    if ($hits | is-empty) {
        return {ok: false, why: $"no `const ($ident)` under ($dir)", file: '', line: 0, value: ''}
    }
    # Prefer a `pub const`; then a unique hit; then the first.
    let preferred = (if ($hits | where {|h| $h.pub} | is-not-empty) {
        $hits | where {|h| $h.pub}
    } else {
        $hits
    })
    let h = ($preferred | first)
    {ok: true, why: '', file: $h.file, line: $h.line, value: $h.value}
}

# Reasons a value is sentinel-shaped, per §C. Empty means it is not.
def sentinel-reasons [value: string]: nothing -> list<string> {
    let v = ($value | str trim)
    if ($v | is-empty) { return [] }
    mut reasons = []

    # Integer `-1`, not the float `-1.0` and not a digit in a larger number.
    if ($v =~ '(^|[^0-9.])-1([^0-9.]|$)') {
        $reasons = ($reasons | append '-1')
    }
    if ($v | str contains 'u16::MAX') { $reasons = ($reasons | append 'u16::MAX') }
    if ($v | str contains 'u32::MAX') { $reasons = ($reasons | append 'u32::MAX') }
    if ($v | str contains 'usize::MAX') { $reasons = ($reasons | append 'usize::MAX') }
    if ($v | str contains 'i32::MIN') { $reasons = ($reasons | append 'i32::MIN') }

    # Hex forms of the same maxima. Underscores optional; do not match a
    # longer word such as `0xFFFF_FFFC`.
    if ($v =~ '(?i)0x_?ffff([^0-9a-f_]|$)') { $reasons = ($reasons | append '0xFFFF') }
    if ($v =~ '(?i)0x_?ffff_?ffff([^0-9a-f_]|$)') { $reasons = ($reasons | append '0xFFFF_FFFF') }

    # The numeric spelling of i32::MIN, underscores optional.
    if ($v | str contains '-2147483648') or ($v | str contains '-2_147_483_648') {
        $reasons = ($reasons | append 'i32::MIN')
    }

    if ($v =~ '(?i)(^|[^A-Za-z])NaN([^A-Za-z]|$)') or ($v | str contains 'f32::NAN') or ($v | str contains 'f64::NAN') {
        $reasons = ($reasons | append 'NaN')
    }

    # An inverted rect: `Rect::new(x0, y0, x1, y1)` with x1 < x0 or y1 < y0.
    if ($v | str contains 'Rect::new') {
        let nums = ($v | parse -r 'Rect::new\(\s*(?<x0>[^,]+),\s*(?<y0>[^,]+),\s*(?<x1>[^,]+),\s*(?<y1>[^)]+)\)')
        if not ($nums | is-empty) {
            let row = ($nums | first)
            let floats = ([$row.x0 $row.y0 $row.x1 $row.y1] | each {|s|
                try { $s | str trim | str replace '_' '' | into float } catch { null }
            })
            if ($floats | all {|x| $x != null }) {
                let x0 = ($floats | get 0)
                let y0 = ($floats | get 1)
                let x1 = ($floats | get 2)
                let y1 = ($floats | get 3)
                if $x1 < $x0 or $y1 < $y0 {
                    $reasons = ($reasons | append 'inverted rect')
                }
            }
        }
    }

    $reasons | uniq
}

def allowlist []: nothing -> list<record> {
    if not ($ALLOWLIST | path exists) { return [] }
    open --raw $ALLOWLIST | lines | each {|l|
        let t = ($l | str trim)
        if ($t | is-empty) or ($t | str starts-with '#') { return null }
        # Constant lines start with a snapshot path (`pdfrum_…::…`).
        if not ($t | str starts-with 'pdfrum') { return null }
        # `path — reason` (em dash) or `path -- reason` or `path: reason`.
        let split = (if ($t | str contains ' — ') {
            $t | split row ' — '
        } else if ($t | str contains ' -- ') {
            $t | split row ' -- '
        } else if ($t | str contains ': ') {
            $t | split row ': '
        } else {
            [$t '']
        })
        {path: ($split | first | str trim), reason: ($split | skip 1 | str join ' — ' | str trim)}
    } | compact
}

def scan []: nothing -> list<record> {
    glob ($BASELINE | path join '*.txt') | sort | each {|file|
        snapshot-consts $file | each {|c|
            let def = (find-definition $c)
            let reasons = (if $def.ok { sentinel-reasons $def.value } else { [] })
            {
                path: $c.path
                type: $c.type
                snapshot: $c.snapshot
                source: $def.file
                source_line: $def.line
                value: $def.value
                found: $def.ok
                why: $def.why
                reasons: $reasons
            }
        }
    } | flatten
}

def main [] {
    cd ($env.FILE_PWD | path dirname)

    print '==> no sentinels in public constants'
    let allowed = (allowlist)
    let allowed_paths = ($allowed | get path)
    let all = (scan)

    let missing_src = ($all | where {|c| not $c.found })
    if not ($missing_src | is-empty) {
        print --stderr 'error: public constants with no defining source line:'
        for c in $missing_src {
            print --stderr $"  ($c.path)  \(($c.snapshot): ($c.why)\)"
        }
        print --stderr '       The sweep reads the value from source. A constant the'
        print --stderr '       snapshot names must exist in the crate the snapshot is of.'
        exit 1
    }

    let stale = ($allowed_paths | where {|p| $p not-in ($all | get path) } | sort)
    if not ($stale | is-empty) {
        print --stderr 'error: allowlist names constants that are not in the snapshots:'
        $stale | each {|p| print --stderr $"  ($p)" } | ignore
        print --stderr $"       Delete these lines from ($ALLOWLIST)."
        exit 1
    }

    let hits = ($all | where {|c| not ($c.reasons | is-empty) })
    let denied = ($hits | where {|c| $c.path not-in $allowed_paths })
    if not ($denied | is-empty) {
        print --stderr 'error: public constant with a sentinel-shaped value:'
        for c in $denied {
            print --stderr $"  ($c.path): ($c.type) = ($c.value)"
            print --stderr $"    ($c.source):($c.source_line)"
            print --stderr $"    sentinel: ($c.reasons | str join ', ')"
        }
        print --stderr ''
        print --stderr '       docs/design/idiomatic-api.md §C: a sentinel value must not'
        print --stderr '       appear in a public constant. Option carries absence; Result'
        print --stderr '       carries failure. If this number is a real limit, identity,'
        print --stderr $"       or table value, list it in ($ALLOWLIST) with the reason."
        exit 1
    }

    let listed = ($hits | where {|c| $c.path in $allowed_paths } | length)
    print $"ok: no unallowlisted sentinel in ($all | length) public constants"
    print $"    \(($listed) allowlisted as a limit/identity/table value, each in ($ALLOWLIST)\)"

    # The converse, so the check cannot pass by having stopped looking.
    print '==> the scan does still find one'
    let probe_snap = ($BASELINE | path join 'zz-wp13-probe.txt')
    let probe_src = 'crates/pdfrum/src/zz_wp13_pub_const_probe.rs'
    'pub const pdfrum::PLANTED_SENTINEL: i32' | save --force $probe_snap
    'pub const PLANTED_SENTINEL: i32 = -1;' | save --force $probe_src
    let seen = (do --ignore-errors { scan } | default []
        | where {|c| ($c.path | str ends-with 'PLANTED_SENTINEL') and ('-1' in $c.reasons) })
    rm --force $probe_snap
    rm --force $probe_src
    if ($seen | is-empty) {
        print --stderr 'error: a planted `pub const PLANTED_SENTINEL: i32 = -1` was not'
        print --stderr '       reported as a sentinel — the scan above is measuring nothing.'
        exit 1
    }
    print 'ok: a planted sentinel constant is found and reported'

    print ''
    print 'pub-const check: green.'
}
