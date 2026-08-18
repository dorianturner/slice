# Quality scorecard

This scorecard is intentionally small and should be updated when a gate or a
known gap changes.

| Area | Grade | Evidence | Known gap |
| --- | --- | --- | --- |
| `slice-core` profile/query semantics | A | Deterministic unit tests and round-trip tests | Format migrations are not yet needed |
| `slice-collector` correlation | A- | Synthetic event tests cover invalidation and quality counters | More property-based event streams would help |
| `slice-render` offline output | B+ | Self-contained HTML integration tests | Browser-driven rendering checks are not yet available |
| `slice-ebpf` transport | B | Strict BPF compile and optional live capture | Live capture requires a dedicated runner |
| Native fixtures | A- | CMake/CTest and ELF symbol discovery | Kernel behavior varies by host |
| Agent/repository contract | B | Repo checks and documented gates | GitHub settings are operator-managed |

Quality regressions should become either executable checks or entries in
`docs/tech-debt.md`; they should not live only in review comments.
