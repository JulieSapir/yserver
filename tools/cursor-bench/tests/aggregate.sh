print_tearing_aggregate() {
  local log_file=$1 wanted=$2
  awk -v wanted="$wanted" '
    function add_samples(raw, metric, i, key) {
      if (raw == "n/a/n/a/n/a") return
      split(raw, sample, "/")
      for (i = 1; i <= 3; i++) {
        if (sample[i] == "" || sample[i] == "n/a") continue
        key = metric SUBSEP i
        sample_sum[key] += sample[i] + 0
        sample_count[key]++
        if (i == 3 && (!(key in sample_max) || sample[i] + 0 > sample_max[key]))
          sample_max[key] = sample[i] + 0
      }
    }
    function sample_value(metric, part, key) {
      key = metric SUBSEP part
      if (!sample_count[key]) return "n/a"
      if (part == 3) return sprintf("%d", sample_max[key])
      return sprintf("%.1f", sample_sum[key] / sample_count[key])
    }
    function triplet(metric) {
      return sample_value(metric, 1) "/" sample_value(metric, 2) "/" sample_value(metric, 3)
    }
    /tearing_summary/ {
      selected = 0
      for (i = 1; i <= NF; i++) {
        split($i, field, "=")
        if (field[1] == "device" && field[2] == wanted) selected = 1
      }
      if (!selected) next
      print
      windows++
      for (i = 1; i <= NF; i++) {
        split($i, field, "=")
        if (field[1] == "async") async += field[2]
        else if (field[1] == "async_direct_flips/s") direct += field[2]
        else if (field[1] == "async_composed_flips/s") composed += field[2]
        else if (field[1] == "client_present_direct/s") presents += field[2]
        else if (field[1] == "vsync_while_on/s") vsync += field[2]
        else if (field[1] == "async_einval_direct") einval_direct += field[2]
        else if (field[1] == "async_einval_composed") einval_composed += field[2]
        else if (field[1] == "skip_completions/s") skips += field[2]
        else if (field[1] == "avg_gpu_render_ns") gpu += field[2]
        else if (field[1] == "present_to_submit_us_direct_p50/p99/max") add_samples(field[2], "direct_submit")
        else if (field[1] == "present_to_submit_us_composed_p50/p99/max") add_samples(field[2], "composed_submit")
        else if (field[1] == "async_flip_turnaround_us_direct_p50/p99/max") add_samples(field[2], "direct_turnaround")
        else if (field[1] == "async_flip_turnaround_us_composed_p50/p99/max") add_samples(field[2], "composed_turnaround")
        else if (field[1] == "present_to_flip_event_us_direct_p50/p99/max") add_samples(field[2], "direct_flip_event")
        else if (field[1] == "present_to_flip_event_us_composed_p50/p99/max") add_samples(field[2], "composed_flip_event")
      }
    }
    END {
      if (!windows) { print "tearing_summary selected-device windows=0"; exit }
      printf "tearing_summary aggregate windows=%d async/s=%.2f async_direct/s=%.2f async_composed/s=%.2f client_present_direct/s=%.2f direct_flip_per_present=%.3f skip_completions/s=%.2f vsync_while_on/s=%.2f async_einval_direct=%d async_einval_composed=%d avg_gpu_render_ns=%.0f\n", \
        windows, async/windows, direct/windows, composed/windows, presents/windows, direct/(presents ? presents : 1), skips/windows, vsync/windows, einval_direct, einval_composed, gpu/windows
      printf "tearing_summary latency window-mean-p50/p99 and max-max us: present_to_submit direct=%s composed=%s; async_flip_turnaround direct=%s composed=%s; present_to_flip_event direct=%s composed=%s\n", \
        triplet("direct_submit"), triplet("composed_submit"), triplet("direct_turnaround"), triplet("composed_turnaround"), triplet("direct_flip_event"), triplet("composed_flip_event")
    }
  ' "$log_file"
}

print_present_latency_aggregate() {
  local log_file=$1 wanted=$2
  awk -v wanted="$wanted" '
    function add_samples(raw, metric, i, key) {
      if (raw == "n/a/n/a/n/a") return
      split(raw, sample, "/")
      for (i = 1; i <= 3; i++) {
        if (sample[i] == "" || sample[i] == "n/a") continue
        key = metric SUBSEP i
        sample_sum[key] += sample[i] + 0
        sample_count[key]++
        if (i == 3 && (!(key in sample_max) || sample[i] + 0 > sample_max[key]))
          sample_max[key] = sample[i] + 0
      }
    }
    function sample_value(metric, part, key) {
      key = metric SUBSEP part
      if (!sample_count[key]) return "n/a"
      if (part == 3) return sprintf("%d", sample_max[key])
      return sprintf("%.1f", sample_sum[key] / sample_count[key])
    }
    function triplet(metric) {
      return sample_value(metric, 1) "/" sample_value(metric, 2) "/" sample_value(metric, 3)
    }
    /present_latency_summary/ {
      selected = 0
      for (i = 1; i <= NF; i++) {
        split($i, field, "=")
        if (field[1] == "device" && field[2] == wanted) selected = 1
      }
      if (!selected) next
      print
      windows++
      for (i = 1; i <= NF; i++) {
        split($i, field, "=")
        if (field[1] == "eligible_presents/s") eligible += field[2]
        else if (field[1] == "flips/s") flips += field[2]
        else if (field[1] == "skip_completions/s") skips += field[2]
        else if (field[1] == "present_to_submit_us_p50/p99/max") add_samples(field[2], "submit")
        else if (field[1] == "present_to_flip_event_us_p50/p99/max") add_samples(field[2], "flip_event")
      }
    }
    END {
      if (!windows) { print "present_latency_summary selected-device windows=0"; exit }
      printf "present_latency_summary aggregate device=%s windows=%d eligible_presents/s=%.2f flips/s=%.2f skip_completions/s=%.2f present_to_submit_us_p50/p99/max=%s present_to_flip_event_us_p50/p99/max=%s\n", \
        wanted, windows, eligible/windows, flips/windows, skips/windows, triplet("submit"), triplet("flip_event")
    }
  ' "$log_file"
}
