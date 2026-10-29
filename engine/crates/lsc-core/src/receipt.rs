// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Exact-head validation receipts (`schema_version: 4`) and composite validity fingerprints.
//!
//! Combines the `facebookexperimental/reverie` schema-v4 validation ledger (`validate.sh` /
//! `libtest-counts.rs`) with `glaeda`'s `ValidityFingerprint` input-set anchoring.
//! Contains zero system I/O.

use serde::{Deserialize, Serialize};

/// Current schema version for [`ValidationReceipt`].
pub const VALIDATION_RECEIPT_SCHEMA_VERSION: u32 = 4;

/// Current schema version for [`ValidityFingerprint`].
pub const VALIDITY_FINGERPRINT_SCHEMA_VERSION: u32 = 1;

/// Category of an input that anchors a [`ValidityFingerprint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidityInputKind {
    /// Git source tree or working-tree CAS digest.
    SourceTree,
    /// Pinned compiler toolchain (`rust-toolchain.toml` + `rustc --version`).
    Toolchain,
    /// Dependency lockfile (`Cargo.lock` or tool pin table).
    Lockfile,
    /// Product specification or schema tree (`specs/` + `projectfile.yaml`).
    ProductSchema,
    /// Verification pipeline definition (`CI_STEPS` / `XWORK_STAGES`).
    VerifyPipeline,
}

impl ValidityInputKind {
    /// Canonical snake_case wire label for this input kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceTree => "source_tree",
            Self::Toolchain => "toolchain",
            Self::Lockfile => "lockfile",
            Self::ProductSchema => "product_schema",
            Self::VerifyPipeline => "verify_pipeline",
        }
    }
}

/// Strength of a [`ValidityFingerprint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidityStrength {
    /// Clean, commit-anchored checkout with all required inputs hashed.
    Exact,
    /// Dirty working tree or partial input set; advisory only.
    Advisory,
}

/// One named digest input contributing to a [`ValidityFingerprint`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ValidityInput {
    /// Input category.
    pub kind: ValidityInputKind,
    /// Human-readable input label (e.g. `"HEAD"`, `"rust-toolchain.toml"`).
    pub label: String,
    /// Hex digest or canonical version string for the input.
    pub digest: String,
}

impl ValidityInput {
    /// Construct a new [`ValidityInput`].
    #[must_use]
    pub fn new(
        kind: ValidityInputKind,
        label: impl Into<String>,
        digest: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            digest: digest.into(),
        }
    }
}

/// Composite BLAKE3 fingerprint over the ordered validation input set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidityFingerprint {
    /// Fingerprint schema version (`1`).
    pub schema_version: u32,
    /// Digest algorithm (`"blake3"`).
    pub algorithm: String,
    /// Whether this fingerprint is exact (commit-anchored) or advisory (dirty tree).
    pub strength: ValidityStrength,
    /// Canonically sorted validation inputs.
    pub inputs: Vec<ValidityInput>,
    /// 64-hex-character BLAKE3 digest over `strength` and `inputs`.
    pub composite_digest: String,
}

impl ValidityFingerprint {
    /// Sort `inputs` canonically and compute the composite BLAKE3 fingerprint.
    #[must_use]
    pub fn compute(strength: ValidityStrength, mut inputs: Vec<ValidityInput>) -> Self {
        inputs.sort();
        let composite_digest = Self::hash_inputs(strength, &inputs);
        Self {
            schema_version: VALIDITY_FINGERPRINT_SCHEMA_VERSION,
            algorithm: "blake3".to_owned(),
            strength,
            inputs,
            composite_digest,
        }
    }

    /// Recompute the composite BLAKE3 digest and verify it matches `composite_digest`.
    #[must_use]
    pub fn verify(&self) -> bool {
        if self.schema_version != VALIDITY_FINGERPRINT_SCHEMA_VERSION || self.algorithm != "blake3"
        {
            return false;
        }
        let mut sorted = self.inputs.clone();
        sorted.sort();
        if sorted != self.inputs {
            return false;
        }
        Self::hash_inputs(self.strength, &self.inputs) == self.composite_digest
    }

