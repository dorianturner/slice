//! Userspace reconstruction for eBPF collector events.
//!
//! The BPF transport is deliberately outside this module. Whatever source
//! produces boundary and stack events (ring buffer in production, deterministic
//! fixtures in tests) is subject to the same association and quality rules.

use std::collections::HashMap;

use slice_core::{ExecutionState, Frame, Invocation, Profile, Sample, Stack};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveInvocation {
    pub id: u64,
    pub function_id: u32,
    pub tid: u32,
    pub start_ns: u64,
    pub valid: bool,
}

#[derive(Clone, Debug)]
struct PendingSample {
    tid: u32,
    timestamp_ns: u64,
    cpu: u32,
    state: ExecutionState,
    weight_ns: u64,
    frames: Vec<Frame>,
}

#[derive(Debug)]
struct SourceInvocation {
    id: u64,
    function_id: Option<u32>,
    tid: u32,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    saw_entry: bool,
    saw_return: bool,
    valid: bool,
    samples: Vec<PendingSample>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectorEvent {
    Entry {
        /// Identity allocated by the capture transport. Deterministic callers
        /// may omit it and use the correlator's local sequence instead.
        invocation_id: Option<u64>,
        function_id: u32,
        tid: u32,
        timestamp_ns: u64,
    },
    Return {
        invocation_id: Option<u64>,
        function_id: u32,
        tid: u32,
        timestamp_ns: u64,
    },
    /// The kernel observed an overlapping selected entry or an unmatched
    /// return. Keep the active invocation associated with the thread, but
    /// invalidate it so no sample can be reported as a valid population item.
    Violation {
        tid: u32,
        timestamp_ns: u64,
    },
    Sample {
        invocation_id: Option<u64>,
        tid: u32,
        timestamp_ns: u64,
        cpu: u32,
        state: ExecutionState,
        weight_ns: u64,
        frames: Vec<Frame>,
    },
    DroppedEvents {
        count: u64,
    },
    DroppedSamples {
        count: u64,
    },
}

#[derive(Debug)]
pub struct Correlator {
    next_invocation_id: u64,
    next_stack_id: u32,
    active: HashMap<u32, ActiveInvocation>,
    stack_ids: HashMap<Vec<Frame>, u32>,
    source_invocations: HashMap<u64, SourceInvocation>,
    source_mode: bool,
    source_violations: Vec<(u32, u64)>,
}

impl Default for Correlator {
    fn default() -> Self {
        Self {
            next_invocation_id: 1,
            next_stack_id: 1,
            active: HashMap::new(),
            stack_ids: HashMap::new(),
            source_invocations: HashMap::new(),
            source_mode: false,
            source_violations: Vec::new(),
        }
    }
}

impl Correlator {
    /// Consume a collector event. A selected-function nested entry invalidates
    /// the existing invocation and increments a visible mismatch counter: it
    /// must never silently assign one sample to overlapping populations.
    pub fn push(&mut self, profile: &mut Profile, event: CollectorEvent) {
        match event {
            CollectorEvent::Entry {
                invocation_id,
                function_id,
                tid,
                timestamp_ns,
            } => {
                profile.quality.events_generated =
                    profile.quality.events_generated.saturating_add(1);
                if let Some(invocation_id) = invocation_id {
                    self.source_mode = true;
                    self.source_entry(invocation_id, function_id, tid, timestamp_ns);
                    return;
                }
                if let Some(existing) = self.active.remove(&tid) {
                    profile.invocations.push(Invocation {
                        id: existing.id,
                        function_id: existing.function_id,
                        parent_id: None,
                        tid,
                        start_ns: existing.start_ns,
                        end_ns: timestamp_ns.max(existing.start_ns),
                        complete: false,
                        valid: false,
                    });
                    profile.quality.incomplete_invocations =
                        profile.quality.incomplete_invocations.saturating_add(1);
                    profile.quality.stack_mismatches =
                        profile.quality.stack_mismatches.saturating_add(1);
                    profile.quality.suspected_async_violations =
                        profile.quality.suspected_async_violations.saturating_add(1);
                }
                let active = ActiveInvocation {
                    id: invocation_id.unwrap_or_else(|| {
                        let id = self.next_invocation_id;
                        self.next_invocation_id = self.next_invocation_id.saturating_add(1);
                        id
                    }),
                    function_id,
                    tid,
                    start_ns: timestamp_ns,
                    valid: true,
                };
                self.active.insert(tid, active);
            }
            CollectorEvent::Return {
                invocation_id,
                function_id,
                tid,
                timestamp_ns,
            } => {
                profile.quality.events_generated =
                    profile.quality.events_generated.saturating_add(1);
                if let Some(invocation_id) = invocation_id {
                    self.source_mode = true;
                    self.source_return(invocation_id, function_id, tid, timestamp_ns);
                    return;
                }
                let Some(active) = self.active.remove(&tid) else {
                    profile.quality.stack_mismatches =
                        profile.quality.stack_mismatches.saturating_add(1);
                    return;
                };
                let valid = active.valid
                    && active.function_id == function_id
                    && invocation_id.is_none_or(|id| id == active.id)
                    && timestamp_ns >= active.start_ns;
                if !valid {
                    profile.quality.stack_mismatches =
                        profile.quality.stack_mismatches.saturating_add(1);
                }
                profile.invocations.push(Invocation {
                    id: active.id,
                    function_id: active.function_id,
                    parent_id: None,
                    tid,
                    start_ns: active.start_ns,
                    end_ns: timestamp_ns,
                    complete: true,
                    valid,
                });
                if valid {
                    profile.quality.complete_invocations =
                        profile.quality.complete_invocations.saturating_add(1);
                } else {
                    profile.quality.incomplete_invocations =
                        profile.quality.incomplete_invocations.saturating_add(1);
                }
            }
            CollectorEvent::Violation { tid, timestamp_ns } => {
                profile.quality.events_generated =
                    profile.quality.events_generated.saturating_add(1);
                self.source_violations.push((tid, timestamp_ns));
                if let Some(active) = self.active.get_mut(&tid) {
                    active.valid = false;
                    profile.quality.stack_mismatches =
                        profile.quality.stack_mismatches.saturating_add(1);
                    profile.quality.suspected_async_violations =
                        profile.quality.suspected_async_violations.saturating_add(1);
                    if timestamp_ns < active.start_ns {
                        profile.quality.stack_mismatches =
                            profile.quality.stack_mismatches.saturating_add(1);
                    }
                } else {
                    profile.quality.stack_mismatches =
                        profile.quality.stack_mismatches.saturating_add(1);
                }
            }
            CollectorEvent::Sample {
                invocation_id,
                tid,
                timestamp_ns,
                cpu,
                state,
                weight_ns,
                frames,
            } => {
                profile.quality.samples_generated =
                    profile.quality.samples_generated.saturating_add(1);
                if let Some(invocation_id) = invocation_id {
                    self.source_mode = true;
                    self.source_invocations
                        .entry(invocation_id)
                        .or_insert_with(|| SourceInvocation {
                            id: invocation_id,
                            function_id: None,
                            tid,
                            start_ns: None,
                            end_ns: None,
                            saw_entry: false,
                            saw_return: false,
                            valid: true,
                            samples: Vec::new(),
                        })
                        .samples
                        .push(PendingSample {
                            tid,
                            timestamp_ns,
                            cpu,
                            state,
                            weight_ns,
                            frames,
                        });
                    return;
                }
                let Some(active) = self.active.get(&tid) else {
                    return;
                };
                if !active.valid {
                    return;
                }
                let stack_id = if let Some(stack_id) = self.stack_ids.get(&frames) {
                    *stack_id
                } else {
                    let stack_id = self.next_stack_id;
                    self.next_stack_id = self.next_stack_id.saturating_add(1);
                    self.stack_ids.insert(frames.clone(), stack_id);
                    profile.stacks.push(Stack {
                        id: stack_id,
                        frames,
                    });
                    stack_id
                };
                profile.samples.push(Sample {
                    timestamp_ns,
                    invocation_id: active.id,
                    stack_id,
                    tid,
                    cpu,
                    state,
                    weight_ns,
                });
            }
            CollectorEvent::DroppedEvents { count } => {
                profile.quality.events_dropped =
                    profile.quality.events_dropped.saturating_add(count)
            }
            CollectorEvent::DroppedSamples { count } => {
                profile.quality.samples_dropped =
                    profile.quality.samples_dropped.saturating_add(count)
            }
        }
    }

