//! Privileged libbpf capture engine.
//!
//! This module is intentionally a thin transport adapter. Invocation validity,
//! deduplicated stacks, and quality counters remain in `slice-collector`, so a
//! synthetic event stream and a live kernel stream are tested identically.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use libbpf_rs::skel::{OpenSkel, SkelBuilder};
use libbpf_rs::{MapCore, MapFlags, RingBufferBuilder, TracepointCategory};
use object::{Object, ObjectSegment, ObjectSymbol, SymbolKind};
use slice_collector::{CollectorEvent, Correlator};
use slice_core::{CaptureQuality, ExecutionState, Frame, Function, Metadata, Profile, Thread};

mod bpf {
    include!(concat!(env!("OUT_DIR"), "/slice.skel.rs"));
}

const SAMPLE_FREQUENCY_HZ: u64 = 999;
const PERF_TYPE_SOFTWARE: u32 = 1;
const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
const PERF_SAMPLE_IP: u64 = 1 << 0;

#[derive(Clone, Debug)]
pub struct CaptureRequest {
    pub pid: u32,
    pub module: PathBuf,
    pub function: Function,
    pub probe_offset: usize,
    pub command: Vec<String>,
    pub stop_requested: Arc<AtomicBool>,
    /// A launched child is stopped before attachment so no early invocation
    /// is lost; resume it only after every probe and sampler link is live.
    pub resume_after_attach: bool,
}

/// Stop a just-launched target before probes are installed. Kept in the
/// transport crate so the CLI remains free of unsafe FFI.
pub fn stop_process(pid: u32) -> Result<()> {
    signal_process(pid, libc::SIGSTOP).context("stopping launched target")
}

