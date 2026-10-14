//! OS-level sandboxed execution primitives for XERJ, ported from `sbx`.
//!
//! Modules:
//! - [`rules`]: Filesystem and network [`AccessRules`] (default deny-all).
//! - [`jail`]: Linux Landlock V2 and macOS Seatbelt (`sandbox-exec`) confinement
//!   ([`Jail`], [`JailBuilder`], [`JailError`], [`current_thread_is_jailed`],
//!   [`jail_write_roots`]).
//! - [`evidence`]: Out-of-band containment evidence pipe emitter
//!   ([`EvidenceChannel`], [`Observation`]) and supervisor verifier
//!   ([`parse_evidence`], [`ParsedEvidence`], [`EnforcedLayer`]).
//! - [`settle`]: Type-level single-consumption custody state machine for retained
//!   sandbox worlds ([`WorldId`], [`RetainedRun`], [`Settlement`], [`Outcome`]).
//! - [`citation`]: Immutable provenance citations ([`OutputCitation`], [`VcsCore`]).
//! - [`executor`]: Dedicated-thread jailed subprocess runner ([`SandboxExecutor`],
//!   [`OsSandboxExecutor`], [`SandboxRequest`], [`SandboxResult`], [`ResourceLimits`]).

pub mod citation;
pub mod evidence;
pub mod executor;
pub mod jail;
pub mod rules;
pub mod settle;

pub use citation::{OutputCitation, VcsCore};
pub use evidence::{
    parse_evidence, take_from_env, EnforcedLayer, EvidenceChannel, EvidenceEmitError,
    EvidenceParseError, LandlockEvidence, Observation, ParsedEvidence, ProbeResult, VerifiedProbe,
    EVIDENCE_CHANNEL_ENV, EVIDENCE_PROBE_DIR_ENV,
};
pub use executor::{
    OsSandboxExecutor, ResourceLimits, SandboxError, SandboxExecutor, SandboxRequest, SandboxResult,
};
pub use jail::{
    build_seatbelt_profile, current_thread_is_jailed, escape_seatbelt_path, handled_access_mask,
    jail_write_roots, Jail, JailBuilder, JailError, JAIL_MECHANISM, LANDLOCK_ABI,
    MACOS_SYSTEM_READ_PATHS,
};
pub use rules::AccessRules;
pub use settle::{Outcome, RetainedRun, SettleError, Settlement, WorldId};
