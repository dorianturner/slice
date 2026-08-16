# Native verification workloads

Build all workloads with the reproducible flags required for reliable user
stack capture:

```bash
cmake -S fixtures -B build/fixtures -G Ninja
cmake --build build/fixtures
ctest --test-dir build/fixtures --output-on-failure
```

`tail_divergence` is the primary correctness fixture. For every 100 calls to
`SliceFixture::work(unsigned int)`, 99 calls execute
`fast_aggregate_a()` for 3ms each and one p99 call executes `slow_tail_b()` for
297ms. Both children therefore occupy exactly 297ms in the aggregate profile,
yet only `slow_tail_b()` appears in p99:p100.

The generated `tail-divergence.slice` profile contains the same construction
and is tested without privilege. Once capture is run with the eBPF backend,
compare the native result to those exact assertions:

```bash
slice symbols build/fixtures/tail_divergence --match 'SliceFixture::work'
# run the capture with the full demangled selector printed above
slice view tail-divergence.slice --percentile 99:100 --output tail.html
```

`off_cpu_wait` validates scheduler-event attribution. `nested_population`
intentionally violates the selected-invocation non-overlap rule and must result
in a visible quality warning rather than percentile analysis of bad data.

