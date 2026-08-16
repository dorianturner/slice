# Slice

<p align="center">
  <img src="docs/assets/slice_logo.svg" alt="Slice logo" width="420">
</p>

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

## Current limitations

- **Language support:** C++ only.
- **Execution model:** The selected function must execute entirely on one thread. Cross-thread or asynchronous handoffs are not currently supported.

## Example output

The self-contained viewer shows the complete invocation population, timeline,
latency histogram, and selected execution paths:

![Slice viewer showing the complete bimodal population](docs/assets/p0-p100.png)

Selecting the slow tail narrows the histogram, timeline, and flame graph to the
chosen percentile window:

![Slice viewer showing the p95:p100 slow tail](docs/assets/p95-p100.png)

## Ubuntu / generic Linux setup

Slice currently targets 64-bit Linux. The commands below are written for
Ubuntu 22.04 or 24.04 and should be straightforward to adapt to other
distributions.

Install the native, BPF, and Rust build dependencies:

```bash
sudo apt update
sudo apt install -y \
  build-essential clang llvm libclang-dev libbpf-dev libelf-dev zlib1g-dev \
  pkg-config cmake ninja-build linux-headers-$(uname -r) curl git
```

Install Rust 1.85 or newer with `rustup` if it is not already available:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup default stable
```

From the repository root, run the tests and build the native fixture:

```bash
cargo test --workspace
cmake -S fixtures -B build/fixtures -G Ninja
cmake --build build/fixtures
```

Build the CLI in release mode:

```bash
cargo build --release
```

Before a live capture, check that the running kernel exposes BTF and the
required scheduler tracepoint. Capture generally needs root privileges because
it uses eBPF and perf events:

```bash
sudo ./target/release/slice doctor
```

If `slice doctor` reports missing BTF or tracefs permissions, use a distro
kernel with BTF enabled and make sure `/sys/kernel/btf/vmlinux` and tracefs
are mounted. The exact kernel configuration is distribution-specific.

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

Choose the CLI path for the rest of the walkthrough:

```bash
# Ubuntu / generic Linux source build
SLICE=./target/release/slice

# NixOS packaged build (run this inside the prepared NixOS shell)
SLICE=slice
```

On Ubuntu, the first prerequisite check is:

```bash
sudo "$SLICE" doctor
```

On NixOS: 

```bash
sudo nix shell .#default --command bash --noprofile --norc -i
```

Then run the commands below using `$SLICE ...`.

### 1. Check capture prerequisites

```bash
sudo "$SLICE" doctor
```

Root is normally enough for this POC on a standard Ubuntu or NixOS kernel. The output
should report available kernel BTF and a permitted `sched_switch` tracepoint.
If it still reports a permission or kernel problem, the issue is with the
host's eBPF/tracefs configuration, not with the Nix shell.

### 2. Find the exact function signature

```bash
"$SLICE" symbols build/fixtures/bimodal_service --match handle_request
```

Copy the complete demangled signature from the output. For this fixture it is:

```text
BimodalFixture::handle_request(unsigned long)
```

### 3. Capture the live workload

Run Slice as the supervisor so it launches the fixture, attaches before the
first request, and captures until you stop it:

```bash
sudo "$SLICE" profile \
  --function 'BimodalFixture::handle_request(unsigned long)' \
  --output bimodal.slice \
  build/fixtures/bimodal_service -- --workers 6
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
"$SLICE" view bimodal.slice --output bimodal.html
```

Open the generated file in a browser:

```bash
firefox bimodal.html
```


## Useful commands

```text
slice symbols <binary> [--module <elf>] [--match <substring>]
slice profile --function '<full signature>' PROGRAM [-- ARGS...]
slice profile --pid <PID> --module <elf> --function '<full signature>'
slice view capture.slice --output profile.html [--percentile 95:100]
slice discover capture.slice
slice doctor
```
