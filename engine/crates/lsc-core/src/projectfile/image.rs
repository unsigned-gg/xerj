// SPDX-License-Identifier: Apache-2.0 OR MIT
//! `org.projectfile.image` Tag Naming and Derivation Engine.
//!
//! Conforms to normative specification <https://projectfile.org/spec/v1>
//! and conformance vectors under `spec/conformance/image/*.yaml`.
//!
//! # Invariants
//! - Zero-I/O: operates purely on in-memory buffers and serde values.
//! - Collision refusal: rejects overlapping tags across distinct matrix cells.
//! - Variant vs head collision refusal: rejects variants that shadow publishing heads.
//! - Deterministic alias default generation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::ci::compute_matrix_cells;

/// Resulting image tags derived for a single matrix cell.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImageCellResult {
    /// Dimension coordinates defining this image variant.
    pub cell: BTreeMap<String, String>,
    /// Derived container image tags in priority order.
    pub tags: Vec<String>,
}

/// Result of `org.projectfile.image` tag derivation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImageResolveResult {
    /// Cells and their derived tags.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub cells: Vec<ImageCellResult>,
    /// Error code if rejected (e.g., `"head-is-variant"` or `"cell-collision"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject: Option<String>,
}

impl ImageResolveResult {
    /// Creates a rejected result with the specified error code.
    #[must_use]
    pub fn rejected(code: impl Into<String>) -> Self {
        Self {
            cells: Vec::new(),
            reject: Some(code.into()),
        }
    }
}

/// Zero-I/O Image Tag Resolver.
pub struct ImageResolver;

impl ImageResolver {
    /// Resolves an image tag derivation fixture.
    #[must_use]
    pub fn resolve_fixture(data: &Value) -> ImageResolveResult {
        let input_val = data.get("input").unwrap_or(data);
        let image_sec = input_val.get("image").unwrap_or(&Value::Null);
        let build_sec = input_val.get("build").unwrap_or(&Value::Null);
        let ci_sec = input_val.get("ci").unwrap_or(&Value::Null);
        let publish_sec = data.get("publish").unwrap_or(&Value::Null);

        // 1. Derive publishing facts and release kind
        let mut facts: BTreeMap<String, String> = BTreeMap::new();
        let mut kind = "release";

        if let Some(ver_val) = publish_sec.get("version") {
            let mut ver = Self::value_to_string(ver_val);
            if ver.starts_with('v') {
                ver = ver[1..].to_string();
            }
            facts.insert("version".to_string(), ver.clone());

            let semver_parts: Vec<&str> = ver.split('.').collect();
            if semver_parts.len() == 3
                && semver_parts
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            {
                kind = "release";
                facts.insert("major".to_string(), semver_parts[0].to_string());
                facts.insert("minor".to_string(), semver_parts[1].to_string());
                facts.insert("patch".to_string(), semver_parts[2].to_string());
            } else {
                kind = "prerelease";
            }
        } else if let Some(branch_val) = publish_sec.get("branch") {
            let b = Self::value_to_string(branch_val);
            let b_clean: String = b
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            facts.insert("branch".to_string(), b_clean);

            if publish_sec.get("trunk").and_then(|v| v.as_bool()) == Some(true) {
                kind = "trunk";
            } else {
                kind = "branch";
            }
        }

        // 2. Expand heads
        let heads_cfg = image_sec.get("heads").and_then(|h| h.as_object());
        let head_templates: Vec<String> = if let Some(cfg) = heads_cfg {
            if let Some(arr) = cfg.get(kind).and_then(|k| k.as_array()) {
                arr.iter().map(Self::value_to_string).collect()
            } else if kind == "release" {
                vec![
                    "${version}".to_string(),
                    "${major}.${minor}".to_string(),
                    "${major}".to_string(),
                    "latest".to_string(),
                ]
            } else {
                vec!["latest".to_string()]
            }
        } else if kind == "release" {
            vec![
                "${version}".to_string(),
                "${major}.${minor}".to_string(),
                "${major}".to_string(),
                "latest".to_string(),
            ]
        } else {
            vec!["latest".to_string()]
        };

        let mut heads = Vec::new();
        for ht in head_templates {
            let mut res = ht;
            for (k, v) in &facts {
                res = res.replace(&format!("${{{k}}}"), v);
            }
            heads.push(res);
        }

        // 3. Matrix raw cells
        let raw_cells = compute_matrix_cells(ci_sec.get("matrix"));
        let raw_cells = if raw_cells.is_empty() {
            vec![BTreeMap::new()]
        } else {
            raw_cells
        };

        // 4. Variant configuration check
        let Some(variant_obj) = image_sec.get("variant").and_then(|v| v.as_object()) else {
            return ImageResolveResult {
                cells: vec![ImageCellResult {
                    cell: BTreeMap::new(),
                    tags: heads,
                }],
                reject: None,
            };
        };

        let parts_def: Vec<String> = variant_obj
            .get("parts")
            .and_then(|p| p.as_array())
            .map(|arr| arr.iter().map(Self::value_to_string).collect())
            .unwrap_or_default();

        let join_char = variant_obj
            .get("join")
            .and_then(|j| j.as_str())
            .unwrap_or("-");

        let tag_template = variant_obj
            .get("tag")
            .and_then(|t| t.as_str())
            .unwrap_or("${head}-${variant}");

        let aliases_defaults =
            variant_obj.get("aliases").and_then(|a| a.as_str()) == Some("defaults");

        let bare_heads: BTreeSet<String> = variant_obj
            .get("bare")
            .and_then(|b| b.as_array())
            .map(|arr| arr.iter().map(Self::value_to_string).collect())
            .unwrap_or_default();

        let mut build_args = BTreeMap::new();
        if let Some(args_obj) = build_sec.get("args").and_then(|a| a.as_object()) {
            for (k, v) in args_obj {
                build_args.insert(k.clone(), Self::value_to_string(v));
            }
        }

        // 5. Build distinct cell values for the declared parts
        let mut distinct_cells: Vec<BTreeMap<String, String>> = Vec::new();

        for raw_c in &raw_cells {
            let mut cell_dict = BTreeMap::new();
            for p in &parts_def {
                let tmpl = image_sec
                    .get(p)
                    .map(Self::value_to_string)
                    .unwrap_or_else(|| p.clone());

                let mut val = tmpl;
                for (ax_k, ax_v) in raw_c {
                    val = val.replace(&format!("{{{ax_k}}}"), ax_v);
                }
                for (ax_k, ax_v) in &build_args {
                    val = val.replace(&format!("{{{ax_k}}}"), ax_v);
                }
                cell_dict.insert(p.clone(), val);
            }

            if !distinct_cells.contains(&cell_dict) {
                distinct_cells.push(cell_dict);
            }
        }

        // 6. Identify default values for each part
        let mut part_defaults: BTreeMap<String, String> = BTreeMap::new();
        for p in &parts_def {
            let tmpl = image_sec
                .get(p)
                .map(Self::value_to_string)
                .unwrap_or_else(|| p.clone());

            let mut val = tmpl;
            for (ax_k, ax_v) in &build_args {
                val = val.replace(&format!("{{{ax_k}}}"), ax_v);
            }
            if !val.contains('{') {
                part_defaults.insert(p.clone(), val);
            }
        }

        // 7. Derive variant aliases and tags per cell
        let mut output_cells = Vec::new();
        let mut all_derived_tags: BTreeMap<String, usize> = BTreeMap::new();

        for (cell_idx, cell) in distinct_cells.iter().enumerate() {
            let part_vals: Vec<String> = parts_def
                .iter()
                .map(|p| cell.get(p).cloned().unwrap_or_default())
                .collect();

            let primary_variant = part_vals.join(join_char);
            let mut variants = vec![primary_variant];

            if aliases_defaults {
                let default_indices: Vec<usize> = parts_def
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| cell.get(*p) == part_defaults.get(*p))
                    .map(|(i, _)| i)
                    .collect();

                for r in 1..=default_indices.len() {
                    let mut combos = generate_combinations(&default_indices, r);
                    combos.sort_by(|a, b| b.cmp(a));

                    for drop_combo in combos {
                        let kept_vals: Vec<String> = part_vals
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| !drop_combo.contains(i))
                            .map(|(_, val)| val.clone())
                            .collect();

                        let v = kept_vals.join(join_char);
                        if !variants.contains(&v) {
                            variants.push(v);
                        }
                    }
                }
            }

