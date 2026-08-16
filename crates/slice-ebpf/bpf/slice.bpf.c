// SPDX-License-Identifier: GPL-2.0-only
//
// Slice's minimal kernel-side transport. It deliberately does only three
// things: maintain the selected-function invocation keyed by TID, sample user
// stacks while that invocation is active, and send compact IDs to userspace.
// Symbolization, percentile selection, and HTML rendering never run in BPF.
#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>

enum slice_event_kind {
  SLICE_EVENT_ENTRY = 1,
  SLICE_EVENT_RETURN = 2,
  SLICE_EVENT_SAMPLE = 3,
  SLICE_EVENT_VIOLATION = 4,
  SLICE_EVENT_OFFCPU = 5,
};

struct slice_config {
  __u32 target_tgid;
  __u32 function_id;
  __u64 sample_period_ns;
};

struct active_invocation {
  __u64 id;
  __u64 start_ns;
  __u32 function_id;
  __u32 reserved;
};

struct offcpu_start {
  __u64 timestamp_ns;
};

struct trace_sched_switch {
  char prev_comm[16];
  __u32 prev_pid;
  __u32 prev_prio;
  __u64 prev_state;
  char next_comm[16];
  __u32 next_pid;
  __u32 next_prio;
};

// Fixed-width event records keep ring-buffer consumption bounded. stack_id is
// looked up in the stack_traces map by userspace; no raw stack is copied into a
// ring-buffer event.
struct slice_event {
  __u32 kind;
  __u32 tid;
  __u64 timestamp_ns;
  __u64 invocation_id;
  __s32 stack_id;
  __u32 cpu;
  __u64 weight_ns;
};

struct {
  __uint(type, BPF_MAP_TYPE_ARRAY);
  __uint(max_entries, 1);
  __type(key, __u32);
  __type(value, struct slice_config);
} config SEC(".maps");

struct {
  __uint(type, BPF_MAP_TYPE_ARRAY);
  __uint(max_entries, 1);
  __type(key, __u32);
  __type(value, __u64);
} next_invocation_id SEC(".maps");

struct {
  __uint(type, BPF_MAP_TYPE_HASH);
  __uint(max_entries, 32768);
  __type(key, __u32);
  __type(value, struct active_invocation);
} active_by_tid SEC(".maps");

struct {
  __uint(type, BPF_MAP_TYPE_HASH);
  __uint(max_entries, 32768);
  __type(key, __u32);
  __type(value, struct offcpu_start);
} offcpu_by_tid SEC(".maps");

struct {
  __uint(type, BPF_MAP_TYPE_STACK_TRACE);
  __uint(max_entries, 32768);
  __type(key, __u32);
  __type(value, __u64[127]);
} stack_traces SEC(".maps");

struct {
  __uint(type, BPF_MAP_TYPE_RINGBUF);
  __uint(max_entries, 1 << 24);
} events SEC(".maps");

struct {
  __uint(type, BPF_MAP_TYPE_ARRAY);
  __uint(max_entries, 1);
  __type(key, __u32);
  __type(value, __u64);
} dropped_events SEC(".maps");

struct {
  __uint(type, BPF_MAP_TYPE_ARRAY);
  __uint(max_entries, 1);
  __type(key, __u32);
  __type(value, __u64);
} dropped_samples SEC(".maps");

static __always_inline struct slice_config *slice_config_for_current(void) {
  __u32 zero = 0;
  struct slice_config *cfg = bpf_map_lookup_elem(&config, &zero);
  if (!cfg)
    return 0;
  if ((__u32)(bpf_get_current_pid_tgid() >> 32) != cfg->target_tgid)
    return 0;
  return cfg;
}

static __always_inline void emit(__u32 kind, __u32 tid, __u64 now,
                                 __u64 invocation_id, __s32 stack_id,
                                 __u64 weight_ns) {
  struct slice_event *event = bpf_ringbuf_reserve(&events, sizeof(*event), 0);
  if (!event) {
    __u32 zero = 0;
    __u64 *dropped = bpf_map_lookup_elem(&dropped_events, &zero);
    if (dropped)
      __sync_fetch_and_add(dropped, 1);
    return;
  }
  event->kind = kind;
  event->tid = tid;
  event->timestamp_ns = now;
  event->invocation_id = invocation_id;
  event->stack_id = stack_id;
  event->cpu = bpf_get_smp_processor_id();
  event->weight_ns = weight_ns;
  bpf_ringbuf_submit(event, 0);
}

