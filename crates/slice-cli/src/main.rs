#![cfg_attr(test, allow(unused_crate_dependencies))]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use cpp_demangle::Symbol;
use object::{Object, ObjectSegment, ObjectSymbol, SymbolKind};
use slice_capture::{CapturePort, CaptureRequest, CheckStatus};
use slice_core::{ExecutionState, Function, Metric, PercentileRange, Profile, Query, TimeRange};
use slice_ebpf::LinuxCaptureAdapter;

#[derive(Debug, Parser)]
#[command(
    name = "slice",
    version,
    about = "Percentile-conditioned C++ profiler POC"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List exact C++ functions that can be used as a population selector.
    Symbols {
        binary: PathBuf,
        /// Read a specific ELF module instead of the main binary.
        #[arg(long)]
        module: Option<PathBuf>,
        /// Case-insensitive substring applied to the demangled name.
        #[arg(long)]
        r#match: Option<String>,
    },
    /// Profile a started or launched process at one exact demangled ELF function.
    Profile {
        /// Running target process to profile.
        #[arg(long)]
        pid: Option<u32>,
        /// ELF module containing the entry point (usually the target binary).
        #[arg(long)]
        module: Option<PathBuf>,
        /// Full demangled function signature printed by `slice symbols`.
        #[arg(long)]
        function: String,
        #[arg(short, long, default_value = "capture.slice")]
        output: PathBuf,
        /// Program to launch and profile. Omit this when using --pid.
        #[arg(value_name = "PROGRAM", conflicts_with = "pid")]
        program: Option<PathBuf>,
        /// Arguments passed to PROGRAM.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Render a capture as one self-contained, interactive HTML file.
    View {
        profile: PathBuf,
        #[arg(short, long, default_value = "profile.html")]
        output: PathBuf,
        /// Comma-separated TIDs; omitted means all threads.
        #[arg(long)]
        threads: Option<String>,
        /// Capture-relative invocation start time range, e.g. 10ms:30ms.
        #[arg(long)]
        time: Option<String>,
        /// Percentile range, e.g. 95:100.
        #[arg(long, default_value = "0:100")]
        percentile: String,
        #[arg(long, value_enum, default_value_t = CliMetric::Wall)]
        metric: CliMetric,
    },
    /// Rank observed functions from an existing capture without pretending to know p95.
    Discover {
        profile: PathBuf,
        /// Restrict discovery to wall, on-CPU, or off-CPU samples.
        #[arg(long, value_enum, default_value_t = CliMetric::Wall)]
        metric: CliMetric,
    },
    /// Check local kernel and permission prerequisites for privileged eBPF capture.
    Doctor,
    /// Validate a profile's file envelope and semantic references.
    Validate {
        profile: PathBuf,
        /// Require at least one complete and valid invocation.
        #[arg(long)]
        require_complete: bool,
        /// Require at least one captured sample.
        #[arg(long)]
        require_samples: bool,
        /// Require at least one scheduler-derived off-CPU sample.
        #[arg(long)]
        require_off_cpu: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliMetric {
    Wall,
    Cpu,
    OffCpu,
}

impl From<CliMetric> for Metric {
    fn from(value: CliMetric) -> Self {
        match value {
            CliMetric::Wall => Self::Wall,
            CliMetric::Cpu => Self::Cpu,
            CliMetric::OffCpu => Self::OffCpu,
        }
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Symbols {
            binary,
            module,
            r#match,
        } => list_symbols(module.as_ref().unwrap_or(&binary), r#match.as_deref()),
        Command::Profile {
            pid,
            module,
            function,
            output,
            program,
            args,
        } => profile(pid, module, function, output, program, args),
        Command::View {
            profile,
            output,
            threads,
            time,
            percentile,
            metric,
        } => view(
            profile,
            output,
            threads.as_deref(),
            time.as_deref(),
            &percentile,
            metric.into(),
        ),
        Command::Discover { profile, metric } => discover(profile, metric.into()),
        Command::Doctor => doctor(),
        Command::Validate {
            profile,
            require_complete,
            require_samples,
            require_off_cpu,
        } => validate(profile, require_complete, require_samples, require_off_cpu),
    }
}

fn list_symbols(path: &std::path::Path, needle: Option<&str>) -> Result<()> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let file = object::File::parse(&*bytes)
        .with_context(|| format!("{} is not a supported ELF object", path.display()))?;
    let needle = needle.map(str::to_lowercase);
    let mut entries = file
        .symbols()
        .chain(file.dynamic_symbols())
        .filter(|symbol| matches!(symbol.kind(), SymbolKind::Text))
        .filter_map(|symbol| {
            let raw = symbol.name().ok()?;
            let demangled = Symbol::new(raw)
                .ok()
                .and_then(|symbol| symbol.demangle().ok())
                .unwrap_or_else(|| raw.to_owned());
            if needle
                .as_ref()
                .is_some_and(|needle| !demangled.to_lowercase().contains(needle))
            {
                return None;
            }
            Some((symbol.address(), raw.to_owned(), demangled))
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries.dedup();
    if entries.is_empty() {
        bail!("no matching executable symbols in {}", path.display());
    }
    println!("# module: {}", path.display());
    println!("# copy a full demangled signature to --function for an exact profile target");
    for (address, _raw, demangled) in entries {
        println!("0x{address:016x}\t{demangled}");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct SelectedSymbol {
    raw_name: String,
    demangled_name: String,
    address: u64,
    probe_offset: usize,
}

fn select_symbol(path: &std::path::Path, requested: &str) -> Result<SelectedSymbol> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let file = object::File::parse(&*bytes)
        .with_context(|| format!("{} is not a supported ELF object", path.display()))?;
    let mut candidates = file
        .symbols()
        .chain(file.dynamic_symbols())
        .filter(|symbol| symbol.kind() == SymbolKind::Text && symbol.address() != 0)
        .filter_map(|symbol| {
            let raw_name = symbol.name().ok()?.to_owned();
            let demangled_name = Symbol::new(&raw_name)
                .ok()
                .and_then(|symbol| symbol.demangle().ok())
                .unwrap_or_else(|| raw_name.clone());
            (demangled_name == requested).then_some((raw_name, demangled_name, symbol.address()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.2);
    candidates.dedup_by_key(|candidate| candidate.2);
    let (raw_name, demangled_name, address) = match candidates.as_slice() {
        [] => bail!(
            "no exact executable function named {requested:?} in {}",
            path.display()
        ),
        [candidate] => candidate.clone(),
        many => {
            let addresses = many
                .iter()
                .map(|(_, _, address)| format!("0x{address:x}"))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("function {requested:?} is ambiguous at {addresses}; use a unique signature")
        }
    };
    let probe_offset = file
        .segments()
        .find_map(|segment| {
            let (file_offset, file_size) = segment.file_range();
            let address_end = segment.address().saturating_add(file_size);
            (address >= segment.address() && address < address_end)
                .then_some(file_offset.saturating_add(address - segment.address()))
        })
        .context("could not translate function virtual address to an ELF file offset")?;
    Ok(SelectedSymbol {
        raw_name,
        demangled_name,
        address,
        probe_offset: probe_offset as usize,
    })
}

fn profile(
    pid: Option<u32>,
    module: Option<PathBuf>,
    function: String,
    output: PathBuf,
    program: Option<PathBuf>,
    args: Vec<String>,
) -> Result<()> {
    let adapter = LinuxCaptureAdapter;
    let (running_pid, module, launch_program) = match (pid, program) {
        (Some(pid), None) => (
            Some(pid),
            module.context("--module is required when using --pid")?,
            None,
        ),
        (None, Some(program)) => {
            let module = module.unwrap_or_else(|| program.clone());
            (None, module, Some(program))
        }
        (Some(_), Some(_)) => bail!("choose either --pid or PROGRAM, not both"),
        (None, None) => bail!("provide --pid or a PROGRAM to launch"),
    };
    let selected = select_symbol(&module, &function)?;

    let (pid, command, resume_after_attach, mut child) = if let Some(pid) = running_pid {
        (pid, Vec::new(), false, None)
    } else {
        let program = launch_program.expect("launch program is present when PID is absent");
        let mut command = std::process::Command::new(&program);
        command.args(&args);
        let launched = command
            .spawn()
            .with_context(|| format!("launching {}", program.display()))?;
        let pid = launched.id();
        // Stop at the earliest safe point, then let capture_pid resume the
        // process only after every link and sampler is live.
        if let Err(error) = adapter
            .stop_process(pid)
            .and_then(|()| adapter.wait_for_stopped(pid))
        {
            let _ = adapter.kill_process(pid);
            return Err(error.into());
        }
        let command = std::iter::once(program.display().to_string())
            .chain(args.iter().cloned())
            .collect();
        (pid, command, true, Some(launched))
    };
    let target = adapter
        .resolve_process_identity(pid)
        .with_context(|| format!("resolving PID namespace identity for {pid}"))?;
    let stop_requested = Arc::new(AtomicBool::new(false));
    let stop_for_handler = Arc::clone(&stop_requested);
    let child_pid = child.as_ref().map(std::process::Child::id);
    if let Err(error) = ctrlc::set_handler(move || {
        stop_for_handler.store(true, Ordering::SeqCst);
        if let Some(child_pid) = child_pid {
            let _ = adapter.interrupt_process(child_pid);
        }
    })
    .context("installing Ctrl-C handler")
    {
        if let Some(launched) = child.as_ref() {
            let _ = adapter.kill_process(launched.id());
        }
        return Err(error);
    }

    eprintln!(
        "capturing {} at PID {pid}; stop the target or press Ctrl-C to finish",
        selected.demangled_name
    );
    let function = Function {
        id: 1,
        module: module.display().to_string(),
        module_build_id: None,
        address: selected.address,
        name: selected.raw_name.clone(),
        demangled_name: selected.demangled_name.clone(),
        source_file: None,
        line: None,
    };
    let request = CaptureRequest {
        target,
        module,
        function,
        probe_offset: selected.probe_offset,
        command,
        stop_requested,
        resume_after_attach,
    };
    let profile = match adapter
        .capture(&request)
        .with_context(|| format!("capturing PID {pid} at {}", selected.demangled_name))
    {
        Ok(profile) => profile,
        Err(error) => {
            if let Some(launched) = child.as_ref() {
                let _ = adapter.kill_process(launched.id());
            }
            return Err(error);
        }
    };
    let complete_invocations = profile
        .invocations
        .iter()
        .filter(|invocation| invocation.complete && invocation.valid)
        .count();
    if complete_invocations == 0 {
        bail!(
            "capture produced no complete valid invocations ({} events, {} samples); use the attachment, transport, and identity diagnostics printed above instead of assuming the function was not reached",
            profile.quality.events_generated,
            profile.samples.len()
        );
    }
    if let Err(error) = write_profile_atomically(&profile, &output) {
        if let Some(launched) = child.as_ref() {
            let _ = adapter.kill_process(launched.id());
        }
        return Err(error);
    }
    if let Some(mut launched) = child.take() {
        launched.wait().context("waiting for launched target")?;
    }
    let (on_cpu_samples, off_cpu_samples, _, off_cpu_ns) = sample_summary(&profile);
    println!(
        "captured {} at PID {pid}: {complete_invocations} complete invocations, {} samples ({on_cpu_samples} on-CPU, {off_cpu_samples} off-CPU / {:.3} ms), {} dropped events, {} dropped samples -> {}",
        selected.demangled_name,
        profile.samples.len(),
        off_cpu_ns as f64 / 1e6,
        profile.quality.events_dropped,
        profile.quality.samples_dropped,
        output.display()
    );
    Ok(())
}

fn write_profile_atomically(profile: &Profile, output: &std::path::Path) -> Result<()> {
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .context("profile output path must have a valid file name")?;
    let temporary = output.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| -> Result<()> {
        std::fs::write(&temporary, profile.to_bytes()?)?;
        std::fs::rename(&temporary, output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn view(
    profile_path: PathBuf,
    output: PathBuf,
    threads: Option<&str>,
    time: Option<&str>,
    percentile: &str,
    metric: Metric,
) -> Result<()> {
    let profile = Profile::read_from_path(&profile_path)
        .with_context(|| format!("could not read profile {}", profile_path.display()))?;
    profile
        .validate()
        .with_context(|| format!("profile validation failed for {}", profile_path.display()))?;
    let function_id = profile
        .functions
        .first()
        .context("profile has no functions")?
        .id;
    let query = Query {
        function_id,
        threads: threads.map(parse_threads).transpose()?,
        time: time
            .map(|value| parse_time(value, profile.capture_bounds()))
            .transpose()?,
        percentile: parse_percentile(percentile)?,
        metric,
    };
    let html = slice_render::render_html(&profile, &query)?;
    std::fs::write(&output, html)
        .with_context(|| format!("could not write {}", output.display()))?;
    println!("wrote {}", output.display());
    Ok(())
}

fn discover(profile_path: PathBuf, metric: Metric) -> Result<()> {
    let profile = Profile::read_from_path(&profile_path)?;
    profile
        .validate()
        .with_context(|| format!("profile validation failed for {}", profile_path.display()))?;
    let stacks = profile
        .stacks
        .iter()
        .map(|stack| (stack.id, stack))
        .collect::<std::collections::HashMap<_, _>>();
    let mut inclusive = std::collections::BTreeMap::<String, u64>::new();
    for sample in &profile.samples {
        if !metric.includes(sample.state) {
            continue;
        }
        if let Some(stack) = stacks.get(&sample.stack_id) {
            for frame in &stack.frames {
                *inclusive.entry(frame.label.clone()).or_default() += sample.weight_ns;
            }
        }
    }
    let (on_cpu_samples, off_cpu_samples, on_cpu_ns, off_cpu_ns) = sample_summary(&profile);
    println!(
        "Sample states: {on_cpu_samples} on-CPU / {:.3} ms, {off_cpu_samples} off-CPU / {:.3} ms",
        on_cpu_ns as f64 / 1e6,
        off_cpu_ns as f64 / 1e6
    );
    let metric_name = match metric {
        Metric::Wall => "wall",
        Metric::Cpu => "CPU",
        Metric::OffCpu => "off-CPU",
    };
    println!(
        "Observed {metric_name} functions (sampled inclusive time; discovery does not report exact p95):"
    );
    let mut inclusive = inclusive.into_iter().collect::<Vec<_>>();
    inclusive.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    for (name, time) in inclusive.into_iter().take(30) {
        println!("{:>12.3} ms  {name}", time as f64 / 1e6);
    }
    Ok(())
}

fn validate(
    profile_path: PathBuf,
    require_complete: bool,
    require_samples: bool,
    require_off_cpu: bool,
) -> Result<()> {
    let profile = Profile::read_from_path(&profile_path)
        .with_context(|| format!("could not read profile {}", profile_path.display()))?;
    profile
        .validate()
        .with_context(|| format!("profile validation failed for {}", profile_path.display()))?;
    if require_complete
        && !profile
            .invocations
            .iter()
            .any(|invocation| invocation.complete && invocation.valid)
    {
        bail!("profile contains no complete valid invocations");
    }
    if require_samples && profile.samples.is_empty() {
        bail!("profile contains no samples");
    }
    let (on_cpu_samples, off_cpu_samples, _, off_cpu_ns) = sample_summary(&profile);
    if require_off_cpu && off_cpu_samples == 0 {
        bail!("profile contains no off-CPU samples");
    }
    println!(
        "valid profile: {} invocations, {} samples ({on_cpu_samples} on-CPU, {off_cpu_samples} off-CPU / {:.3} ms), {} dropped events, {} dropped samples",
        profile.invocations.len(),
        profile.samples.len(),
        off_cpu_ns as f64 / 1e6,
        profile.quality.events_dropped,
        profile.quality.samples_dropped
    );
    Ok(())
}

fn sample_summary(profile: &Profile) -> (usize, usize, u64, u64) {
    profile.samples.iter().fold(
        (0, 0, 0_u64, 0_u64),
        |(on_count, off_count, on_ns, off_ns), sample| match sample.state {
            ExecutionState::OnCpu => (
                on_count + 1,
                off_count,
                on_ns.saturating_add(sample.weight_ns),
                off_ns,
            ),
            ExecutionState::OffCpu => (
                on_count,
                off_count + 1,
                on_ns,
                off_ns.saturating_add(sample.weight_ns),
            ),
        },
    )
}

fn doctor() -> Result<()> {
    let report = LinuxCaptureAdapter.doctor()?;
    println!("Slice doctor (adapter: {})", report.adapter);
    for check in &report.checks {
        let marker = match check.status {
            CheckStatus::Pass => "ok",
            CheckStatus::Warning => "warn",
            CheckStatus::Failure => "fail",
        };
        println!("  [{marker}] {}: {}", check.label, check.detail);
        if let Some(remediation) = &check.remediation {
            println!("         fix: {remediation}");
        }
    }
    let failures = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Failure)
        .count();
    let warnings = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Warning)
        .count();
    println!(
        "Doctor summary: {} passed, {warnings} warnings, {failures} failures",
        report.checks.len() - warnings - failures
    );
    if report.has_failures() {
        bail!("capture prerequisites failed; apply the fixes shown above and rerun `slice doctor`");
    }
    Ok(())
}

fn parse_threads(value: &str) -> Result<BTreeSet<u32>> {
    value
        .split(',')
        .map(|part| part.trim().parse::<u32>().context("invalid thread ID"))
        .collect()
}

fn parse_percentile(value: &str) -> Result<PercentileRange> {
    let (low, high) = value
        .split_once(':')
        .context("percentile must be LOW:HIGH")?;
    let range = PercentileRange {
        low: low.parse().context("invalid percentile lower bound")?,
        high: high.parse().context("invalid percentile upper bound")?,
    };
    if range.low >= range.high || range.high > 100 {
        bail!("percentile must satisfy 0 <= LOW < HIGH <= 100");
    }
    Ok(range)
}

fn parse_time(value: &str, bounds: Option<TimeRange>) -> Result<TimeRange> {
    let bounds = bounds.context("profile has no invocations")?;
    let (low, high) = value.split_once(':').context("time must be FROM:TO")?;
    let from_ns = bounds.from_ns + parse_duration_ns(low)?;
    let to_ns = bounds.from_ns + parse_duration_ns(high)?;
    if from_ns >= to_ns {
        bail!("time must satisfy FROM < TO");
    }
    Ok(TimeRange { from_ns, to_ns })
}

fn parse_duration_ns(value: &str) -> Result<u64> {
    let units = [
        ("ns", 1_u64),
        ("us", 1_000),
        ("ms", 1_000_000),
        ("s", 1_000_000_000),
    ];
    for (suffix, multiplier) in units {
        if let Some(number) = value.strip_suffix(suffix) {
            return Ok(number.trim().parse::<u64>()? * multiplier);
        }
    }
    bail!("duration must use ns, us, ms, or s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_ranges() {
        assert_eq!(
            parse_percentile("99:100").unwrap(),
            PercentileRange { low: 99, high: 100 }
        );
        assert_eq!(parse_duration_ns("2ms").unwrap(), 2_000_000);
        assert!(parse_percentile("100:99").is_err());
    }
}