            // Head collision refusal
            for v in &variants {
                if !v.is_empty() && heads.contains(v) {
                    return ImageResolveResult::rejected("head-is-variant");
                }
            }

            // Expand tags
            let mut cell_tags = Vec::new();
            for v in &variants {
                for h in &heads {
                    let tag = if v.is_empty() {
                        h.clone()
                    } else if bare_heads.contains(h) {
                        v.clone()
                    } else {
                        tag_template.replace("${head}", h).replace("${variant}", v)
                    };

                    if !cell_tags.contains(&tag) {
                        cell_tags.push(tag);
                    }
                }
            }

            // Inter-cell collision refusal
            for t in &cell_tags {
                if let Some(&prev_idx) = all_derived_tags.get(t) {
                    if prev_idx != cell_idx {
                        return ImageResolveResult::rejected("cell-collision");
                    }
                }
                all_derived_tags.insert(t.clone(), cell_idx);
            }

            output_cells.push(ImageCellResult {
                cell: cell.clone(),
                tags: cell_tags,
            });
        }

        ImageResolveResult {
            cells: output_cells,
            reject: None,
        }
    }

    fn value_to_string(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            _ => v.to_string(),
        }
    }
}

fn generate_combinations(items: &[usize], k: usize) -> Vec<Vec<usize>> {
    let mut result = Vec::new();
    let mut current = Vec::new();
    fn backtrack(
        items: &[usize],
        k: usize,
        start: usize,
        current: &mut Vec<usize>,
        result: &mut Vec<Vec<usize>>,
    ) {
        if current.len() == k {
            result.push(current.clone());
            return;
        }
        for i in start..items.len() {
            current.push(items[i]);
            backtrack(items, k, i + 1, current, result);
            current.pop();
        }
    }
    backtrack(items, k, 0, &mut current, &mut result);
    result
}
