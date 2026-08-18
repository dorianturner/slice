#!/usr/bin/env bash
set -euo pipefail

pr_number=${1:?usage: scripts/agent-loop.sh PR_NUMBER}
max_attempts=${AGENT_MAX_REPAIR_ATTEMPTS:-5}

for ((attempt = 1; attempt <= max_attempts; attempt++)); do
  echo "agent-loop: waiting for required checks (attempt $attempt/$max_attempts)"
  if gh pr checks "$pr_number" --required --watch --interval 10; then
    echo "agent-loop: all required checks passed"
    exit 0
  fi

  if [[ -z "${CODEX_FIX_COMMAND:-}" ]]; then
    echo "agent-loop: CODEX_FIX_COMMAND is not configured; cannot repair the PR" >&2
    exit 1
  fi

  CODEX_FIX_PR="$pr_number" \
    CODEX_FIX_ATTEMPT="$attempt" \
    bash -lc "$CODEX_FIX_COMMAND"
done

gh pr comment "$pr_number" --body "Agent repair loop exhausted after $max_attempts attempts; human intervention is required."
echo "agent-loop: repair budget exhausted" >&2
exit 1