    fn hash_inputs(strength: ValidityStrength, inputs: &[ValidityInput]) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"lsc-validity-fingerprint-v1\n");
        hasher.update(match strength {
            ValidityStrength::Exact => b"exact\n",
            ValidityStrength::Advisory => b"advisory\n",
        });
        for input in inputs {
            hasher.update(input.kind.as_str().as_bytes());
            hasher.update(b":");
            hasher.update(input.label.as_bytes());
            hasher.update(b":");
            hasher.update(input.digest.as_bytes());
            hasher.update(b"\n");
        }
        hasher.finalize().to_hex().to_string()
    }
}

/// Aggregated test execution counts parsed from `cargo test` (`libtest`) or `cargo nextest`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibtestCounts {
    /// Total tests executed (`passed_tests + failed_tests`).
    pub executed_tests: u64,
    /// Total tests that passed.
    pub passed_tests: u64,
    /// Total tests ignored or filtered out.
    pub filtered_tests: u64,
    /// Total tests that failed.
    pub failed_tests: u64,
}

/// Parse `cargo test` (`test result:`) or `cargo nextest` (`Summary [...]`) output into
/// [`LibtestCounts`].
///
/// Fails closed when no summary line is present or when any counter overflows or fails to parse.
///
/// # Errors
/// Returns a descriptive error string if no test summary lines are found or if a count is invalid.
pub fn parse_libtest_counts(output: &str) -> Result<LibtestCounts, String> {
    let mut counts = LibtestCounts::default();
    let mut saw_summary = false;

    for raw_line in output.lines() {
        let line = strip_ansi(raw_line);
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("test result:") {
            saw_summary = true;
            let after_status = rest
                .split_once('.')
                .map(|(_, tail)| tail)
                .ok_or_else(|| format!("malformed libtest summary line: {trimmed}"))?;
            let mut passed: Option<u64> = None;
            let mut failed: Option<u64> = None;
            let mut ignored: u64 = 0;
            let mut filtered: Option<u64> = None;

            for clause in after_status.split(';') {
                let clause = clause.trim();
                if let Some(val) = clause.strip_suffix(" passed") {
                    passed = Some(parse_u64_field(val.trim(), "passed", trimmed)?);
                } else if let Some(val) = clause.strip_suffix(" failed") {
                    failed = Some(parse_u64_field(val.trim(), "failed", trimmed)?);
                } else if let Some(val) = clause.strip_suffix(" ignored") {
                    ignored = parse_u64_field(val.trim(), "ignored", trimmed)?;
                } else if let Some(val) = clause.strip_suffix(" filtered out") {
                    let num_str = val
                        .split_whitespace()
                        .last()
                        .ok_or_else(|| format!("missing filtered count in: {trimmed}"))?;
                    filtered = Some(parse_u64_field(num_str, "filtered out", trimmed)?);
                }
            }

            let p = passed.ok_or_else(|| format!("missing passed count in: {trimmed}"))?;
            let f = failed.ok_or_else(|| format!("missing failed count in: {trimmed}"))?;
            let flt = filtered.ok_or_else(|| format!("missing filtered count in: {trimmed}"))?;
            let exec = p
                .checked_add(f)
                .ok_or_else(|| format!("executed count overflow in: {trimmed}"))?;
            let total_filtered = flt
                .checked_add(ignored)
                .ok_or_else(|| format!("filtered count overflow in: {trimmed}"))?;

            counts.executed_tests = counts
                .executed_tests
                .checked_add(exec)
                .ok_or_else(|| "total executed_tests overflow".to_owned())?;
            counts.passed_tests = counts
                .passed_tests
                .checked_add(p)
                .ok_or_else(|| "total passed_tests overflow".to_owned())?;
            counts.failed_tests = counts
                .failed_tests
                .checked_add(f)
                .ok_or_else(|| "total failed_tests overflow".to_owned())?;
            counts.filtered_tests = counts
                .filtered_tests
                .checked_add(total_filtered)
                .ok_or_else(|| "total filtered_tests overflow".to_owned())?;
        } else if trimmed.starts_with("Summary [") && trimmed.contains("tests run:") {
            saw_summary = true;
            let (_, after_colon) = trimmed
                .split_once("tests run:")
                .ok_or_else(|| format!("malformed nextest summary: {trimmed}"))?;
            let mut passed: Option<u64> = None;
            let mut failed: u64 = 0;
            let mut skipped: u64 = 0;

            for part in after_colon.split(',') {
                let part = part.trim();
                for token_pair in part.split_whitespace().collect::<Vec<_>>().windows(2) {
                    if token_pair[1] == "passed" {
                        passed = Some(parse_u64_field(token_pair[0], "passed", trimmed)?);
                    } else if token_pair[1] == "failed" || token_pair[1] == "leaky" {
                        if token_pair[1] == "failed" {
                            failed = parse_u64_field(token_pair[0], "failed", trimmed)?;
                        }
                    } else if token_pair[1] == "skipped" {
                        skipped = parse_u64_field(token_pair[0], "skipped", trimmed)?;
                    }
                }
            }

            let p = passed.ok_or_else(|| format!("missing passed count in nextest: {trimmed}"))?;
            let exec = p
                .checked_add(failed)
                .ok_or_else(|| format!("executed count overflow in nextest: {trimmed}"))?;
            counts.executed_tests = counts
                .executed_tests
                .checked_add(exec)
                .ok_or_else(|| "total executed_tests overflow".to_owned())?;
            counts.passed_tests = counts
                .passed_tests
                .checked_add(p)
                .ok_or_else(|| "total passed_tests overflow".to_owned())?;
            counts.failed_tests = counts
                .failed_tests
                .checked_add(failed)
                .ok_or_else(|| "total failed_tests overflow".to_owned())?;
            counts.filtered_tests = counts
                .filtered_tests
                .checked_add(skipped)
                .ok_or_else(|| "total filtered_tests overflow".to_owned())?;
        }
    }

    if !saw_summary {
        return Err("no libtest or nextest summary lines found in test output".to_owned());
    }

    Ok(counts)
}

