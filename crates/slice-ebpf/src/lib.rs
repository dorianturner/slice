//! Privileged libbpf capture engine.
//!
//! This module is intentionally a thin transport adapter. Invocation validity,
//! deduplicated stacks, and quality counters remain in `slice-collector`, so
//! transport concerns stay separate from profile semantics.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use libbpf_rs::skel::{OpenSkel, SkelBuilder};
use libbpf_rs::{MapCore, MapFlags, RingBufferBuilder, TracepointCategory, UprobeMultiOpts};
use object::{Object, ObjectSegment, ObjectSymbol, SymbolKind};
use slice_capture::{
    CaptureError, CapturePort, CaptureRequest, CheckStatus, DoctorReport, PrerequisiteCheck,
    ProcessIdentity,
};
use slice_collector::{CollectorEvent, Correlator};
use slice_core::{CaptureQuality, ExecutionState, Frame, Metadata, Profile, Thread};

mod bpf {
    include!(concat!(env!("OUT_DIR"), "/slice.skel.rs"));
}

const SAMPLE_FREQUENCY_HZ: u64 = 999;
const PERF_TYPE_SOFTWARE: u32 = 1;
const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
const PERF_SAMPLE_IP: u64 = 1 << 0;

#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxCaptureAdapter;

impl CapturePort for LinuxCaptureAdapter {
    fn doctor(&self) -> Result<DoctorReport, CaptureError> {
        Ok(linux_doctor_report())
    }

    fn resolve_process_identity(&self, pid: u32) -> Result<ProcessIdentity, CaptureError> {
        resolve_process_identity(pid).map_err(port_error)
    }

    fn stop_process(&self, pid: u32) -> Result<(), CaptureError> {
        stop_process(pid).map_err(port_error)
    }

    fn wait_for_stopped(&self, pid: u32) -> Result<(), CaptureError> {
        wait_for_stopped(pid).map_err(port_error)
    }

    fn kill_process(&self, pid: u32) -> Result<(), CaptureError> {
        kill_process(pid).map_err(port_error)
    }

    fn interrupt_process(&self, pid: u32) -> Result<(), CaptureError> {
        interrupt_process(pid).map_err(port_error)
    }

    fn capture(&self, request: &CaptureRequest) -> Result<Profile, CaptureError> {
        capture_pid(request).map_err(port_error)
    }
}

fn port_error(error: anyhow::Error) -> CaptureError {
    CaptureError::new(format!("{error:#}"))
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

fn resolve_process_identity(pid: u32) -> Result<ProcessIdentity> {
    if pid == 0 {
        bail!("PID 0 is not a valid capture target");
    }
    let status_path = format!("/proc/{pid}/status");
    let status = std::fs::read_to_string(&status_path)
        .with_context(|| format!("reading process identity from {status_path}"))?;
    let kernel_tgid = parse_kernel_tgid(&status)
        .context("process status contained neither a valid NSpid nor Tgid")?;
    Ok(ProcessIdentity { pid, kernel_tgid })
}

fn parse_kernel_tgid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u32>().ok())
        .or_else(|| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Tgid:"))
                .and_then(|value| value.trim().parse::<u32>().ok())
        })
}

