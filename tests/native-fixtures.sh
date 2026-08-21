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
bimodal_symbols=$(cargo run --quiet -p slice-cli -- symbols "$slice_test_build/bimodal_service" --match 'BimodalFixture::handle_request')
grep -F $'\tBimodalFixture::handle_request(unsigned long)' <<<"$bimodal_symbols"
bimodal_work_symbols=$(cargo run --quiet -p slice-cli -- symbols "$slice_test_build/bimodal_service" --match 'BimodalFixture::normal_distribution')
grep -F $'\tBimodalFixture::normal_distribution' <<<"$bimodal_work_symbols"
bimodal_spin_symbols=$(cargo run --quiet -p slice-cli -- symbols "$slice_test_build/bimodal_service" --match 'BimodalFixture::spin_for')
grep -F $'\tBimodalFixture::spin_for' <<<"$bimodal_spin_symbols"
bimodal_sleep_symbols=$(cargo run --quiet -p slice-cli -- symbols "$slice_test_build/bimodal_service" --match 'BimodalFixture::sleep_for')
grep -F $'\tBimodalFixture::sleep_for' <<<"$bimodal_sleep_symbols"
off_cpu_symbols=$(cargo run --quiet -p slice-cli -- symbols "$slice_test_build/off_cpu_wait" --match 'SliceFixture::sleep_work')
grep -F $'\tSliceFixture::sleep_work(unsigned int)' <<<"$off_cpu_symbols"
