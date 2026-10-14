//! Out-of-band containment-evidence emission and supervisor verification.
//!
//! Ported from `sbx` (`crates/sbx/src/evidence.rs` and `crates/sbx-serve/src/evidence.rs`):
//!
//! 1. **Child / Jailed Emitter ([`EvidenceChannel`], [`Observation`]):**
//!    - Immediately sets `FD_CLOEXEC` on the inherited pipe file descriptor
//!      (Condition C1) so the untrusted child command cannot forge a second line.
//!    - Runs a pre-jail DAC control write inside `probe_dir` before [`Jail::enter`],
//!      proving `EACCES` after `Jail::enter` comes from Landlock rather than POSIX
//!      mode bits.
//!    - Probes a sibling path in `probe_dir` post-jail, records the `errno`,
//!      writes a single newline-terminated JSON record, and performs a checked
//!      `libc::close(fd)` (Condition C2) before spawning the user command.
//!
//! 2. **Supervisor Parser & Verifier ([`parse_evidence`], [`ParsedEvidence`], [`EnforcedLayer`]):**
//!    - Enforces single-line framing (`<= 4096` bytes, exactly one `\n`).
//!    - Parses with `#[serde(deny_unknown_fields)]` across all wire structs.
//!    - Validates RFC 3339 UTC timestamp, `enforced == true`, `abi` and
//!      `handled_access_mask` in `1..=2^53 - 1`, `errno == "EACCES"`, and
//!      canonicalized containment of `probe.path` inside `probe_dir`.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::jail::{handled_access_mask, Jail, LANDLOCK_ABI};

/// Primary environment variable holding the decimal file descriptor number of
/// the supervisor-supplied evidence pipe.
pub const EVIDENCE_CHANNEL_ENV: &str = "XERJ_SBX_EVIDENCE_FD";

/// Legacy environment variable accepted as a fallback by [`take_from_env`] for
/// compatibility with standalone `sbx` supervisors.
pub const LEGACY_EVIDENCE_CHANNEL_ENV: &str = "SBX_EVIDENCE_FD";

/// Environment variable pointing to a supervisor-owned directory that sits
/// **outside** the jail's `write_paths` (and outside `/tmp`) while remaining
/// writable by the current UID under standard POSIX DAC before [`Jail::enter`].
pub const EVIDENCE_PROBE_DIR_ENV: &str = "XERJ_SBX_EVIDENCE_PROBE_DIR";

/// Legacy probe dir environment variable fallback.
pub const LEGACY_EVIDENCE_PROBE_DIR_ENV: &str = "SBX_EVIDENCE_PROBE_DIR";

/// Default directory probed after [`Jail::enter`] when no explicit probe dir
/// is supplied.
pub const DEFAULT_DENIED_PROBE_DIR: &str = "/var/tmp";

/// Maximum accepted byte length of the raw evidence pipe payload (including the
/// trailing `\n`).
pub const MAX_EVIDENCE_BYTES: usize = 4096;

/// Upper bound of the IEEE-754 double-precision exact-integer range (`2^53 - 1`).
pub const JS_MAX_SAFE_INT: u64 = 9_007_199_254_740_991;

/// Canonical tier name for the `process` containment tier (`landlock`).
pub const PROCESS_TIER: &str = "process";

/// Canonical layer name for the `process` containment tier (`landlock`).
pub const LANDLOCK_LAYER: &str = "landlock";

#[derive(Debug, Error)]
pub enum EvidenceEmitError {
    #[error("{var} is set to {raw:?}, which is not a valid file descriptor (>= 3): {reason}")]
    InvalidFdEnv {
        var: &'static str,
        raw: String,
        reason: &'static str,
    },
    #[error("failed to set FD_CLOEXEC on evidence pipe fd {fd}: {source}")]
    Cloexec { fd: RawFd, source: io::Error },
    #[error(
        "pre-jail control probe in {dir} failed before Jail::enter ({errno}): \
         cannot attribute a post-jail EACCES to Landlock when the probe directory \
         is already non-writable under DAC/mount policy"
    )]
    PreJailControlFailed {
        dir: PathBuf,
        errno: String,
        #[source]
        source: io::Error,
    },
    #[error("failed to serialize containment evidence JSON: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("failed to write containment evidence to pipe fd {fd}: {source}")]
    Write { fd: RawFd, source: io::Error },
    #[error("failed to close containment evidence pipe fd {fd}: {source}")]
    Close { fd: RawFd, source: io::Error },
}

