#!/usr/bin/env bash
set -euo pipefail

# Ordinary developer sessions may not grant CAP_BPF/CAP_PERFMON. CI sets
# REQUIRE_PRIVILEGED=1, in which case missing prerequisites and attachment
# failures are hard failures rather than silent skips.
slice_require_privileged=${REQUIRE_PRIVILEGED:-0}
if [[ "$(id -u)" != 0 ]]; then
  if [[ "$slice_require_privileged" == 1 ]]; then
    echo "FAIL: live capture requires root or CAP_BPF/CAP_PERFMON" >&2
    exit 1
  fi
  echo "SKIP: live capture requires root or CAP_BPF/CAP_PERFMON"
  exit 0
fi
if [[ ! -e /sys/kernel/btf/vmlinux ]]; then
  if [[ "$slice_require_privileged" == 1 ]]; then
    echo "FAIL: kernel BTF is unavailable" >&2
    exit 1
  fi
  echo "SKIP: kernel BTF is unavailable"
  exit 0
fi
slice_kernel_release=$(uname -r)
slice_kernel_major=${slice_kernel_release%%.*}
slice_kernel_remainder=${slice_kernel_release#*.}
slice_kernel_minor=${slice_kernel_remainder%%.*}
if [[ ! "$slice_kernel_major" =~ ^[0-9]+$ \
  || ! "$slice_kernel_minor" =~ ^[0-9]+$ \
  || "$slice_kernel_major" -lt 6 \
  || ( "$slice_kernel_major" -eq 6 && "$slice_kernel_minor" -lt 6 ) ]]; then
  if [[ "$slice_require_privileged" == 1 ]]; then
    echo "FAIL: process-wide uprobe-multi links require Linux 6.6+ (running $slice_kernel_release)" >&2
    exit 1
  fi
  echo "SKIP: process-wide uprobe-multi links require Linux 6.6+ (running $slice_kernel_release)"
  exit 0
fi
if [[ ! -e /sys/kernel/tracing/events/sched/sched_switch/format \
  && ! -e /sys/kernel/debug/tracing/events/sched/sched_switch/format ]]; then
  if [[ "$slice_require_privileged" == 1 ]]; then
    echo "FAIL: sched_switch tracepoint is unavailable" >&2
    exit 1
  fi
  echo "SKIP: sched_switch tracepoint is unavailable"
  exit 0
fi

slice_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_build=$(mktemp -d)
slice_output=$(mktemp -d)
cleanup() {
  rm -rf "$slice_build" "$slice_output"
}
trap cleanup EXIT

cmake -S "$slice_root/fixtures" -B "$slice_build" -G Ninja >/dev/null
cmake --build "$slice_build" >/dev/null

if ! cargo run --quiet -p slice-cli -- profile \
  --module "$slice_build/bimodal_service" \
  --function 'BimodalFixture::handle_request(unsigned long)' \
  --output "$slice_output/capture.slice" \
  "$slice_build/bimodal_service" -- \
  --iterations 200 --workers 4; then
  if [[ "$slice_require_privileged" == 1 ]]; then
    echo "FAIL: privileged eBPF capture failed" >&2
    exit 1
  fi
  echo "SKIP: kernel refused the privileged eBPF attach"
  exit 0
fi

cargo run --quiet -p slice-cli -- validate "$slice_output/capture.slice" \
  --require-complete --require-samples --require-off-cpu
cargo run --quiet -p slice-cli -- discover "$slice_output/capture.slice" \
  --metric off-cpu >"$slice_output/off-cpu.txt"
grep -F 'BimodalFixture::sleep_for' "$slice_output/off-cpu.txt"

cargo run --quiet -p slice-cli -- view "$slice_output/capture.slice" \
  --output "$slice_output/profile.html" --percentile 0:100 --metric wall
grep -F 'BimodalFixture::slow_path()' "$slice_output/profile.html"
grep -F 'BimodalFixture::fast_path()' "$slice_output/profile.html"
grep -F 'BimodalFixture::spin_for' "$slice_output/profile.html"
grep -F 'BimodalFixture::normal_distribution' "$slice_output/profile.html"
grep -F 'id="timeline"' "$slice_output/profile.html"
echo "PASS: finite live wall-time capture exposes CPU and blocked execution stacks"
