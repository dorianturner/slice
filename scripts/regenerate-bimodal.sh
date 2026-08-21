#!/usr/bin/env bash
set -euo pipefail

# Regenerate the inspectable bimodal report from a native capture. This script
# intentionally has no synthetic fallback: the report is evidence of the
# executable's sampled stacks, so an unavailable privileged capture is an
# actionable failure.
slice_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_build=$(mktemp -d)
slice_output=$(mktemp -d)
slice_profile="$slice_output/bimodal.slice"
slice_staged_profile="$slice_root/.bimodal.slice.tmp-$$"
cleanup() {
  rm -rf "$slice_build" "$slice_output" "$slice_staged_profile"
}
trap cleanup EXIT

slice_workers=${SLICE_BIMODAL_WORKERS:-10}
slice_iterations=${SLICE_BIMODAL_ITERATIONS:-400}

if [[ "$(id -u)" == 0 ]]; then
  slice_privileged=()
else
  if ! command -v sudo >/dev/null; then
    echo "error: sudo is required for live capture (or run this script as root)" >&2
    exit 1
  fi
  slice_privileged=(sudo)
fi

cmake -S "$slice_root/fixtures" -B "$slice_build" -G Ninja
cmake --build "$slice_build"
cargo build --manifest-path "$slice_root/Cargo.toml" --release -p slice-cli
slice_cli="$slice_root/target/release/slice"

"${slice_privileged[@]}" "$slice_cli" doctor
"${slice_privileged[@]}" "$slice_cli" profile \
  --module "$slice_build/bimodal_service" \
  --function 'BimodalFixture::handle_request(unsigned long)' \
  --output "$slice_profile" \
  "$slice_build/bimodal_service" -- \
  --workers "$slice_workers" --iterations "$slice_iterations"

"$slice_cli" validate "$slice_profile" \
  --require-complete --require-samples --require-off-cpu
cp "$slice_profile" "$slice_staged_profile"
mv -f "$slice_staged_profile" "$slice_root/bimodal.slice"
"$slice_cli" view "$slice_root/bimodal.slice" \
  --output "$slice_root/bimodal-neo-brutalist.html" \
  --percentile 0:100 --metric wall

echo "wrote $slice_root/bimodal.slice and $slice_root/bimodal-neo-brutalist.html"
echo "both artifacts came from a validated live native capture"