/// Handle to the inherited containment-evidence pipe, plus the directory in
/// which the pre-jail control write and post-jail denial probe run.
#[derive(Debug)]
pub struct EvidenceChannel {
    fd: RawFd,
    probe_dir: PathBuf,
}

/// Wire payload emitted as a single `\n`-terminated JSON line on the evidence pipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFrame {
    pub observed_at: String,
    pub evidence: LandlockEvidence,
}

/// Landlock-specific containment evidence recorded immediately after [`Jail::enter`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandlockEvidence {
    pub abi: u32,
    pub handled_access_mask: u64,
    pub enforced: bool,
    pub probe: ProbeResult,
}

/// Result of attempting a denied filesystem write after [`Jail::enter`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub path: String,
    pub errno: String,
}

/// Snapshot of post-[`Jail::enter`] state ready to be emitted over an [`EvidenceChannel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub observed_at: String,
    pub evidence: LandlockEvidence,
}

impl Observation {
    /// Capture a post-[`Jail::enter`] observation by running the denied-path
    /// write probe in `probe_dir` and pairing the result with the jail's
    /// enforcement status and Landlock ruleset parameters.
    pub fn from_jail(jail: &Jail, probe_dir: &Path) -> Self {
        let probe = run_denied_write_probe_in(probe_dir);
        Self {
            observed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            evidence: LandlockEvidence {
                abi: LANDLOCK_ABI,
                handled_access_mask: handled_access_mask(),
                enforced: jail.enforced(),
                probe,
            },
        }
    }
}

/// Inspect [`EVIDENCE_CHANNEL_ENV`] (or [`LEGACY_EVIDENCE_CHANNEL_ENV`]); if set,
/// scrub the variable from the current process environment, validate the file
/// descriptor, set `FD_CLOEXEC` (Condition C1), and run the pre-jail DAC control
/// write in the configured probe directory.
pub fn take_from_env() -> Result<Option<EvidenceChannel>, EvidenceEmitError> {
    let (var, raw) = match std::env::var(EVIDENCE_CHANNEL_ENV) {
        Ok(v) => (EVIDENCE_CHANNEL_ENV, v),
        Err(std::env::VarError::NotPresent) => match std::env::var(LEGACY_EVIDENCE_CHANNEL_ENV) {
            Ok(v) => (LEGACY_EVIDENCE_CHANNEL_ENV, v),
            Err(std::env::VarError::NotPresent) => return Ok(None),
            Err(std::env::VarError::NotUnicode(os)) => {
                std::env::remove_var(LEGACY_EVIDENCE_CHANNEL_ENV);
                return Err(EvidenceEmitError::InvalidFdEnv {
                    var: LEGACY_EVIDENCE_CHANNEL_ENV,
                    raw: os.to_string_lossy().into_owned(),
                    reason: "value is not valid UTF-8",
                });
            }
        },
        Err(std::env::VarError::NotUnicode(os)) => {
            std::env::remove_var(EVIDENCE_CHANNEL_ENV);
            return Err(EvidenceEmitError::InvalidFdEnv {
                var: EVIDENCE_CHANNEL_ENV,
                raw: os.to_string_lossy().into_owned(),
                reason: "value is not valid UTF-8",
            });
        }
    };

    std::env::remove_var(EVIDENCE_CHANNEL_ENV);
    std::env::remove_var(LEGACY_EVIDENCE_CHANNEL_ENV);

    let probe_dir = std::env::var_os(EVIDENCE_PROBE_DIR_ENV)
        .or_else(|| std::env::var_os(LEGACY_EVIDENCE_PROBE_DIR_ENV))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DENIED_PROBE_DIR));
    std::env::remove_var(EVIDENCE_PROBE_DIR_ENV);
    std::env::remove_var(LEGACY_EVIDENCE_PROBE_DIR_ENV);

    let fd = parse_fd(var, &raw)?;
    EvidenceChannel::from_raw_fd_with_probe_dir(fd, probe_dir).map(Some)
}

impl EvidenceChannel {
    /// Wrap an already-open file descriptor using [`DEFAULT_DENIED_PROBE_DIR`].
    pub fn from_raw_fd(fd: RawFd) -> Result<Self, EvidenceEmitError> {
        Self::from_raw_fd_with_probe_dir(fd, PathBuf::from(DEFAULT_DENIED_PROBE_DIR))
    }

