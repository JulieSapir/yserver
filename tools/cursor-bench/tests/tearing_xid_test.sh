#!/bin/bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
CARD=/dev/null
source "$SCRIPT_DIR/tearing.sh"

BASE=$(mktemp -d)
trap 'rm -rf "$BASE"' EXIT
DURATION=0
case_trace=()

stat() { echo 'e2:0'; }
journalctl() { return 0; }
xid_journal_cursor() { echo teardown-test-cursor; }
xid_count_after_cursor() {
  local count=0
  [ ! -f "$BASE/xid-query-count" ] || count=$(<"$BASE/xid-query-count")
  count=$((count + 1))
  echo "$count" > "$BASE/xid-query-count"
  if (( count == 1 )); then echo 0; else echo 1; fi
}
start_server() { SP=; }
stop_server() { SP=; }
mpv() { :; }
watch_xid() { :; }
move_cursor_250hz() { :; }
sleep() { :; }
print_tearing_aggregate() { :; }
print_present_latency_aggregate() { :; }
log_checks() { :; }
step() { case_trace+=("$*"); }

# Mutation: let a teardown-only Xid delta pass and the matrix starts another case.
if main /path/to/stub-yserver; then
  echo "expected a teardown-only Xid to stop the matrix" >&2
  exit 1
fi

if (( ${#case_trace[@]} != 1 )); then
  echo "expected one case before teardown Xid stopped the matrix, got ${#case_trace[@]}" >&2
  exit 1
fi

echo "tearing Xid teardown guard: ok"
