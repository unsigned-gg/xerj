// SPDX-License-Identifier: Apache-2.0 OR MIT
//! `ls` — LeanSpec AST parser, architectural conformance checker, and CAS listing engine.
//!
//! Provides fast, deterministic validation of engineering specifications, dependency trees,
//! and content-addressed storage listings.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Filesystem scanner and CAS tree traversal engine.
pub mod scanner;

pub use lsc_core::*;
pub use scanner::{scan_cas_directory, scan_cas_path, scan_specs_dir, ScanResult};

use std::fmt::Write;
use std::path::Path;

/// Check a specifications directory and produce a formatted human-readable report.
///
/// # Errors
/// Returns an I/O error if the directory cannot be scanned.
pub fn check_directory(dir: &Path) -> Result<(ScanResult, String), std::io::Error> {
    let result = scan_specs_dir(dir)?;
    let output = format_report(&result.report);
    Ok((result, output))
}

/// Formats a `ValidationReport` into an ANSI-styled diagnostic text string.
#[must_use]
pub fn format_report(report: &ValidationReport) -> String {
    let mut out = String::new();

    for notice in &report.notices {
        let _ = writeln!(out, "ls: notice: {notice}");
    }

    for warning in &report.warnings {
        if let Some(line) = warning.line {
            let _ = writeln!(
                out,
                "ls: warning: {}:{}: [{}] {}",
                warning.file, line, warning.rule, warning.message
            );
        } else {
            let _ = writeln!(
                out,
                "ls: warning: {}: [{}] {}",
                warning.file, warning.rule, warning.message
            );
        }
    }

    for error in &report.errors {
        if let Some(line) = error.line {
            let _ = writeln!(
                out,
                "ls: error: {}:{}: [{}] {}",
                error.file, line, error.rule, error.message
            );
        } else {
            let _ = writeln!(
                out,
                "ls: error: {}: [{}] {}",
                error.file, error.rule, error.message
            );
        }
    }

    let _ = writeln!(
        out,
        "ls: {} error(s), {} warning(s)",
        report.errors.len(),
        report.warnings.len()
    );

    out
}
