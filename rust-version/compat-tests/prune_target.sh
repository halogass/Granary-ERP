#!/usr/bin/env bash
# Frees disk in target/debug: deletes older duplicate test/bin executables in deps (keeps the newest per name).
# Only files untouched for 90 minutes are candidates: a lib test and a bin of one crate share a name, so a fresh
# binary of the current run must never be removed. With --incremental also deletes target/debug/incremental (only when nothing builds).
cd "$(dirname "$0")/../target/debug/deps" 2>/dev/null || exit 0
find . -maxdepth 1 -type f -executable -name '*-*' -mmin +90 -printf '%T@ %f\n' | awk '{n=$2; sub(/-[0-9a-f]{16}$/,"",n); print n, $1, $2}' \
  | sort -k1,1 -k2,2nr | awk 'seen[$1]++ {print $3}' | xargs -r rm -f --
[ "${1:-}" = "--incremental" ] && rm -rf ../incremental
exit 0
