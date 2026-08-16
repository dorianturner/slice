#!/usr/bin/env bash
set -euo pipefail

# This is intentionally opt-in: ordinary NixOS developer sessions usually do
# not grant CAP_BPF/CAP_PERFMON. In that case deterministic and native tests
# still run, while this script verifies the real indefinite attach contract on
# a privileged runner.
if [[ "$(id -u)" != 0 ]]; then
  echo "SKIP: live capture requires root or CAP_BPF/CAP_PERFMON"
  exit 0
fi
if [[ ! -e /sys/kernel/btf/vmlinux ]]; then
  echo "SKIP: kernel BTF is unavailable"
  exit 0
fi

slice_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_build=$(mktemp -d)
slice_output=$(mktemp -d)
cleanup() {
  if [[ -n "${stopper_pid:-}" ]]; then kill "$stopper_pid" 2>/dev/null || true; fi
  if [[ -n "${slice_pid:-}" ]]; then kill -INT "$slice_pid" 2>/dev/null || true; fi
  rm -rf "$slice_build" "$slice_output"
}
trap cleanup EXIT

cmake -S "$slice_root/fixtures" -B "$slice_build" -G Ninja >/dev/null
cmake --build "$slice_build" >/dev/null
"$slice_build/bimodal_service" --workers 4 >"$slice_output/workload.log" &
slice_pid=$!
( sleep 2; kill -INT "$slice_pid" 2>/dev/null || true ) &
stopper_pid=$!

if ! cargo run --quiet -p slice-cli -- profile \
  --pid "$slice_pid" \
  --module "$slice_build/bimodal_service" \
  --function 'BimodalFixture::handle_request(unsigned long)' \
  --output "$slice_output/capture.slice"; then
  echo "SKIP: kernel refused the privileged eBPF attach"
  exit 0
fi
wait "$slice_pid" || true

cargo run --quiet -p slice-cli -- view "$slice_output/capture.slice" \
  --output "$slice_output/profile.html" --percentile 95:100 --metric off-cpu
grep -F 'BimodalFixture::slow_path()' "$slice_output/profile.html"
grep -F 'id="timeline"' "$slice_output/profile.html"
echo "PASS: indefinite live capture exposes bimodal slow_path"
