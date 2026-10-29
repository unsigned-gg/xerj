// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Conformance Test Suite for Projectfile v1 Engine.
//!
//! Validates 100% of upstream specification conformance vectors:
//! - §3.8 Interpolation vectors (`tests/conformance/interpolation/*.yaml`)
//! - `org.projectfile.ci` DAG vectors (`tests/conformance/ci/*.yaml`)
//! - `org.projectfile.image` Tag vectors (`tests/conformance/image/*.yaml`)
//! - Deterministic `Justfile` Lowering

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use lsc_core::projectfile::{
    CiResolveResult, CiResolver, ImageResolver, JustTargetOptions, JustfileCompiler,
    ProjectfileInterpolator,
};
use serde_json::Value;

fn test_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/conformance")
}

#[test]
fn test_conformance_interpolation_vectors() {
    let dir = test_fixtures_dir().join("interpolation");
    assert!(dir.is_dir(), "Interpolation conformance directory missing");

    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("read interpolation fixtures dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("yaml"))
        .collect();
    entries.sort_by_key(|e| e.path());

    assert!(!entries.is_empty(), "No interpolation fixtures found");

    let mut total_passed = 0;
    let mut total_cases = 0;

    for entry in entries {
        let path = entry.path();
        let content = fs::read_to_string(&path).expect("read fixture file");
        let data: Value = serde_yaml::from_str(&content).expect("parse fixture YAML");

        let doc = data
            .get("document")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        let interpolator = ProjectfileInterpolator::new(&doc);

        let cases = data
            .get("cases")
            .and_then(|c| c.as_array())
            .expect("fixture cases array");

        for (idx, case) in cases.iter().enumerate() {
            let in_str = case
                .get("in")
                .and_then(|i| i.as_str())
                .expect("case in string");
            let expected_out = case
                .get("out")
                .and_then(|o| o.as_str())
                .expect("case out string");

            let actual_out = interpolator.interpolate_string(in_str, None, 0);
            assert_eq!(
                actual_out,
                expected_out,
                "Failed case {} in {}:\n  In:       {}\n  Expected: {}\n  Actual:   {}",
                idx + 1,
                path.file_name().unwrap().to_string_lossy(),
                in_str,
                expected_out,
                actual_out
            );

            if let Some(expected_lines) = case.get("lines").and_then(|l| l.as_array()) {
                let exp_lines_vec: Vec<String> = expected_lines
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(ToString::to_string)
                    .collect();
                let actual_lines = interpolator.interpolate_lines(in_str);
                assert_eq!(
                    actual_lines,
                    exp_lines_vec,
                    "Failed lines case {} in {}:\n  In:       {}\n  Expected: {:?}\n  Actual:   {:?}",
                    idx + 1,
                    path.file_name().unwrap().to_string_lossy(),
                    in_str,
                    exp_lines_vec,
                    actual_lines
                );
            }

            total_passed += 1;
            total_cases += 1;
        }
    }

    assert_eq!(total_passed, total_cases);
    println!("Interpolation Conformance: {total_passed}/{total_cases} test vectors passed (100%)");
}

#[test]
fn test_conformance_ci_dag_vectors() {
    let dir = test_fixtures_dir().join("ci");
    assert!(dir.is_dir(), "CI conformance directory missing");

    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("read ci fixtures dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("yaml"))
        .collect();
    entries.sort_by_key(|e| e.path());

    assert!(!entries.is_empty(), "No CI fixtures found");

    let mut count = 0;
    for entry in entries {
        let path = entry.path();
        let fname = path.file_name().unwrap().to_string_lossy().to_string();
        let content = fs::read_to_string(&path).expect("read fixture file");
        let data: Value = serde_yaml::from_str(&content).expect("parse fixture YAML");

        let res = CiResolver::resolve_fixture(&data);
        let exp = data.get("expect").expect("expect section in CI fixture");

        if let Some(exp_reject) = exp.get("reject").and_then(|r| r.as_str()) {
            assert_eq!(
                res.reject.as_deref(),
                Some(exp_reject),
                "{fname}: expected reject '{exp_reject}', got {:?}",
                res.reject
            );
        } else {
            assert!(
                res.reject.is_none(),
                "{fname}: unexpected rejection: {:?}",
                res.reject
            );

            if let Some(exp_runs) = exp.get("runs").and_then(|r| r.as_array()) {
                let exp_runs_vec: Vec<String> = exp_runs
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(ToString::to_string)
                    .collect();
                let actual_set: std::collections::BTreeSet<_> = res.runs.iter().collect();
                let expected_set: std::collections::BTreeSet<_> = exp_runs_vec.iter().collect();
                assert_eq!(actual_set, expected_set, "{fname}: runs set mismatch");
            }

            if let Some(exp_counts) = exp.get("counts").and_then(|c| c.as_object()) {
                for (tool, count_val) in exp_counts {
                    let expected_c = count_val.as_u64().unwrap() as usize;
                    let actual_c = res.counts.get(tool).copied().unwrap_or(0);
                    assert_eq!(
                        actual_c, expected_c,
                        "{fname}: count mismatch for tool {tool}"
                    );
                }
            }

            if let Some(exp_args) = exp.get("args").and_then(|a| a.as_object()) {
                for (tool, arg_val) in exp_args {
                    let expected_a = arg_val.as_str().unwrap();
                    let actual_a = res.args.get(tool).map(String::as_str).unwrap_or("");
                    assert_eq!(
                        actual_a, expected_a,
                        "{fname}: args mismatch for tool {tool}"
                    );
                }
            }

            if let Some(exp_before) = exp.get("before").and_then(|b| b.as_array()) {
                for pair in exp_before {
                    let arr = pair.as_array().expect("pair array");
                    let p0 = arr[0].as_str().unwrap();
                    let p1 = arr[1].as_str().unwrap();
                    let has_pair = res.before.iter().any(|(a, b)| a == p0 && b == p1);
                    assert!(
                        has_pair,
                        "{fname}: expected before pair [{p0}, {p1}] not found in {:?}",
                        res.before
                    );
                }
            }
        }

        count += 1;
    }

    assert_eq!(count, 11);
    println!("CI DAG Conformance: {count}/11 fixtures passed (100%)");
}

#[test]
fn test_conformance_image_tagging_vectors() {
    let dir = test_fixtures_dir().join("image");
    assert!(dir.is_dir(), "Image conformance directory missing");

    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("read image fixtures dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("yaml"))
        .collect();
    entries.sort_by_key(|e| e.path());

    assert!(!entries.is_empty(), "No image fixtures found");

    let mut count = 0;
    for entry in entries {
        let path = entry.path();
        let fname = path.file_name().unwrap().to_string_lossy().to_string();
        let content = fs::read_to_string(&path).expect("read fixture file");
        let data: Value = serde_yaml::from_str(&content).expect("parse fixture YAML");

        let res = ImageResolver::resolve_fixture(&data);
        let exp = data.get("expect").expect("expect section in image fixture");

        if let Some(exp_reject) = exp.get("reject").and_then(|r| r.as_str()) {
            assert_eq!(
                res.reject.as_deref(),
                Some(exp_reject),
                "{fname}: expected reject '{exp_reject}', got {:?}",
                res.reject
            );
        } else {
            assert!(
                res.reject.is_none(),
                "{fname}: unexpected rejection: {:?}",
                res.reject
            );

            let exp_cells = exp
                .get("cells")
                .and_then(|c| c.as_array())
                .expect("cells in expect");
            assert_eq!(
                res.cells.len(),
                exp_cells.len(),
                "{fname}: cell count mismatch"
            );

            for exp_cell_val in exp_cells {
                let exp_cell_map = exp_cell_val
                    .get("cell")
                    .and_then(|c| c.as_object())
                    .expect("cell map");
                let mut exp_map: BTreeMap<String, String> = BTreeMap::new();
                for (k, v) in exp_cell_map {
                    let s = match v {
                        Value::String(st) => st.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => v.to_string(),
                    };
                    exp_map.insert(k.clone(), s);
                }

                let exp_tags: Vec<String> = exp_cell_val
                    .get("tags")
                    .and_then(|t| t.as_array())
                    .expect("tags array")
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        _ => v.to_string(),
                    })
                    .collect();

                let matched_cell = res.cells.iter().find(|c| c.cell == exp_map);
                assert!(
                    matched_cell.is_some(),
                    "{fname}: expected cell {exp_map:?} not found in {:?}",
                    res.cells
                );

                let actual_cell = matched_cell.unwrap();
                assert_eq!(
                    actual_cell.tags, exp_tags,
                    "{fname}: tags mismatch for cell {exp_map:?}"
                );
            }
        }

        count += 1;
    }

    assert_eq!(count, 9);
    println!("Image Tag Conformance: {count}/9 fixtures passed (100%)");
}

#[test]
fn test_justfile_lowering_compiler() {
    let mut counts = BTreeMap::new();
    counts.insert("golangci-lint".to_string(), 1);
    counts.insert("build-binaries".to_string(), 5);
    counts.insert("publish-images".to_string(), 1);

    let mut args = BTreeMap::new();
    args.insert("build-binaries".to_string(), "--release".to_string());

    let resolved = CiResolveResult {
        runs: vec![
            "golangci-lint".to_string(),
            "build-binaries".to_string(),
            "publish-images".to_string(),
        ],
        counts,
        args,
        before: vec![
            ("golangci-lint".to_string(), "build-binaries".to_string()),
            ("build-binaries".to_string(), "publish-images".to_string()),
        ],
        reject: None,
    };

    let options = JustTargetOptions::default();
    let justfile = JustfileCompiler::compile(&resolved, &options);

    assert!(justfile.contains("set shell :="));
    assert!(justfile.contains("default: ci"));
    assert!(justfile.contains("ci: publish-images"));
    assert!(justfile.contains("build-binaries: golangci-lint"));
    assert!(justfile.contains("    build-binaries --release"));
    assert!(justfile.contains("publish-images: build-binaries"));
}
