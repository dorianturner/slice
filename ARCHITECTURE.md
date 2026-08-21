# Slice architecture contract

Slice is a percentile-conditioned profiler. `slice-core::Profile` is the
semantic boundary shared by capture, analysis, serialization, and rendering.
The `.slice` file format is versioned and currently remains at format version 1.

## Hexagonal boundary

```text
                    inbound application
                         slice-cli
                        /         \
                       v           v
            slice-capture       slice-core
             capture port      profile domain
                   ^            ^    ^
                   | implements |    | consumes
             slice-ebpf      slice-collector  slice-render
             Linux adapter       domain       HTML adapter
```

`slice-capture` is the inbound port for live profiling. It owns platform-neutral
capture requests, process identities, structured prerequisite reports, errors,
and the `CapturePort` trait. `slice-ebpf` is the first adapter. Future Linux
backends, test doubles, remote capture agents, or other supported platforms can
implement the same port without changing the CLI orchestration or profile
domain.

The stable `.slice` profile remains the boundary between capture and offline
analysis. Hexagonal boundaries do not move profile semantics into the adapter:
the collector and core still decide what constitutes a valid invocation.

## Dependency layers

The allowed internal dependency edges are:

| Package | May depend on |
| --- | --- |
| `slice-core` | no workspace package |
| `slice-capture` | `slice-core` |
| `slice-collector` | `slice-core` |
| `slice-render` | `slice-core` |
| `slice-ebpf` | `slice-capture`, `slice-core`, `slice-collector` |
| `slice-cli` | `slice-capture`, `slice-core`, `slice-render`, `slice-ebpf` |
| `repo-check` | tooling only; it is not a product dependency |

The `repo-check` tool validates these edges in CI. New edges require an
architecture document and an execution plan before they are added.

## Boundary rules

- `slice-core` owns profile semantics, query behavior, serialization, and
  validation. It must not depend on eBPF, HTML, or process/filesystem capture.
- `slice-capture` owns capture-facing ports and data contracts. It contains no
  platform syscalls, libbpf types, CLI formatting, or unsafe code.
- `slice-collector` converts event streams into valid invocations and explicit
  quality counters. It must not load the kernel or render output.
- `slice-ebpf` is the Linux adapter. It owns libbpf, `/proc` identity mapping,
  process control, capability-sensitive doctor checks, and the narrow unsafe
  FFI boundary. It implements `slice-capture::CapturePort`.
- `slice-render` consumes validated profiles and produces self-contained HTML.
- `slice-cli` is the application shell. It invokes ports and formats results;
  it does not inspect Linux capabilities or duplicate core profile semantics.
- Kernel code performs bounded capture only. Symbolization, correlation,
  percentile selection, and rendering stay in userspace.

## Compatibility rules

- Platform identity is explicit: adapters translate a caller-visible PID into
  the identity their event source uses. Domain code never assumes PID namespace
  values are interchangeable.
- Scheduler adapters also translate between event-source identities. The Linux
  adapter learns the `sched_switch` tracepoint TID to BPF-helper TID mapping
  while the outgoing task is current, which keeps WSL namespace differences
  outside the collector and profile domain.
- Probe attachment supplies the trust boundary. Linux uprobe-multi links scope
  entry and return events to one process address space; active invocation IDs
  then scope sampling and scheduler attribution without a second PID filter.
- The Linux adapter preserves the kernel invocation ID on every boundary and
  sample event and orders buffered records by kernel timestamp before handing
  them to the collector. Ring-buffer delivery order must not affect profile
  association, including when a thread migrates between CPUs.
- Doctor checks are structured data with pass, warning, failure, detail, and
  remediation fields. New adapters provide equivalent checks through the port;
  the CLI owns only presentation and exit behavior.
- Adapter diagnostics report attachment, transport, and identity separately so
  compatibility failures do not collapse into “function was not reached.”
- Off-CPU samples retain the user stack captured at switch-out. A missing stack
  may fall back to the selected function, but the blocked interval and its
  capture-quality failure remain explicit.

## Change protocol

Changes to the profile schema, event ABI, dependency layers, unsafe boundary,
or capture-quality semantics must update this document, add focused tests, and
include a checked-in execution plan under `docs/plans/active/`.