    /// Mark invocations active when capture stopped as incomplete.  They remain
    /// in the profile for diagnosis but are never included in percentile math.
    pub fn finish(mut self, profile: &mut Profile, timestamp_ns: u64) {
        if self.source_mode {
            for (tid, violation_timestamp_ns) in self.source_violations.iter().copied() {
                for invocation in self.source_invocations.values_mut() {
                    let matches = invocation.tid == tid
                        && invocation.start_ns.is_some_and(|start| {
                            start <= violation_timestamp_ns
                                && invocation
                                    .end_ns
                                    .is_none_or(|end| violation_timestamp_ns <= end)
                        });
                    if matches {
                        invocation.valid = false;
                        profile.quality.suspected_async_violations =
                            profile.quality.suspected_async_violations.saturating_add(1);
                    }
                }
            }

            let mut source_invocations = self
                .source_invocations
                .drain()
                .map(|(_, invocation)| invocation)
                .collect::<Vec<_>>();
            source_invocations.sort_by_key(|invocation| invocation.id);
            for invocation in source_invocations {
                let Some(function_id) = invocation.function_id else {
                    continue;
                };
                let Some(start_ns) = invocation.start_ns else {
                    continue;
                };
                let end_ns = invocation
                    .end_ns
                    .unwrap_or_else(|| timestamp_ns.max(start_ns));
                let complete = invocation.saw_entry && invocation.saw_return;
                let valid = invocation.valid && complete && end_ns >= start_ns;
                profile.invocations.push(Invocation {
                    id: invocation.id,
                    function_id,
                    parent_id: None,
                    tid: invocation.tid,
                    start_ns,
                    end_ns,
                    complete,
                    valid,
                });
                if valid {
                    profile.quality.complete_invocations =
                        profile.quality.complete_invocations.saturating_add(1);
                    for sample in invocation.samples {
                        if sample.timestamp_ns < start_ns || sample.timestamp_ns > end_ns {
                            profile.quality.stack_mismatches =
                                profile.quality.stack_mismatches.saturating_add(1);
                            continue;
                        }
                        self.append_sample(profile, invocation.id, sample);
                    }
                } else {
                    profile.quality.incomplete_invocations =
                        profile.quality.incomplete_invocations.saturating_add(1);
                    if complete {
                        profile.quality.stack_mismatches =
                            profile.quality.stack_mismatches.saturating_add(1);
                    }
                }
            }
        }

        for (_, active) in self.active {
            profile.invocations.push(Invocation {
                id: active.id,
                function_id: active.function_id,
                parent_id: None,
                tid: active.tid,
                start_ns: active.start_ns,
                end_ns: timestamp_ns.max(active.start_ns),
                complete: false,
                valid: false,
            });
            profile.quality.incomplete_invocations =
                profile.quality.incomplete_invocations.saturating_add(1);
        }
    }

