# eBPF transport

`bpf/slice.bpf.c` is the low-level half of Slice's collector contract. It is
compiled with libbpf headers and attached by the privileged userspace runner:

- `slice_entry` / `slice_return` are attached as a process-scoped
  uprobe-multi/uretprobe-multi pair to the exact ELF file offset selected by
  `slice symbols` for the target module.
- `slice_sample` is attached to a per-CPU software `perf_event` at 999 Hz.
- `slice_sched_switch` bridges tracepoint and BPF-helper thread identities,
  records a blocked user stack at switch-out, and emits its weighted interval
  when that thread is switched back in. This explicit translation is required
  on hosts such as WSL where those identity sources need not use the same PID
  namespace.
- Every boundary and sample event carries the BPF invocation ID. The Rust
  adapter replays buffered events by kernel timestamp before correlation, so
  ring-buffer delivery order and CPU migration cannot attach a sample to the
  next invocation on the same thread.
- Stack IDs are deduplicated by `BPF_MAP_TYPE_STACK_TRACE`; userspace reads the
  resulting IP arrays, maps them through `/proc/<pid>/maps`, and feeds the
  tested `slice-collector::Correlator`.

The Rust transport enables the matching vendored libbpf build supplied by
`libbpf-rs`; this keeps the generated skeleton ABI aligned with the runtime
library instead of depending on an unrelated system `libbpf.so` version.

The source intentionally rejects a nested selected-function entry. This is the
POC contract: a selected population must be synchronous and non-overlapping on
each OS thread. A dropped ring-buffer event also becomes profile-quality data;
it is never concealed as a valid invocation.

Both the existing-PID and launch paths use Linux 6.6+ uprobe-multi links scoped
to the target process's shared address space. This covers existing and future
worker threads without carrying a perf-event pathname pointer through
`clone()`. Only those scoped probes populate `active_by_tid`; periodic sampling
and scheduler attribution require an active entry instead of comparing PIDs
from potentially different namespaces. Transport diagnostics distinguish
attachment, event acceptance, identity, and userspace delivery failures.

Run `bash tests/bpf-build.sh` from the repository development environment to
compile the BPF object. Nix is optional for this local check. Loading/attaching
it needs root or the capabilities reported by `slice doctor`.
