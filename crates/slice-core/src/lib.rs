//! The execution-aware profile format and deterministic percentile query engine.
//!
//! This crate intentionally has no eBPF, filesystem, or HTML dependencies.  It
//! is the semantic contract shared by capture, tests, command-line export, and
//! the interactive renderer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Cursor;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const FILE_MAGIC: &[u8] = b"SLICE\0\x01\n";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub format_version: u16,
    pub metadata: Metadata,
    pub functions: Vec<Function>,
    pub threads: Vec<Thread>,
    pub invocations: Vec<Invocation>,
    pub stacks: Vec<Stack>,
    pub samples: Vec<Sample>,
    pub quality: CaptureQuality,
}

impl Profile {
    pub fn write_to_path(&self, path: impl AsRef<std::path::Path>) -> Result<(), ProfileError> {
        std::fs::write(path, self.to_bytes()?)?;
        Ok(())
    }

    pub fn read_from_path(path: impl AsRef<std::path::Path>) -> Result<Self, ProfileError> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ProfileError> {
        let json = serde_json::to_vec(self)?;
        let compressed = zstd::stream::encode_all(Cursor::new(json), 9)?;
        let mut output = Vec::with_capacity(FILE_MAGIC.len() + compressed.len());
        output.extend_from_slice(FILE_MAGIC);
        output.extend_from_slice(&compressed);
        Ok(output)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProfileError> {
        let payload = bytes
            .strip_prefix(FILE_MAGIC)
            .ok_or(ProfileError::InvalidMagic)?;
        let json = zstd::stream::decode_all(Cursor::new(payload))?;
        let profile: Self = serde_json::from_slice(&json)?;
        if profile.format_version != 1 {
            return Err(ProfileError::UnsupportedVersion(profile.format_version));
        }
        profile.validate()?;
        Ok(profile)
    }

    /// Validate all cross-references and invariants required by consumers.
    ///
    /// Deserialization validates the file envelope and format version; this
    /// method validates the semantic graph inside that envelope. Keeping the
    /// check here lets the CLI, live-capture gate, and future integrations use
    /// exactly the same contract.
    pub fn validate(&self) -> Result<(), ProfileValidationError> {
        if self.format_version != 1 {
            return Err(ProfileValidationError::UnsupportedVersion(
                self.format_version,
            ));
        }

        let function_ids = unique_u32_ids(
            self.functions.iter().map(|function| function.id),
            "function",
        )?;
        let thread_ids = unique_u32_ids(self.threads.iter().map(|thread| thread.tid), "thread")?;
        let invocation_ids = unique_u64_ids(
            self.invocations.iter().map(|invocation| invocation.id),
            "invocation",
        )?;
        let stack_ids = unique_u32_ids(self.stacks.iter().map(|stack| stack.id), "stack")?;
        let parents = self
            .invocations
            .iter()
            .map(|invocation| (invocation.id, invocation.parent_id))
            .collect::<HashMap<_, _>>();

        for invocation in &self.invocations {
            if !function_ids.contains(&invocation.function_id) {
                return Err(ProfileValidationError::UnknownFunction {
                    record: "invocation",
                    id: invocation.function_id,
                });
            }
            if !thread_ids.contains(&invocation.tid) {
                return Err(ProfileValidationError::UnknownThread(invocation.tid));
            }
            if invocation.end_ns < invocation.start_ns {
                return Err(ProfileValidationError::InvalidInvocationBounds(
                    invocation.id,
                ));
            }
            if let Some(parent_id) = invocation.parent_id {
                if !invocation_ids.contains(&parent_id) {
                    return Err(ProfileValidationError::UnknownInvocation(parent_id));
                }
            }
            let mut ancestry = BTreeSet::new();
            let mut current = Some(invocation.id);
            while let Some(id) = current {
                if !ancestry.insert(id) {
                    return Err(ProfileValidationError::CyclicInvocationParent(
                        invocation.id,
                    ));
                }
                current = parents.get(&id).copied().flatten();
            }
        }

        for stack in &self.stacks {
            for frame in &stack.frames {
                if let Some(function_id) = frame.function_id {
                    if !function_ids.contains(&function_id) {
                        return Err(ProfileValidationError::UnknownFunction {
                            record: "frame",
                            id: function_id,
                        });
                    }
                }
            }
        }

        for sample in &self.samples {
            if !invocation_ids.contains(&sample.invocation_id) {
                return Err(ProfileValidationError::UnknownInvocation(
                    sample.invocation_id,
                ));
            }
            if !stack_ids.contains(&sample.stack_id) {
                return Err(ProfileValidationError::UnknownStack(sample.stack_id));
            }
            let invocation = self
                .invocations
                .iter()
                .find(|invocation| invocation.id == sample.invocation_id)
                .expect("sample invocation was checked above");
            if sample.tid != invocation.tid {
                return Err(ProfileValidationError::SampleThreadMismatch {
                    sample_invocation: sample.invocation_id,
                    sample_tid: sample.tid,
                    invocation_tid: invocation.tid,
                });
            }
            if sample.timestamp_ns < invocation.start_ns || sample.timestamp_ns > invocation.end_ns
            {
                return Err(ProfileValidationError::SampleOutsideInvocation(
                    sample.invocation_id,
                ));
            }
        }

        let complete = self
            .invocations
            .iter()
            .filter(|invocation| invocation.complete && invocation.valid)
            .count() as u64;
        let incomplete = self
            .invocations
            .iter()
            .filter(|invocation| !invocation.complete || !invocation.valid)
            .count() as u64;
        if self.quality.complete_invocations > complete
            || self.quality.incomplete_invocations > incomplete
            || self.quality.samples_generated < self.samples.len() as u64
        {
            return Err(ProfileValidationError::QualityCountersInconsistent);
        }
        Ok(())
    }

    pub fn capture_bounds(&self) -> Option<TimeRange> {
        let mut invocations = self.invocations.iter();
        let first = invocations.next()?;
        let mut from_ns = first.start_ns;
        let mut to_ns = first.end_ns;
        for invocation in invocations {
            from_ns = from_ns.min(invocation.start_ns);
            to_ns = to_ns.max(invocation.end_ns);
        }
        Some(TimeRange { from_ns, to_ns })
    }
}

fn unique_u32_ids<I>(ids: I, kind: &'static str) -> Result<BTreeSet<u32>, ProfileValidationError>
where
    I: IntoIterator<Item = u32>,
{
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(ProfileValidationError::DuplicateId {
                kind,
                id: u64::from(id),
            });
        }
    }
    Ok(seen)
}

