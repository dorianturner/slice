#!/usr/bin/env bash
set -euo pipefail

slice_test_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_test_root_tmp=$(mktemp -d)
trap 'rm -rf "$slice_test_root_tmp"' EXIT

if [[ -z "${SLICE_VIEW_PROFILE:-}" ]]; then
  echo "SKIP: set SLICE_VIEW_PROFILE to a real captured .slice file for viewer smoke"
  exit 0
fi

cargo run --quiet -p slice-cli -- view "$SLICE_VIEW_PROFILE" \
  --output "$slice_test_root_tmp/capture.html" --percentile 0:100 --metric wall
node "$slice_test_root/tests/viewer-smoke.js" "$slice_test_root_tmp/capture.html"
