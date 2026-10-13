//! Repository automation for `xerj-org/xerj`, run from `engine/` via `cargo xtask <command>`.
//!
//! Commands:
//! - `projectfile-check`: validate `../projectfile.yaml` against Projectfile v1 required invariants.
//! - `workflow-timeout-check`: verify every GitHub Actions job declares `timeout-minutes` in `1..=90`.
//! - `xwork`: run the scoped verification pipeline (`projectfile-check`, `workflow-timeout-check`,
//!   `cargo fmt --all -- --check`, and scoped `-p xtask` clippy/test with `-j 2`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn engine_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn repo_root() -> PathBuf {
    engine_root().join("..")
}

fn check_projectfile_content(text: &str) -> Result<(), String> {
    let required = [
        "$schema: https://projectfile.org/schema/v1.json",
        "kind: SoftwareSourceCode",
        "namespace: org.xerj",
        "name: xerj",
        "spdx: Apache-2.0",
        "projectfile:",
        "ci:",
        "nodes:",
    ];
    for needle in required {
        if !text.contains(needle) {
            return Err(format!("missing required Projectfile v1 field: `{needle}`"));
        }
    }
    Ok(())
}

fn projectfile_check(repo: &Path) -> ExitCode {
    let path = repo.join("projectfile.yaml");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("projectfile-check: cannot read {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match check_projectfile_content(&text) {
        Ok(()) => {
            println!("projectfile-check: ok (projectfile.yaml conforms to Projectfile v1)");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("projectfile-check: {err}");
            ExitCode::FAILURE
        }
    }
}

fn workflow_timeout_check(repo: &Path) -> ExitCode {
    let script = repo.join(".github/scripts/workflow-timeout-guard.py");
    let status = Command::new("python3")
        .arg(&script)
        .current_dir(repo)
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => {
            eprintln!("workflow-timeout-check: workflow-timeout-guard.py reported errors");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("workflow-timeout-check: cannot run python3: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_cargo(engine: &Path, args: &[&str]) -> ExitCode {
    let status = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(engine)
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("xwork: cannot run cargo {}: {err}", args.join(" "));
            ExitCode::FAILURE
        }
    }
}

fn run_xwork(repo: &Path, engine: &Path) -> ExitCode {
    let jobs = env::var("JOBS")
        .or_else(|_| env::var("CARGO_BUILD_JOBS"))
        .unwrap_or_else(|_| "2".to_string());

    println!("=== xwork: deterministic scoped verification pipeline (jobs={jobs}) ===");

    println!("xwork: [projectfile-check] running...");
    if projectfile_check(repo) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    println!("xwork: [workflow-timeout-check] running...");
    if workflow_timeout_check(repo) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    println!("xwork: [fmt-check] running...");
    if run_cargo(engine, &["fmt", "--all", "--", "--check"]) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    println!("xwork: [xtask-clippy] running...");
    if run_cargo(
        engine,
        &[
            "clippy",
            "-p",
            "xtask",
            "--all-targets",
            "-j",
            &jobs,
            "--",
            "-D",
            "warnings",
        ],
    ) != ExitCode::SUCCESS
    {
        return ExitCode::FAILURE;
    }

    println!("xwork: [xtask-test] running...");
    if run_cargo(engine, &["test", "-p", "xtask", "-j", &jobs]) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    println!("=== xwork: all stages PASSED ===");
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let engine = engine_root();
    let repo = repo_root();
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("projectfile-check") => projectfile_check(&repo),
        Some("workflow-timeout-check") => workflow_timeout_check(&repo),
        Some("xwork") => run_xwork(&repo, &engine),
        _ => {
            eprintln!("usage: cargo xtask <projectfile-check|workflow-timeout-check|xwork>");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_projectfile_is_valid() {
        let path = repo_root().join("projectfile.yaml");
        let text = fs::read_to_string(&path).expect("projectfile.yaml must exist");
        check_projectfile_content(&text).expect("projectfile.yaml must be valid");
    }

    #[test]
    fn missing_schema_is_rejected() {
        let err = check_projectfile_content("kind: SoftwareSourceCode\n").unwrap_err();
        assert!(err.contains("$schema"));
    }
}
