# Slice

Slice is a Linux x86-64 percentile profiler for one focused question:

> What does the call stack look like during the slowest executions of one C++ function?

This POC attaches to one exact demangled ELF function, records its invocations
and sampled user stacks, and renders percentile-conditioned results as a
self-contained HTML flame graph.

The walkthrough below uses the `bimodal_service` fixture. It has an approximate
70/30 latency split with overlapping normal jitter around each mode:

- 70% of calls take roughly 10 ms of CPU work with a 5 ms standard deviation;
- 30% of calls take roughly 20 ms total with a 5 ms standard deviation, split
  between scheduler wait and 5 ms of CPU work.

The jitter makes the histogram look like a real bimodal population, with
samples between and across the two peaks instead of only two exact-duration
bars.

That makes the slow tail easy to find with a `p95:p100` query and makes the
off-CPU contribution visible.

## NixOS setup

This repository has two different Nix environments. Use them in separate
shells:

- `nix develop` provides the pinned compiler, CMake, Ninja, Rust, and BPF
  build tools. It does not provide the packaged `slice` executable.
- `nix shell .#default --command slice ...` provides the packaged `slice`
  executable. For live eBPF capture on NixOS, run that command with `sudo`.

Do not rely on bare `sudo nix shell .#default` to create the interactive shell
on NixOS. On some configurations the root shell startup files replace the
`PATH` that Nix prepared, leaving `slice` unavailable. Use the explicit
`--command` form below. Also do not run `nix develop` inside it: that is a
different environment and does not contain the packaged CLI.

From the repository root, build the native fixture as your normal user:

```bash
nix develop
cmake -S fixtures -B build/fixtures -G Ninja
cmake --build build/fixtures
```

You should now have:

```text
build/fixtures/bimodal_service
```

Leave the development shell before starting the privileged runtime shell:

```bash
exit
```

The dirty-tree warning printed by Nix is informational; it does not prevent
the local flake from being used.

## Profile the bimodal fixture

There are two reliable ways to start the privileged CLI. For a single
command, use:

```bash
sudo nix shell .#default --command slice doctor
```

For an interactive root shell, use this form instead:

```bash
sudo nix shell .#default --command bash --noprofile --norc -i
```

Then run the commands below as `slice ...`. Do not run `nix develop` in that
shell.

### 1. Check capture prerequisites

```bash
slice doctor
```

Root is normally enough for this POC on a local NixOS machine. The output
should report available kernel BTF and a permitted `sched_switch` tracepoint.
If it still reports a permission or kernel problem, the issue is with the
host's eBPF/tracefs configuration, not with the Nix shell.

### 2. Find the exact function signature

```bash
slice symbols build/fixtures/bimodal_service --match handle_request
```

Copy the complete demangled signature from the output. For this fixture it is:

```text
BimodalFixture::handle_request(unsigned long)
```

### 3. Capture the live workload

Run Slice as the supervisor so it launches the fixture, attaches before the
first request, and captures until you stop it:

```bash
slice profile \
  --function 'BimodalFixture::handle_request(unsigned long)' \
  --output bimodal.slice \
  build/fixtures/bimodal_service -- --workers 4
```

The fixture runs continuously. Let it run for roughly 10 seconds, then press
Ctrl-C in the profiling shell. Slice stops the child, drains pending events,
and writes `bimodal.slice`.

A successful run ends with a line similar to:

```text
captured BimodalFixture::handle_request(unsigned long) at PID ... -> bimodal.slice
```

If capture fails before that line, no usable profile was written. Fix the
reported `slice doctor` or kernel error before trying to view the file.

### 4. Render the complete capture

Render the capture through the packaged CLI:

```bash
slice view bimodal.slice --output bimodal.html
```

Open the generated file in a browser:

```bash
firefox bimodal.html
```

## Inspect the slow percentile

The slow mode is the upper tail of the bimodal population. Render only the
slowest 5% of invocations and select the off-CPU metric:

```bash
nix shell .#default --command slice view bimodal.slice \
  --output bimodal-p95.html \
  --percentile 95:100 \
  --metric off-cpu
```

Open it with:

```bash
firefox bimodal-p95.html
```

The report should emphasize `BimodalFixture::slow_path()`. Its wall-time view
includes the roughly 15 ms scheduler wait, while its CPU-time view mostly shows
the roughly 5 ms of post-wait work. The off-CPU view exposes the scheduler
wait, and middle latency windows can include both paths because the bands
overlap.
The HTML file is self-contained and can be shared without the `.slice` file.

## Useful commands

```text
slice symbols <binary> [--module <elf>] [--match <substring>]
slice profile --function '<full signature>' PROGRAM [-- ARGS...]
slice profile --pid <PID> --module <elf> --function '<full signature>'
slice view capture.slice --output profile.html [--percentile 95:100]
slice discover capture.slice
slice doctor
```
