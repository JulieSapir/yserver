#!/usr/bin/env bash
# Host half of rootless-headless.sh: boots yserver --headless --rootless with
# the recording host writing into the artifacts (no scanout to dump).
#   tools/vng-scenarios/rootless-headless-host.sh [name] [vng-shot args...]
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
name=${1:-rootless-headless}
shift || true
out_root=${VNG_OUT:-$repo/target/vng}
mkdir -p "$out_root"
out=$(cd -- "$out_root" && pwd)/$name
exec "$repo/tools/vng-shot.sh" --dump none --name "$name" "$@" \
    --server-arg --headless --server-arg --rootless \
    --server-arg --rootless-record --server-arg "$out/rec" \
    --scenario "$repo/tools/vng-scenarios/rootless-headless.sh"
