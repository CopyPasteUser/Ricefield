#!/usr/bin/env bash
set -euo pipefail
project="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
bash -n "$project/scripts/profile_host_memory.sh"
sleep 4 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT
result="$(bash "$project/scripts/profile_host_memory.sh" "$pid" 1)"
grep -q '^process = "sleep"$' <<< "$result"
grep -q '^sample_count = 2$' <<< "$result"
grep -q '^pss_avg_kib = ' <<< "$result"
grep -q '^cpu_percent_one_core = ' <<< "$result"
if bash "$project/scripts/profile_host_memory.sh" "$pid" 0 >/dev/null 2>&1; then
  echo 'Zero-duration sampling must be rejected' >&2
  exit 1
fi
echo 'Read-only host-memory profiler smoke passed'
