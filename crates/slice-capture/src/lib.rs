//! Platform-neutral ports for live Slice capture.
//!
//! The application and domain layers depend on these contracts. Platform
//! adapters such as `slice-ebpf` implement them without leaking libbpf, perf,
//! `/proc`, capabilities, or signal handling into the CLI.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

use slice_core::{Function, Profile};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    /// PID used by the caller's `/proc` mount and process-control syscalls.
    pub pid: u32,
    /// TGID visible to BPF helpers in the outermost reported PID namespace.
    pub kernel_tgid: u32,
}

#[derive(Clone, Debug)]
pub struct CaptureRequest {
    pub target: ProcessIdentity,
    pub module: PathBuf,
    pub function: Function,
    pub probe_offset: usize,
    pub command: Vec<String>,
    pub stop_requested: Arc<AtomicBool>,
    /// A launched child is stopped before attachment and resumed only after
    /// every probe and sampler link is live.
    pub resume_after_attach: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckStatus {
    Pass,
    Warning,
    Failure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrerequisiteCheck {
    pub key: &'static str,
    pub label: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    pub remediation: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorReport {
    pub adapter: &'static str,
    pub checks: Vec<PrerequisiteCheck>,
}

impl DoctorReport {
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == CheckStatus::Failure)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CaptureError {
    message: String,
}

impl CaptureError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Inbound port implemented by a platform capture adapter.
pub trait CapturePort {
    fn doctor(&self) -> Result<DoctorReport, CaptureError>;
    fn resolve_process_identity(&self, pid: u32) -> Result<ProcessIdentity, CaptureError>;
    fn stop_process(&self, pid: u32) -> Result<(), CaptureError>;
    fn wait_for_stopped(&self, pid: u32) -> Result<(), CaptureError>;
    fn kill_process(&self, pid: u32) -> Result<(), CaptureError>;
    fn interrupt_process(&self, pid: u32) -> Result<(), CaptureError>;
    fn capture(&self, request: &CaptureRequest) -> Result<Profile, CaptureError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_failure_is_derived_from_structured_checks() {
        let report = DoctorReport {
            adapter: "test",
            checks: vec![PrerequisiteCheck {
                key: "kernel",
                label: "kernel",
                status: CheckStatus::Failure,
                detail: "missing".to_owned(),
                remediation: Some("install one".to_owned()),
            }],
        };
        assert!(report.has_failures());
    }
}
