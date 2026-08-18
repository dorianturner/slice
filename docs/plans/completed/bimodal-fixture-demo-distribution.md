# Bimodal fixture demo distribution

## Intent

Make the synthetic `bimodal` fixture a useful offline viewer demo: enough
invocations to form a clear histogram, two latency peaks centered at 10 ms and
20 ms, and deterministic normal-shaped tapering on either side of each peak.

## Acceptance criteria

- Ten worker streams produce 9,000 valid invocations with a reproducible 70/30
  fast/slow split.
- Fast durations range from 2 ms to 18 ms around a 10 ms mean; slow durations
  range from 12 ms to 28 ms around a 20 ms mean, with visible overlap.
- The slow population retains both 5 ms of CPU work and off-CPU wait time.
- The generated histogram has readable intermediate time ticks, including
  useful values between its endpoints.
- The inspectable neo-brutalist HTML artifact is regenerated at
  `bimodal-neo-brutalist.html`.

## Verification

- `cargo fmt --all -- --check` passed.
- `cargo test -p slice-core` passed.
- `cargo test -p slice-render` passed.
- `tests/viewer-smoke.js bimodal-neo-brutalist.html` passed.
- `just check` passed; optional actionlint, cargo-deny, and gitleaks scanners
  were skipped because they are not installed locally.