SEC("uprobe")
int slice_entry(struct pt_regs *ctx) {
  struct slice_config *cfg = slice_config_for_current();
  if (!cfg)
    return 0;
  __u32 tid = (__u32)bpf_get_current_pid_tgid();
  __u64 now = bpf_ktime_get_ns();
  if (bpf_map_lookup_elem(&active_by_tid, &tid)) {
    // The POC's selected population may not overlap on a thread. Userspace
    // turns this into a visible invalid-invocation quality warning.
    emit(SLICE_EVENT_VIOLATION, tid, now, 0, -1, 0);
    return 0;
  }
  __u32 zero = 0;
  __u64 *sequence = bpf_map_lookup_elem(&next_invocation_id, &zero);
  if (!sequence)
    return 0;
  struct active_invocation active = {
      .id = __sync_fetch_and_add(sequence, 1) + 1,
      .start_ns = now,
      .function_id = cfg->function_id,
  };
  if (bpf_map_update_elem(&active_by_tid, &tid, &active, BPF_ANY))
    return 0;
  emit(SLICE_EVENT_ENTRY, tid, now, active.id, -1, 0);
  return 0;
}

SEC("uretprobe")
int slice_return(struct pt_regs *ctx) {
  if (!slice_config_for_current())
    return 0;
  __u32 tid = (__u32)bpf_get_current_pid_tgid();
  struct active_invocation *active = bpf_map_lookup_elem(&active_by_tid, &tid);
  if (!active) {
    emit(SLICE_EVENT_VIOLATION, tid, bpf_ktime_get_ns(), 0, -1, 0);
    return 0;
  }
  emit(SLICE_EVENT_RETURN, tid, bpf_ktime_get_ns(), active->id, -1, 0);
  bpf_map_delete_elem(&active_by_tid, &tid);
  bpf_map_delete_elem(&offcpu_by_tid, &tid);
  return 0;
}

SEC("perf_event")
int slice_sample(struct bpf_perf_event_data *ctx) {
  if (!slice_config_for_current())
    return 0;
  __u32 tid = (__u32)bpf_get_current_pid_tgid();
  struct active_invocation *active = bpf_map_lookup_elem(&active_by_tid, &tid);
  if (!active)
    return 0;
  __s32 stack_id = bpf_get_stackid(ctx, &stack_traces, BPF_F_USER_STACK);
  if (stack_id < 0) {
    __u32 zero = 0;
    __u64 *dropped = bpf_map_lookup_elem(&dropped_samples, &zero);
    if (dropped)
      __sync_fetch_and_add(dropped, 1);
    return 0;
  }
  struct slice_config *cfg = slice_config_for_current();
  emit(SLICE_EVENT_SAMPLE, tid, bpf_ktime_get_ns(), active->id, stack_id,
       cfg ? cfg->sample_period_ns : 0);
  return 0;
}

SEC("tracepoint/sched/sched_switch")
int slice_sched_switch(struct trace_sched_switch *ctx) {
  __u64 now = bpf_ktime_get_ns();
  __u32 prev_tid = ctx->prev_pid;
  struct slice_config *cfg = slice_config_for_current();
  if (cfg && bpf_map_lookup_elem(&active_by_tid, &prev_tid)) {
    struct offcpu_start start = {.timestamp_ns = now};
    bpf_map_update_elem(&offcpu_by_tid, &prev_tid, &start, BPF_ANY);
  }

  __u32 next_tid = ctx->next_pid;
  struct offcpu_start *start = bpf_map_lookup_elem(&offcpu_by_tid, &next_tid);
  struct active_invocation *active = bpf_map_lookup_elem(&active_by_tid, &next_tid);
  if (!start || !active) {
    if (!active)
      bpf_map_delete_elem(&offcpu_by_tid, &next_tid);
    return 0;
  }
  __u64 weight = now > start->timestamp_ns ? now - start->timestamp_ns : 0;
  emit(SLICE_EVENT_OFFCPU, next_tid, now, active->id, -1, weight);
  bpf_map_delete_elem(&offcpu_by_tid, &next_tid);
  return 0;
}

char LICENSE[] SEC("license") = "GPL";