/// Wait until a just-launched child has observed SIGSTOP. The child remains a
/// live process; WUNTRACED reports the stop without reaping it.
pub fn wait_for_stopped(pid: u32) -> Result<()> {
    let mut status = 0_i32;
    let result = unsafe { libc::waitpid(pid as i32, &mut status, libc::WUNTRACED) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if !libc::WIFSTOPPED(status) {
        bail!("launched target exited before probe attachment");
    }
    Ok(())
}

/// Kill a launched target when capture setup fails before ownership can be
/// handed back to the caller.
pub fn kill_process(pid: u32) -> Result<()> {
    signal_process(pid, libc::SIGKILL).context("killing launched target")
}

/// Ask a launched target to stop cleanly. SIGCONT is needed when the target
/// was held in SIGSTOP while probes were being installed.
pub fn interrupt_process(pid: u32) -> Result<()> {
    signal_process(pid, libc::SIGINT).context("interrupting target")?;
    signal_process(pid, libc::SIGCONT).context("resuming interrupted target")
}

fn signal_process(pid: u32, signal: i32) -> Result<()> {
    let result = unsafe { libc::kill(pid as i32, signal) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Event {
    kind: u32,
    tid: u32,
    timestamp_ns: u64,
    invocation_id: u64,
    stack_id: i32,
    cpu: u32,
    weight_ns: u64,
}

/// Attach to an already-running process and collect until the target exits or
/// a stop token is raised. The target process itself is never modified or
/// stopped.
pub fn capture_pid(request: &CaptureRequest) -> Result<Profile> {
    if request.pid == 0 {
        bail!("PID 0 is not a valid capture target");
    }
    if !request.module.is_file() {
        bail!(
            "capture module does not exist: {}",
            request.module.display()
        );
    }
    // Resolve the target's current load mappings before capture. A short-lived
    // target may exit immediately after the sampling window, at which point
    // /proc/<pid>/maps would no longer be available for PIE address rebasing.
    let resolver = Resolver::new(&request.module, request.pid)?;
    let thread_names = read_thread_names(request.pid);

    let skel_builder = bpf::SliceSkelBuilder::default();
    let mut open_object = MaybeUninit::uninit();
    let open = skel_builder
        .open(&mut open_object)
        .context("opening Slice BPF skeleton")?;
    // libbpf needs a sufficiently large locked-memory limit on kernels that
    // still account BPF maps against RLIMIT_MEMLOCK. Raise the soft limit up
    // to the hard limit when the caller is allowed to do so; if not, libbpf's
    // load error below remains the useful privilege diagnostic.
    let _ = raise_memlock_limit();
    let skel = open.load().context(
        "loading Slice BPF program; check CAP_BPF/CAP_PERFMON, RLIMIT_MEMLOCK, and kernel support",
    )?;

    let config = [request.pid, request.function.id];
    let mut config_bytes = Vec::with_capacity(16);
    config_bytes.extend_from_slice(&config[0].to_ne_bytes());
    config_bytes.extend_from_slice(&config[1].to_ne_bytes());
    config_bytes.extend_from_slice(&(1_000_000_000_u64 / SAMPLE_FREQUENCY_HZ).to_ne_bytes());
    let zero = 0_u32.to_ne_bytes();
    skel.maps
        .config
        .update(&zero, &config_bytes, MapFlags::ANY)
        .context("configuring target TGID")?;
    skel.maps
        .next_invocation_id
        .update(&zero, &1_u64.to_ne_bytes(), MapFlags::ANY)
        .context("initializing invocation ID map")?;

    let entry_link = skel
        .progs
        .slice_entry
        .attach_uprobe(
            false,
            request.pid as i32,
            &request.module,
            request.probe_offset,
        )
        .context("attaching entry uprobe")?;
    let return_link = skel
        .progs
        .slice_return
        .attach_uprobe(
            true,
            request.pid as i32,
            &request.module,
            request.probe_offset,
        )
        .context("attaching return uprobe")?;
    let sched_link = skel
        .progs
        .slice_sched_switch
        .attach_tracepoint(TracepointCategory::Sched, "sched_switch")
        .context("attaching sched_switch tracepoint")?;
    let mut perf_links = Vec::new();
    let mut perf_fds = Vec::new();
    for cpu in 0..available_cpus()? {
        let fd = open_sampling_event(cpu)?;
        let link = skel
            .progs
            .slice_sample
            .attach_perf_event(fd)
            .with_context(|| format!("attaching perf sampler on CPU {cpu}"))?;
        perf_fds.push(fd);
        perf_links.push(link);
    }

    let events = &skel.maps.events;
    let stack_traces = &skel.maps.stack_traces;
    let stacks = std::sync::Arc::new(std::sync::Mutex::new(HashMap::<i32, Vec<u64>>::new()));
    let events_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Event>::new()));
    let event_store = events_seen.clone();
    let stack_store = stacks.clone();
    let mut ring = RingBufferBuilder::new();
    ring.add(events, move |bytes| {
        if bytes.len() != std::mem::size_of::<Event>() {
            return 0;
        }
        let event = read_event(bytes);
        if event.stack_id >= 0 {
            // The stack map lookup is performed outside BPF, where symbol and
            // module resolution is safe and unbounded.
            if let Ok(Some(raw)) = stack_traces.lookup(&event.stack_id.to_ne_bytes(), MapFlags::ANY)
            {
                let addresses = raw
                    .chunks_exact(8)
                    .map(|chunk| u64::from_ne_bytes(chunk.try_into().unwrap()))
                    .take_while(|address| *address != 0)
                    .collect::<Vec<_>>();
                stack_store
                    .lock()
                    .unwrap()
                    .insert(event.stack_id, addresses);
            }
        }
        event_store.lock().unwrap().push(event);
        0
    })
    .context("registering BPF ring buffer")?;
    let mut ring = ring.build().context("building BPF ring buffer")?;
    if request.resume_after_attach {
        let result = unsafe { libc::kill(request.pid as i32, libc::SIGCONT) };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).context("resuming launched target");
        }
    }
    while process_exists(request.pid) && !request.stop_requested.load(Ordering::Relaxed) {
        if !poll_ring(
            &mut ring,
            Duration::from_millis(100),
            &request.stop_requested,
        )? {
            break;
        }
    }
    // Give callbacks already queued in the ring a final chance to arrive
    // before links are detached and the profile is reconstructed.
    for _ in 0..3 {
        if !poll_ring(&mut ring, Duration::ZERO, &request.stop_requested)? {
            break;
        }
    }
    drop(ring);
    drop(perf_links);
    drop(entry_link);
    drop(return_link);
    drop(sched_link);
    for fd in perf_fds {
        close_fd(fd);
    }

    let mut profile = empty_profile(request);
    profile.quality.events_dropped = read_counter(&skel.maps.dropped_events);
    profile.quality.samples_dropped = read_counter(&skel.maps.dropped_samples);
    let mut correlator = Correlator::default();
    for event in events_seen.lock().unwrap().iter().copied() {
        match event.kind {
            1 => correlator.push(
                &mut profile,
                CollectorEvent::Entry {
                    function_id: request.function.id,
                    tid: event.tid,
                    timestamp_ns: event.timestamp_ns,
                },
            ),
            2 => correlator.push(
                &mut profile,
                CollectorEvent::Return {
                    function_id: request.function.id,
                    tid: event.tid,
                    timestamp_ns: event.timestamp_ns,
                },
            ),
            3 => {
                let addresses = stacks
                    .lock()
                    .unwrap()
                    .get(&event.stack_id)
                    .cloned()
                    .unwrap_or_default();
                let frames = resolver.frames(&addresses);
                correlator.push(
                    &mut profile,
                    CollectorEvent::Sample {
                        tid: event.tid,
                        timestamp_ns: event.timestamp_ns,
                        cpu: event.cpu,
                        state: ExecutionState::OnCpu,
                        weight_ns: event.weight_ns,
                        frames,
                    },
                );
            }
            4 => correlator.push(
                &mut profile,
                CollectorEvent::Violation {
                    tid: event.tid,
                    timestamp_ns: event.timestamp_ns,
                },
            ),
            5 => correlator.push(
                &mut profile,
                CollectorEvent::Sample {
                    tid: event.tid,
                    timestamp_ns: event.timestamp_ns,
                    cpu: event.cpu,
                    state: ExecutionState::OffCpu,
                    weight_ns: event.weight_ns,
                    frames: vec![Frame {
                        function_id: Some(request.function.id),
                        label: request.function.demangled_name.clone(),
                        module: Some(request.function.module.clone()),
                        address: Some(request.function.address),
                    }],
                },
            ),
            _ => profile.quality.events_dropped = profile.quality.events_dropped.saturating_add(1),
        }
    }
    correlator.finish(&mut profile, monotonic_ns());
    rebuild_threads(&mut profile, &thread_names);
    profile.quality.events_generated = profile
        .quality
        .events_generated
        .max(profile.invocations.len() as u64 * 2);
    Ok(profile)
}

