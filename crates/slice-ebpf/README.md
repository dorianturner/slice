# eBPF transport

`bpf/slice.bpf.c` is the low-level half of Slice's collector contract. It is
compiled with libbpf headers and attached by the privileged userspace runner:

- `slice_entry` / `slice_return` are attached as a uprobe/uretprobe pair to
  the exact ELF file offset selected by `slice symbols`.
- `slice_sample` is attached to a per-CPU software `perf_event` at 999 Hz.
- Stack IDs are deduplicated by `BPF_MAP_TYPE_STACK_TRACE`; userspace reads the
  resulting IP arrays, maps them through `/proc/<pid>/maps`, and feeds the
  tested `slice-collector::Correlator`.

The source intentionally rejects a nested selected-function entry. This is the
POC contract: a selected population must be synchronous and non-overlapping on
each OS thread. A dropped ring-buffer event also becomes profile-quality data;
it is never concealed as a valid invocation.

Run `bash tests/bpf-build.sh` inside `nix develop` to compile the BPF object.
Loading/attaching it needs root or the capabilities reported by `slice doctor`.