fn linux_doctor_report() -> DoctorReport {
    let kernel_release = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_else(|_| "unavailable".to_owned());
    let kernel_release = kernel_release.trim();
    let btf_path = Path::new("/sys/kernel/btf/vmlinux");
    let btf_available = btf_path.is_file();
    let uprobe_multi_named = std::fs::read(btf_path)
        .ok()
        .is_some_and(|btf| contains_bytes(&btf, b"BPF_TRACE_UPROBE_MULTI\0"));
    let tracepoint = tracepoint_access();
    let is_root = unsafe { libc::geteuid() } == 0;
    let has_bpf = effective_capability(39);
    let has_perfmon = effective_capability(38);
    let has_ptrace = effective_capability(19);
    let capture_authorized = is_root || (has_bpf && has_perfmon && has_ptrace);

    let mut checks = vec![
        PrerequisiteCheck {
            key: "architecture",
            label: "architecture",
            status: if std::env::consts::ARCH == "x86_64" {
                CheckStatus::Pass
            } else {
                CheckStatus::Failure
            },
            detail: std::env::consts::ARCH.to_owned(),
            remediation: (std::env::consts::ARCH != "x86_64")
                .then(|| "run Slice on Linux x86-64".to_owned()),
        },
        PrerequisiteCheck {
            key: "kernel",
            label: "kernel release",
            status: if kernel_at_least(kernel_release, 6, 6) {
                CheckStatus::Pass
            } else {
                CheckStatus::Failure
            },
            detail: kernel_release.to_owned(),
            remediation: (!kernel_at_least(kernel_release, 6, 6)).then(|| {
                "upgrade to Linux 6.6+; for WSL run `wsl --update` and `wsl --shutdown` in Administrator PowerShell"
                    .to_owned()
            }),
        },
        PrerequisiteCheck {
            key: "btf",
            label: "kernel BTF",
            status: if btf_available {
                CheckStatus::Pass
            } else {
                CheckStatus::Failure
            },
            detail: if btf_available {
                "available".to_owned()
            } else {
                "missing /sys/kernel/btf/vmlinux".to_owned()
            },
            remediation: (!btf_available)
                .then(|| "boot a kernel built with BTF information".to_owned()),
        },
        PrerequisiteCheck {
            key: "uprobe_multi",
            label: "process-wide uprobe-multi links",
            status: if uprobe_multi_named {
                CheckStatus::Pass
            } else {
                CheckStatus::Failure
            },
            detail: if uprobe_multi_named {
                "BPF_TRACE_UPROBE_MULTI present in kernel BTF".to_owned()
            } else {
                "BPF_TRACE_UPROBE_MULTI not found in kernel BTF".to_owned()
            },
            remediation: (!uprobe_multi_named)
                .then(|| "boot a Linux 6.6+ kernel with CONFIG_UPROBES and CONFIG_BPF_EVENTS".to_owned()),
        },
        PrerequisiteCheck {
            key: "sched_switch",
            label: "sched_switch tracepoint",
            status: match tracepoint {
                AccessStatus::Available => CheckStatus::Pass,
                AccessStatus::PermissionDenied if !capture_authorized => CheckStatus::Warning,
                AccessStatus::PermissionDenied | AccessStatus::Missing => CheckStatus::Failure,
            },
            detail: tracepoint.as_str().to_owned(),
            remediation: match tracepoint {
                AccessStatus::Available => None,
                AccessStatus::PermissionDenied => {
                    Some("rerun `slice doctor` with the same sudo/capabilities used for capture".to_owned())
                }
                AccessStatus::Missing => Some("mount tracefs and enable sched_switch tracing".to_owned()),
            },
        },
        PrerequisiteCheck {
            key: "authority",
            label: "capture authority",
            status: if capture_authorized {
                CheckStatus::Pass
            } else {
                CheckStatus::Warning
            },
            detail: if is_root {
                "root".to_owned()
            } else {
                format!(
                    "CAP_BPF={}, CAP_PERFMON={}, CAP_SYS_PTRACE={}",
                    yes_no(has_bpf),
                    yes_no(has_perfmon),
                    yes_no(has_ptrace)
                )
            },
            remediation: (!capture_authorized).then(|| {
                "run `slice doctor` and `slice profile` with sudo, or grant CAP_BPF,CAP_PERFMON,CAP_SYS_PTRACE"
                    .to_owned()
            }),
        },
    ];

    let perf = std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .unwrap_or_else(|_| "unavailable".to_owned());
    checks.push(PrerequisiteCheck {
        key: "perf_event_paranoid",
        label: "perf_event_paranoid",
        status: CheckStatus::Pass,
        detail: perf.trim().to_owned(),
        remediation: None,
    });
    let unprivileged_bpf = std::fs::read_to_string("/proc/sys/kernel/unprivileged_bpf_disabled")
        .unwrap_or_else(|_| "unavailable".to_owned());
    checks.push(PrerequisiteCheck {
        key: "unprivileged_bpf_disabled",
        label: "unprivileged_bpf_disabled",
        status: CheckStatus::Pass,
        detail: unprivileged_bpf.trim().to_owned(),
        remediation: None,
    });
    let memlock = std::fs::read_to_string("/proc/self/limits")
        .ok()
        .and_then(|limits| {
            limits
                .lines()
                .find(|line| line.starts_with("Max locked memory"))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unavailable".to_owned());
    checks.push(PrerequisiteCheck {
        key: "memlock",
        label: "memlock limit",
        status: CheckStatus::Pass,
        detail: memlock,
        remediation: None,
    });
    if kernel_release.to_ascii_lowercase().contains("microsoft") {
        checks.push(PrerequisiteCheck {
            key: "wsl",
            label: "WSL",
            status: CheckStatus::Pass,
            detail: "WSL 2 detected".to_owned(),
            remediation: None,
        });
    }

    DoctorReport {
        adapter: "linux-ebpf",
        checks,
    }
}

#[derive(Clone, Copy)]
enum AccessStatus {
    Available,
    PermissionDenied,
    Missing,
}

impl AccessStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::PermissionDenied => "permission denied",
            Self::Missing => "missing",
        }
    }
}

