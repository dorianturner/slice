# Native verification workloads

Build all workloads with the same flags used for reliable user-stack capture:

```bash
cmake -S fixtures -B build/fixtures -G Ninja
cmake --build build/fixtures
ctest --test-dir build/fixtures --output-on-failure
```

## `bimodal_service`

This is the primary README demonstration. It runs until SIGINT by default and
supports multiple workers; the walkthrough uses ten. Every selected
`BimodalFixture::handle_request` call
uses a deterministic global sequence:

- sequence values ending in 0–6 call `fast_path()` and request a normally
  distributed 10ms ± 5ms CPU interval after the distribution work;
- sequence values ending in 7–9 call `slow_path()`, spending a normally
  distributed 15ms ± 5ms waiting and 5ms on CPU, for a 20ms ± 5ms timed
  interval, plus the distribution work.

That gives an approximate 70/30 bimodal latency histogram with useful overlap
between the two bands, including samples around 15ms. The slow mode is
especially useful for comparing wall-time, CPU-time, and off-CPU flame views.
Use `--iterations N` for a finite native test run.

The native workload keeps its real `normal_distribution()`, `spin_for()`, and
`sleep_for()` application helpers out of line. The distribution helper performs
65,536 real `std::normal_distribution` draws per request, while `sleep_for()`
provides a stable frame above the inline standard-library sleep wrapper on a
blocked stack. This makes both CPU and off-CPU behavior observable without
inserting synthetic stack data. Generate the inspectable report from an actual
capture with the repository wrapper:

```bash
just regenerate-bimodal
```

The wrapper builds in a temporary directory, builds the release CLI, runs the
doctor and live capture steps with the required privilege, validates the
result, and atomically replaces `bimodal.slice` before rendering the report.
Set `SLICE_BIMODAL_WORKERS` or `SLICE_BIMODAL_ITERATIONS` to adjust the finite
fixture run. To inspect each low-level command, use the root
[README walkthrough](../README.md#profile-the-bimodal-fixture).

The capture command requires Linux 6.6+, BPF/perf privileges, kernel BTF, and
the `sched_switch` tracepoint. The launch form stops the child before it
creates workers, installs process-wide uprobe-multi and uretprobe-multi links,
scopes them to the child process address space, and resumes it. It captures the
real worker executions on whichever CPUs they use, validates a nonzero off-CPU
population, and renders the wall-time report. It fails rather than creating a
replacement profile when the host cannot capture.

For the optional viewer smoke test against that real capture:

```bash
SLICE_VIEW_PROFILE="$PWD/bimodal.slice" just test-viewer
```

## Fixture matrix

| Workload | Use it to demonstrate | Main selector / command |
| --- | --- | --- |
| `tail_divergence` | Percentile-conditioned flame graphs: aggregate paths can differ from p99 paths. | `SliceFixture::work(unsigned int)` |
| `bimodal_service` | Multi-thread timelines, overlapping latency modes, and wall/CPU/off-CPU comparisons. | `BimodalFixture::handle_request(unsigned long)` |
| `off_cpu_wait` | Scheduler-event attribution and a wait-heavy off-CPU flame graph. | `SliceFixture::sleep_work(unsigned int)` |
| `nested_population` | The invalid-input guard: nested selected invocations become a quality warning instead of double-counting. | `SliceFixture::work(unsigned int)` |

`tail_divergence` makes 99 of 100 `SliceFixture::work(unsigned int)` calls
execute `fast_aggregate_a()` for 3ms and one p99 call execute `slow_tail_b()`
for 297ms. Both children occupy exactly 297ms in the aggregate profile, yet
only `slow_tail_b()` appears in p99:p100.

`nested_population` intentionally violates the selected-invocation non-overlap
rule and remains a native capture/collector fixture; it must result in a
visible quality warning rather than misleading percentile analysis.
