#!/usr/bin/env nu
# Reclaim the disk that cargo target directories eat.
#
# Every agent builds this workspace in its own `CARGO_TARGET_DIR` under
# `$PDFRUM_TARGET_ROOT/<name>` (scripts/env.nu; default `<repo>/../cargo-target`),
# and a finished agent leaves its tree behind. Measured 2026-09-02: thirty-three of them, **600 GB**, of which
# two were in use. Incremental compilation is already off
# (`.cargo/config.toml`); this is the other half of the problem — not that
# each tree is large, but that nothing removed the ones nobody was reading.
#
# What it removes: every directory under the target root that no running
# process names in its `CARGO_TARGET_DIR`, plus any `target/` inside a
# worktree (a worktree that forgot to redirect). What it keeps: anything a
# live process holds, the root's own files, and whatever `--keep` names.
#
# "In use" is read from `/proc/*/environ` of the processes this user can
# read — a build, a test run, or a conformance run all carry the variable.
# A process that sets it some other way (a `[build] target-dir` in a config
# file) is not seen; pass `--keep` for those.
#
# Run standalone, or let the session's periodic sweep call it. `--dry-run`
# prints what would go and touches nothing.

use env.nu [target-root worktree-root]


# Target directories some running process is using, from its environment.
def in-use []: nothing -> list<string> {
    glob /proc/[0-9]*/environ
    | each {|f|
        # Not every process is ours to read; a denied one is not in use by us.
        let r = (do --ignore-errors { open --raw $f })
        if $r == null { return null }
        $r | split row (char nul)
        | where {|kv| $kv starts-with "CARGO_TARGET_DIR=" }
        | each {|kv| $kv | str replace "CARGO_TARGET_DIR=" "" }
    }
    | flatten
    | uniq
}

def size-of [dir: string]: nothing -> filesize {
    (^du -sb $dir | split column "\t" bytes | get bytes.0 | into int) * 1b
}

def main [
    --dry-run       # print what would be removed, remove nothing
    --keep: list<string> = []  # target-dir names to keep whatever the scan says
] {
    let live = (in-use)
    let target_root = (target-root)
    let worktree_root = (worktree-root)
    let candidates = if ($target_root | path exists) {
        ls $target_root | where type == dir | get name
    } else { [] }
    let worktree_targets = (glob $"($worktree_root)/*/target" | where {|p| $p | path exists })

    mut freed = 0b
    mut kept = []
    for dir in ($candidates ++ $worktree_targets) {
        let name = ($dir | path basename)
        let parent = ($dir | path dirname | path basename)
        let label = if $parent == "worktrees" { $"worktrees/($parent)/target" } else { $name }
        if ($dir in $live) or ($name in $keep) {
            $kept = ($kept | append $label)
            continue
        }
        let size = (size-of $dir)
        $freed += $size
        if $dry_run {
            print $"would remove ($label)  ($size)"
        } else {
            print $"removing ($label)  ($size)"
            rm -rf $dir
        }
    }
    let verb = if $dry_run { "would free" } else { "freed" }
    print $"target root ($target_root)"
    print $"($verb) ($freed); kept ($kept | str join ', ')"
}
