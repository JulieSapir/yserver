#!/bin/bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
source "$SCRIPT_DIR/aggregate.sh"

BASE=$(mktemp -d)
trap 'rm -rf "$BASE"' EXIT
DEVICE=8:1

emit_tearing_row() {
  local samples=$1
  printf 'INFO tearing_summary device=%s async=1 async_direct_flips/s=1 async_composed_flips/s=0 client_present_direct/s=1 vsync_while_on/s=0 async_einval_direct=0 async_einval_composed=0 skip_completions/s=0 avg_gpu_render_ns=100 present_to_submit_us_direct_p50/p99/max=%s present_to_submit_us_composed_p50/p99/max=%s async_flip_turnaround_us_direct_p50/p99/max=%s async_flip_turnaround_us_composed_p50/p99/max=%s present_to_flip_event_us_direct_p50/p99/max=%s present_to_flip_event_us_composed_p50/p99/max=%s\n' \
    "$DEVICE" "$samples" "$samples" "$samples" "$samples" "$samples" "$samples"
}

emit_present_row() {
  local samples=$1
  printf 'INFO present_latency_summary device=%s eligible_presents/s=10 flips/s=8 skip_completions/s=1 present_to_submit_us_p50/p99/max=%s present_to_flip_event_us_p50/p99/max=%s\n' \
    "$DEVICE" "$samples" "$samples"
}

write_fixture() {
  local scenario=$1 fixture=$BASE/$1.log
  case "$scenario" in
    unavailable)
      emit_tearing_row 'n/a/n/a/n/a' > "$fixture"
      emit_present_row 'n/a/n/a/n/a' >> "$fixture"
      ;;
    valid_then_unavailable)
      emit_tearing_row '150/250/350' > "$fixture"
      emit_tearing_row 'n/a/n/a/n/a' >> "$fixture"
      emit_present_row '150/250/350' >> "$fixture"
      emit_present_row 'n/a/n/a/n/a' >> "$fixture"
      ;;
    mixed)
      emit_tearing_row '100/200/900' > "$fixture"
      emit_tearing_row '200/400/300' >> "$fixture"
      emit_tearing_row 'n/a/n/a/n/a' >> "$fixture"
      emit_present_row '100/200/900' >> "$fixture"
      emit_present_row '200/400/300' >> "$fixture"
      emit_present_row 'n/a/n/a/n/a' >> "$fixture"
      ;;
    *)
      echo "unknown fixture scenario: $scenario" >&2
      return 2
      ;;
  esac
  printf '%s\n' "$fixture"
}

check_scenario() {
  local scenario=$1 expected_tearing=$2 expected_triplet=$3 fixture actual_tearing actual_present
  fixture=$(write_fixture "$scenario")
  actual_tearing=$(print_tearing_aggregate "$fixture" "$DEVICE" | awk '/^tearing_summary latency/ { line=$0 } END { print line }')
  actual_present=$(print_present_latency_aggregate "$fixture" "$DEVICE" | awk '/^present_latency_summary aggregate/ { line=$0 } END { print line }')
  if [[ "$actual_tearing" != "$expected_tearing" ]]; then
    printf 'tearing %s fixture mismatch\nexpected: %s\nactual:   %s\n' "$scenario" "$expected_tearing" "$actual_tearing" >&2
    return 1
  fi
  if [[ "$actual_present" != *"present_to_submit_us_p50/p99/max=$expected_triplet"* ]] ||
    [[ "$actual_present" != *"present_to_flip_event_us_p50/p99/max=$expected_triplet"* ]]; then
    printf 'present %s fixture mismatch\nexpected triplet: %s\nactual: %s\n' "$scenario" "$expected_triplet" "$actual_present" >&2
    return 1
  fi
  printf 'aggregate %s: ok\n' "$scenario"
}

# Mutations that must fail: count n/a as zero, divide valid means by all windows,
# or replace a maximum with the latest window's value instead of the max of valid windows.
check_scenario unavailable 'tearing_summary latency window-mean-p50/p99 and max-max us: present_to_submit direct=n/a/n/a/n/a composed=n/a/n/a/n/a; async_flip_turnaround direct=n/a/n/a/n/a composed=n/a/n/a/n/a; present_to_flip_event direct=n/a/n/a/n/a composed=n/a/n/a/n/a' 'n/a/n/a/n/a'
check_scenario valid_then_unavailable 'tearing_summary latency window-mean-p50/p99 and max-max us: present_to_submit direct=150.0/250.0/350 composed=150.0/250.0/350; async_flip_turnaround direct=150.0/250.0/350 composed=150.0/250.0/350; present_to_flip_event direct=150.0/250.0/350 composed=150.0/250.0/350' '150.0/250.0/350'
check_scenario mixed 'tearing_summary latency window-mean-p50/p99 and max-max us: present_to_submit direct=150.0/300.0/900 composed=150.0/300.0/900; async_flip_turnaround direct=150.0/300.0/900 composed=150.0/300.0/900; present_to_flip_event direct=150.0/300.0/900 composed=150.0/300.0/900' '150.0/300.0/900'

echo 'latency aggregators: ok'
