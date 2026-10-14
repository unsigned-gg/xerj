//! Sandboxed command execution with dedicated OS-thread confinement, fail-closed
//! enforcement, and optional out-of-band containment evidence verification.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::evidence::{
    parse_evidence, EnforcedLayer, EvidenceChannel, EvidenceEmitError, EvidenceParseError,
    Observation,
};
use crate::jail::{
    current_thread_is_jailed, jail_write_roots, JailBuilder, JailError, JAIL_MECHANISM,
};
use crate::rules::AccessRules;

/// Resource limits applied to a sandboxed command execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum wall-clock execution time in seconds (`0` disables timeout).
    pub max_duration_secs: u64,
    /// Advisory memory ceiling in bytes (`0` means unspecified).
    pub max_memory_bytes: u64,
    /// Maximum bytes captured from each of `stdout` and `stderr` (`0` means unlimited).
    pub max_output_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_duration_secs: 60,
            max_memory_bytes: 512 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
        }
    }
}

/// Specification for executing a command inside an OS sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRequest {
    /// Command argv (`command[0]` is executable, `command[1..]` are arguments).
    pub command: Vec<String>,
    /// Working directory for the child process (also added to write roots via
    /// [`jail_write_roots`]).
    pub workdir: PathBuf,
    /// Additional read-only filesystem roots granted to the jail.
    pub read_paths: Vec<PathBuf>,
    /// Additional read-write filesystem roots granted to the jail.
    pub write_paths: Vec<PathBuf>,
    /// Whether loopback (`localhost`) TCP connections are permitted.
    pub allow_localhost_tcp: bool,
    /// Environment variables explicitly set on the child process.
    pub env: BTreeMap<String, String>,
    /// Resource limits for runtime and output capture.
    pub limits: ResourceLimits,
    /// Explicit opt-out when the host kernel lacks Landlock/Seatbelt support.
    /// When `false` (the default), execution fails closed with
    /// [`SandboxError::EnforcementUnavailable`] if `!jail.enforced()`.
    pub unjailed: bool,
    /// Optional supervisor-owned directory outside the jail's write roots used
    /// to run the pre-jail DAC control probe and post-jail `EACCES` containment
    /// probe over an OS pipe, producing a verified [`EnforcedLayer`].
    pub probe_dir: Option<PathBuf>,
}

impl SandboxRequest {
    /// Create a default fail-closed [`SandboxRequest`] for `command` running in `workdir`.
    pub fn new(command: Vec<String>, workdir: impl Into<PathBuf>) -> Self {
        Self {
            command,
            workdir: workdir.into(),
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            allow_localhost_tcp: true,
            env: BTreeMap::new(),
            limits: ResourceLimits::default(),
            unjailed: false,
            probe_dir: None,
        }
    }

    /// Populate `write_paths` with [`jail_write_roots`] for `self.workdir` and
    /// an optional `store_root` (including [`std::env::temp_dir`]).
    pub fn with_store_write_roots(mut self, store_root: Option<&std::path::Path>) -> Self {
        for root in jail_write_roots(&self.workdir, store_root) {
            if root != self.workdir && !self.write_paths.contains(&root) {
                self.write_paths.push(root);
            }
        }
        self
    }
}