fn parse_u64_field(raw: &str, field: &str, line: &str) -> Result<u64, String> {
    raw.parse::<u64>()
        .map_err(|_| format!("invalid {field} count '{raw}' in: {line}"))
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Outcome status of a validation receipt or individual gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptResult {
    /// Gate or pipeline succeeded.
    Pass,
    /// Gate or pipeline failed.
    Fail,
    /// Gate did not run because an earlier gate failed.
    #[serde(rename = "not_run")]
    NotRun,
}

/// Build cache state observed when the validation pipeline started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptCacheState {
    /// Target directory already existed with build artifacts.
    Warm,
    /// Target directory was absent or empty before validation.
    Cold,
    /// Cache state was not inspected.
    Unknown,
}

/// Per-gate entry within a [`ValidationReceipt`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationGateReceipt {
    /// Canonical gate name (e.g. `"fmt-check"`, `"clippy"`, `"test"`).
    pub name: String,
    /// Gate execution outcome.
    pub result: ReceiptResult,
    /// Process exit code (`0` on pass, non-zero on fail, `-1` when not run).
    pub exit_code: i32,
    /// Wall-clock duration in seconds.
    pub real_seconds: f64,
    /// Executed test count if this gate ran a test harness.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executed_tests: Option<u64>,
    /// Passed test count if this gate ran a test harness.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passed_tests: Option<u64>,
    /// Ignored/filtered test count if this gate ran a test harness.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filtered_tests: Option<u64>,
}

