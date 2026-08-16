# Native verification workloads

Build all workloads with the same flags used for reliable user-stack capture:

```bash
cmake -S fixtures -B build/fixtures -G Ninja
cmake --build build/fixtures
ctest --test-dir build/fixtures --output-on-failure
```

## `bimodal_service`

This is the primary README demonstration. It runs until SIGINT by default and
starts several workers. Every selected `BimodalFixture::handle_request` call
uses a deterministic global sequence:

- sequence values ending in 0–6 call `fast_path()` and take a normally
  distributed 10ms ± 5ms of CPU time;
- sequence values ending in 7–9 call `slow_path()`, spending a normally
  distributed 15ms ± 5ms waiting and 5ms on CPU, for a 20ms ± 5ms total.

That gives an approximate 70/30 bimodal latency histogram with a useful overlap
between the two bands. The slow mode is especially useful for comparing
wall-time, CPU-time, and off-CPU flame views, while the overlap makes the
middle of the flame graph contain both paths.
Use `--iterations N` for a finite native test run.

```bash
./build/fixtures/bimodal_service --workers 4
slice symbols build/fixtures/bimodal_service --match handle_request
```

## Other fixtures

`tail_divergence` is the percentile correctness fixture. Across every 100
`SliceFixture::work(unsigned int)` calls, 99 calls execute
`fast_aggregate_a()` for 3ms and one p99 call executes `slow_tail_b()` for
297ms. Both children occupy exactly 297ms in the aggregate profile, yet only
`slow_tail_b()` appears in p99:p100.

`off_cpu_wait` validates scheduler-event attribution. `nested_population`
intentionally violates the selected-invocation non-overlap rule and must result
in a visible quality warning rather than misleading percentile analysis.
