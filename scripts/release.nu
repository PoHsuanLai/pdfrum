#!/usr/bin/env nu
# Publish the workspace to crates.io, in dependency order.
#
# A version can be uploaded once and never replaced, so the two failure modes
# worth designing against are publishing in the wrong order and publishing
# half the workspace. The order comes from `scripts/publish-order.nu`, and the
# run is resumable: a crate whose version is already on the index is skipped
# rather than failing the run, so a release interrupted midway is finished by
# running this again.
#
# The third failure mode is crates.io's publish leaky bucket. New crate names
# get a burst of 5 and then one every 10 minutes; new versions of crates that
# already exist get a burst of 30 and then one per minute
# (https://crates.io/docs/rate-limits). Twenty-three crates sit under the
# version burst and over the new-crate burst, so the first release has to
# wait, and a `cargo publish` that still hits 429 has to honour Retry-After
# rather than abort the run. The two buckets are independent.
#
# Verification stays on. `cargo publish` builds each packaged crate against
# the versions it will actually resolve to on crates.io, which is the only
# check that the uploaded manifest — not the workspace one — is coherent.
#
# `--dry-run` here means "do everything except upload". Note that cargo's own
# `--dry-run` cannot see unpublished siblings, so it is not used: the check
# this performs is `cargo package`, which the release workflow runs with every
# crate visible at once.

const REGISTRY = "https://crates.io/api/v1/crates"
const USER_AGENT = "pdfrum-release (https://github.com/PoHsuanLai/pdfrum)"

# Documented leaky-bucket limits. Used to pace before we hit 429; the retry
# path still parses the server's "try again after" so a resume, a partial
# burst, or a limit change does not depend on these numbers being exact.
const NEW_BURST = 5
const NEW_RATE = 10min
const UPDATE_BURST = 30
const UPDATE_RATE = 1min
const RATE_BUFFER = 5sec
const RATE_WAIT_CAP = 15min
const PUBLISH_RETRIES = 8

# GET a crates.io crate/version path. Only 200 and 404 are answers; 403 and
# 429 are retried, because treating them as "not published" would re-upload a
# crate that already landed or spin the 300s index wait on one that did.
def crates-io [path: string]: nothing -> int {
    let url = $"($REGISTRY)/($path)"
    mut attempt = 0
    mut status = 0
    loop {
        let res = (try {
            http get --headers [User-Agent $USER_AGENT] --max-time 30sec --allow-errors --full $url
        } catch {
            {status: 0}
        } | default {status: 0})
        match $res.status {
            200 | 404 => {
                $status = $res.status
                break
            }
            429 | 403 => {
                if $attempt >= 6 {
                    print --stderr $"error: crates.io returned ($res.status) for ($path)"
                    print --stderr "       identify the client (User-Agent is set) or wait out the rate limit"
                    exit 1
                }
                sleep 10sec
                $attempt = $attempt + 1
            }
            _ => {
                print --stderr $"error: unexpected HTTP ($res.status) from ($url)"
                exit 1
            }
        }
    }
    $status
}

# Whether this exact name and version is already on crates.io.
def published [name: string, version: string]: nothing -> bool {
    (crates-io $"($name)/($version)") == 200
}

# Whether the crate *name* exists (any version). Distinguishes the new-crate
# bucket from the version-update bucket.
def crate-exists [name: string]: nothing -> bool {
    (crates-io $name) == 200
}

def rate-limited [text: string]: nothing -> bool {
    $text =~ '(?i)(got 429|status 429|too many requests|too many new crates|too many updates|try again after)'
}

def already-on-index [text: string]: nothing -> bool {
    $text =~ '(?i)already (uploaded|exists on crates.io)'
}

# How long crates.io asked us to wait. Prefer the timestamp in the body
# ("Please try again after … GMT"), then Retry-After as an HTTP-date (what
# crates.io actually sends), then Retry-After as seconds. Cap so a bogus
# hour-long header does not stall the job; the retry loop will wait again
# if the bucket still has no token.
def retry-wait [text: string, fallback: duration]: nothing -> duration {
    mut raw = $fallback

    let body = ($text | parse --regex '(?i)try again after ([A-Za-z]+, \d+ [A-Za-z]+ \d+ \d+:\d+:\d+ GMT)')
    let date_hdr = ($text | parse --regex '(?i)retry-after:\s*([A-Za-z]+, \d+ [A-Za-z]+ \d+ \d+:\d+:\d+ GMT)')
    let secs_hdr = ($text | parse --regex '(?i)retry-after:\s*(\d+)')

    if not ($body | is-empty) {
        let target = ($body.0.capture0 | into datetime)
        let w = $target - (date now)
        $raw = (if $w > 0sec { $w } else { 1sec })
    } else if not ($date_hdr | is-empty) {
        let target = ($date_hdr.0.capture0 | into datetime)
        let w = $target - (date now)
        $raw = (if $w > 0sec { $w } else { 1sec })
    } else if not ($secs_hdr | is-empty) {
        $raw = ($"($secs_hdr.0.capture0)sec" | into duration)
    }

    let with_buf = $raw + $RATE_BUFFER
    if $with_buf > $RATE_WAIT_CAP { $RATE_WAIT_CAP } else { $with_buf }
}

