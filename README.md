# Slice

Slice is a Linux x86-64 profiler POC for answering a narrower question than an
aggregate flame graph: *which stack paths occur in the selected slowest
invocations of one C++ function?*

The repository currently provides the versioned profile format, deterministic
query engine, self-contained interactive HTML renderer, exact ELF symbol
selection, and controlled C++ workloads. The eBPF collection layer is isolated
behind the same profile model so its privileged capture path cannot change
percentile semantics.

## Quick proof of the tail-use case

```bash
nix develop
cargo run -- fixture-profile --output tail-divergence.slice
cargo run -- view tail-divergence.slice --output tail-divergence.html --percentile 99:100
```

Open `tail-divergence.html` directly in a browser. The aggregate profile gives
`fast_aggregate_a` and `slow_tail_b` equal width. Filtering to p99:p100 exposes
only `slow_tail_b`, which is the deliberately hidden culprit.

`fixtures/` contains matching native C++ workloads built with CMake. They use
frame pointers, debug symbols, noinline functions, and deterministic latency
classes so the privileged collector can be verified end-to-end.

## Current commands

```text
slice symbols <binary> [--module <elf>] [--match <substring>]
slice fixture-profile --output capture.slice
slice profile --pid <PID> --module <elf> \
  --function 'SliceFixture::work(unsigned int)' --duration 2s \
  --output capture.slice
slice profile --function 'Server::handleRequest(Request const&)' \
  --output capture.slice ./server --config prod.toml
slice view capture.slice --output profile.html [--threads 42,43] \
  [--time 10ms:700ms] [--percentile 99:100] [--metric wall]
slice doctor
```

The live `profile` command attaches an entry/return uprobe at the ELF file
offset corresponding to the exact demangled function, samples user stacks, and
also records scheduler deschedule intervals for `--metric off-cpu`. Use
`--pid` for an existing process or pass a program; launched targets are stopped
until every probe is attached. Capture requires root or a least-privilege
wrapper with `CAP_BPF`, `CAP_PERFMON`, and (for unrelated attach targets)
`CAP_SYS_PTRACE`.

`slice doctor` reports whether the host can support the capture path.