    /// Wrap an already-open file descriptor with an explicit `probe_dir`,
    /// immediately setting `FD_CLOEXEC` (Condition C1) and running the pre-jail
    /// DAC control probe in `probe_dir`.
    pub fn from_raw_fd_with_probe_dir(
        fd: RawFd,
        probe_dir: PathBuf,
    ) -> Result<Self, EvidenceEmitError> {
        if fd < 3 {
            return Err(EvidenceEmitError::InvalidFdEnv {
                var: EVIDENCE_CHANNEL_ENV,
                raw: fd.to_string(),
                reason: "fd must be >= 3 (stdin/stdout/stderr cannot be the evidence pipe)",
            });
        }
        set_cloexec(fd)?;
        run_pre_jail_control_probe(&probe_dir)?;
        Ok(Self { fd, probe_dir })
    }

    /// Directory in which the pre-jail control probe succeeded and in which the
    /// post-jail denial probe must run.
    pub fn probe_dir(&self) -> &Path {
        &self.probe_dir
    }

    /// Emit `obs` as a single newline-terminated JSON line and close the file
    /// descriptor with a checked `libc::close` (Condition C2).
    pub fn emit(self, obs: &Observation) -> Result<(), EvidenceEmitError> {
        let fd = self.fd;
        let frame = WireFrame {
            observed_at: obs.observed_at.clone(),
            evidence: obs.evidence.clone(),
        };
        let mut line = serde_json::to_vec(&frame)?;
        line.push(b'\n');

        let write_res = write_all_fd(fd, &line);
        let close_res = close_fd(fd);

        write_res?;
        close_res?;
        Ok(())
    }
}

fn parse_fd(var: &'static str, raw: &str) -> Result<RawFd, EvidenceEmitError> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(EvidenceEmitError::InvalidFdEnv {
            var,
            raw: raw.to_string(),
            reason: "expected non-empty ASCII decimal digits",
        });
    }
    let fd: RawFd = raw.parse().map_err(|_| EvidenceEmitError::InvalidFdEnv {
        var,
        raw: raw.to_string(),
        reason: "integer overflow parsing file descriptor",
    })?;
    if fd < 3 {
        return Err(EvidenceEmitError::InvalidFdEnv {
            var,
            raw: raw.to_string(),
            reason: "fd must be >= 3 (stdin/stdout/stderr cannot be the evidence pipe)",
        });
    }
    Ok(fd)
}

fn set_cloexec(fd: RawFd) -> Result<(), EvidenceEmitError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(EvidenceEmitError::Cloexec {
            fd,
            source: io::Error::last_os_error(),
        });
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if rc < 0 {
        return Err(EvidenceEmitError::Cloexec {
            fd,
            source: io::Error::last_os_error(),
        });
    }
    Ok(())
}

fn write_all_fd(fd: RawFd, mut buf: &[u8]) -> Result<(), EvidenceEmitError> {
    while !buf.is_empty() {
        let n = unsafe { libc::write(fd, buf.as_ptr().cast::<libc::c_void>(), buf.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(EvidenceEmitError::Write { fd, source: err });
        }
        if n == 0 {
            return Err(EvidenceEmitError::Write {
                fd,
                source: io::Error::new(
                    io::ErrorKind::WriteZero,
                    "write(2) returned 0 on evidence pipe",
                ),
            });
        }
        buf = &buf[n as usize..];
    }
    Ok(())
}

fn close_fd(fd: RawFd) -> Result<(), EvidenceEmitError> {
    let rc = unsafe { libc::close(fd) };
    if rc < 0 {
        return Err(EvidenceEmitError::Close {
            fd,
            source: io::Error::last_os_error(),
        });
    }
    Ok(())
}

fn run_pre_jail_control_probe(probe_dir: &Path) -> Result<(), EvidenceEmitError> {
    let pid = std::process::id();
    let control_path = probe_dir.join(format!(".xerj-sbx-pre-jail-control-{pid}"));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&control_path)
    {
        Ok(mut f) => {
            let write_res = f.write_all(b"ok");
            drop(f);
            let unlink_res = std::fs::remove_file(&control_path);
            if let Err(err) = write_res.and(unlink_res) {
                return Err(EvidenceEmitError::PreJailControlFailed {
                    dir: probe_dir.to_path_buf(),
                    errno: errno_name(err.raw_os_error()),
                    source: err,
                });
            }
            Ok(())
        }
        Err(err) => Err(EvidenceEmitError::PreJailControlFailed {
            dir: probe_dir.to_path_buf(),
            errno: errno_name(err.raw_os_error()),
            source: err,
        }),
    }
}

