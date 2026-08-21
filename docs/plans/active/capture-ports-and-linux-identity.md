# Capture ports and Linux identity

## Intent

Turn live capture into an explicit hexagonal port, keep Linux/libbpf details in
an adapter, and make failures diagnosable across WSL, native Ubuntu, and NixOS.

## Interfaces and scope

- Add `slice-capture` with platform-neutral capture, process identity, doctor,
  and error contracts.
- Make `slice-ebpf::LinuxCaptureAdapter` implement the capture port.
- Keep `.slice` format version 1 and collector semantics unchanged.
- Scope Linux entry/return through PID-scoped uprobe-multi links and scope
  sampling/scheduler events through `active_by_tid`, avoiding PID-namespace
  comparisons in BPF.
- Bridge namespace-relative `sched_switch` TIDs to BPF-helper TIDs at
  switch-out and preserve the blocked user stack on each off-CPU interval.
- Report doctor checks and empty-capture diagnostics as distinct architecture,
  kernel, privilege, attachment, transport, and identity facts.

## Acceptance criteria

- The threaded bimodal fixture records complete invocations and samples on a
  current WSL 2 Ubuntu kernel and the privileged Linux runner.
- Validation reports separate on-CPU and off-CPU counts, and the live demo
  requires a nonzero off-CPU population containing its stable `sleep_for`
  application frame.
- `slice doctor` emits structured pass/warn/fail lines and actionable fixes.
- Repository policy mechanically enforces the new dependency direction.
- README and architecture diagrams show ports, adapters, domain services, and
  future adapter extension points.
- All ordinary gates pass; the live gate passes on a privileged host.

## Verification

- `cargo test --workspace --all-targets`
- `just check`
- `just test-live` on a privileged Linux 6.6+ host
- `bash scripts/regenerate-bimodal.sh` on WSL 2
