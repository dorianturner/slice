# Engineering knowledge map

This directory is the repository-local system of record for engineering
decisions. `AGENTS.md` is intentionally short and points here rather than
duplicating these documents.

- [Architecture](../ARCHITECTURE.md) — layers and invariants.
- [Testing](testing.md) — local and GitHub checks.
- [Security](security.md) — eBPF, runners, dependencies, and artifacts.
- [GitHub workflow](github.md) — branch rules and agent lifecycle.
- [Quality](quality.md) — current quality grades and known gaps.
- [Viewer](viewer.md) — offline report interactions and sampled-stack semantics.
- [Plans](plans/active/index.md) — active execution plans.
- [Technical debt](tech-debt.md) — small, continuously maintained debt list.

User-facing profiling instructions remain in the root [README](../README.md).
When behavior changes, update the authoritative document and link to it from
the user-facing guide if needed.
