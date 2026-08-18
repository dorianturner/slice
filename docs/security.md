# Security contract

Slice loads eBPF programs, observes process execution, and may target a PID
owned by another process. Treat capture as privileged infrastructure.

## Runner rules

- The live gate runs only on a dedicated ephemeral Linux x86-64 runner.
- The runner is isolated from unrelated workloads and is destroyed or reset
  after each job.
- It must provide BTF, `sched_switch`, and the capabilities reported by
  `slice doctor`.
- Ordinary pull-request jobs run without secrets and without write tokens.
- Do not run untrusted fork code on a privileged runner. External changes must
  be promoted to a trusted same-repository branch or a sandboxed equivalent.
- Raw `.slice` captures are not uploaded by default because they may contain
  process names, paths, symbols, and timing information.

## Code and dependency rules

- Unsafe Rust is confined to `slice-ebpf` and reviewed through its boundary
  contract.
- The BPF source is compiled with warnings treated as errors.
- Cargo advisories, licenses, duplicate versions, and allowed registries are
  checked by `cargo-deny`.
- GitHub Actions are pinned to immutable commit SHAs and use least-privilege
  permissions.
- Secret scanning and workflow linting are required policy checks.

Report security issues through the process described in `SECURITY.md` if one is
added for public disclosure; do not put exploit details in ordinary issues.
