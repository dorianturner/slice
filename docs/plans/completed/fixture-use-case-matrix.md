# Fixture use-case matrix

## Intent

Make the repository fixtures useful as repeatable native workloads for the
profiler's supported workflows, not only as build smoke tests.

## Acceptance criteria

- Native fixtures cover tail, bimodal, and off-CPU workloads.
- Tests exercise profile validation, sampled-function discovery, HTML viewing,
  thread/time filtering, and off-CPU metric selection on captured profiles.
- Native fixtures continue to cover real ELF symbol discovery and the
  intentionally invalid nested-population quality path.
- README and engineering docs explain which fixture to use for each workflow.

## Verification

- `cargo test -p slice-core`
- `cargo test -p slice-cli`
- `just check`
- Hosted policy, Rust, BPF, native, and Nix checks