/// Attempt to create a file inside `probe_dir` and return the resulting
/// [`ProbeResult`].
pub fn run_denied_write_probe_in(probe_dir: &Path) -> ProbeResult {
    let pid = std::process::id();
    let probe_path = probe_dir.join(format!(".xerj-sbx-landlock-probe-{pid}"));
    let errno = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)
    {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe_path);
            "OK".to_string()
        }
        Err(err) => errno_name(err.raw_os_error()),
    };
    ProbeResult {
        path: probe_path.to_string_lossy().into_owned(),
        errno,
    }
}

fn errno_name(raw: Option<i32>) -> String {
    match raw {
        Some(libc::EACCES) => "EACCES".to_string(),
        Some(libc::EPERM) => "EPERM".to_string(),
        Some(libc::EROFS) => "EROFS".to_string(),
        Some(libc::ENOENT) => "ENOENT".to_string(),
        Some(libc::EEXIST) => "EEXIST".to_string(),
        Some(libc::ENOSYS) => "ENOSYS".to_string(),
        Some(libc::EOPNOTSUPP) => "EOPNOTSUPP".to_string(),
        Some(code) => format!("ERRNO_{code}"),
        None => "IO_ERROR".to_string(),
    }
}

// ============================================================================
// Supervisor-side evidence parser & verifier (ported from sbx-serve/evidence.rs)
// ============================================================================

/// Validated containment layer entry ready for supervisor attestation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcedLayer {
    pub tier: String,
    pub layer: String,
    pub observed_at: String,
    pub evidence: serde_json::Value,
}

/// Validated Landlock evidence returned by [`parse_evidence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvidence {
    pub observed_at: DateTime<Utc>,
    pub observed_at_wire: String,
    pub abi: u64,
    pub handled_access_mask: u64,
    pub enforced: bool,
    pub probe: VerifiedProbe,
}

/// Validated denied-write probe inside [`ParsedEvidence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProbe {
    /// Canonicalized probe directory + file name (symlinks resolved).
    pub path: String,
    pub errno: String,
}

impl ParsedEvidence {
    /// Convert this validated Landlock observation into an [`EnforcedLayer`]
    /// tagged with `tier = "process"` and `layer = "landlock"`.
    pub fn into_enforced_layer(self) -> EnforcedLayer {
        let evidence = serde_json::json!({
            "abi": self.abi,
            "handled_access_mask": self.handled_access_mask,
            "enforced": self.enforced,
            "probe": {
                "path": self.probe.path,
                "errno": self.probe.errno,
            }
        });
        EnforcedLayer {
            tier: PROCESS_TIER.to_string(),
            layer: LANDLOCK_LAYER.to_string(),
            observed_at: self.observed_at_wire,
            evidence,
        }
    }
}

