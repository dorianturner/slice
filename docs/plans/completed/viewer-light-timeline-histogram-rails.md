# Light timeline and two-zone range interactions

## Intent

Keep the generated viewer on the neo-brutalist light theme, add matching
selection behavior to the latency histogram, and make the empty capture-start
and histogram-rail areas move an existing window instead of redrawing it.

## Interfaces and architecture

- Preserve profile semantics, the `.slice` format, and crate dependency
  direction.
- Keep all changes inside the self-contained HTML renderer, its smoke test,
  documentation, and the local completion hook.
- The completion hook runs `just check` directly; it no longer selects a Nix
  development shell.

## Acceptance criteria

- Timeline labels, lanes, and backgrounds use the report’s light theme.
- Dragging timeline lanes or the histogram body creates a range from the drag
  endpoints.
- Dragging the timeline capture-start strip or histogram rail moves the
  existing range without changing its width.
- The histogram rail is visibly distinct and extends above the bars.
- The flame graph explains that it intentionally starts at the selected named
  function and includes descendants, not callers above it.

## Verification

- `cargo fmt --all -- --check` passed.
- `cargo test -p slice-render` passed.
- `tests/viewer-smoke.sh` passed, including timeline and histogram drag
  simulations.
- Full `just check` passed, including policy, Rust, BPF, native, and viewer
  gates; optional local scanners were skipped because they are not installed.

