# Engineering knowledge map

This directory is the repository-local system of record for engineering
decisions. `AGENTS.md` is intentionally short and points here rather than
duplicating these documents.

Read the documents in this order when changing behavior:

1. [Architecture](../ARCHITECTURE.md) for package boundaries and profile
   invariants.
2. [Testing](testing.md) and [Security](security.md) for the executable gates
   and privilege model.
3. The relevant package or fixture guide for implementation details.
4. [Active plans](plans/active/index.md) for unfinished work and acceptance
   criteria.

| Question | Authoritative document |
| --- | --- |
| How do I capture and view a profile? | [root README](../README.md) |
| Which native workload should I use? | [fixture guide](../fixtures/README.md) |
| What does a sample or metric mean? | [viewer contract](viewer.md) |
| Which checks must pass? | [testing contract](testing.md) |
| What privilege and runner constraints apply? | [security contract](security.md) |
| How do branches and PRs merge? | [GitHub contract](github.md) |

Additional references:

- [Quality](quality.md) — current quality grades and known gaps.
- [Plans](plans/active/index.md) — active execution plans.
- [Technical debt](tech-debt.md) — small, continuously maintained debt list.
- [Viewer](viewer.md) — offline report interactions and sampled-stack semantics.

User-facing profiling instructions remain in the root [README](../README.md).
When behavior changes, update the authoritative document and link to it from
the user-facing guide if needed.
