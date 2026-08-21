# Testing contract

The commands below are exposed through the repository `justfile` and can run
in a normal development shell. Nix remains available for the separate
reproducibility check, but the local test loop does not require it.

| Gate | Command | Purpose |
| --- | --- | --- |
| Policy | `just policy` | Architecture, docs, workflow, and repository checks |
| Formatting | `just format` | Rust formatting is stable and checked |
| Rust | `just lint` / `just test` | Warnings and deterministic unit/integration behavior |
| BPF | `just test-bpf` | Compile the kernel program with warnings as errors |
| Native | `just test-native` | Build C++ fixtures and test ELF symbol discovery |
| Viewer | `SLICE_VIEW_PROFILE=path/to/capture.slice just test-viewer` and `cargo test -p slice-cli` | Render and execute a report from a real capture; without `SLICE_VIEW_PROFILE`, the smoke script reports an explicit skip |
| Nix | `nix flake check -L` | Reproducible package and declared flake checks |
| Live capture | `just test-live` | Required only on the privileged ephemeral runner |

The ordinary `just check` gate never requires kernel capabilities. The live
gate is intentionally separate so Rust, renderer, collector, and fixture work
remain fast and deterministic.

## Live-capture contract

The CI live test launches the native workload through Slice. Linux 6.6+
process-scoped uprobe-multi links cover worker threads created after the child
is resumed, including migrations between CPUs, without perf inheritance.
The test must fail, rather than skip, when architecture, BTF, tracepoints,
capabilities, or eBPF attachment are unavailable. It must produce a validated
profile with complete invocations, nonzero samples, expected fixture symbols,
a nonzero off-CPU population whose blocked stack contains the fixture's stable
`sleep_for` frame, and a self-contained HTML report.

Local developers may run the script without `REQUIRE_PRIVILEGED=1`; unavailable
privileges can then produce an explicit skip for convenience.
