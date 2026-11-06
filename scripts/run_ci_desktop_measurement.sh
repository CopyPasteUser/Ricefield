#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
log="$(mktemp)"
trap 'rm -f -- "$log"' EXIT

for attempt in 1 2; do
  if timeout -k 5s 60s xvfb-run -e /dev/stderr -a \
      bash "$project_dir/scripts/measure-desktop.sh" > "$log" 2>&1; then
    cat "$log"
    exit 0
  else
    status=$?
  fi

  cat "$log" >&2
  if (( attempt == 1 )) && grep -Fq 'XOpenDisplayFailed' "$log"; then
    # A full GUI smoke already ran; retry only the documented Xvfb
    # initialization race, never a failing measurement/assertion.
    echo 'Xvfb display startup failed; retrying desktop measurement once' >&2
    sleep 1
  else
    exit "$status"
  fi
done