/// A Ctrl-C handler interrupts a blocking ring-buffer poll so capture can
/// finish promptly. Treat that EINTR as the requested stop, but preserve all
/// other libbpf polling failures as errors.
fn poll_ring(
    ring: &mut libbpf_rs::RingBuffer,
    timeout: Duration,
    stop_requested: &AtomicBool,
) -> Result<bool> {
    match ring.poll(timeout) {
        Ok(_) => Ok(true),
        Err(error)
            if error.to_string().contains("Interrupted system call")
                && stop_requested.load(Ordering::Relaxed) =>
        {
            Ok(false)
        }
        Err(error) if error.to_string().contains("Interrupted system call") => Ok(true),
        Err(error) => Err(error).context("polling BPF ring buffer"),
    }
}

fn read_counter(map: &libbpf_rs::MapMut<'_>) -> u64 {
    let zero = 0_u32.to_ne_bytes();
    map.lookup(&zero, MapFlags::ANY)
        .ok()
        .flatten()
        .and_then(|bytes| {
            bytes
                .get(..8)
                .map(|bytes| u64::from_ne_bytes(bytes.try_into().unwrap()))
        })
        .unwrap_or(0)
}

fn empty_profile(request: &CaptureRequest) -> Profile {
    Profile {
        format_version: 1,
        metadata: Metadata {
            captured_at_unix_ns: unix_time_ns(),
            command: request.command.clone(),
            kernel_release: std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .unwrap_or_default()
                .trim()
                .to_owned(),
            sample_period_ns: 1_000_000_000 / SAMPLE_FREQUENCY_HZ,
        },
        functions: vec![request.function.clone()],
        threads: Vec::new(),
        invocations: Vec::new(),
        stacks: Vec::new(),
        samples: Vec::new(),
        quality: CaptureQuality::default(),
    }
}

