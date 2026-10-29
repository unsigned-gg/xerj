//! Folder specification layout (SPEC-0005; CER-2936): `specs/NNN-slug/README.md` alongside
//! flat `specs/NNNN-slug.md`, with identity from the folder name.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

use lsc::{scan_specs_dir, ScanResult};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/folder-layout")
        .join(name)
        .join("specs")
}

fn scan(name: &str) -> ScanResult {
    scan_specs_dir(&fixture(name)).unwrap()
}

fn rules(result: &ScanResult) -> Vec<&str> {
    result
        .report
        .errors
        .iter()
        .map(|e| e.rule.as_str())
        .collect()
}

#[test]
fn leanspec_generated_folder_spec_has_zero_errors() {
    // Verbatim output of `leanspec create my-first-feature` (lean-spec 0.3.0).
    let res = scan("leanspec");
    assert!(res.report.is_clean(), "{:?}", res.report.errors);
    assert_eq!(res.documents.len(), 1);
    let doc = &res.documents[0];
    assert_eq!(doc.stem, "001-my-first-feature");
    assert_eq!(doc.number, "001");
    assert_eq!(doc.slug, "my-first-feature");
    assert_eq!(doc.h1, "My First Feature");
}

#[test]
fn flat_spec_r04_still_requires_h1_equal_to_stem() {
    // Control: the folder rule must not relax R04 for flat specifications.
    let res = scan("flat-r04");
    assert_eq!(rules(&res), ["R04"], "{:?}", res.report.errors);
}

#[test]
fn mixed_tree_parses_both_layouts_with_zero_errors() {
    let res = scan("mixed");
    assert!(res.report.is_clean(), "{:?}", res.report.errors);
    let mut stems: Vec<&str> = res.documents.iter().map(|d| d.stem.as_str()).collect();
    stems.sort_unstable();
    // `formal/` is ignored and `003-folder-spec/IMPLEMENTATION.md` is not a specification.
    assert_eq!(
        stems,
        ["0002-flat-spec", "003-folder-spec", "004-other-folder"]
    );
    // Both catalog link forms index the folder specifications, so R11 stays silent.
    assert!(
        !res.report.warnings.iter().any(|w| w.rule == "R11"),
        "{:?}",
        res.report.warnings
    );
}

#[test]
fn flat_and_folder_spec_with_same_number_is_r01_duplicate() {
    let res = scan("duplicate");
    let dups: Vec<_> = res
        .report
        .errors
        .iter()
        .filter(|e| e.rule == "R01" && e.message.contains("already used"))
        .collect();
    assert_eq!(dups.len(), 1, "{:?}", res.report.errors);
}

#[test]
fn matching_folder_without_readme_is_r01() {
    let res = scan("missing-readme");
    assert!(
        res.report
            .errors
            .iter()
            .any(|e| e.rule == "R01" && e.file.ends_with("005-no-readme")),
        "{:?}",
        res.report.errors
    );
}

#[test]
fn parse_command_treats_folder_readme_as_folder_spec() {
    let path = fixture("leanspec").join("001-my-first-feature/README.md");
    let out = Command::new(env!("CARGO_BIN_EXE_lsc"))
        .arg("parse")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["stem"], "001-my-first-feature");
    assert_eq!(doc["h1"], "My First Feature");
}
