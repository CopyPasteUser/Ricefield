#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -r -- "$fixture"' EXIT
mkdir -p -- "$fixture/bin"

# Simulate the Xvfb process and time limiter without starting a real display.
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'shift 3' \
  'exec "$@"' > "$fixture/bin/timeout"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'count=0' \
  '[[ ! -f "$RICEFIELD_TEST_ATTEMPTS" ]] || count="$(<"$RICEFIELD_TEST_ATTEMPTS")"' \
  'count=$((count + 1))' \
  'printf "%s\n" "$count" > "$RICEFIELD_TEST_ATTEMPTS"' \
  'case "$RICEFIELD_TEST_SCENARIO" in' \
  '  transient) if (( count == 1 )); then echo XOpenDisplayFailed >&2; exit 1; fi ;;' \
  '  repeated) echo XOpenDisplayFailed >&2; exit 1 ;;' \
  '  functional) echo "GUI assertion failed" >&2; exit 5 ;;' \
  'esac' \
  'echo "Desktop measurement succeeded"' > "$fixture/bin/xvfb-run"
chmod +x "$fixture/bin/timeout" "$fixture/bin/xvfb-run"

export PATH="$fixture/bin:$PATH"
export RICEFIELD_TEST_ATTEMPTS="$fixture/attempts"
export RICEFIELD_TEST_SCENARIO=transient
bash "$project_dir/scripts/run_ci_desktop_measurement.sh" > "$fixture/transient.log" 2>&1
test "$(<"$RICEFIELD_TEST_ATTEMPTS")" = 2
grep -Fq 'Desktop measurement succeeded' "$fixture/transient.log"

rm -f -- "$RICEFIELD_TEST_ATTEMPTS"
export RICEFIELD_TEST_SCENARIO=functional
if bash "$project_dir/scripts/run_ci_desktop_measurement.sh" > "$fixture/functional.log" 2>&1; then
  echo 'A real measurement failure was incorrectly masked' >&2
  exit 1
fi
test "$(<"$RICEFIELD_TEST_ATTEMPTS")" = 1
grep -Fq 'GUI assertion failed' "$fixture/functional.log"

rm -f -- "$RICEFIELD_TEST_ATTEMPTS"
export RICEFIELD_TEST_SCENARIO=repeated
if bash "$project_dir/scripts/run_ci_desktop_measurement.sh" > "$fixture/repeated.log" 2>&1; then
  echo 'Repeated display-start failure was incorrectly ignored' >&2
  exit 1
fi
test "$(<"$RICEFIELD_TEST_ATTEMPTS")" = 2
grep -Fq 'XOpenDisplayFailed' "$fixture/repeated.log"

printf 'Desktop measurement CI retries only transient Xvfb initialization errors\n'
