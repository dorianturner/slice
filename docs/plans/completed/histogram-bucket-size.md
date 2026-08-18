# Histogram bucket-size control

## Intent

Let report readers choose a fixed latency bucket width when the automatically
selected bin count is not useful for the question they are investigating.

## Acceptance criteria

- The histogram header exposes Auto plus fixed 0.25, 0.50, 1, 2, and 5 ms
  choices.
- Auto preserves the existing range-derived binning.
- Fixed widths align the histogram bounds to the selected width and redraw only
  the histogram geometry.
- The selected invocation population, percentile window, drag interactions, and
  flame graph remain unchanged.

## Verification

- `cargo test -p slice-render` passed.
- `tests/viewer-smoke.sh` passed, including a bucket-size change and bin-count
  assertion.
