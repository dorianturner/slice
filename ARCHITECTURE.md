# Slice architecture contract

Slice is a percentile-conditioned profiler. `slice-core::Profile` is the
semantic boundary shared by capture, analysis, serialization, and rendering.
The `.slice` file format is versioned and currently remains at format version 1.

## Dependency layers

```text
slice-core
   ↑
slice-collector       slice-render
   ↑                       ↑
slice-ebpf  ───────────────┘
   ↑
slice-cli
```

The allowed internal dependency edges are:

| Package | May depend on |
| --- | --- |
| `slice-core` | no workspace package |
| `slice-collector` | `slice-core` |
| `slice-render` | `slice-core` |
| `slice-ebpf` | `slice-core`, `slice-collector` |
| `slice-cli` | `slice-core`, `slice-render`, `slice-ebpf` |
| `repo-check` | tooling only; it is not a product dependency |

The `repo-check` tool validates these edges in CI. New edges require an
architecture document and an execution plan before they are added.

## Boundary rules

- `slice-core` owns profile semantics, query behavior, serialization, and
  validation. It must not depend on eBPF, HTML, or process/filesystem capture.
- `slice-collector` converts event streams into valid invocations and explicit
  quality counters. It must not load the kernel or render output.
- `slice-ebpf` owns libbpf, process control, capability-sensitive operations,
  and the narrow unsafe FFI boundary.
- `slice-render` consumes validated profiles and produces self-contained HTML.
- `slice-cli` wires user-facing commands together; it does not duplicate core
  profile semantics.
- Kernel code performs bounded capture only. Symbolization, correlation,
  percentile selection, and rendering stay in userspace.

## Change protocol

Changes to the profile schema, event ABI, dependency layers, unsafe boundary,
or capture-quality semantics must update this document, add focused tests, and
include a checked-in execution plan under `docs/plans/active/`.
