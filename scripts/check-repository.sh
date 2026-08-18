#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

cargo run --quiet -p repo-check

if command -v actionlint >/dev/null 2>&1; then
  actionlint
elif [[ "${CI:-}" == "true" ]]; then
  echo "actionlint is required in CI but is not installed" >&2
  exit 1
else
  echo "SKIP: actionlint is not installed locally"
fi

if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check
elif [[ "${CI:-}" == "true" ]]; then
  echo "cargo-deny is required in CI but is not installed" >&2
  exit 1
else
  echo "SKIP: cargo-deny is not installed locally"
fi

if command -v gitleaks >/dev/null 2>&1; then
  gitleaks detect --no-banner --redact
elif [[ "${CI:-}" == "true" ]]; then
  echo "gitleaks is required in CI but is not installed" >&2
else
  echo "SKIP: gitleaks is not installed locally"
fi
