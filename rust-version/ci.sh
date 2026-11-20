#!/usr/bin/env bash
# Local CI: format, lint, unit and integration tests, golden tests; prints a summary.
#
#   ./ci.sh              # fmt, clippy, cargo test, golden tests
#
# Clippy runs with -D warnings: any lint fails the step (workspace settings in clippy.toml; a deliberate
# exception is an #[allow] with a one-line reason at the site). Every step runs even when an
# earlier one failed; the exit code is non-zero if any step failed.
set -uo pipefail
cd "$(dirname "$0")"
# No incremental cache for the full run: it adds ~9 GB of disk and a CI run rebuilds everything anyway.
export CARGO_INCREMENTAL=0

for arg in "$@"; do
  echo "unknown option: $arg" >&2; exit 2
done

LOG_DIR="${CI_LOG_DIR:-target/ci-logs}"
mkdir -p "$LOG_DIR"
declare -a NAMES=() RESULTS=() TIMES=()
FAILED=0

step() {
  local name="$1"; shift
  local log="$LOG_DIR/$(echo "$name" | tr ' /:' '___').log"
  local t0=$SECONDS
  printf '== %s\n' "$name"
  if "$@" >"$log" 2>&1; then
    RESULTS+=("PASS")
  else
    RESULTS+=("FAIL")
    FAILED=1
    tail -n 25 "$log" | sed 's/^/   | /'
  fi
  NAMES+=("$name")
  TIMES+=("$((SECONDS - t0))s")
}

step "cargo fmt --check" cargo fmt --all -- --check
step "cargo clippy -D warnings" cargo clippy --workspace --all-targets -- -D warnings
step "cargo test (workspace)" cargo test --workspace --no-fail-fast

# Golden tests: every crates/<crate>/tests/*golden*.rs target, run on its own so the summary shows
# which fixture set (compat-tests/golden) broke.
shopt -s nullglob
for test_file in crates/*/tests/*golden*.rs; do
  crate_dir="$(dirname "$(dirname "$test_file")")"
  pkg="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$crate_dir/Cargo.toml" | head -n1)"
  target="$(basename "$test_file" .rs)"
  step "golden $pkg/$target" cargo test -q -p "$pkg" --test "$target"
done
shopt -u nullglob

echo
echo "Summary (logs in $LOG_DIR)"
for i in "${!NAMES[@]}"; do
  printf '  %-4s %-45s %s\n' "${RESULTS[$i]}" "${NAMES[$i]}" "${TIMES[$i]}"
done
if [ "$FAILED" = 0 ]; then echo "All steps passed."; else echo "Some steps FAILED."; fi
exit "$FAILED"