    fn source_entry(&mut self, invocation_id: u64, function_id: u32, tid: u32, timestamp_ns: u64) {
        let invocation = self
            .source_invocations
            .entry(invocation_id)
            .or_insert_with(|| SourceInvocation {
                id: invocation_id,
                function_id: Some(function_id),
                tid,
                start_ns: Some(timestamp_ns),
                end_ns: None,
                saw_entry: false,
                saw_return: false,
                valid: true,
                samples: Vec::new(),
            });
        if invocation.saw_entry {
            invocation.valid = false;
            return;
        }
        invocation.saw_entry = true;
        invocation.function_id = Some(function_id);
        invocation.tid = tid;
        invocation.start_ns = Some(timestamp_ns);
        if invocation
            .end_ns
            .is_some_and(|end_ns| end_ns < timestamp_ns)
        {
            invocation.valid = false;
        }
    }

    fn source_return(&mut self, invocation_id: u64, function_id: u32, tid: u32, timestamp_ns: u64) {
        let invocation = self
            .source_invocations
            .entry(invocation_id)
            .or_insert_with(|| SourceInvocation {
                id: invocation_id,
                function_id: Some(function_id),
                tid,
                start_ns: None,
                end_ns: Some(timestamp_ns),
                saw_entry: false,
                saw_return: false,
                valid: true,
                samples: Vec::new(),
            });
        if invocation.saw_return {
            invocation.valid = false;
        }
        invocation.saw_return = true;
        if invocation.function_id.is_none() {
            invocation.function_id = Some(function_id);
        }
        if invocation.function_id != Some(function_id) || invocation.tid != tid {
            invocation.valid = false;
        }
        invocation.end_ns = Some(timestamp_ns);
        if invocation
            .start_ns
            .is_some_and(|start_ns| timestamp_ns < start_ns)
        {
            invocation.valid = false;
        }
    }