/// Completed outcome of a sandboxed command execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub mechanism: String,
    pub enforced: bool,
    pub enforced_layer: Option<EnforcedLayer>,
}

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("sandbox command must not be empty")]
    EmptyCommand,
    #[error("failed to enter OS jail: {0}")]
    Jail(#[from] JailError),
    #[error(
        "OS jail ({mechanism}) is NOT enforced on this host kernel; refusing to execute \
         command unjailed (set unjailed = true only when explicitly authorized)"
    )]
    EnforcementUnavailable { mechanism: &'static str },
    #[error("containment evidence emission failed: {0}")]
    EvidenceEmit(#[from] EvidenceEmitError),
    #[error("containment evidence verification failed: {0}")]
    EvidenceParse(#[from] EvidenceParseError),
    #[error("failed to spawn or wait for sandboxed process: {0}")]
    Spawn(#[source] io::Error),
    #[error("sandboxed command exceeded timeout of {limit_secs}s")]
    Timeout { limit_secs: u64 },
    #[error("jailed worker thread panicked before returning a result")]
    WorkerPanic,
}

/// Pluggable executor interface for running [`SandboxRequest`] specifications.
pub trait SandboxExecutor: Send + Sync {
    fn execute(&self, req: &SandboxRequest) -> Result<SandboxResult, SandboxError>;
}

/// Production [`SandboxExecutor`] backed by [`crate::jail::Jail`] (Linux
/// Landlock V2 / macOS Seatbelt) on a dedicated OS thread per invocation.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsSandboxExecutor;

impl OsSandboxExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl SandboxExecutor for OsSandboxExecutor {
    fn execute(&self, req: &SandboxRequest) -> Result<SandboxResult, SandboxError> {
        if req.command.is_empty() {
            return Err(SandboxError::EmptyCommand);
        }

        let req_owned = req.clone();
        let handle = std::thread::spawn(move || execute_on_dedicated_thread(req_owned));
        handle.join().map_err(|_| SandboxError::WorkerPanic)?
    }
}

fn execute_on_dedicated_thread(req: SandboxRequest) -> Result<SandboxResult, SandboxError> {
    let start = Instant::now();

    // If a supervisor probe_dir is configured, set up an OS pipe and run the
    // pre-jail DAC control write BEFORE calling Jail::enter.
    let mut evidence_pipe: Option<(EvidenceChannel, std::fs::File, PathBuf)> = None;
    if let Some(ref probe_dir) = req.probe_dir {
        let mut fds = [0 as libc::c_int; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if rc != 0 {
            return Err(SandboxError::Spawn(io::Error::last_os_error()));
        }
        let read_file = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        let write_fd = fds[1];
        match EvidenceChannel::from_raw_fd_with_probe_dir(write_fd, probe_dir.clone()) {
            Ok(channel) => {
                evidence_pipe = Some((channel, read_file, probe_dir.clone()));
            }
            Err(err) => {
                unsafe {
                    libc::close(write_fd);
                }
                return Err(SandboxError::EvidenceEmit(err));
            }
        }
    }

    // Build AccessRules from workdir + caller-specified write_paths and read_paths.
    let mut rules = AccessRules {
        read_paths: Vec::new(),
        write_paths: vec![req.workdir.clone()],
        allow_localhost_tcp: req.allow_localhost_tcp,
    };
    for write_path in &req.write_paths {
        if !rules.write_paths.contains(write_path) {
            rules = rules.with_write(write_path);
        }
    }
    for read_path in &req.read_paths {
        if !rules.read_paths.contains(read_path) {
            rules = rules.with_read(read_path);
        }
    }

    let jail = JailBuilder::new(rules).enter()?;
    if !jail.enforced() && !req.unjailed {
        return Err(SandboxError::EnforcementUnavailable {
            mechanism: JAIL_MECHANISM,
        });
    }

    if jail.enforced() {
        debug_assert!(
            current_thread_is_jailed(),
            "Command::spawn MUST run on the same OS thread that called Jail::enter"
        );
    }

    // Emit and verify post-jail containment evidence if requested and enforced.
    let enforced_layer = if let Some((channel, mut read_file, probe_dir)) = evidence_pipe {
        let obs = Observation::from_jail(&jail, channel.probe_dir());
        channel.emit(&obs)?;
        let mut raw_bytes = Vec::new();
        read_file
            .read_to_end(&mut raw_bytes)
            .map_err(SandboxError::Spawn)?;
        if jail.enforced() && cfg!(target_os = "linux") {
            let parsed = parse_evidence(&raw_bytes, &probe_dir)?;
            Some(parsed.into_enforced_layer())
        } else {
            None
        }
    } else {
        None
    };

    let prefix = jail.command_prefix();
    let mut full_cmd = prefix;
    full_cmd.extend(req.command.iter().cloned());

    let mut cmd = Command::new(&full_cmd[0]);
    if full_cmd.len() > 1 {
        cmd.args(&full_cmd[1..]);
    }
    cmd.current_dir(&req.workdir)
        .envs(&req.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(SandboxError::Spawn)?;

    // Read stdout and stderr on helper threads so pipe buffers never deadlock.
    let max_out = req.limits.max_output_bytes;
    let stdout_handle = child
        .stdout
        .take()
        .map(|pipe| std::thread::spawn(move || read_bounded(pipe, max_out)));
    let stderr_handle = child
        .stderr
        .take()
        .map(|pipe| std::thread::spawn(move || read_bounded(pipe, max_out)));

    let status = if req.limits.max_duration_secs == 0 {
        child.wait().map_err(SandboxError::Spawn)?
    } else {
        let timeout = Duration::from_secs(req.limits.max_duration_secs);
        loop {
            if let Some(status) = child.try_wait().map_err(SandboxError::Spawn)? {
                break status;
            }
            if start.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SandboxError::Timeout {
                    limit_secs: req.limits.max_duration_secs,
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    let stdout_bytes = stdout_handle
        .and_then(|h| h.join().ok())
        .transpose()
        .map_err(SandboxError::Spawn)?
        .unwrap_or_default();
    let stderr_bytes = stderr_handle
        .and_then(|h| h.join().ok())
        .transpose()
        .map_err(SandboxError::Spawn)?
        .unwrap_or_default();

    let duration_ms = start.elapsed().as_millis() as u64;
    Ok(SandboxResult {
        exit_code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        duration_ms,
        mechanism: jail.mechanism().to_string(),
        enforced: jail.enforced(),
        enforced_layer,
    })
}

fn read_bounded(mut reader: impl Read, max_bytes: usize) -> io::Result<Vec<u8>> {
    if max_bytes == 0 {
        let mut out = Vec::new();
        reader.read_to_end(&mut out)?;
        return Ok(out);
    }
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(out.len());
        if remaining > 0 {
            let take = remaining.min(n);
            out.extend_from_slice(&chunk[..take]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_command_is_rejected() {
        let ws = TempDir::new().unwrap();
        let exec = OsSandboxExecutor::new();
        let req = SandboxRequest::new(vec![], ws.path());
        assert!(matches!(
            exec.execute(&req),
            Err(SandboxError::EmptyCommand)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn executes_jailed_command_with_containment_evidence_and_blocks_outside_writes() {
        let ws = TempDir::new().unwrap();
        let forbidden = TempDir::new().unwrap();
        let probe_dir = TempDir::new().unwrap();

        let inside_target = ws.path().join("allowed.txt");
        let outside_target = forbidden.path().join("forbidden.txt");

        let script = format!(
            "echo -n ok > '{}' && (echo -n bad > '{}' 2>/dev/null || echo denied)",
            inside_target.display(),
            outside_target.display()
        );

        let mut req = SandboxRequest::new(
            vec!["/bin/sh".to_string(), "-c".to_string(), script],
            ws.path(),
        );
        req.unjailed = true; // allow fallback if host kernel lacks Landlock
        req.probe_dir = Some(probe_dir.path().to_path_buf());

        let exec = OsSandboxExecutor::new();
        let res = exec.execute(&req).expect("sandbox execution failed");
        assert_eq!(res.exit_code, 0);
        assert_eq!(std::fs::read_to_string(&inside_target).unwrap(), "ok");

        if res.enforced {
            assert!(
                !outside_target.exists(),
                "Landlock must prevent child process from creating file outside write_paths"
            );
            assert_eq!(res.stdout.trim(), "denied");
            let layer = res
                .enforced_layer
                .expect("enforced Linux run with probe_dir must return verified EnforcedLayer");
            assert_eq!(layer.tier, "process");
            assert_eq!(layer.layer, "landlock");
        }
    }
}
