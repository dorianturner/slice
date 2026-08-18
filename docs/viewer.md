# Offline viewer contract

The slice view command writes one self-contained HTML report. It embeds the
profile, query, CSS, and JavaScript, so opening the file does not require a
server or network access.

## Selection controls

- Wall time is the default metric. CPU and off-CPU metrics change which sampled
  weights contribute to the flame graph; wall latency still determines the
  invocation population.
- The thread selector is a multi-select control.
- Timeline lane drags create a time window. The capture-start strip moves the
  existing window while preserving its width.
- Histogram body drags create a latency window. Its upper rail moves the current
  window, and the percentile fields provide exact percentile controls.
- The histogram Bucket control defaults to Auto. Fixed choices use the selected
  time width, align the histogram bounds to that width, and redraw the bins
  without changing the selected invocation population.
- The 0:100 percentile range includes every valid invocation. A narrower range
  is intentionally a population filter.

## Flame graph semantics

The flame graph is a tree of recorded sampled user stacks. It begins at the
selected named function and walks down the recorded descendants. It does not
invent frames, reconstruct unsampled call paths, or show callers above the
selected population function.

Consequently, a function can be absent when no sample landed in it, when the
selected metric excludes its state, or when the compiler inlined it. Showing a
complete call tree for every invocation would require call tracing or
instrumentation; it is not what statistical stack sampling promises.

The repository ships three deterministic offline scenarios for exercising the
viewer without privileges: `tail` demonstrates percentile filtering, `bimodal`
demonstrates overlapping multi-thread latency and all metrics, and `off-cpu`
demonstrates wait-heavy attribution. The native `nested_population` workload
is intentionally invalid and is reserved for collector quality handling. Real
captures use the stacks returned by the eBPF/perf sampling path and therefore
reflect the actual program and compiler output.