fn tracepoint_access() -> AccessStatus {
    let paths = [
        "/sys/kernel/tracing/events/sched/sched_switch/format",
        "/sys/kernel/debug/tracing/events/sched/sched_switch/format",
    ];
    let mut permission_denied = false;
    for path in paths {
        match std::fs::read(path) {
            Ok(_) => return AccessStatus::Available,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                permission_denied = true;
            }
            Err(_) => {}
        }
    }
    if permission_denied {
        AccessStatus::PermissionDenied
    } else {
        AccessStatus::Missing
    }
}

fn kernel_at_least(release: &str, required_major: u32, required_minor: u32) -> bool {
    let mut components = release.split('.');
    components
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .zip(
            components
                .next()
                .and_then(|minor| minor.parse::<u32>().ok()),
        )
        .is_some_and(|version| version >= (required_major, required_minor))
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn effective_capability(bit: u32) -> bool {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:\t"))
        .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
        .is_some_and(|value| value & (1_u64 << bit) != 0)
}

const fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
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
    if request.target.pid == 0 || request.target.kernel_tgid == 0 {
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
    let resolver = Resolver::new(&request.module, request.target.pid)?;
    let thread_names = read_thread_names(request.target.pid);

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
        "loading Slice BPF program; check CAP_BPF/CAP_PERFMON, RLIMIT_MEMLOCK, and Linux 6.6+ BPF uprobe-multi support",
    )?;

    let config = [request.function.id, 0];
    let mut config_bytes = Vec::with_capacity(16);
    config_bytes.extend_from_slice(&config[0].to_ne_bytes());
    config_bytes.extend_from_slice(&config[1].to_ne_bytes());
    config_bytes.extend_from_slice(&(1_000_000_000_u64 / SAMPLE_FREQUENCY_HZ).to_ne_bytes());
    let zero = 0_u32.to_ne_bytes();
    skel.maps
        .config
        .update(&zero, &config_bytes, MapFlags::ANY)
        .context("configuring capture function and sample period")?;
    skel.maps
        .next_invocation_id
        .update(&zero, &1_u64.to_ne_bytes(), MapFlags::ANY)
        .context("initializing invocation ID map")?;

    let cpu_ids = available_cpu_ids()?;
    let mut uprobe_links = Vec::new();
    // A uprobe-multi link scopes by the target process's shared address space,
    // so it follows existing and future pthreads without perf-event
    // inheritance. Singular task-bound perf uprobes only cover one thread;
    // setting perf's inherit bit is invalid because config1 is a userspace
    // pathname pointer that clone may dereference in the target address space.
    let module = std::fs::canonicalize(&request.module)
        .with_context(|| format!("canonicalizing {}", request.module.display()))?;
    let entry_link = attach_process_uprobe(
        &skel.progs.slice_entry,
        &module,
        request.probe_offset,
        false,
        request.target.pid,
    )?;
    uprobe_links.push(entry_link);
    let return_link = attach_process_uprobe(
        &skel.progs.slice_return,
        &module,
        request.probe_offset,
        true,
        request.target.pid,
    )?;
    uprobe_links.push(return_link);
    let sched_link = skel
        .progs
        .slice_sched_switch
        .attach_tracepoint(TracepointCategory::Sched, "sched_switch")
        .context("attaching sched_switch tracepoint")?;
    let mut perf_links = Vec::new();
    for cpu in cpu_ids {
        let fd = open_sampling_event(cpu)?;
        let link = match skel.progs.slice_sample.attach_perf_event(fd) {
            Ok(link) => link,
            Err(error) => {
                close_fd(fd);
                return Err(error).with_context(|| format!("attaching perf sampler on CPU {cpu}"));
            }
        };
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
        let result = unsafe { libc::kill(request.target.pid as i32, libc::SIGCONT) };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).context("resuming launched target");
        }
    }
    while process_exists(request.target.pid) && !request.stop_requested.load(Ordering::Relaxed) {
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
    drop(uprobe_links);
    drop(sched_link);

    let mut profile = empty_profile(request);
    profile.quality.events_dropped = read_counter(&skel.maps.dropped_events);
    profile.quality.samples_dropped = read_counter(&skel.maps.dropped_samples);
    let raw_entries = read_indexed_counter(&skel.maps.probe_diagnostics, 0);
    let accepted_entries = read_indexed_counter(&skel.maps.probe_diagnostics, 1);
    let raw_returns = read_indexed_counter(&skel.maps.probe_diagnostics, 2);
    let scheduler_switch_outs = read_indexed_counter(&skel.maps.probe_diagnostics, 3);
    let scheduler_intervals = read_indexed_counter(&skel.maps.probe_diagnostics, 4);
    if events_seen.lock().unwrap().is_empty() {
        let observed_tgid = read_u32(&skel.maps.observed_tgid, 0);
        eprintln!(
            "capture diagnostics:\n  attachment: {raw_entries} raw entry hits, {raw_returns} raw return hits\n  transport: {accepted_entries} accepted entries, 0 ring-buffer events consumed, {} dropped events, {} dropped samples\n  identity: userspace PID {}, resolved kernel TGID {}, observed BPF TGID {}\n  interpretation: {}",
            profile.quality.events_dropped,
            profile.quality.samples_dropped,
            request.target.pid,
            request.target.kernel_tgid,
            observed_tgid.map_or_else(|| "unavailable".to_owned(), |tgid| tgid.to_string()),
            diagnose_empty_capture(raw_entries, accepted_entries),
        );
    }
    let mut events = events_seen.lock().unwrap().clone();
    sort_events(&mut events);
    let mut correlator = Correlator::default();
    for event in events {
        match event.kind {
            1 => correlator.push(
                &mut profile,
                CollectorEvent::Entry {
                    invocation_id: Some(event.invocation_id),
                    function_id: request.function.id,
                    tid: event.tid,
                    timestamp_ns: event.timestamp_ns,
                },
            ),
            2 => correlator.push(
                &mut profile,
                CollectorEvent::Return {
                    invocation_id: Some(event.invocation_id),
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
                        invocation_id: Some(event.invocation_id),
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
                    invocation_id: Some(event.invocation_id),
                    tid: event.tid,
                    timestamp_ns: event.timestamp_ns,
                    cpu: event.cpu,
                    state: ExecutionState::OffCpu,
                    weight_ns: event.weight_ns,
                    frames: {
                        let addresses = stacks
                            .lock()
                            .unwrap()
                            .get(&event.stack_id)
                            .cloned()
                            .unwrap_or_default();
                        let mut frames = resolver.frames(&addresses);
                        if !frames.iter().any(|frame| {
                            frame.label == request.function.demangled_name
                                || frame.function_id == Some(request.function.id)
                        }) {
                            frames.insert(
                                0,
                                Frame {
                                    function_id: Some(request.function.id),
                                    label: request.function.demangled_name.clone(),
                                    module: Some(request.function.module.clone()),
                                    address: Some(request.function.address),
                                },
                            );
                        }
                        frames
                    },
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
    if scheduler_switch_outs > 0 && scheduler_intervals == 0 {
        eprintln!(
            "capture warning: scheduler observed {scheduler_switch_outs} active switch-outs but matched 0 switch-ins; the platform scheduler identity adapter did not correlate off-CPU intervals"
        );
    }
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
    read_indexed_counter(map, 0)
}

fn read_indexed_counter(map: &libbpf_rs::MapMut<'_>, index: u32) -> u64 {
    map.lookup(&index.to_ne_bytes(), MapFlags::ANY)
        .ok()
        .flatten()
        .and_then(|bytes| {
            bytes
                .get(..8)
                .map(|bytes| u64::from_ne_bytes(bytes.try_into().unwrap()))
        })
        .unwrap_or(0)
}

fn read_u32(map: &libbpf_rs::MapMut<'_>, index: u32) -> Option<u32> {
    map.lookup(&index.to_ne_bytes(), MapFlags::ANY)
        .ok()
        .flatten()
        .and_then(|bytes| bytes.get(..4)?.try_into().ok().map(u32::from_ne_bytes))
}

fn diagnose_empty_capture(raw_entries: u64, accepted_entries: u64) -> &'static str {
    if raw_entries == 0 {
        "the selected probe never fired; verify the module, symbol offset, and target lifetime"
    } else if accepted_entries == 0 {
        "the attachment fired but the adapter rejected every entry before transport"
    } else {
        "entries passed process scoping but no events reached userspace; inspect ring-buffer drops and BPF map updates"
    }
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

fn event_kind_order(kind: u32) -> u32 {
    match kind {
        1 => 0,     // entry
        3 | 5 => 1, // on/off-CPU sample
        2 => 2,     // return
        4 => 3,     // violation
        _ => 4,
    }
}

fn sort_events(events: &mut [Event]) {
    events.sort_by(|left, right| {
        left.timestamp_ns
            .cmp(&right.timestamp_ns)
            .then_with(|| left.tid.cmp(&right.tid))
            // BPF invocation IDs are allocated in entry order. They make
            // same-timestamp boundaries deterministic even when a target
            // migrates between CPUs while the ring buffer is being drained.
            .then_with(|| left.invocation_id.cmp(&right.invocation_id))
            .then_with(|| event_kind_order(left.kind).cmp(&event_kind_order(right.kind)))
    });
}

fn available_cpu_ids() -> Result<Vec<usize>> {
    let online = match std::fs::read_to_string("/sys/devices/system/cpu/online") {
        Ok(online) => online,
        Err(_) => {
            return Ok((0..std::thread::available_parallelism()?.get()).collect());
        }
    };
    let mut cpus = Vec::new();
    for range in online.trim().split(',').filter(|range| !range.is_empty()) {
        let (first, last) = range
            .split_once('-')
            .map_or((range, range), |(first, last)| (first, last));
        let first = first
            .parse::<usize>()
            .with_context(|| format!("parsing online CPU range {range:?}"))?;
        let last = last
            .parse::<usize>()
            .with_context(|| format!("parsing online CPU range {range:?}"))?;
        if last < first {
            bail!("online CPU range is reversed: {range:?}");
        }
        cpus.extend(first..=last);
    }
    if cpus.is_empty() {
        bail!("/sys/devices/system/cpu/online contained no CPUs");
    }
    Ok(cpus)
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

fn attach_process_uprobe(
    program: &libbpf_rs::ProgramMut,
    module: &Path,
    offset: usize,
    retprobe: bool,
    pid: u32,
) -> Result<libbpf_rs::Link> {
    let opts = UprobeMultiOpts {
        offsets: vec![offset],
        retprobe,
        ..Default::default()
    };
    program
        .attach_uprobe_multi_with_opts(pid as i32, module, "", opts)
        .with_context(|| {
            format!(
                "attaching process-wide {} uprobe for PID {pid}; Slice requires Linux 6.6 or newer with BPF uprobe-multi support",
                if retprobe { "return" } else { "entry" }
            )
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kernel_floor_without_assuming_distribution_suffixes() {
        assert!(kernel_at_least("6.18.33.2-microsoft-standard-WSL2", 6, 6));
        assert!(kernel_at_least("6.8.0-ubuntu", 6, 6));
        assert!(!kernel_at_least("5.15.167.4-microsoft", 6, 6));
        assert!(!kernel_at_least("unavailable", 6, 6));
    }

    #[test]
    fn resolves_current_process_identity_from_proc() {
        let identity = resolve_process_identity(std::process::id()).unwrap();
        assert_eq!(identity.pid, std::process::id());
        assert_ne!(identity.kernel_tgid, 0);
    }

    #[test]
    fn process_identity_prefers_outermost_namespace_pid() {
        assert_eq!(
            parse_kernel_tgid("Tgid:\t41\nNSpid:\t9001\t41\n"),
            Some(9001)
        );
        assert_eq!(parse_kernel_tgid("Tgid:\t41\n"), Some(41));
    }

    #[test]
    fn empty_capture_diagnosis_distinguishes_attachment_and_transport() {
        assert!(diagnose_empty_capture(0, 0).contains("never fired"));
        assert!(diagnose_empty_capture(4_000, 0).contains("rejected"));
        assert!(diagnose_empty_capture(4_000, 4_000).contains("userspace"));
    }

    #[test]
    fn replay_orders_events_by_kernel_time_not_ring_delivery_order() {
        let mut events = vec![
            Event {
                kind: 2,
                tid: 42,
                timestamp_ns: 30,
                invocation_id: 2,
                ..Event::default()
            },
            Event {
                kind: 1,
                tid: 42,
                timestamp_ns: 10,
                invocation_id: 1,
                ..Event::default()
            },
            Event {
                kind: 5,
                tid: 42,
                timestamp_ns: 20,
                invocation_id: 1,
                ..Event::default()
            },
        ];
        sort_events(&mut events);
        assert_eq!(
            events
                .iter()
                .map(|event| (event.kind, event.invocation_id))
                .collect::<Vec<_>>(),
            vec![(1, 1), (5, 1), (2, 2)]
        );
    }

    #[test]
    fn finds_named_kernel_feature_in_binary_btf_data() {
        assert!(contains_bytes(
            b"prefix\0BPF_TRACE_UPROBE_MULTI\0suffix",
            b"BPF_TRACE_UPROBE_MULTI\0"
        ));
    }
}
