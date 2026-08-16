#![cfg_attr(test, allow(unused_crate_dependencies))]

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use cpp_demangle::Symbol;
use object::{Object, ObjectSegment, ObjectSymbol, SymbolKind};
use slice_core::{
    Function, Metric, PercentileRange, Profile, Query, TimeRange, tail_divergence_profile,
};

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
    /// Create the deterministic tail-divergence profile used by tests and demos.
    FixtureProfile {
        #[arg(short, long, default_value = "tail-divergence.slice")]
        output: PathBuf,
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
        /// Capture duration, for example 2s or 500ms.
        #[arg(long, default_value = "10s")]
        duration: String,
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
    Discover { profile: PathBuf },
    /// Check local kernel and permission prerequisites for privileged eBPF capture.
    Doctor,
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
        Command::FixtureProfile { output } => {
            tail_divergence_profile().write_to_path(&output)?;
            println!("wrote {}", output.display());
            Ok(())
        }
        Command::Profile {
            pid,
            module,
            function,
            duration,
            output,
            program,
            args,
        } => profile(pid, module, function, duration, output, program, args),
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
        Command::Discover { profile } => discover(profile),
        Command::Doctor => doctor(),
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
    duration: String,
    output: PathBuf,
    program: Option<PathBuf>,
    args: Vec<String>,
) -> Result<()> {
    let mut child = None;
    let (pid, module, command, resume_after_attach) = match (pid, program) {
        (Some(pid), None) => (
            pid,
            module.context("--module is required when using --pid")?,
            Vec::new(),
            false,
        ),
        (None, Some(program)) => {
            let module = module.unwrap_or_else(|| program.clone());
            let mut command = std::process::Command::new(&program);
            command.args(&args);
            let launched = command
                .spawn()
                .with_context(|| format!("launching {}", program.display()))?;
            let pid = launched.id();
            // Stop at the earliest safe point, then let capture_pid resume the
            // process only after all links and samplers have been installed.
            slice_ebpf::stop_process(pid)?;
            std::thread::sleep(std::time::Duration::from_millis(10));
            let command = std::iter::once(program.display().to_string())
                .chain(args.iter().cloned())
                .collect();
            child = Some(launched);
            (pid, module, command, true)
        }
        (Some(_), Some(_)) => bail!("choose either --pid or PROGRAM, not both"),
        (None, None) => bail!("provide --pid or a PROGRAM to launch"),
    };
    let selected = match select_symbol(&module, &function) {
        Ok(selected) => selected,
        Err(error) => {
            if let Some(launched) = child.as_ref() {
                let _ = slice_ebpf::kill_process(launched.id());
            }
            return Err(error);
        }
    };
    let duration = std::time::Duration::from_nanos(parse_duration_ns(&duration)?);
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
    let request = slice_ebpf::CaptureRequest {
        pid,
        module,
        function,
        probe_offset: selected.probe_offset,
        duration,
        command,
        resume_after_attach,
    };
    let profile = match slice_ebpf::capture_pid(&request)
        .with_context(|| format!("capturing PID {pid} at {}", selected.demangled_name))
    {
        Ok(profile) => profile,
        Err(error) => {
            if let Some(launched) = child.as_ref() {
                let _ = slice_ebpf::kill_process(launched.id());
            }
            return Err(error);
        }
    };
    if let Err(error) = profile.write_to_path(&output) {
        if let Some(launched) = child.as_ref() {
            let _ = slice_ebpf::kill_process(launched.id());
        }
        return Err(error.into());
    }
    if let Some(mut launched) = child {
        launched.wait().context("waiting for launched target")?;
    }
    println!(
        "captured {} at PID {pid} -> {}",
        selected.demangled_name,
        output.display()
    );
    Ok(())
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

fn discover(profile_path: PathBuf) -> Result<()> {
    let profile = Profile::read_from_path(&profile_path)?;
    let stacks = profile
        .stacks
        .iter()
        .map(|stack| (stack.id, stack))
        .collect::<std::collections::HashMap<_, _>>();
    let mut inclusive = std::collections::BTreeMap::<String, u64>::new();
    for sample in &profile.samples {
        if let Some(stack) = stacks.get(&sample.stack_id) {
            for frame in &stack.frames {
                *inclusive.entry(frame.label.clone()).or_default() += sample.weight_ns;
            }
        }
    }
    println!("Observed functions (sampled inclusive time; discovery does not report exact p95):");
    for (name, time) in inclusive.into_iter().rev().take(30) {
        println!("{:>12.3} ms  {name}", time as f64 / 1e6);
    }
    Ok(())
}

fn doctor() -> Result<()> {
    let btf = std::path::Path::new("/sys/kernel/btf/vmlinux").exists();
    let tracefs = [
        "/sys/kernel/tracing/events/sched/sched_switch/format",
        "/sys/kernel/debug/tracing/events/sched/sched_switch/format",
    ]
    .iter()
    .any(|path| std::path::Path::new(path).exists());
    let perf = std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .unwrap_or_else(|_| "unavailable".to_owned());
    let unprivileged_bpf = std::fs::read_to_string("/proc/sys/kernel/unprivileged_bpf_disabled")
        .unwrap_or_else(|_| "unavailable".to_owned());
    println!("Slice capture prerequisites");
    println!("  architecture: {}", std::env::consts::ARCH);
    println!(
        "  kernel BTF: {}",
        if btf { "available" } else { "missing" }
    );
    println!(
        "  sched_switch tracepoint: {}",
        if tracefs { "available" } else { "missing" }
    );
    println!("  perf_event_paranoid: {}", perf.trim());
    println!("  unprivileged_bpf_disabled: {}", unprivileged_bpf.trim());
    println!(
        "  capture requires root or CAP_BPF,CAP_PERFMON and CAP_SYS_PTRACE for unrelated --pid targets"
    );
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
