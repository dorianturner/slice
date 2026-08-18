set shell := ["bash", "-euo", "pipefail", "-c"]

format:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace --all-targets

test-native:
    bash tests/native-fixtures.sh

test-viewer:
    bash tests/viewer-smoke.sh

test-bpf:
    bash tests/bpf-build.sh

test-live:
    REQUIRE_PRIVILEGED=1 bash tests/privileged-capture.sh

policy:
    bash scripts/check-repository.sh

check: policy format lint test test-bpf test-native test-viewer
