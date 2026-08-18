# Agent entrypoint

Slice is a Rust workspace with a small C eBPF transport. Treat the repository
as the source of truth: do not rely on undocumented chat or local assumptions.

## Start here

- [ARCHITECTURE.md](ARCHITECTURE.md) — crate layers and stable profile boundary.
- [docs/index.md](docs/index.md) — engineering knowledge map.
- [docs/testing.md](docs/testing.md) — canonical checks and test classes.
- [docs/security.md](docs/security.md) — privilege and runner constraints.
- [docs/github.md](docs/github.md) — PR, gate, and agent-loop contract.
- [.codex/hooks.json](.codex/hooks.json) — the local Codex stop hook that runs
  the repository checks before a task can finish.

## Commands

Run from the repository development environment. Nix is optional for the
ordinary local loop:

```text
just check       # all ordinary repository gates
just test-live   # privileged, self-hosted-runner-only capture test
```

When this repository is trusted by Codex, its `Stop` hook runs the ordinary
`just check` gate automatically. A failed check blocks task completion and
asks Codex to repair the branch, up to a bounded number of attempts. Run
`/hooks` in Codex to review and trust project-local hooks; GitHub required
checks remain authoritative for merging.

Use `just format`, `just lint`, `just test`, `just test-bpf`, and
`just test-native` when iterating on one layer. Never weaken a gate to make a
check pass; fix the repository contract or record a plan when the contract is
wrong.

## Change rules

- Keep the dependency direction in `ARCHITECTURE.md` mechanically valid.
- Keep unsafe code inside `slice-ebpf` and keep the kernel/userspace ABI tested.
- Treat `.slice` format version 1 as a compatibility contract.
- Update documentation and an execution plan when behavior or architecture
  changes.
- PRs must include acceptance criteria, architecture/profile/security impact,
  test evidence, and documentation status.
- Agents may iterate and merge only after every required GitHub check passes.