fn unix_time_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn read_thread_names(pid: u32) -> HashMap<u32, String> {
    let task_path = format!("/proc/{pid}/task");
    let Ok(entries) = std::fs::read_dir(task_path) else {
        return HashMap::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let tid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            let name = std::fs::read_to_string(entry.path().join("comm")).ok()?;
            Some((tid, name.trim().to_owned()))
        })
        .collect()
}

fn rebuild_threads(profile: &mut Profile, names: &HashMap<u32, String>) {
    let mut tids = profile
        .invocations
        .iter()
        .map(|invocation| invocation.tid)
        .chain(profile.samples.iter().map(|sample| sample.tid))
        .collect::<Vec<_>>();
    tids.sort_unstable();
    tids.dedup();
    profile.threads = tids
        .into_iter()
        .map(|tid| Thread {
            tid,
            name: names.get(&tid).cloned(),
        })
        .collect();
}

struct Resolver {
    symbols: Vec<(u64, String)>,
    mappings: Vec<Mapping>,
}

#[derive(Clone, Copy, Debug)]
struct Mapping {
    runtime_start: u64,
    runtime_end: u64,
    file_start: u64,
}

impl Resolver {
    fn new(path: &Path, pid: u32) -> Result<Self> {
        let data = std::fs::read(path)?;
        let file = object::File::parse(&*data)?;
        let mut symbols = file
            .symbols()
            .chain(file.dynamic_symbols())
            .filter(|symbol| symbol.kind() == SymbolKind::Text && symbol.address() != 0)
            .filter_map(|symbol| {
                let name = symbol.name().ok()?.to_owned();
                let name = cpp_demangle::Symbol::new(&name)
                    .ok()
                    .and_then(|symbol| symbol.demangle().ok())
                    .unwrap_or(name);
                Some((symbol.address(), name))
            })
            .collect::<Vec<_>>();
        symbols.sort_by_key(|(address, _)| *address);
        symbols.dedup_by_key(|(address, _)| *address);
        let module = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let mut mappings = Vec::new();
        for line in std::fs::read_to_string(format!("/proc/{pid}/maps"))
            .unwrap_or_default()
            .lines()
        {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 6 || !fields[5].starts_with('/') {
                continue;
            }
            let mapped = fields[5].trim_end_matches(" (deleted)");
            if std::fs::canonicalize(mapped).ok().as_deref() != Some(module.as_path()) {
                continue;
            }
            let Some((start, end)) = fields[0].split_once('-').and_then(|(start, end)| {
                Some((
                    u64::from_str_radix(start, 16).ok()?,
                    u64::from_str_radix(end, 16).ok()?,
                ))
            }) else {
                continue;
            };
            let Some(offset) = u64::from_str_radix(fields[2], 16).ok() else {
                continue;
            };
            let Some(segment) = file.segments().find(|segment| {
                let (file_offset, file_size) = segment.file_range();
                offset >= file_offset && offset < file_offset.saturating_add(file_size)
            }) else {
                continue;
            };
            let (file_offset, _) = segment.file_range();
            mappings.push(Mapping {
                runtime_start: start,
                runtime_end: end,
                file_start: segment
                    .address()
                    .saturating_add(offset.saturating_sub(file_offset)),
            });
        }
        Ok(Self { symbols, mappings })
    }
    fn frames(&self, addresses: &[u64]) -> Vec<Frame> {
        let mut frames = addresses
            .iter()
            .map(|address| {
                let file_address = self
                    .mappings
                    .iter()
                    .find(|mapping| {
                        *address >= mapping.runtime_start && *address < mapping.runtime_end
                    })
                    .map(|mapping| {
                        mapping
                            .file_start
                            .saturating_add(*address - mapping.runtime_start)
                    });
                let symbol = file_address.and_then(|file_address| {
                    self.symbols
                        .iter()
                        .rev()
                        .find(|(start, _)| *start <= file_address)
                });
                Frame {
                    function_id: None,
                    label: symbol
                        .map(|(_, name)| name.clone())
                        .unwrap_or_else(|| format!("[unknown 0x{address:x}]")),
                    module: file_address.map(|_| String::from("target")),
                    address: Some(*address),
                }
            })
            .collect::<Vec<_>>();
        frames.reverse();
        frames
    }
}

