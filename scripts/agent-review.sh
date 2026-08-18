#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ -n "${CODEX_REVIEW_COMMAND:-}" ]]; then
  bash -lc "$CODEX_REVIEW_COMMAND"
  exit $?
fi

if ! command -v codex >/dev/null 2>&1; then
  echo "agent-review: Codex CLI is not provisioned on this runner" >&2
  exit 1
fi

codex exec --sandbox read-only \
  "Review the current pull request diff against AGENTS.md, ARCHITECTURE.md, docs/security.md, and docs/testing.md. Check dependency layering, unsafe-code boundaries, profile-format compatibility, tests, documentation, and security. Do not edit files. Exit nonzero for any unresolved must-fix issue; otherwise report a concise review summary and exit zero."
