//! Repository automation for `xerj-org/xerj`, run from `engine/` via `cargo xtask <command>`.
//!
//! Note on manifest & `lsc` roadmap:
//! Workspace metadata and CI hygiene invariants are sourced directly from the canonical Cargo
//! workspace manifest (`engine/Cargo.toml`) rather than requiring `projectfile.yaml` or an
//! unpublished git dependency on `lsc`. Keeping `xtask` self-contained avoids external
//! non-crates.io dependencies in the workspace lockfile; `lsc` is planned to be published to
//! crates.io in the future for cross-language projectfile lowering.
//!
//! Commands:
//! - `projectfile-check`: validate `../projectfile.yaml` against Projectfile v1 required invariants.
//! - `manifest-check`: validate `engine/Cargo.toml` workspace metadata and member layout invariants.
//! - `workflow-timeout-check`: verify every GitHub Actions job declares `timeout-minutes` in `1..=90`.
//! - `xwork`: run the scoped verification pipeline (`projectfile-check`, `manifest-check`,
//!   `workflow-timeout-check`, `cargo fmt --all -- --check`, and scoped `-p xtask` clippy/test
//!   with `-j 2`).

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
            return Err(format!(
                "missing required Projectfile v1 clause: `{needle}`"
            ));
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
            println!("projectfile-check: ok (projectfile.yaml matches v1 schema invariants)");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("projectfile-check: {err}");
            ExitCode::FAILURE
        }
    }
}

fn check_cargo_manifest_content(text: &str, engine_dir: &Path) -> Result<(), String> {
    let parsed: toml::Value =
        toml::from_str(text).map_err(|e| format!("invalid TOML in Cargo.toml: {e}"))?;

    let ws = parsed
        .get("workspace")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "missing `[workspace]` table".to_string())?;

    let resolver = ws
        .get("resolver")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "missing `workspace.resolver`".to_string())?;
    if resolver != "2" {
        return Err(format!(
            "expected `workspace.resolver = \"2\"`, got `{resolver}`"
        ));
    }

    let pkg = ws
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "missing `[workspace.package]` table".to_string())?;

    for required_key in [
        "version",
        "edition",
        "rust-version",
        "license",
        "repository",
    ] {
        let val = pkg
            .get(required_key)
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("missing required `workspace.package.{required_key}`"))?;
        if val.trim().is_empty() {
            return Err(format!(
                "`workspace.package.{required_key}` must not be empty"
            ));
        }
    }

    let license = pkg
        .get("license")
        .and_then(toml::Value::as_str)
        .unwrap_or_default();
    if license != "Apache-2.0" {
        return Err(format!(
            "expected `workspace.package.license = \"Apache-2.0\"`, got `{license}`"
        ));
    }

    let members = ws
        .get("members")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "missing `workspace.members` array".to_string())?;
    if members.is_empty() {
        return Err("`workspace.members` must not be empty".to_string());
    }

    for member in members {
        let rel = member
            .as_str()
            .ok_or_else(|| "every entry in `workspace.members` must be a string".to_string())?;
        let member_manifest = engine_dir.join(rel).join("Cargo.toml");
        if !member_manifest.is_file() {
            return Err(format!(
                "workspace member `{rel}` is missing `{}`",
                member_manifest.display()
            ));
        }
    }

    Ok(())
}

fn manifest_check(engine: &Path) -> ExitCode {
    let path = engine.join("Cargo.toml");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("manifest-check: cannot read {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match check_cargo_manifest_content(&text, engine) {
        Ok(()) => {
            println!(
                "manifest-check: ok (engine/Cargo.toml workspace metadata and members verified)"
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("manifest-check: {err}");
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
    let jobs = env::var("CARGO_BUILD_JOBS").unwrap_or_else(|_| "2".to_string());
    println!("=== xwork: deterministic scoped verification pipeline (-j {jobs}) ===");

    println!("xwork: [projectfile-check] running...");
    if projectfile_check(repo) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    println!("xwork: [manifest-check] running...");
    if manifest_check(engine) != ExitCode::SUCCESS {
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
            "-j",
            &jobs,
            "--all-targets",
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
        Some("manifest-check") => manifest_check(&engine),
        Some("workflow-timeout-check") => workflow_timeout_check(&repo),
        Some("xwork") => run_xwork(&repo, &engine),
        _ => {
            eprintln!(
                "usage: cargo xtask <projectfile-check|manifest-check|workflow-timeout-check|xwork>"
            );
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

    #[test]
    fn workspace_cargo_manifest_is_valid() {
        let engine = engine_root();
        let path = engine.join("Cargo.toml");
        let text = fs::read_to_string(&path).expect("engine/Cargo.toml must exist");
        check_cargo_manifest_content(&text, &engine).expect("engine/Cargo.toml must be valid");
    }

    #[test]
    fn missing_workspace_package_field_is_rejected() {
        let engine = engine_root();
        let minimal = r#"
[workspace]
resolver = "2"
members = ["crates/xtask"]

[workspace.package]
version = "1.0.0"
edition = "2021"
"#;
        let err = check_cargo_manifest_content(minimal, &engine).unwrap_err();
        assert!(err.contains("workspace.package.rust-version"));
    }
}
