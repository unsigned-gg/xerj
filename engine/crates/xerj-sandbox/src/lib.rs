//! OS-level sandboxed execution primitives for XERJ, adapted from
//! [`unsigned-gg/sbx`](https://github.com/unsigned-gg/sbx).
//!
//! # Adaptation & Open-Source Collaboration (`unsigned-gg/sbx`)
//!
//! This crate consolidates and adapts the core isolate primitives from
//! [`unsigned-gg/sbx`](https://github.com/unsigned-gg/sbx) (`sbx-core`, `sbx-jail`,
//! `sbx-evidence`, and `sbx-settle`) into a self-contained XERJ workspace crate, adding
//! the [`executor`] subprocess runner for untrusted tool, hook, and plugin workloads.
//!
//! We actively invite open-source collaboration on [`unsigned-gg/sbx`](https://github.com/unsigned-gg/sbx)
//! to advance cross-platform isolate primitives—including unprivileged kernel sandboxing
//! (Linux Landlock ABI evolution, seccomp-bpf, macOS Seatbelt), out-of-band cryptographic
//! and kernel containment evidence, and linear/single-consumption custody state machines.
//!
//! # Modules
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
