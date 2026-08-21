# Bimodal real wall-time profile

## Intent

Make the native bimodal fixture useful for inspecting real wall-time sampled
stacks, including its distribution and CPU loop, and make regeneration of the
checked-out viewer artifact use a live capture only.

## Interfaces and scope

- Keep the `.slice` schema and dependency layers intact. Transport-level
  invocation identity may evolve so the collector can correlate live records
  without changing offline profile semantics.
- Give the native fixture an out-of-line distribution workload that executes
  `std::normal_distribution` enough to be observable by the existing sampler.
- Require the live wall-time capture check to expose both paths, `spin_for`, and
  the distribution wrapper.
- Run the threaded fixture through launch-mode profiling; Slice binds Linux
  6.6+ uprobe-multi links to the target process address space and uses active
  invocation IDs to cover workers created after resume across CPU migrations.
- Add `just regenerate-bimodal` backed by `scripts/regenerate-bimodal.sh`; it
  must fail when native capture cannot run and writes only live-capture data.
- Document a complete Ubuntu/WSL 2 setup that avoids unavailable WSL kernel
  header packages, builds without root, and elevates only capture operations.

## Acceptance criteria

- Native symbol discovery finds `normal_distribution` and `spin_for`.
- The privileged capture test launches the fixture through Slice, validates a
  complete profile, and checks all four expected wall-time frames.
- The regeneration script builds the fixture, captures it, validates it, and
  preserves `bimodal.slice` and renders `bimodal-neo-brutalist.html` from that
  capture.
- `slice doctor` reports the Linux 6.6+ process-wide uprobe-multi requirement
  and gives WSL-specific context when running on a Microsoft kernel.
- No synthetic profile generator or synthetic capture artifact remains.

## Verification

- `just test-native`
- `just test-live` on a host with BPF, perf, and scheduler tracepoint access
- `scripts/regenerate-bimodal.sh` on the same privileged host
- `just check`

## Capture-correlation invariant

The live adapter preserves the BPF invocation ID on entry, return, on-CPU,
and off-CPU records. It reorders the buffered records by kernel timestamp
before invoking the collector, and the collector assembles records by that ID,
so ring-buffer delivery order cannot move a sample across invocation
boundaries. A genuinely out-of-bounds sample is rejected as capture-quality
data rather than making the profile unreadable. Metric selection remains
entirely offline: wall, CPU, and off-CPU filters consume the same validated
samples.