/// Schema-v4 validation receipt for an `xtask ci` or `xwork` execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationReceipt {
    /// Receipt schema version (`4`).
    pub schema_version: u32,
    /// Tool that produced the receipt (e.g. `"cargo-xtask-ci"`).
    pub producer: String,
    /// Repository name or slug.
    pub repo: String,
    /// UTC ISO-8601 start timestamp.
    pub started_at: String,
    /// UTC ISO-8601 finish timestamp.
    pub finished_at: String,
    /// Hostname where validation ran.
    pub host: String,
    /// Slot or workspace identifier (`"worktree"` or slot name).
    pub slot: String,
    /// Working directory path.
    pub cwd: String,
    /// Build cache warmth (`warm`, `cold`, `unknown`).
    pub cache_state: ReceiptCacheState,
    /// Resolved `HEAD` commit SHA, or `"uncommitted"` if unavailable.
    pub commit: String,
    /// True if and only if `commit` is a clean, anchored checkout (`!tree_dirty`).
    pub commit_anchored: bool,
    /// True if the working tree or index had uncommitted modifications.
    pub tree_dirty: bool,
    /// Composite BLAKE3 validity fingerprint over the validation inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validity_fingerprint: Option<ValidityFingerprint>,
    /// Overall pipeline outcome.
    pub result: ReceiptResult,
    /// Overall exit code (`0` on pass, non-zero on failure).
    pub exit_code: i32,
    /// Total executed tests across all test gates.
    pub executed_tests: u64,
    /// Total passed tests across all test gates.
    pub passed_tests: u64,
    /// Total ignored/filtered tests across all test gates.
    pub filtered_tests: u64,
    /// Number of gates in `gates`.
    pub checks: u64,
    /// Number of failed gates in `gates`.
    pub failures: u64,
    /// Total wall-clock duration in seconds.
    pub real_seconds: f64,
    /// Ordered per-gate receipts.
    pub gates: Vec<ValidationGateReceipt>,
}

impl ValidationReceipt {
    /// Validate all structural and fail-closed invariants of this receipt.
    ///
    /// When `require_tests` is true, a `Pass` receipt must also have
    /// `executed_tests > 0` and `passed_tests == executed_tests`.
    ///
    /// # Errors
    /// Returns the list of invariant violations when the receipt is invalid.
    pub fn validate_fail_closed(&self, require_tests: bool) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.schema_version != VALIDATION_RECEIPT_SCHEMA_VERSION {
            errors.push(format!(
                "unsupported schema_version {}, expected {}",
                self.schema_version, VALIDATION_RECEIPT_SCHEMA_VERSION
            ));
        }

        let valid_commit =
            !self.commit.is_empty() && self.commit != "uncommitted" && !self.tree_dirty;
        if self.commit_anchored != valid_commit {
            errors.push(format!(
                "commit_anchored={} contradicts commit='{}' and tree_dirty={}",
                self.commit_anchored, self.commit, self.tree_dirty
            ));
        }

        if let Some(ref fp) = self.validity_fingerprint {
            if !fp.verify() {
                errors.push("validity_fingerprint composite_digest verification failed".to_owned());
            }
            if self.tree_dirty && fp.strength == ValidityStrength::Exact {
                errors.push("dirty tree cannot claim Exact validity_fingerprint".to_owned());
            }
        }

        let expected_checks = self.gates.len() as u64;
        if self.checks != expected_checks {
            errors.push(format!(
                "checks={} does not match gates.len()={}",
                self.checks, expected_checks
            ));
        }

        let actual_failures = self
            .gates
            .iter()
            .filter(|g| g.result == ReceiptResult::Fail)
            .count() as u64;
        if self.failures != actual_failures {
            errors.push(format!(
                "failures={} does not match failed gate count {}",
                self.failures, actual_failures
            ));
        }

