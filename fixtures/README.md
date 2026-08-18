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

- sequence values ending in 0–6 call `fast_path()` and take a normally
  distributed 10ms ± 5ms of CPU time;
- sequence values ending in 7–9 call `slow_path()`, spending a normally
  distributed 15ms ± 5ms waiting and 5ms on CPU, for a 20ms ± 5ms total.

That gives an approximate 70/30 bimodal latency histogram with useful overlap
between the two bands, including samples around 15ms. The slow mode is
especially useful for comparing wall-time, CPU-time, and off-CPU flame views.
Use `--iterations N` for a finite native test run.

```bash
./build/fixtures/bimodal_service --workers 10
slice symbols build/fixtures/bimodal_service --match handle_request
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

The first three workloads also have deterministic offline equivalents for
viewer development. Generate one with:

```bash
slice fixture-profile --scenario tail --output tail.slice
slice fixture-profile --scenario bimodal --output bimodal.slice
slice fixture-profile --scenario off-cpu --output off-cpu.slice
slice view off-cpu.slice --metric off-cpu --output off-cpu.html
```

`nested_population` intentionally violates the selected-invocation non-overlap
rule and remains a native capture/collector fixture; it must result in a
visible quality warning rather than misleading percentile analysis.
