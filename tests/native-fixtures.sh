#!/usr/bin/env bash
set -euo pipefail

slice_test_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_test_build=$(mktemp -d)
trap 'rm -rf "$slice_test_build"' EXIT

cmake -S "$slice_test_root/fixtures" -B "$slice_test_build" -G Ninja
cmake --build "$slice_test_build"
ctest --test-dir "$slice_test_build" --output-on-failure

# The C++ target must expose an exact, copyable population selector rather than
# relying on a fuzzy display name. This also exercises ELF parsing + demangling
# against a real PIE executable built by the fixture toolchain.
slice_symbols=$(cargo run --quiet -p slice-cli -- symbols "$slice_test_build/tail_divergence" --match 'SliceFixture::work')
grep -F $'\tSliceFixture::work(unsigned int)' <<<"$slice_symbols"
