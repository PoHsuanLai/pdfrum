#!/bin/bash
# Run each test PDF under /usr/bin/time -v with a timeout, capturing
# wall time, exit status/signal, peak RSS, and stdout/stderr.
# ulimit -v is UNSET here (default). A separate capped run is done for string.
set -u
cd "$(dirname "$0")"
DRIVER=./driver
OUT=results
mkdir -p "$OUT"

run() {
  local name="$1" pdf="$2" tmo="$3"
  echo "======== $name (timeout ${tmo}) ========"
  ulimit -v unlimited 2>/dev/null
  /usr/bin/time -v timeout --signal=KILL "$tmo" "$DRIVER" "$pdf" \
      >"$OUT/$name.out" 2>"$OUT/$name.time"
  local rc=$?
  echo "exit_code=$rc"
  echo "--- stdout ---"; cat "$OUT/$name.out"
  echo "--- time -v (subset) ---"
  grep -E "Elapsed \(wall|Maximum resident|Command terminated|Exit status" "$OUT/$name.time"
  echo
}

run control  control.pdf  120s
run string    string.pdf   120s
run regex20   regex20.pdf  120s
run regex24   regex24.pdf  120s
run regex26   regex26.pdf  120s
run regex28   regex28.pdf  120s
run loop      loop.pdf     120s
