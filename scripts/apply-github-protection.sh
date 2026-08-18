#!/usr/bin/env bash
set -euo pipefail

repo=${GITHUB_REPOSITORY:-}
if [[ -z "$repo" ]]; then
  repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
fi

payload='{
  "required_status_checks": {
    "strict": true,
    "contexts": ["policy", "rust", "bpf", "native", "nix"]
  },
  "enforce_admins": true,
  "required_pull_request_reviews": {
    "dismiss_stale_reviews": true,
    "require_code_owner_reviews": false,
    "required_approving_review_count": 0
  },
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "required_conversation_resolution": true
}'

gh api --method PUT "repos/$repo/branches/main/protection" \
  -H 'Accept: application/vnd.github+json' \
  --input - <<<"$payload"

gh api --method PATCH "repos/$repo" \
  -H 'Accept: application/vnd.github+json' \
  --input - <<'JSON'
{
  "allow_merge_commit": false,
  "allow_rebase_merge": false,
  "allow_squash_merge": true,
  "allow_auto_merge": true,
  "delete_branch_on_merge": true,
  "squash_merge_commit_title": "PR_TITLE",
  "squash_merge_commit_message": "PR_BODY"
}
JSON

gh api --method PUT "repos/$repo/actions/permissions" \
  -H 'Accept: application/vnd.github+json' \
  --input - <<'JSON'
{
  "enabled": true,
  "allowed_actions": "selected",
  "sha_pinning_required": true
}
JSON

gh api --method PUT "repos/$repo/actions/permissions/selected-actions" \
  -H 'Accept: application/vnd.github+json' \
  --input - <<'JSON'
{
  "github_owned_allowed": true,
  "verified_allowed": false,
  "patterns_allowed": [
    "openai/codex-action@*",
    "DeterminateSystems/nix-installer-action@*"
  ]
}
JSON

gh api --method PUT "repos/$repo/vulnerability-alerts" \
  -H 'Accept: application/vnd.github+json'
gh api --method PUT "repos/$repo/automated-security-fixes" \
  -H 'Accept: application/vnd.github+json'

ruleset_payload='{
  "name": "main-agentic-gate",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [],
  "conditions": {
    "ref_name": {
      "include": ["refs/heads/main"],
      "exclude": []
    }
  },
  "rules": [
    {
      "type": "pull_request",
      "parameters": {
        "allowed_merge_methods": ["squash"],
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_approving_review_count": 0,
        "required_review_thread_resolution": true
      }
    },
    {
      "type": "required_status_checks",
      "parameters": {
        "do_not_enforce_on_create": false,
        "required_status_checks": [
          {"context": "policy"},
          {"context": "rust"},
          {"context": "bpf"},
          {"context": "native"},
          {"context": "nix"}
        ],
        "strict_required_status_checks_policy": true
      }
    },
    {"type": "non_fast_forward"},
    {"type": "deletion"},
    {"type": "required_linear_history"}
  ]
}'

ruleset_id=$(gh api "repos/$repo/rulesets" --jq '.[] | select(.name == "main-agentic-gate") | .id' | head -n 1)
if [[ -n "$ruleset_id" ]]; then
  gh api --method PUT "repos/$repo/rulesets/$ruleset_id" \
    -H 'Accept: application/vnd.github+json' \
    --input - <<<"$ruleset_payload"
else
  gh api --method POST "repos/$repo/rulesets" \
    -H 'Accept: application/vnd.github+json' \
    --input - <<<"$ruleset_payload"
fi

echo "Applied protected main branch, squash-only merge settings, and selected actions for $repo."
echo "Merge queue could not be configured through this account's GitHub API; strict up-to-date checks remain enforced."
echo "Hosted agent-review is temporarily disabled until an OPENAI_API_KEY repository secret is provisioned."