fn unique_u64_ids<I>(ids: I, kind: &'static str) -> Result<BTreeSet<u64>, ProfileValidationError>
where
    I: IntoIterator<Item = u64>,
{
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(ProfileValidationError::DuplicateId { kind, id });
        }
    }
    Ok(seen)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub captured_at_unix_ns: u64,
    pub command: Vec<String>,
    pub kernel_release: String,
    pub sample_period_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub id: u32,
    pub module: String,
    pub module_build_id: Option<String>,
    pub address: u64,
    pub name: String,
    pub demangled_name: String,
    pub source_file: Option<String>,
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub tid: u32,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Invocation {
    pub id: u64,
    pub function_id: u32,
    pub parent_id: Option<u64>,
    pub tid: u32,
    pub start_ns: u64,
    pub end_ns: u64,
    pub complete: bool,
    pub valid: bool,
}

impl Invocation {
    pub fn duration_ns(&self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    pub id: u32,
    /// Frames are ordered root-to-leaf, the form required by the flame tree.
    pub frames: Vec<Frame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct Frame {
    pub function_id: Option<u32>,
    pub label: String,
    pub module: Option<String>,
    pub address: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    OnCpu,
    OffCpu,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub timestamp_ns: u64,
    pub invocation_id: u64,
    pub stack_id: u32,
    pub tid: u32,
    pub cpu: u32,
    pub state: ExecutionState,
    /// On-CPU samples use the configured sampling period; off-CPU samples use
    /// the exact deschedule interval.
    pub weight_ns: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaptureQuality {
    pub events_generated: u64,
    pub events_dropped: u64,
    pub samples_generated: u64,
    pub samples_dropped: u64,
    pub complete_invocations: u64,
    pub incomplete_invocations: u64,
    pub stack_mismatches: u64,
    pub suspected_async_violations: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Wall,
    Cpu,
    OffCpu,
}

impl Metric {
    pub fn includes(self, state: ExecutionState) -> bool {
        matches!(
            (self, state),
            (Self::Wall, _)
                | (Self::Cpu, ExecutionState::OnCpu)
                | (Self::OffCpu, ExecutionState::OffCpu)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    pub from_ns: u64,
    pub to_ns: u64,
}

impl TimeRange {
    pub fn contains_start(self, timestamp_ns: u64) -> bool {
        timestamp_ns >= self.from_ns && timestamp_ns < self.to_ns
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub function_id: u32,
    pub threads: Option<BTreeSet<u32>>,
    pub time: Option<TimeRange>,
    pub percentile: PercentileRange,
    pub metric: Metric,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PercentileRange {
    pub low: u8,
    pub high: u8,
}

impl PercentileRange {
    pub const ALL: Self = Self { low: 0, high: 100 };

    fn validate(self) -> Result<(), QueryError> {
        if self.low >= self.high || self.high > 100 {
            return Err(QueryError::InvalidPercentileRange(self));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub selected_invocation_ids: Vec<u64>,
    pub available_invocation_count: usize,
    pub latency_min_ns: Option<u64>,
    pub latency_max_ns: Option<u64>,
    pub percentile_low_value_ns: Option<f64>,
    pub percentile_high_value_ns: Option<f64>,
    pub sampled_cpu_ns: u64,
    pub off_cpu_ns: u64,
    pub root: FlameNode,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FlameNode {
    pub name: String,
    pub value_ns: u64,
    pub children: Vec<FlameNode>,
}

impl FlameNode {
    fn empty() -> Self {
        Self {
            name: "root".to_owned(),
            value_ns: 0,
            children: Vec::new(),
        }
    }
}

#[derive(Default)]
struct TreeBuilder {
    value_ns: u64,
    children: BTreeMap<String, TreeBuilder>,
}

impl TreeBuilder {
    fn insert(&mut self, frames: &[Frame], weight_ns: u64) {
        self.value_ns = self.value_ns.saturating_add(weight_ns);
        if let Some((frame, rest)) = frames.split_first() {
            self.children
                .entry(frame.label.clone())
                .or_default()
                .insert(rest, weight_ns);
        }
    }

    fn finish(self, name: String) -> FlameNode {
        FlameNode {
            name,
            value_ns: self.value_ns,
            children: self
                .children
                .into_iter()
                .map(|(name, child)| child.finish(name))
                .collect(),
        }
    }
}

pub fn execute_query(profile: &Profile, query: &Query) -> Result<QueryResult, QueryError> {
    query.percentile.validate()?;
    if let Some(time) = query.time {
        if time.from_ns >= time.to_ns {
            return Err(QueryError::InvalidTimeRange(time));
        }
    }

    let mut candidates = profile
        .invocations
        .iter()
        .filter(|invocation| invocation.function_id == query.function_id)
        .filter(|invocation| invocation.complete && invocation.valid)
        .filter(|invocation| {
            query
                .threads
                .as_ref()
                .is_none_or(|threads| threads.contains(&invocation.tid))
        })
        .filter(|invocation| {
            query
                .time
                .is_none_or(|time| time.contains_start(invocation.start_ns))
        })
        .collect::<Vec<_>>();

    candidates.sort_by_key(|invocation| (invocation.duration_ns(), invocation.id));
    let available_invocation_count = candidates.len();
    let durations = candidates
        .iter()
        .map(|invocation| invocation.duration_ns())
        .collect::<Vec<_>>();
    let (start, end) = rank_bounds(candidates.len(), query.percentile);
    let selected = &candidates[start..end];
    let selected_ids = selected
        .iter()
        .map(|invocation| invocation.id)
        .collect::<BTreeSet<_>>();

    let stacks = profile
        .stacks
        .iter()
        .map(|stack| (stack.id, stack))
        .collect::<HashMap<_, _>>();
    let selected_name = profile
        .functions
        .iter()
        .find(|function| function.id == query.function_id)
        .map(|function| function.demangled_name.as_str());
    let mut tree = TreeBuilder::default();
    let mut sampled_cpu_ns = 0_u64;
    let mut off_cpu_ns = 0_u64;
    for sample in &profile.samples {
        if !selected_ids.contains(&sample.invocation_id) || !query.metric.includes(sample.state) {
            continue;
        }
        let stack = stacks
            .get(&sample.stack_id)
            .ok_or(QueryError::UnknownStack(sample.stack_id))?;
        let Some(start_frame) = stack.frames.iter().position(|frame| {
            frame.function_id == Some(query.function_id)
                || selected_name.is_some_and(|name| frame.label == name)
        }) else {
            continue;
        };
        tree.insert(&stack.frames[start_frame..], sample.weight_ns);
        match sample.state {
            ExecutionState::OnCpu => {
                sampled_cpu_ns = sampled_cpu_ns.saturating_add(sample.weight_ns);
            }
            ExecutionState::OffCpu => off_cpu_ns = off_cpu_ns.saturating_add(sample.weight_ns),
        }
    }

    Ok(QueryResult {
        selected_invocation_ids: selected.iter().map(|invocation| invocation.id).collect(),
        available_invocation_count,
        latency_min_ns: selected
            .iter()
            .map(|invocation| invocation.duration_ns())
            .min(),
        latency_max_ns: selected
            .iter()
            .map(|invocation| invocation.duration_ns())
            .max(),
        percentile_low_value_ns: percentile_r7(&durations, f64::from(query.percentile.low)),
        percentile_high_value_ns: percentile_r7(&durations, f64::from(query.percentile.high)),
        sampled_cpu_ns,
        off_cpu_ns,
        root: if selected_ids.is_empty() {
            FlameNode::empty()
        } else {
            tree.finish("root".to_owned())
        },
    })
}

/// Return a deterministic rank range. Percentile *display* uses R-7 interpolation,
/// while population selection uses ordinal ranks so p99:p100 means exactly the
/// slowest one percent when the count is divisible by 100.
pub fn rank_bounds(count: usize, range: PercentileRange) -> (usize, usize) {
    let start = (count
        .saturating_mul(usize::from(range.low))
        .saturating_add(99))
        / 100;
    let end = (count
        .saturating_mul(usize::from(range.high))
        .saturating_add(99))
        / 100;
    (start.min(count), end.min(count))
}

pub fn percentile_r7(sorted_durations: &[u64], percentile: f64) -> Option<f64> {
    let first = *sorted_durations.first()? as f64;
    if sorted_durations.len() == 1 {
        return Some(first);
    }
    let bounded = percentile.clamp(0.0, 100.0) / 100.0;
    let rank = bounded * (sorted_durations.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let fraction = rank - lower as f64;
    let low = sorted_durations[lower] as f64;
    let high = sorted_durations[upper] as f64;
    Some(low + (high - low) * fraction)
}

/// A deterministic, compact profile matching the native tail-divergence fixture.
/// Aggregate sampled time is split exactly 50/50 between the fast and slow paths;
/// p99:p100 selects only the slow path.
pub fn tail_divergence_profile() -> Profile {
    let work = 1;
    let fast = 2;
    let slow = 3;
    let fast_stack = 10;
    let slow_stack = 11;
    let mut invocations = Vec::new();
    let mut samples = Vec::new();
    let mut start_ns = 0_u64;
    for id in 1..=100_u64 {
        let is_tail = id == 100;
        let duration_ns = if is_tail { 297_000_000 } else { 3_000_000 };
        let stack_id = if is_tail { slow_stack } else { fast_stack };
        invocations.push(Invocation {
            id,
            function_id: work,
            parent_id: None,
            tid: 4242,
            start_ns,
            end_ns: start_ns + duration_ns,
            complete: true,
            valid: true,
        });
        samples.push(Sample {
            timestamp_ns: start_ns + duration_ns / 2,
            invocation_id: id,
            stack_id,
            tid: 4242,
            cpu: 0,
            state: ExecutionState::OnCpu,
            weight_ns: duration_ns,
        });
        start_ns += duration_ns;
    }

    Profile {
        format_version: 1,
        metadata: Metadata {
            captured_at_unix_ns: 0,
            command: vec!["fixtures/tail_divergence".to_owned()],
            kernel_release: "synthetic-test-profile".to_owned(),
            sample_period_ns: 1_000_000,
        },
        functions: vec![
            Function {
                id: work,
                module: "fixtures/tail_divergence".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401000,
                name: "_ZN12SliceFixture4workEj".to_owned(),
                demangled_name: "SliceFixture::work(unsigned int)".to_owned(),
                source_file: Some("fixtures/tail_divergence.cpp".to_owned()),
                line: Some(42),
            },
            Function {
                id: fast,
                module: "fixtures/tail_divergence".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401100,
                name: "_ZN12SliceFixture16fast_aggregate_aEv".to_owned(),
                demangled_name: "SliceFixture::fast_aggregate_a()".to_owned(),
                source_file: Some("fixtures/tail_divergence.cpp".to_owned()),
                line: Some(21),
            },
            Function {
                id: slow,
                module: "fixtures/tail_divergence".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401200,
                name: "_ZN12SliceFixture11slow_tail_bEv".to_owned(),
                demangled_name: "SliceFixture::slow_tail_b()".to_owned(),
                source_file: Some("fixtures/tail_divergence.cpp".to_owned()),
                line: Some(28),
            },
        ],
        threads: vec![Thread {
            tid: 4242,
            name: Some("fixture-worker".to_owned()),
        }],
        invocations,
        stacks: vec![
            Stack {
                id: fast_stack,
                frames: vec![
                    frame(work, "SliceFixture::work(unsigned int)"),
                    frame(fast, "SliceFixture::fast_aggregate_a()"),
                ],
            },
            Stack {
                id: slow_stack,
                frames: vec![
                    frame(work, "SliceFixture::work(unsigned int)"),
                    frame(slow, "SliceFixture::slow_tail_b()"),
                ],
            },
        ],
        samples,
        quality: CaptureQuality {
            events_generated: 200,
            samples_generated: 100,
            complete_invocations: 100,
            ..CaptureQuality::default()
        },
    }
}

/// A deterministic multi-threaded profile with a visible 70/30 latency split.
/// Each mode uses the same deterministic, normal-shaped set of histogram bins
/// centered at 10ms or 20ms. The fast mode spans 2ms..18ms and the slow mode
/// spans 12ms..28ms, leaving a visible overlap around the middle. The slow
/// population contains both CPU and off-CPU work so the viewer can demonstrate
/// all three metrics without requiring a privileged capture.
pub fn bimodal_profile() -> Profile {
    let work = 1;
    let fast = 2;
    let slow = 3;
    let wait = 4;
    let fast_stack = 20;
    let slow_cpu_stack = 21;
    let slow_wait_stack = 22;
    let mut invocations = Vec::new();
    let mut samples = Vec::new();
    let mut invocation_id = 1_u64;
    const NORMAL_BINS_NS: [(i64, u64); 17] = [
        (-8_000_000, 1),
        (-7_000_000, 1),
        (-6_000_000, 2),
        (-5_000_000, 3),
        (-4_000_000, 5),
        (-3_000_000, 7),
        (-2_000_000, 9),
        (-1_000_000, 11),
        (0, 12),
        (1_000_000, 11),
        (2_000_000, 9),
        (3_000_000, 7),
        (4_000_000, 5),
        (5_000_000, 3),
        (6_000_000, 2),
        (7_000_000, 1),
        (8_000_000, 1),
    ];

    for (thread_index, tid) in [
        7101_u32, 7102, 7103, 7104, 7105, 7106, 7107, 7108, 7109, 7110,
    ]
    .into_iter()
    .enumerate()
    {
        let mut start_ns = thread_index as u64 * 100_000;
        for &(mode_offset_ns, bin_repetitions) in &NORMAL_BINS_NS {
            for _ in 0..bin_repetitions {
                for mode_ordinal in 0..10_u64 {
                    let is_slow = mode_ordinal >= 7;
                    let mode_center_ns = if is_slow { 20_000_000 } else { 10_000_000 };
                    let duration_ns = (mode_center_ns as i64 + mode_offset_ns) as u64;
                    let cpu_ns = if is_slow { 5_000_000 } else { duration_ns };
                    let off_cpu_ns = duration_ns.saturating_sub(cpu_ns);
                    let invocation = Invocation {
                        id: invocation_id,
                        function_id: work,
                        parent_id: None,
                        tid,
                        start_ns,
                        end_ns: start_ns + duration_ns,
                        complete: true,
                        valid: true,
                    };
                    invocations.push(invocation);
                    let (stack_id, timestamp_ns) = if is_slow {
                        (slow_cpu_stack, start_ns + off_cpu_ns + cpu_ns / 2)
                    } else {
                        (fast_stack, start_ns + cpu_ns / 2)
                    };
                    samples.push(Sample {
                        timestamp_ns,
                        invocation_id,
                        stack_id,
                        tid,
                        cpu: thread_index as u32,
                        state: ExecutionState::OnCpu,
                        weight_ns: cpu_ns,
                    });
                    if is_slow {
                        samples.push(Sample {
                            timestamp_ns: start_ns + off_cpu_ns / 2,
                            invocation_id,
                            stack_id: slow_wait_stack,
                            tid,
                            cpu: thread_index as u32,
                            state: ExecutionState::OffCpu,
                            weight_ns: off_cpu_ns,
                        });
                    }
                    invocation_id += 1;
                    start_ns += duration_ns + 1_000_000;
                }
            }
        }
    }

    Profile {
        format_version: 1,
        metadata: Metadata {
            captured_at_unix_ns: 0,
            command: vec!["fixtures/bimodal_service".to_owned()],
            kernel_release: "synthetic-bimodal-profile".to_owned(),
            sample_period_ns: 1_001_001,
        },
        functions: vec![
            Function {
                id: work,
                module: "fixtures/bimodal_service".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401000,
                name: "_ZN14BimodalFixture14handle_requestEm".to_owned(),
                demangled_name: "BimodalFixture::handle_request(unsigned long)".to_owned(),
                source_file: Some("fixtures/bimodal_service.cpp".to_owned()),
                line: Some(54),
            },
            Function {
                id: fast,
                module: "fixtures/bimodal_service".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401100,
                name: "_ZN14BimodalFixture9fast_pathEv".to_owned(),
                demangled_name: "BimodalFixture::fast_path()".to_owned(),
                source_file: Some("fixtures/bimodal_service.cpp".to_owned()),
                line: Some(35),
            },
            Function {
                id: slow,
                module: "fixtures/bimodal_service".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401200,
                name: "_ZN14BimodalFixture9slow_pathEv".to_owned(),
                demangled_name: "BimodalFixture::slow_path()".to_owned(),
                source_file: Some("fixtures/bimodal_service.cpp".to_owned()),
                line: Some(42),
            },
            Function {
                id: wait,
                module: "fixtures/bimodal_service".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401300,
                name: "nanosleep".to_owned(),
                demangled_name: "std::this_thread::sleep_for(...)".to_owned(),
                source_file: None,
                line: None,
            },
        ],
        threads: [
            7101_u32, 7102, 7103, 7104, 7105, 7106, 7107, 7108, 7109, 7110,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, tid)| Thread {
            tid,
            name: Some(format!("slice-worker-{}", index + 1)),
        })
        .collect(),
        invocations,
        stacks: vec![
            Stack {
                id: fast_stack,
                frames: vec![
                    frame_with_module(
                        work,
                        "BimodalFixture::handle_request(unsigned long)",
                        "fixtures/bimodal_service",
                    ),
                    frame_with_module(
                        fast,
                        "BimodalFixture::fast_path()",
                        "fixtures/bimodal_service",
                    ),
                ],
            },
            Stack {
                id: slow_cpu_stack,
                frames: vec![
                    frame_with_module(
                        work,
                        "BimodalFixture::handle_request(unsigned long)",
                        "fixtures/bimodal_service",
                    ),
                    frame_with_module(
                        slow,
                        "BimodalFixture::slow_path()",
                        "fixtures/bimodal_service",
                    ),
                ],
            },
            Stack {
                id: slow_wait_stack,
                frames: vec![
                    frame_with_module(
                        work,
                        "BimodalFixture::handle_request(unsigned long)",
                        "fixtures/bimodal_service",
                    ),
                    frame_with_module(
                        slow,
                        "BimodalFixture::slow_path()",
                        "fixtures/bimodal_service",
                    ),
                    frame_with_module(wait, "std::this_thread::sleep_for(...)", "libstdc++.so"),
                ],
            },
        ],
        samples,
        quality: CaptureQuality {
            events_generated: 18_000,
            samples_generated: 11_700,
            complete_invocations: 9_000,
            ..CaptureQuality::default()
        },
    }
}

/// A deterministic two-thread profile for comparing CPU, off-CPU, and wall
/// metrics. Each invocation spends 2ms on CPU and the rest waiting, with
/// alternating 20ms and 40ms wall durations like the native `off_cpu_wait`
/// workload.
pub fn off_cpu_profile() -> Profile {
    let work = 1;
    let cpu = 2;
    let wait = 3;
    let on_cpu_stack = 30;
    let off_cpu_stack = 31;
    let tids = [7201_u32, 7202_u32];
    let mut invocations = Vec::new();
    let mut samples = Vec::new();
    let mut invocation_id = 1_u64;

    for (thread_index, tid) in tids.into_iter().enumerate() {
        let mut start_ns = thread_index as u64 * 500_000;
        for call_index in 0..8_u64 {
            let duration_ns = if call_index % 2 == 0 {
                20_000_000
            } else {
                40_000_000
            };
            let cpu_ns = 2_000_000;
            let off_cpu_ns = duration_ns - cpu_ns;
            invocations.push(Invocation {
                id: invocation_id,
                function_id: work,
                parent_id: None,
                tid,
                start_ns,
                end_ns: start_ns + duration_ns,
                complete: true,
                valid: true,
            });
            samples.push(Sample {
                timestamp_ns: start_ns + cpu_ns / 2,
                invocation_id,
                stack_id: on_cpu_stack,
                tid,
                cpu: thread_index as u32,
                state: ExecutionState::OnCpu,
                weight_ns: cpu_ns,
            });
            samples.push(Sample {
                timestamp_ns: start_ns + cpu_ns + off_cpu_ns / 2,
                invocation_id,
                stack_id: off_cpu_stack,
                tid,
                cpu: thread_index as u32,
                state: ExecutionState::OffCpu,
                weight_ns: off_cpu_ns,
            });
            invocation_id += 1;
            start_ns += duration_ns + 2_000_000;
        }
    }

    Profile {
        format_version: 1,
        metadata: Metadata {
            captured_at_unix_ns: 0,
            command: vec!["fixtures/off_cpu_wait".to_owned()],
            kernel_release: "synthetic-off-cpu-profile".to_owned(),
            sample_period_ns: 1_000_000,
        },
        functions: vec![
            Function {
                id: work,
                module: "fixtures/off_cpu_wait".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401000,
                name: "_ZN12SliceFixture11sleep_workEj".to_owned(),
                demangled_name: "SliceFixture::sleep_work(unsigned int)".to_owned(),
                source_file: Some("fixtures/off_cpu_wait.cpp".to_owned()),
                line: Some(17),
            },
            Function {
                id: cpu,
                module: "fixtures/off_cpu_wait".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0x401100,
                name: "_ZN12SliceFixture11cpu_preludeEv".to_owned(),
                demangled_name: "SliceFixture::cpu_prelude()".to_owned(),
                source_file: Some("fixtures/off_cpu_wait.cpp".to_owned()),
                line: Some(10),
            },
            Function {
                id: wait,
                module: "libstdc++.so".to_owned(),
                module_build_id: Some("synthetic".to_owned()),
                address: 0,
                name: "nanosleep".to_owned(),
                demangled_name: "std::this_thread::sleep_for(...)".to_owned(),
                source_file: None,
                line: None,
            },
        ],
        threads: tids
            .into_iter()
            .enumerate()
            .map(|(index, tid)| Thread {
                tid,
                name: Some(format!("wait-worker-{}", index + 1)),
            })
            .collect(),
        invocations,
        stacks: vec![
            Stack {
                id: on_cpu_stack,
                frames: vec![
                    frame_with_module(
                        work,
                        "SliceFixture::sleep_work(unsigned int)",
                        "fixtures/off_cpu_wait",
                    ),
                    frame_with_module(cpu, "SliceFixture::cpu_prelude()", "fixtures/off_cpu_wait"),
                ],
            },
            Stack {
                id: off_cpu_stack,
                frames: vec![
                    frame_with_module(
                        work,
                        "SliceFixture::sleep_work(unsigned int)",
                        "fixtures/off_cpu_wait",
                    ),
                    frame_with_module(wait, "std::this_thread::sleep_for(...)", "libstdc++.so"),
                ],
            },
        ],
        samples,
        quality: CaptureQuality {
            events_generated: 48,
            samples_generated: 32,
            complete_invocations: 16,
            ..CaptureQuality::default()
        },
    }
}

fn frame(function_id: u32, label: &str) -> Frame {
    frame_with_module(function_id, label, "fixtures/tail_divergence")
}

fn frame_with_module(function_id: u32, label: &str, module: &str) -> Frame {
    Frame {
        function_id: Some(function_id),
        label: label.to_owned(),
        module: Some(module.to_owned()),
        address: None,
    }
}

#[derive(Debug, Eq, PartialEq, Error)]
pub enum ProfileValidationError {
    #[error("unsupported Slice profile version {0}")]
    UnsupportedVersion(u16),
    #[error("duplicate {kind} id {id}")]
    DuplicateId { kind: &'static str, id: u64 },
    #[error("invocation references unknown function {id} ({record})")]
    UnknownFunction { record: &'static str, id: u32 },
    #[error("invocation references unknown thread {0}")]
    UnknownThread(u32),
    #[error("record references unknown invocation {0}")]
    UnknownInvocation(u64),
    #[error("invocation {0} has a cyclic parent chain")]
    CyclicInvocationParent(u64),
    #[error("sample references unknown stack {0}")]
    UnknownStack(u32),
    #[error("invocation {0} has an end before its start")]
    InvalidInvocationBounds(u64),
    #[error(
        "sample for invocation {sample_invocation} has TID {sample_tid}, expected {invocation_tid}"
    )]
    SampleThreadMismatch {
        sample_invocation: u64,
        sample_tid: u32,
        invocation_tid: u32,
    },
    #[error("sample is outside invocation {0} bounds")]
    SampleOutsideInvocation(u64),
    #[error("capture quality counters are inconsistent with profile records")]
    QualityCountersInconsistent,
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("not a Slice v1 profile")]
    InvalidMagic,
    #[error("unsupported Slice profile version {0}")]
    UnsupportedVersion(u16),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("profile serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid profile: {0}")]
    InvalidProfile(#[from] ProfileValidationError),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum QueryError {
    #[error("percentile range must satisfy 0 <= low < high <= 100, received {0:?}")]
    InvalidPercentileRange(PercentileRange),
    #[error("time range must have from < to, received {0:?}")]
    InvalidTimeRange(TimeRange),
    #[error("sample referenced unknown stack {0}")]
    UnknownStack(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(percentile: PercentileRange) -> Query {
        Query {
            function_id: 1,
            threads: None,
            time: None,
            percentile,
            metric: Metric::Wall,
        }
    }

    #[test]
    fn aggregate_hides_the_tail_but_p99_reveals_it() {
        let profile = tail_divergence_profile();
        let aggregate = execute_query(&profile, &query(PercentileRange::ALL)).unwrap();
        assert_eq!(aggregate.root.children[0].children.len(), 2);
        let fast = &aggregate.root.children[0].children[0];
        let slow = &aggregate.root.children[0].children[1];
        assert_eq!(
            fast.value_ns, slow.value_ns,
            "aggregate paths must look equally costly"
        );

        let tail = execute_query(&profile, &query(PercentileRange { low: 99, high: 100 })).unwrap();
        assert_eq!(tail.selected_invocation_ids, vec![100]);
        assert_eq!(tail.root.children[0].children.len(), 1);
        assert_eq!(
            tail.root.children[0].children[0].name,
            "SliceFixture::slow_tail_b()"
        );
    }

    #[test]
    fn query_flame_root_starts_at_selected_function() {
        let mut profile = tail_divergence_profile();
        for stack in &mut profile.stacks {
            stack.frames.insert(
                0,
                Frame {
                    function_id: None,
                    label: "caller_above_selected_function()".to_owned(),
                    module: None,
                    address: None,
                },
            );
        }
        let result = execute_query(&profile, &query(PercentileRange::ALL)).unwrap();
        assert_eq!(
            result.root.children[0].name,
            "SliceFixture::work(unsigned int)"
        );
    }

    #[test]
    fn percentile_ranges_use_exact_ordinal_selection_and_r7_display_values() {
        let profile = tail_divergence_profile();
        let p95 = execute_query(&profile, &query(PercentileRange { low: 95, high: 100 })).unwrap();
        assert_eq!(p95.selected_invocation_ids, vec![96, 97, 98, 99, 100]);
        assert_eq!(p95.percentile_low_value_ns, Some(3_000_000.0));
        assert_eq!(p95.percentile_high_value_ns, Some(297_000_000.0));
    }

    #[test]
    fn thread_time_and_metric_filters_precede_percentiles() {
        let mut profile = tail_divergence_profile();
        profile.invocations[99].tid = 9999;
        profile.samples[99].tid = 9999;
        let result = execute_query(
            &profile,
            &Query {
                function_id: 1,
                threads: Some(BTreeSet::from([4242])),
                time: Some(TimeRange {
                    from_ns: 0,
                    to_ns: 297_000_000,
                }),
                percentile: PercentileRange::ALL,
                metric: Metric::Cpu,
            },
        )
        .unwrap();
        assert_eq!(result.available_invocation_count, 99);
        assert_eq!(result.selected_invocation_ids.len(), 99);
        assert_eq!(result.off_cpu_ns, 0);
    }

    #[test]
    fn profile_round_trip_is_versioned_and_lossless() {
        let profile = tail_divergence_profile();
        let bytes = profile.to_bytes().unwrap();
        assert!(bytes.starts_with(FILE_MAGIC));
        assert_eq!(Profile::from_bytes(&bytes).unwrap(), profile);
        profile.validate().unwrap();
        bimodal_profile().validate().unwrap();
        off_cpu_profile().validate().unwrap();
    }

    #[test]
    fn profile_validation_rejects_unknown_sample_references() {
        let mut profile = tail_divergence_profile();
        profile.samples[0].stack_id = 999;
        assert_eq!(
            profile.validate(),
            Err(ProfileValidationError::UnknownStack(999))
        );
    }

    #[test]
    fn profile_deserialization_rejects_invalid_semantic_graphs() {
        let mut profile = tail_divergence_profile();
        profile.samples[0].stack_id = 999;
        let error = Profile::from_bytes(&profile.to_bytes().unwrap()).unwrap_err();
        assert!(matches!(
            error,
            ProfileError::InvalidProfile(ProfileValidationError::UnknownStack(999))
        ));
    }

    #[test]
    fn profile_validation_rejects_cyclic_invocation_parents() {
        let mut profile = tail_divergence_profile();
        profile.invocations[0].parent_id = Some(profile.invocations[1].id);
        profile.invocations[1].parent_id = Some(profile.invocations[0].id);
        assert_eq!(
            profile.validate(),
            Err(ProfileValidationError::CyclicInvocationParent(1))
        );
    }

    #[test]
    fn wall_cpu_and_off_cpu_metrics_use_their_documented_weights() {
        let mut profile = tail_divergence_profile();
        profile.samples.push(Sample {
            timestamp_ns: 2_000_000,
            invocation_id: 1,
            stack_id: 10,
            tid: 4242,
            cpu: 0,
            state: ExecutionState::OffCpu,
            weight_ns: 7_000_000,
        });
        let cpu = execute_query(
            &profile,
            &Query {
                metric: Metric::Cpu,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        let off_cpu = execute_query(
            &profile,
            &Query {
                metric: Metric::OffCpu,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        let wall = execute_query(
            &profile,
            &Query {
                metric: Metric::Wall,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        assert_eq!(cpu.root.value_ns, 594_000_000);
        assert_eq!(off_cpu.root.value_ns, 7_000_000);
        assert_eq!(wall.root.value_ns, 601_000_000);
        assert_eq!(wall.sampled_cpu_ns, cpu.sampled_cpu_ns);
        assert_eq!(wall.off_cpu_ns, off_cpu.off_cpu_ns);
    }

    #[test]
    fn bimodal_profile_exposes_two_latency_modes_and_slow_tail() {
        let profile = bimodal_profile();
        let all = execute_query(&profile, &query(PercentileRange::ALL)).unwrap();
        let tail = execute_query(&profile, &query(PercentileRange { low: 95, high: 100 })).unwrap();
        assert_eq!(all.available_invocation_count, 9_000);
        assert_eq!(all.latency_min_ns, Some(2_000_000));
        assert_eq!(all.latency_max_ns, Some(28_000_000));
        assert_eq!(tail.selected_invocation_ids.len(), 450);
        let fast_max = profile
            .invocations
            .iter()
            .filter(|invocation| {
                profile
                    .samples
                    .iter()
                    .any(|sample| sample.invocation_id == invocation.id && sample.stack_id == 20)
            })
            .map(Invocation::duration_ns)
            .max()
            .unwrap();
        let slow_min = profile
            .invocations
            .iter()
            .filter(|invocation| {
                profile
                    .samples
                    .iter()
                    .any(|sample| sample.invocation_id == invocation.id && sample.stack_id == 21)
            })
            .map(Invocation::duration_ns)
            .min()
            .unwrap();
        let slow_max = profile
            .invocations
            .iter()
            .filter(|invocation| {
                profile
                    .samples
                    .iter()
                    .any(|sample| sample.invocation_id == invocation.id && sample.stack_id == 21)
            })
            .map(Invocation::duration_ns)
            .max()
            .unwrap();
        let fast_total: u64 = profile
            .invocations
            .iter()
            .filter(|invocation| {
                profile
                    .samples
                    .iter()
                    .any(|sample| sample.invocation_id == invocation.id && sample.stack_id == 20)
            })
            .map(Invocation::duration_ns)
            .sum();
        let slow_total: u64 = profile
            .invocations
            .iter()
            .filter(|invocation| {
                profile
                    .samples
                    .iter()
                    .any(|sample| sample.invocation_id == invocation.id && sample.stack_id == 21)
            })
            .map(Invocation::duration_ns)
            .sum();
        assert_eq!(fast_total / 6_300, 10_000_000);
        assert_eq!(slow_total / 2_700, 20_000_000);
        assert_eq!(fast_max, 18_000_000);
        assert_eq!(slow_min, 12_000_000);
        assert_eq!(slow_max, 28_000_000);
        assert!(fast_max > slow_min);
        assert_eq!(
            tail.root.children[0].children[0].name,
            "BimodalFixture::slow_path()"
        );
        assert!(tail.off_cpu_ns > 0);
    }

    #[test]
    fn off_cpu_profile_exposes_waiting_work_across_two_threads() {
        let profile = off_cpu_profile();
        let wall = execute_query(&profile, &query(PercentileRange::ALL)).unwrap();
        let cpu = execute_query(
            &profile,
            &Query {
                metric: Metric::Cpu,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        let off_cpu = execute_query(
            &profile,
            &Query {
                metric: Metric::OffCpu,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        assert_eq!(profile.threads.len(), 2);
        assert_eq!(wall.available_invocation_count, 16);
        assert_eq!(wall.latency_min_ns, Some(20_000_000));
        assert_eq!(wall.latency_max_ns, Some(40_000_000));
        assert_eq!(cpu.root.value_ns, 32_000_000);
        assert_eq!(off_cpu.root.value_ns, 448_000_000);
        assert_eq!(
            off_cpu.root.children[0].children[0].name,
            "std::this_thread::sleep_for(...)"
        );
    }
}
