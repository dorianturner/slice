# GitHub and agent contract

The repository expects agents to work through short-lived branches and pull
requests using `gh`. The agent runs local checks, opens a PR, reads check
failures and review comments, updates the branch, and repeats until the bounded
repair budget in [`.github/agent-policy.yml`](../.github/agent-policy.yml) is
exhausted.

[`scripts/agent-loop.sh`](../scripts/agent-loop.sh) provides the bounded check,
repair, and escalation loop. The runner supplies `CODEX_FIX_COMMAND`; the
script never pretends a missing agent or exhausted repair budget is success.

## Required workflows

The required check names are:

- `policy`
- `rust`
- `bpf`
- `native`
- `nix`
- `agent-review`

`privileged-live-capture` is currently optional and manually dispatched because
the repository has no capable self-hosted runner. It must be promoted to a
required check only after such a runner is provisioned.

The `main` branch must require pull requests, all six required checks, up-to-date
branches, merge queue participation, squash merges, and no ordinary bypass.
Direct pushes should be disabled. Configure these settings with a GitHub
ruleset or equivalent repository administration; committed workflow files alone
cannot enforce branch protection. The committed
[`scripts/apply-github-protection.sh`](../scripts/apply-github-protection.sh)
applies branch protection, repository merge settings, selected-action policy,
and the `main-agentic-gate` ruleset through `gh`. The current personal GitHub
repository rejects the merge-queue rule through the available API, so the
repository uses strict up-to-date checks and auto-merge until GitHub account
support for a required queue is available.

## Agent permissions

The agent identity may create branches, open/update PRs, request checks, and
merge through the merge queue. It must not receive broad organization access,
repository secrets, or arbitrary workflow administration permissions.

The agent-review job uses the hosted `openai/codex-action` and requires an
`OPENAI_API_KEY` repository secret. It must return a failing check when Codex
is unavailable or reports unresolved must-fix findings; a placeholder success
is not acceptable.

## Local Codex completion hook

The repository includes [`.codex/hooks.json`](../.codex/hooks.json), which
runs [`stop-check.sh`](../.codex/hooks/stop-check.sh) on Codex's `Stop` event.
After the project-local hook is trusted, it runs `just check` (through
`nix develop` when Nix is available) before allowing a task to finish. A
failure blocks completion and asks Codex to repair the branch, with a bounded
retry count. Set `SLICE_STOP_CHECK_COMMAND` only for a deliberate local
environment override; the hook must never be changed to claim success when a
required check failed.

This is a developer-loop guardrail, not GitHub enforcement: project hooks are
local and require explicit trust in Codex. The required GitHub workflows and
branch protection are the merge gates.

## Trusted privileged path

The privileged workflow is restricted to trusted same-repository branches and
an ephemeral runner. Fork pull requests cannot execute arbitrary code with BPF
capabilities. A maintainer-controlled promotion or isolated sandbox is required
before that check runs.