fn read_event(bytes: &[u8]) -> Event {
    Event {
        kind: u32::from_ne_bytes(bytes[0..4].try_into().unwrap()),
        tid: u32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
        timestamp_ns: u64::from_ne_bytes(bytes[8..16].try_into().unwrap()),
        invocation_id: u64::from_ne_bytes(bytes[16..24].try_into().unwrap()),
        stack_id: i32::from_ne_bytes(bytes[24..28].try_into().unwrap()),
        cpu: u32::from_ne_bytes(bytes[28..32].try_into().unwrap()),
        weight_ns: u64::from_ne_bytes(bytes[32..40].try_into().unwrap()),
    }
}

fn available_cpus() -> Result<usize> {
    Ok(std::thread::available_parallelism()?.get())
}

fn raise_memlock_limit() -> std::io::Result<()> {
    let mut current = MaybeUninit::<libc::rlimit>::uninit();
    let result = unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, current.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let current = unsafe { current.assume_init() };
    if current.rlim_cur >= current.rlim_max {
        return Ok(());
    }
    let raised = libc::rlimit {
        rlim_cur: current.rlim_max,
        rlim_max: current.rlim_max,
    };
    let result = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &raised) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn process_exists(pid: u32) -> bool {
    let status_path = format!("/proc/{pid}/status");
    let Ok(status) = std::fs::read_to_string(status_path) else {
        return false;
    };
    status
        .lines()
        .find(|line| line.starts_with("State:"))
        .and_then(|line| line.split_whitespace().nth(1))
        != Some("Z")
}

fn monotonic_ns() -> u64 {
    unsafe {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }
}

#[repr(C)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup: u32,
    bp_type: u32,
    config1: u64,
    config2: u64,
    branch_sample_type: u64,
    sample_regs_user: u64,
    sample_stack_user: u32,
    clockid: i32,
    sample_regs_intr: u64,
    aux_watermark: u32,
    sample_max_stack: u16,
    __reserved_2: u16,
}

fn open_sampling_event(cpu: usize) -> Result<RawFd> {
    let attr = PerfEventAttr {
        type_: PERF_TYPE_SOFTWARE,
        size: std::mem::size_of::<PerfEventAttr>() as u32,
        config: PERF_COUNT_SW_CPU_CLOCK,
        // PERF_COUNT_SW_CPU_CLOCK accepts a nanosecond period here. Using the
        // attr.freq bit with sample_period=999 makes perf_event_open reject
        // the event with EINVAL on current kernels.
        sample_period: 1_000_000_000 / SAMPLE_FREQUENCY_HZ,
        // Request a conventional IP sample; the BPF program uses the perf
        // overflow primarily as a clock and obtains its user stack itself.
        sample_type: PERF_SAMPLE_IP,
        read_format: 0,
        flags: 1 << 0,
        wakeup: 1,
        bp_type: 0,
        config1: 0,
        config2: 0,
        branch_sample_type: 0,
        sample_regs_user: 0,
        sample_stack_user: 0,
        clockid: 0,
        sample_regs_intr: 0,
        aux_watermark: 0,
        sample_max_stack: 0,
        __reserved_2: 0,
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr,
            -1_i32,
            cpu as i32,
            -1_i32,
            0_u64,
        ) as i32
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("opening perf event on CPU {cpu}"));
    }
    Ok(fd)
}

fn close_fd(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}
