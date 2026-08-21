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

    fn profile() -> Profile {
        let frame = |label: &str| Frame {
            function_id: None,
            label: label.to_owned(),
            module: Some("test-module".to_owned()),
            address: None,
        };
        Profile {
            format_version: 1,
            metadata: Metadata {
                captured_at_unix_ns: 1,
                command: vec!["test-program".to_owned()],
                kernel_release: "test-kernel".to_owned(),
                sample_period_ns: 1_000_000,
            },
            functions: vec![Function {
                id: 1,
                module: "test-module".to_owned(),
                module_build_id: Some("test-build".to_owned()),
                address: 0x1000,
                name: "work".to_owned(),
                demangled_name: "Test::work()".to_owned(),
                source_file: None,
                line: None,
            }],
            threads: vec![
                Thread {
                    tid: 10,
                    name: Some("worker-a".to_owned()),
                },
                Thread {
                    tid: 11,
                    name: Some("worker-b".to_owned()),
                },
            ],
            invocations: vec![
                Invocation {
                    id: 1,
                    function_id: 1,
                    parent_id: None,
                    tid: 10,
                    start_ns: 0,
                    end_ns: 10_000_000,
                    complete: true,
                    valid: true,
                },
                Invocation {
                    id: 2,
                    function_id: 1,
                    parent_id: None,
                    tid: 10,
                    start_ns: 20_000_000,
                    end_ns: 40_000_000,
                    complete: true,
                    valid: true,
                },
                Invocation {
                    id: 3,
                    function_id: 1,
                    parent_id: None,
                    tid: 11,
                    start_ns: 50_000_000,
                    end_ns: 80_000_000,
                    complete: true,
                    valid: true,
                },
            ],
            stacks: vec![
                Stack {
                    id: 10,
                    frames: vec![frame("Test::work()"), frame("child_a()")],
                },
                Stack {
                    id: 11,
                    frames: vec![frame("Test::work()"), frame("child_b()")],
                },
            ],
            samples: vec![
                Sample {
                    timestamp_ns: 5_000_000,
                    invocation_id: 1,
                    stack_id: 10,
                    tid: 10,
                    cpu: 0,
                    state: ExecutionState::OnCpu,
                    weight_ns: 10_000_000,
                },
                Sample {
                    timestamp_ns: 25_000_000,
                    invocation_id: 2,
                    stack_id: 11,
                    tid: 10,
                    cpu: 0,
                    state: ExecutionState::OnCpu,
                    weight_ns: 13_000_000,
                },
                Sample {
                    timestamp_ns: 30_000_000,
                    invocation_id: 2,
                    stack_id: 11,
                    tid: 10,
                    cpu: 0,
                    state: ExecutionState::OffCpu,
                    weight_ns: 7_000_000,
                },
                Sample {
                    timestamp_ns: 65_000_000,
                    invocation_id: 3,
                    stack_id: 11,
                    tid: 11,
                    cpu: 1,
                    state: ExecutionState::OnCpu,
                    weight_ns: 30_000_000,
                },
            ],
            quality: CaptureQuality {
                events_generated: 6,
                samples_generated: 4,
                complete_invocations: 3,
                ..CaptureQuality::default()
            },
        }
    }

    #[test]
    fn query_flame_root_starts_at_selected_function() {
        let mut profile = profile();
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
        assert_eq!(result.root.children[0].name, "Test::work()");
    }

    #[test]
    fn percentile_ranges_use_exact_ordinal_selection_and_r7_display_values() {
        let result =
            execute_query(&profile(), &query(PercentileRange { low: 33, high: 100 })).unwrap();
        assert_eq!(result.selected_invocation_ids, vec![2, 3]);
        assert_eq!(result.percentile_low_value_ns, Some(16_600_000.0));
        assert_eq!(result.percentile_high_value_ns, Some(30_000_000.0));
    }

    #[test]
    fn thread_time_and_metric_filters_precede_percentiles() {
        let result = execute_query(
            &profile(),
            &Query {
                threads: Some(BTreeSet::from([10])),
                time: Some(TimeRange {
                    from_ns: 0,
                    to_ns: 45_000_000,
                }),
                percentile: PercentileRange { low: 50, high: 100 },
                metric: Metric::Cpu,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        assert_eq!(result.available_invocation_count, 2);
        assert_eq!(result.selected_invocation_ids, vec![2]);
        assert_eq!(result.off_cpu_ns, 0);
    }

    #[test]
    fn profile_round_trip_is_versioned_and_lossless() {
        let profile = profile();
        let bytes = profile.to_bytes().unwrap();
        assert!(bytes.starts_with(FILE_MAGIC));
        assert_eq!(Profile::from_bytes(&bytes).unwrap(), profile);
        profile.validate().unwrap();
    }

    #[test]
    fn profile_validation_rejects_unknown_sample_references() {
        let mut profile = profile();
        profile.samples[0].stack_id = 999;
        assert_eq!(
            profile.validate(),
            Err(ProfileValidationError::UnknownStack(999))
        );
    }

    #[test]
    fn profile_deserialization_rejects_invalid_semantic_graphs() {
        let mut profile = profile();
        profile.samples[0].stack_id = 999;
        let error = Profile::from_bytes(&profile.to_bytes().unwrap()).unwrap_err();
        assert!(matches!(
            error,
            ProfileError::InvalidProfile(ProfileValidationError::UnknownStack(999))
        ));
    }

    #[test]
    fn profile_validation_rejects_cyclic_invocation_parents() {
        let mut profile = profile();
        profile.invocations[0].parent_id = Some(profile.invocations[1].id);
        profile.invocations[1].parent_id = Some(profile.invocations[0].id);
        assert_eq!(
            profile.validate(),
            Err(ProfileValidationError::CyclicInvocationParent(1))
        );
    }

    #[test]
    fn wall_cpu_and_off_cpu_metrics_use_their_documented_weights() {
        let cpu = execute_query(
            &profile(),
            &Query {
                metric: Metric::Cpu,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        let off_cpu = execute_query(
            &profile(),
            &Query {
                metric: Metric::OffCpu,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        let wall = execute_query(
            &profile(),
            &Query {
                metric: Metric::Wall,
                ..query(PercentileRange::ALL)
            },
        )
        .unwrap();
        assert_eq!(cpu.root.value_ns, 53_000_000);
        assert_eq!(off_cpu.root.value_ns, 7_000_000);
        assert_eq!(wall.root.value_ns, 60_000_000);
        assert_eq!(wall.sampled_cpu_ns, cpu.sampled_cpu_ns);
        assert_eq!(wall.off_cpu_ns, off_cpu.off_cpu_ns);
    }
}