def pace [
    count: int,
    last: datetime,
    burst: int,
    rate: duration,
    label: string,
] {
    if $count < $burst { return }
    let wait = ($last + $rate + $RATE_BUFFER) - (date now)
    if $wait <= 0sec { return }
    print $"    crates.io ($label) burst of ($burst) spent; waiting ($wait)"
    sleep $wait
}

# `cargo publish` with live output, then a 429 retry. Verification stays on
# for every attempt: a rate-limited upload is the only thing we retry, and
# rebuilding is cheaper than uploading a crate we have not just checked.
def publish-crate [name: string, version: string, kind: string] {
    let fallback = (if $kind == "new" { $NEW_RATE } else { $UPDATE_RATE })
    mut attempt = 0
    loop {
        print $"==> publishing ($name) ($version)"
        let result = (
            do { ^cargo publish -p $name --locked }
            o+e>| tee { each {|l| print -n --stderr $l } }
            | complete
        )
        if $result.exit_code == 0 {
            return
        }
        let text = $result.stdout
        if (already-on-index $text) {
            print $"    ($name) ($version) landed while we were publishing; treating as done"
            return
        }
        if not (rate-limited $text) {
            print --stderr $"error: cargo publish -p ($name) failed"
            exit 1
        }
        if $attempt >= $PUBLISH_RETRIES {
            print --stderr $"error: crates.io still rate-limiting ($name) after ($PUBLISH_RETRIES) retries"
            print --stderr "       re-run this script; published crates are skipped"
            exit 1
        }
        let wait = (retry-wait $text $fallback)
        print $"    crates.io 429 on ($name); waiting ($wait) then retrying"
        sleep $wait
        $attempt = $attempt + 1
    }
}

def main [
    --dry-run  # Package and verify every crate, but upload nothing.
] {
    cd ($env.FILE_PWD | path dirname)

    let version = (^cargo metadata --no-deps --format-version 1
        | from json
        | get packages
        | where name == "pdfrum"
        | first
        | get version)

    let order = (^./scripts/publish-order.nu | lines | where {|l| $l != "" })
    if ($order | is-empty) {
        print --stderr "error: publish-order.nu returned no crates"
        exit 1
    }

    print $"==> pdfrum ($version): ($order | length) crates"
    if $dry_run {
        print "    dry run: packaging and verifying, uploading nothing"
    }
    print ""

    if $dry_run {
        # One `cargo package` over the whole set, so each crate resolves its
        # unpublished siblings from the workspace instead of the index. Run
        # per-crate this would fail on the first internal dependency.
        let excluded = (^cargo metadata --no-deps --format-version 1
            | from json
            | get packages
            | where {|p| $p.publish == [] }
            | get name)
        let flags = ($excluded | each {|n| [--exclude $n] } | flatten)
        ^cargo package --workspace --locked ...$flags
        print ""
        print $"packaged ($order | length) crates."
        return
    }

    mut new_count = 0
    mut new_last = (date now)
    mut update_count = 0
    mut update_last = (date now)

    for name in $order {
        if (published $name $version) {
            print $"    ($name) ($version) is already published; skipping"
            continue
        }

        let kind = (if (crate-exists $name) { "update" } else { "new" })
        if $kind == "new" {
            pace $new_count $new_last $NEW_BURST $NEW_RATE "new-crate"
        } else {
            pace $update_count $update_last $UPDATE_BURST $UPDATE_RATE "version-update"
        }

        publish-crate $name $version $kind

        if $kind == "new" {
            $new_count = $new_count + 1
            $new_last = date now
        } else {
            $update_count = $update_count + 1
            $update_last = date now
        }

        # crates.io serves the index through a cache, so the next crate can
        # ask for this one before it is visible. Wait for it rather than
        # racing, and give up loudly instead of publishing out of order.
        print $"    waiting for ($name) ($version) on the index"
        mut waited = 0
        while not (published $name $version) {
            if $waited >= 300 {
                print --stderr $"error: ($name) ($version) did not appear on the index within 300s"
                print --stderr "       re-run this script once it does; published crates are skipped"
                exit 1
            }
            sleep 5sec
            $waited = $waited + 5
        }
    }

    print ""
    print $"published pdfrum ($version)."
}
