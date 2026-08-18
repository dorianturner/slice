#!/usr/bin/env bash
set -euo pipefail

body_file=${1:-}
if [[ -z "$body_file" || ! -f "$body_file" ]]; then
  echo "PR contract: no PR body supplied; skipping path-sensitive metadata check"
  exit 0
fi

base_sha=${BASE_SHA:-HEAD~1}
head_sha=${HEAD_SHA:-HEAD}
changed=$(git diff --name-only "$base_sha" "$head_sha")

if grep -Eq '^(Cargo\.toml|Cargo\.lock|crates/|fixtures/|flake\.nix|tests/)' <<<"$changed"; then
  grep -Eiq 'architecture impact:' "$body_file" || {
    echo "PR contract: architecture impact is required for code/build/test changes" >&2
    exit 1
  }
  grep -Eiq 'test evidence:|tests run:' "$body_file" || {
    echo "PR contract: test evidence or tests run is required for code/build/test changes" >&2
    exit 1
  }
fi

if grep -Eq '^crates/slice-(core|collector|ebpf)/|^crates/slice-ebpf/bpf/|^\.github/workflows/' <<<"$changed"; then
  grep -Eiq 'security or privilege impact:' "$body_file" || {
    echo "PR contract: security or privilege impact is required for protected paths" >&2
    exit 1
  }
fi
