#!/usr/bin/env bash
set -euo pipefail

# Codex supplies the hook event as JSON on stdin. The hook deliberately does
# not turn a failed local check into a fake success: decision=block asks Codex
# to continue, inspect the failure, and repair the branch.
event=$(cat)
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"

stop_hook_active=false
if [[ "$event" =~ \"stop_hook_active\"[[:space:]]*:[[:space:]]*true ]]; then
  stop_hook_active=true
fi

session_id="unknown"
if command -v jq >/dev/null 2>&1; then
  session_id=$(jq -r '.session_id // "unknown"' <<<"$event")
fi
session_id=${session_id//[^A-Za-z0-9_.-]/_}
state_file="${TMPDIR:-/tmp}/slice-codex-stop-${session_id}"
max_attempts=${SLICE_STOP_CHECK_ATTEMPTS:-5}
attempt=0
if [[ -f "$state_file" ]]; then
  read -r attempt <"$state_file" || attempt=0
fi

if [[ -n "${SLICE_STOP_CHECK_COMMAND:-}" ]]; then
  check_command=(bash -lc "$SLICE_STOP_CHECK_COMMAND")
elif command -v nix >/dev/null 2>&1; then
  check_command=(nix develop --accept-flake-config -c just check)
elif command -v just >/dev/null 2>&1; then
  check_command=(just check)
else
  if [[ "$stop_hook_active" == true ]]; then
    echo "Codex stop hook could not find Nix or just after a repair attempt; stopping with checks unresolved." >&2
    exit 0
  fi
  printf '%s\n' '{"decision":"block","reason":"Cannot run repository checks: install Nix or just, then rerun the task. The stop hook will not claim success without checks."}'
  exit 0
fi

if "${check_command[@]}"; then
  rm -f "$state_file"
  exit 0
fi

attempt=$((attempt + 1))
printf '%s\n' "$attempt" >"$state_file"
if (( attempt > max_attempts )); then
  echo "Repository checks still fail after $max_attempts stop-hook repair attempts; stopping so the failure can be recorded." >&2
  rm -f "$state_file"
  exit 0
fi

printf '%s\n' "{\"decision\":\"block\",\"reason\":\"Repository checks failed (attempt $attempt/$max_attempts). Run just check or the Nix-backed equivalent, inspect the failure, fix the branch, and try to finish again.\"}"