/// Every distinct failure mode of [`parse_evidence`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvidenceParseError {
    #[error("containment evidence pipe was empty (0 bytes read before EOF)")]
    EmptyPipe,
    #[error("containment evidence pipe exceeded {max} bytes (got {actual} bytes)")]
    OversizePipe { actual: usize, max: usize },
    #[error("containment evidence pipe payload was not terminated by a newline ('\\n')")]
    MissingTrailingNewline,
    #[error(
        "containment evidence pipe contained multiple lines ({lines} newline bytes; expected 1)"
    )]
    MultipleLines { lines: usize },
    #[error("containment evidence JSON failed to parse: {0}")]
    InvalidJson(String),
    #[error("observed_at {raw:?} is not a valid RFC 3339 UTC timestamp: {reason}")]
    InvalidObservedAt { raw: String, reason: String },
    #[error("evidence.enforced is false; refusing to attest an unenforced Landlock ruleset")]
    NotEnforced,
    #[error("evidence.{field} value {value} is out of valid range (1..={JS_MAX_SAFE_INT})")]
    IntegerOutOfRange { field: &'static str, value: u64 },
    #[error(
        "evidence.probe.errno is {actual:?}; expected \"EACCES\" from denied Landlock write probe"
    )]
    ProbeErrnoNotEacces { actual: String },
    #[error("evidence.probe.path {probe_path:?} could not be verified inside probe_dir {probe_dir:?}: {reason}")]
    ProbePathOutsideProbeDir {
        probe_path: String,
        probe_dir: String,
        reason: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictWireEnvelope {
    observed_at: String,
    evidence: StrictWireEvidence,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictWireEvidence {
    abi: u64,
    handled_access_mask: u64,
    enforced: bool,
    probe: StrictWireProbe,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictWireProbe {
    path: String,
    errno: String,
}

/// Parse and validate the raw bytes read from the child's evidence pipe until
/// EOF against the supervisor-supplied `probe_dir`.
pub fn parse_evidence(
    bytes: &[u8],
    probe_dir: &Path,
) -> Result<ParsedEvidence, EvidenceParseError> {
    if bytes.is_empty() {
        return Err(EvidenceParseError::EmptyPipe);
    }
    if bytes.len() > MAX_EVIDENCE_BYTES {
        return Err(EvidenceParseError::OversizePipe {
            actual: bytes.len(),
            max: MAX_EVIDENCE_BYTES,
        });
    }
    if *bytes.last().expect("non-empty checked above") != b'\n' {
        return Err(EvidenceParseError::MissingTrailingNewline);
    }
    let newline_count = bytes.iter().filter(|&&b| b == b'\n').count();
    if newline_count != 1 {
        return Err(EvidenceParseError::MultipleLines {
            lines: newline_count,
        });
    }

    let line = &bytes[..bytes.len() - 1];
    let envelope: StrictWireEnvelope =
        serde_json::from_slice(line).map_err(|e| EvidenceParseError::InvalidJson(e.to_string()))?;

    let observed_at = parse_rfc3339_utc(&envelope.observed_at)?;

    if !envelope.evidence.enforced {
        return Err(EvidenceParseError::NotEnforced);
    }
    check_safe_positive_int("abi", envelope.evidence.abi)?;
    check_safe_positive_int("handled_access_mask", envelope.evidence.handled_access_mask)?;

    if envelope.evidence.probe.errno != "EACCES" {
        return Err(EvidenceParseError::ProbeErrnoNotEacces {
            actual: envelope.evidence.probe.errno,
        });
    }

    let canonical_probe_path = verify_probe_path_in_dir(&envelope.evidence.probe.path, probe_dir)?;

    Ok(ParsedEvidence {
        observed_at,
        observed_at_wire: envelope.observed_at,
        abi: envelope.evidence.abi,
        handled_access_mask: envelope.evidence.handled_access_mask,
        enforced: true,
        probe: VerifiedProbe {
            path: canonical_probe_path,
            errno: envelope.evidence.probe.errno,
        },
    })
}

fn parse_rfc3339_utc(raw: &str) -> Result<DateTime<Utc>, EvidenceParseError> {
    if !(raw.ends_with('Z') || raw.ends_with("+00:00")) {
        return Err(EvidenceParseError::InvalidObservedAt {
            raw: raw.to_string(),
            reason: "timestamp must carry UTC offset ('Z' or '+00:00')".to_string(),
        });
    }
    let parsed =
        DateTime::parse_from_rfc3339(raw).map_err(|e| EvidenceParseError::InvalidObservedAt {
            raw: raw.to_string(),
            reason: e.to_string(),
        })?;
    Ok(parsed.with_timezone(&Utc))
}

fn check_safe_positive_int(field: &'static str, value: u64) -> Result<(), EvidenceParseError> {
    if value == 0 || value > JS_MAX_SAFE_INT {
        return Err(EvidenceParseError::IntegerOutOfRange { field, value });
    }
    Ok(())
}

fn verify_probe_path_in_dir(
    probe_path: &str,
    probe_dir: &Path,
) -> Result<String, EvidenceParseError> {
    let bail = |reason: String| EvidenceParseError::ProbePathOutsideProbeDir {
        probe_path: probe_path.to_string(),
        probe_dir: probe_dir.display().to_string(),
        reason,
    };

    if probe_path.is_empty() {
        return Err(bail("probe.path is empty".to_string()));
    }
    let p = Path::new(probe_path);
    if !p.is_absolute() {
        return Err(bail("probe.path must be an absolute path".to_string()));
    }
    let file_name = p
        .file_name()
        .ok_or_else(|| bail("probe.path has no file name component".to_string()))?;
    let parent = p
        .parent()
        .ok_or_else(|| bail("probe.path has no parent directory".to_string()))?;

    let canon_probe_dir = fs::canonicalize(probe_dir)
        .map_err(|e| bail(format!("failed to canonicalize supervisor probe_dir: {e}")))?;
    let canon_parent = fs::canonicalize(parent)
        .map_err(|e| bail(format!("failed to canonicalize probe.path parent: {e}")))?;

    if canon_parent != canon_probe_dir {
        return Err(bail(format!(
            "canonical parent {} does not match canonical probe_dir {}",
            canon_parent.display(),
            canon_probe_dir.display()
        )));
    }

    Ok(canon_probe_dir
        .join(file_name)
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::AccessRules;
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use tempfile::TempDir;

    fn make_pipe() -> (RawFd, std::fs::File) {
        let mut fds = [0 as libc::c_int; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe(2) failed: {}", io::Error::last_os_error());
        let read_file = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        (fds[1], read_file)
    }

    #[test]
    fn from_raw_fd_sets_cloexec_and_emit_collaborates_with_parse_evidence() {
        let probe_dir = TempDir::new().unwrap();
        let (write_fd, mut read_file) = make_pipe();

        let channel =
            EvidenceChannel::from_raw_fd_with_probe_dir(write_fd, probe_dir.path().to_path_buf())
                .expect("from_raw_fd_with_probe_dir failed");

        let flags = unsafe { libc::fcntl(write_fd, libc::F_GETFD) };
        assert!(flags >= 0, "fcntl(F_GETFD) failed");
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "Condition C1 violated: FD_CLOEXEC must be set on the evidence pipe fd"
        );

        let fake_probe = probe_dir.path().join(".xerj-sbx-landlock-probe-42");
        let obs = Observation {
            observed_at: "2026-10-14T00:00:00.000Z".to_string(),
            evidence: LandlockEvidence {
                abi: 2,
                handled_access_mask: 0x1fff,
                enforced: true,
                probe: ProbeResult {
                    path: fake_probe.to_string_lossy().into_owned(),
                    errno: "EACCES".to_string(),
                },
            },
        };
        channel.emit(&obs).expect("emit failed");

        let post_close = unsafe { libc::fcntl(write_fd, libc::F_GETFD) };
        assert_eq!(
            post_close, -1,
            "Condition C2 violated: write_fd must be closed after emit()"
        );

        let mut buf = Vec::new();
        read_file.read_to_end(&mut buf).unwrap();
        let parsed = parse_evidence(&buf, probe_dir.path()).expect("supervisor parse failed");
        assert_eq!(parsed.abi, 2);
        assert_eq!(parsed.handled_access_mask, 0x1fff);
        assert!(parsed.enforced);
        assert_eq!(parsed.probe.errno, "EACCES");
        let layer = parsed.into_enforced_layer();
        assert_eq!(layer.tier, "process");
        assert_eq!(layer.layer, "landlock");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_landlock_observation_emits_and_verifies_end_to_end() {
        let allowed_ws = TempDir::new().unwrap();
        let denied_probe_dir = TempDir::new().unwrap();
        let ws_path = allowed_ws.path().to_path_buf();
        let probe_dir_path = denied_probe_dir.path().to_path_buf();

        let (write_fd, mut read_file) = make_pipe();

        let handle = std::thread::spawn(move || {
            let channel =
                EvidenceChannel::from_raw_fd_with_probe_dir(write_fd, probe_dir_path.clone())
                    .expect("pre-jail channel setup must succeed");

            let rules = AccessRules::default().with_write(&ws_path);
            let jail = Jail::enter(rules).expect("Jail::enter failed");
            let obs = Observation::from_jail(&jail, channel.probe_dir());
            let enforced = jail.enforced();
            channel.emit(&obs).expect("emit failed");
            enforced
        });

        let enforced = handle.join().expect("worker thread panicked");
        let mut buf = Vec::new();
        read_file.read_to_end(&mut buf).unwrap();

        if enforced {
            let parsed = parse_evidence(&buf, denied_probe_dir.path())
                .expect("live Landlock observation must verify");
            assert_eq!(parsed.abi, u64::from(LANDLOCK_ABI));
            assert_eq!(parsed.handled_access_mask, handled_access_mask());
            assert_eq!(parsed.probe.errno, "EACCES");
        }
    }
}
