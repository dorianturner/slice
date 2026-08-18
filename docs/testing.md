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
| Viewer | `just test-viewer` and `cargo test -p slice-cli` | Execute the generated offline report and exercise tail, bimodal, and off-CPU scenarios through the CLI |
| Nix | `nix flake check -L` | Reproducible package and declared flake checks |
| Live capture | `just test-live` | Required only on the privileged ephemeral runner |

The ordinary `just check` gate never requires kernel capabilities. The live
gate is intentionally separate so Rust, renderer, collector, and fixture work
remain fast and deterministic.

## Live-capture contract

The CI live test uses a finite native workload launched by Slice itself. It
must fail, rather than skip, when architecture, BTF, tracepoints, capabilities,
or eBPF attachment are unavailable. It must produce a validated profile with
complete invocations, nonzero samples, expected fixture symbols, and a
self-contained HTML report.

Local developers may run the script without `REQUIRE_PRIVILEGED=1`; unavailable
privileges can then produce an explicit skip for convenience.
