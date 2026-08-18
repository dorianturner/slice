#!/usr/bin/env bash
set -euo pipefail

slice_test_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_test_root_tmp=$(mktemp -d)
trap 'rm -rf "$slice_test_root_tmp"' EXIT

cargo run --quiet -p slice-cli -- fixture-profile \
  --scenario bimodal --output "$slice_test_root_tmp/bimodal.slice"
cargo run --quiet -p slice-cli -- view "$slice_test_root_tmp/bimodal.slice" \
  --output "$slice_test_root_tmp/bimodal.html" --percentile 95:100 --metric off-cpu
node "$slice_test_root/tests/viewer-smoke.js" "$slice_test_root_tmp/bimodal.html"
