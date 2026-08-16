#!/usr/bin/env bash
set -euo pipefail

# This is intentionally opt-in: ordinary NixOS developer sessions usually do
# not grant CAP_BPF/CAP_PERFMON. In that case the deterministic collector and
# native-fixture tests still run, while this test documents the real attach
# contract for a privileged CI runner.
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
  if [[ -n "${slice_pid:-}" ]]; then kill "$slice_pid" 2>/dev/null || true; fi
  rm -rf "$slice_build" "$slice_output"
}
trap cleanup EXIT

cmake -S "$slice_root/fixtures" -B "$slice_build" -G Ninja >/dev/null
cmake --build "$slice_build" >/dev/null
"$slice_build/tail_divergence" 1000 1 >/dev/null &
slice_pid=$!
sleep 0.05

if ! cargo run --quiet -p slice-cli -- profile \
  --pid "$slice_pid" \
  --module "$slice_build/tail_divergence" \
  --function 'SliceFixture::work(unsigned int)' \
  --duration 1s \
  --output "$slice_output/capture.slice"; then
  echo "SKIP: kernel refused the privileged eBPF attach"
  exit 0
fi

cargo run --quiet -p slice-cli -- view "$slice_output/capture.slice" \
  --output "$slice_output/profile.html" --percentile 99:100
grep -F 'SliceFixture::slow_tail_b()' "$slice_output/profile.html"
echo "PASS: live p99 capture exposes slow_tail_b"