        if self.result == ReceiptResult::Pass {
            if self.exit_code != 0 {
                errors.push(format!(
                    "pass receipt must have exit_code=0, found {}",
                    self.exit_code
                ));
            }
            if self.failures != 0 {
                errors.push(format!(
                    "pass receipt must have failures=0, found {}",
                    self.failures
                ));
            }
            if self.gates.iter().any(|g| g.result != ReceiptResult::Pass) {
                errors.push("pass receipt contains non-passing gate".to_owned());
            }
            if require_tests {
                if self.executed_tests == 0 {
                    errors.push(
                        "pass receipt requires executed_tests > 0 (zero-test pass rejected)"
                            .to_owned(),
                    );
                }
                if self.passed_tests != self.executed_tests {
                    errors.push(format!(
                        "pass receipt requires passed_tests ({}) == executed_tests ({})",
                        self.passed_tests, self.executed_tests
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validity_fingerprint_sorts_inputs_and_verifies() {
        let i1 = ValidityInput::new(ValidityInputKind::Toolchain, "rustc", "1.85.0");
        let i2 = ValidityInput::new(ValidityInputKind::SourceTree, "HEAD", "1c35983");
        let fp_a =
            ValidityFingerprint::compute(ValidityStrength::Exact, vec![i1.clone(), i2.clone()]);
        let fp_b = ValidityFingerprint::compute(ValidityStrength::Exact, vec![i2, i1]);
        assert_eq!(fp_a, fp_b);
        assert!(fp_a.verify());
    }

    #[test]
    fn parse_libtest_counts_sums_multiple_binaries() {
        let output = "\
running 12 tests
test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 2 filtered out; finished in 0.01s

running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";
        let counts = parse_libtest_counts(output).unwrap();
        assert_eq!(counts.executed_tests, 17);
        assert_eq!(counts.passed_tests, 17);
        assert_eq!(counts.failed_tests, 0);
        assert_eq!(counts.filtered_tests, 3);
    }

    #[test]
    fn parse_libtest_counts_parses_nextest_summary() {
        let output = "    Summary [   0.412s] 48 tests run: 48 passed, 2 skipped\n";
        let counts = parse_libtest_counts(output).unwrap();
        assert_eq!(counts.executed_tests, 48);
        assert_eq!(counts.passed_tests, 48);
        assert_eq!(counts.failed_tests, 0);
        assert_eq!(counts.filtered_tests, 2);
    }

    #[test]
    fn parse_libtest_counts_rejects_missing_summary() {
        let err = parse_libtest_counts("Compiling lsc-core v0.1.0\n").unwrap_err();
        assert!(err.contains("no libtest or nextest summary lines"));
    }

    #[test]
    fn validation_receipt_rejects_zero_tests_and_dirty_exact_claim() {
        let fp = ValidityFingerprint::compute(
            ValidityStrength::Exact,
            vec![ValidityInput::new(
                ValidityInputKind::SourceTree,
                "HEAD",
                "1c35983",
            )],
        );
        let receipt = ValidationReceipt {
            schema_version: 4,
            producer: "cargo-xtask-ci".to_owned(),
            repo: "ls".to_owned(),
            started_at: "2026-10-15T04:00:00Z".to_owned(),
            finished_at: "2026-10-15T04:00:05Z".to_owned(),
            host: "ceres".to_owned(),
            slot: "worktree".to_owned(),
            cwd: "/repo".to_owned(),
            cache_state: ReceiptCacheState::Warm,
            commit: "1c35983".to_owned(),
            commit_anchored: false,
            tree_dirty: true,
            validity_fingerprint: Some(fp),
            result: ReceiptResult::Pass,
            exit_code: 0,
            executed_tests: 0,
            passed_tests: 0,
            filtered_tests: 0,
            checks: 1,
            failures: 0,
            real_seconds: 5.0,
            gates: vec![ValidationGateReceipt {
                name: "test".to_owned(),
                result: ReceiptResult::Pass,
                exit_code: 0,
                real_seconds: 5.0,
                executed_tests: Some(0),
                passed_tests: Some(0),
                filtered_tests: Some(0),
            }],
        };
        let errs = receipt.validate_fail_closed(true).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.contains("dirty tree cannot claim Exact")));
        assert!(errs.iter().any(|e| e.contains("executed_tests > 0")));
    }
}