    fn append_sample(&mut self, profile: &mut Profile, invocation_id: u64, sample: PendingSample) {
        let stack_id = if let Some(stack_id) = self.stack_ids.get(&sample.frames) {
            *stack_id
        } else {
            let stack_id = self.next_stack_id;
            self.next_stack_id = self.next_stack_id.saturating_add(1);
            self.stack_ids.insert(sample.frames.clone(), stack_id);
            profile.stacks.push(Stack {
                id: stack_id,
                frames: sample.frames,
            });
            stack_id
        };
        profile.samples.push(Sample {
            timestamp_ns: sample.timestamp_ns,
            invocation_id,
            stack_id,
            tid: sample.tid,
            cpu: sample.cpu,
            state: sample.state,
            weight_ns: sample.weight_ns,
        });
    }
}

/// BPF ring-buffer events should use this wire contract. The C BPF program and
/// a future libbpf-rs transport map it directly into `CollectorEvent`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WireEventHeader {
    pub kind: u32,
    pub tid: u32,
    pub timestamp_ns: u64,
    pub function_id: u32,
    pub cpu: u32,
    pub weight_ns: u64,
}

#[cfg(test)]
mod tests {
    use slice_core::{CaptureQuality, Function, Metadata, Profile, Thread};

    use super::*;

    fn profile() -> Profile {
        Profile {
            format_version: 1,
            metadata: Metadata {
                captured_at_unix_ns: 0,
                command: vec![],
                kernel_release: "test".into(),
                sample_period_ns: 1,
            },
            functions: vec![],
            threads: vec![Thread {
                tid: 42,
                name: None,
            }],
            invocations: vec![],
            stacks: vec![],
            samples: vec![],
            quality: CaptureQuality::default(),
        }
    }

    fn frames() -> Vec<Frame> {
        vec![
            Frame {
                function_id: Some(1),
                label: "work".into(),
                module: None,
                address: None,
            },
            Frame {
                function_id: Some(2),
                label: "culprit".into(),
                module: None,
                address: None,
            },
        ]
    }

