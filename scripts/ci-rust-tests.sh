#!/usr/bin/env bash
# Run the Rust test suite one test target at a time, deleting each test binary after it has run.
#
# Every integration test links the whole backend statically, so all of them together take tens of
# gigabytes: more than a hosted CI runner has free. Building and running them one by one keeps the
# disk use to the shared libraries plus one binary. Run from rust-version/.
# SHARD/SHARDS (1-based, default 1/1) split the targets over several runners; shard 1 also runs the unit and doc tests.
set -uo pipefail
SHARD=${SHARD:-1}; SHARDS=${SHARDS:-1}

failed=()

run() { # <label> <cargo test args...>
  local label=$1; shift
  echo "::group::$label"
  local out
  out=$(cargo test --no-run --no-fail-fast --message-format=json-render-diagnostics "$@" 2>/dev/null </dev/null \
    | python3 -c 'import json,sys
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["profile"].get("test"): print(m["executable"])')
  cargo test --no-fail-fast "$@" </dev/null || failed+=("$label")
  # shellcheck disable=SC2086
  [ -n "$out" ] && rm -f $out
  echo "::endgroup::"
}

# unit tests of every crate (library and binaries)
[ "$SHARD" = 1 ] && run "unit tests" --workspace --lib --bins

# integration tests, one target at a time
while IFS=$'\t' read -r pkg test; do
  extra=()
  # each of these tests starts a Chromium; started together on a fresh runner they can miss the 30 s launch limit
  [ "$test" = chromium ] && extra=(-- --test-threads=1)
  run "$pkg --test $test" -p "$pkg" --test "$test" "${extra[@]}"
done < <(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys
shard, shards = int(sys.argv[1]), int(sys.argv[2])
targets = sorted((p["name"], t["name"]) for p in json.load(sys.stdin)["packages"] for t in p["targets"] if "test" in t["kind"])
for i, (pkg, name) in enumerate(targets):
    if i % shards == shard - 1: print(pkg + "\t" + name)' "$SHARD" "$SHARDS")

# documentation examples
[ "$SHARD" = 1 ] && run "doc tests" --workspace --doc

df -h . | tail -1
if [ ${#failed[@]} -gt 0 ]; then
  printf 'FAILED: %s\n' "${failed[@]}"
  exit 1
fi
echo "all Rust tests passed"
