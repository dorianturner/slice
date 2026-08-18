# Fixture use-case matrix

## Intent

Make the repository fixtures useful as repeatable demonstrations of the
profiler's supported workflows, not only as build smoke tests.

## Acceptance criteria

- The CLI can generate deterministic `tail`, `bimodal`, and `off-cpu` profiles.
- Tests exercise profile validation, sampled-function discovery, HTML viewing,
  thread/time filtering, and off-CPU metric selection across the scenarios.
- Native fixtures continue to cover real ELF symbol discovery and the
  intentionally invalid nested-population quality path.
- README and engineering docs explain which fixture to use for each workflow.

## Verification

- `cargo test -p slice-core`
- `cargo test -p slice-cli`
- `just check`
- Hosted policy, Rust, BPF, native, and Nix checks