    #[test]
    fn correlates_one_thread_and_deduplicates_stacks() {
        let mut profile = profile();
        let mut correlator = Correlator::default();
        correlator.push(
            &mut profile,
            CollectorEvent::Entry {
                invocation_id: None,
                function_id: 1,
                tid: 42,
                timestamp_ns: 10,
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Sample {
                invocation_id: None,
                tid: 42,
                timestamp_ns: 12,
                cpu: 0,
                state: ExecutionState::OnCpu,
                weight_ns: 2,
                frames: frames(),
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Sample {
                invocation_id: None,
                tid: 42,
                timestamp_ns: 14,
                cpu: 0,
                state: ExecutionState::OnCpu,
                weight_ns: 2,
                frames: frames(),
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Return {
                invocation_id: None,
                function_id: 1,
                tid: 42,
                timestamp_ns: 20,
            },
        );
        assert_eq!(profile.invocations.len(), 1);
        assert_eq!(profile.stacks.len(), 1);
        assert_eq!(profile.samples.len(), 2);
        assert!(profile.invocations[0].valid);
    }

    #[test]
    fn nested_population_is_invalid_and_unfinished_work_is_excluded() {
        let mut profile = profile();
        let mut correlator = Correlator::default();
        correlator.push(
            &mut profile,
            CollectorEvent::Entry {
                invocation_id: None,
                function_id: 1,
                tid: 42,
                timestamp_ns: 10,
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Entry {
                invocation_id: None,
                function_id: 1,
                tid: 42,
                timestamp_ns: 11,
            },
        );
        correlator.finish(&mut profile, 20);
        assert_eq!(profile.quality.stack_mismatches, 1);
        assert_eq!(profile.quality.incomplete_invocations, 2);
        assert!(
            profile
                .invocations
                .iter()
                .all(|invocation| !invocation.complete)
        );
    }

    #[test]
    fn violation_invalidates_the_active_population_without_creating_a_fake_entry() {
        let mut profile = profile();
        let mut correlator = Correlator::default();
        correlator.push(
            &mut profile,
            CollectorEvent::Entry {
                invocation_id: None,
                function_id: 1,
                tid: 42,
                timestamp_ns: 10,
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Violation {
                tid: 42,
                timestamp_ns: 12,
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Sample {
                invocation_id: None,
                tid: 42,
                timestamp_ns: 14,
                cpu: 0,
                state: ExecutionState::OffCpu,
                weight_ns: 9,
                frames: frames(),
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Return {
                invocation_id: None,
                function_id: 1,
                tid: 42,
                timestamp_ns: 20,
            },
        );
        assert_eq!(profile.invocations.len(), 1);
        assert!(!profile.invocations[0].valid);
        assert!(profile.samples.is_empty());
        assert_eq!(profile.quality.suspected_async_violations, 1);
    }

    #[test]
    fn transport_invocation_id_keeps_a_late_sample_with_its_invocation() {
        let mut profile = profile();
        let mut correlator = Correlator::default();
        correlator.push(
            &mut profile,
            CollectorEvent::Entry {
                invocation_id: Some(101),
                function_id: 1,
                tid: 42,
                timestamp_ns: 10,
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Return {
                invocation_id: Some(101),
                function_id: 1,
                tid: 42,
                timestamp_ns: 20,
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Entry {
                invocation_id: Some(102),
                function_id: 1,
                tid: 42,
                timestamp_ns: 30,
            },
        );
        // This is the transport ordering that previously attached the sample
        // to invocation 102 and made it fall outside that invocation's bounds.
        correlator.push(
            &mut profile,
            CollectorEvent::Sample {
                invocation_id: Some(101),
                tid: 42,
                timestamp_ns: 15,
                cpu: 0,
                state: ExecutionState::OffCpu,
                weight_ns: 5,
                frames: frames(),
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Return {
                invocation_id: Some(102),
                function_id: 1,
                tid: 42,
                timestamp_ns: 40,
            },
        );
        correlator.finish(&mut profile, 50);

        assert_eq!(profile.samples.len(), 1);
        assert_eq!(profile.samples[0].invocation_id, 101);
        assert_eq!(profile.invocations.len(), 2);
        assert!(
            profile
                .invocations
                .iter()
                .all(|invocation| invocation.valid)
        );
    }

    #[test]
    fn out_of_bounds_transport_sample_is_dropped_without_poisoning_profile() {
        let mut profile = profile();
        profile.functions.push(Function {
            id: 1,
            module: "fixture".into(),
            module_build_id: None,
            address: 1,
            name: "work".into(),
            demangled_name: "work()".into(),
            source_file: None,
            line: None,
        });
        let mut correlator = Correlator::default();
        correlator.push(
            &mut profile,
            CollectorEvent::Entry {
                invocation_id: Some(103),
                function_id: 1,
                tid: 42,
                timestamp_ns: 10,
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Sample {
                invocation_id: Some(103),
                tid: 42,
                timestamp_ns: 25,
                cpu: 0,
                state: ExecutionState::OffCpu,
                weight_ns: 5,
                frames: frames(),
            },
        );
        correlator.push(
            &mut profile,
            CollectorEvent::Return {
                invocation_id: Some(103),
                function_id: 1,
                tid: 42,
                timestamp_ns: 20,
            },
        );
        correlator.finish(&mut profile, 30);

        assert!(profile.samples.is_empty());
        assert!(profile.quality.stack_mismatches > 0);
        profile.validate().unwrap();
    }
}
