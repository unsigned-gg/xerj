//! Core LeanSpec validation rules and pure parser logic.
//!
//! Evaluates specifications against formal architectural rules (R01-R16).
//! Extracts embedded Lean 4 theorem statements and computes BLAKE3 CAS hashes.
//! Zero I/O is performed in this module.

use crate::cas::compute_blake3_hash;
use crate::document::{
    SpecDocument, SpecFrontmatter, SpecPriority, SpecSection, SpecStatus, ValidationMessage,
    ValidationReport,
};
use crate::lean::{
    extract_lean_blocks, parse_lean_source, LeanTheorem, ProofStatus, MAX_AST_RECURSION_DEPTH,
};

/// Modern LPSL canonical section order.
pub const MODERN_SECTIONS: &[&str] = &["Requirements", "Design", "Tasks", "Verification", "Notes"];

/// Legacy LeanSpec section order.
pub const LEGACY_SECTIONS: &[&str] = &["Overview", "Design", "Plan", "Test", "Notes"];

/// Parse and validate the specification number and slug from a file name.
///
/// Returns `Some((number, slug))` if format matches `NNNN-kebab-slug.md` or `SPEC-NNNN-kebab-slug.md`.
#[must_use]
pub fn parse_file_name(file_name: &str) -> Option<(String, String)> {
    let name = file_name.strip_suffix(".md")?;
    let stripped = name.strip_prefix("SPEC-").unwrap_or(name);
    let (num_part, slug_part) = stripped.split_once('-')?;

    if num_part.len() == 4 && num_part.chars().all(|c| c.is_ascii_digit()) && !slug_part.is_empty()
    {
        let is_kebab = slug_part
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if is_kebab {
            return Some((num_part.to_owned(), slug_part.to_owned()));
        }
    }
    None
}

/// Parse the specification number and slug from a folder-layout directory name.
///
/// Returns `Some((number, slug))` if the name matches `NNN-kebab-slug` or `NNNN-kebab-slug`
/// (SPEC-0005), for example the `001-my-first-feature` folder that `leanspec create` writes.
#[must_use]
pub fn parse_folder_name(dir_name: &str) -> Option<(String, String)> {
    let (num_part, slug_part) = dir_name.split_once('-')?;
    let number_ok =
        (3..=4).contains(&num_part.len()) && num_part.chars().all(|c| c.is_ascii_digit());
    let slug_ok = !slug_part.is_empty()
        && slug_part
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (number_ok && slug_ok).then(|| (num_part.to_owned(), slug_part.to_owned()))
}

/// Identity of a specification, from its flat file name or its folder name.
struct SpecIdentity {
    stem: String,
    number: String,
    slug: String,
    /// R04: the H1 must equal the stem (flat layout), or need only exist (folder layout).
    h1_equals_stem: bool,
}

/// Parse raw markdown text into a `SpecDocument` without filesystem access.
///
/// # Errors
/// Emits errors in `ValidationReport` if frontmatter or structural syntax is broken.
pub fn parse_spec(
    file_name: &str,
    text: &str,
    report: &mut ValidationReport,
) -> Option<SpecDocument> {
    let (number, slug) = match parse_file_name(file_name) {
        Some(pair) => pair,
        None => {
            report.errors.push(ValidationMessage {
                file: file_name.to_owned(),
                line: Some(1),
                rule: "R01".to_owned(),
                message: "file name must match NNNN-kebab-slug.md or SPEC-NNNN-kebab-slug.md"
                    .to_owned(),
            });
            return None;
        }
    };

    let stem = file_name
        .strip_suffix(".md")
        .unwrap_or(file_name)
        .to_owned();
    let identity = SpecIdentity {
        stem,
        number,
        slug,
        h1_equals_stem: true,
    };
    parse_spec_body(file_name, identity, text, report)
}

/// Parse a folder-layout specification (`NNN-kebab-slug/README.md`) without filesystem access.
///
/// The folder name is the stem, number and slug (SPEC-0005). R04 requires only that the
/// document has an H1, because tools such as `leanspec` write a human title there.
///
/// # Errors
/// Emits errors in `ValidationReport` if the folder name, frontmatter or structural syntax
/// is broken.
pub fn parse_folder_spec(
    dir_name: &str,
    text: &str,
    report: &mut ValidationReport,
) -> Option<SpecDocument> {
    let file_name = format!("{dir_name}/README.md");
    let Some((number, slug)) = parse_folder_name(dir_name) else {
        report.errors.push(ValidationMessage {
            file: file_name,
            line: Some(1),
            rule: "R01".to_owned(),
            message: "folder name must match NNN-kebab-slug or NNNN-kebab-slug".to_owned(),
        });
        return None;
    };
    let identity = SpecIdentity {
        stem: dir_name.to_owned(),
        number,
        slug,
        h1_equals_stem: false,
    };
    parse_spec_body(&file_name, identity, text, report)
}

fn parse_spec_body(
    file_name: &str,
    identity: SpecIdentity,
    text: &str,
    report: &mut ValidationReport,
) -> Option<SpecDocument> {
    let SpecIdentity {
        stem,
        number,
        slug,
        h1_equals_stem,
    } = identity;
    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len();

    // Line limit checks (R10)
    if line_count > 400 {
        report.errors.push(ValidationMessage {
            file: file_name.to_owned(),
            line: Some(line_count),
            rule: "R10".to_owned(),
            message: format!("file exceeds maximum limit: {line_count} lines > 400"),
        });
    } else if line_count > 300 {
        report.warnings.push(ValidationMessage {
            file: file_name.to_owned(),
            line: Some(line_count),
            rule: "R10".to_owned(),
            message: format!("file is long: {line_count} lines > 300; consider splitting"),
        });
    }

    // Parse Frontmatter (R02)
    let frontmatter = parse_frontmatter(file_name, &lines, report)?;

    // R15: Self-dependency cycle check
    if frontmatter.depends_on.contains(&stem) {
        report.errors.push(ValidationMessage {
            file: file_name.to_owned(),
            line: Some(1),
            rule: "R15".to_owned(),
            message: format!("specification cannot depend on itself: '{stem}'"),
        });
    }

    // Find H1 (R04)
    let mut h1 = None;
    let mut start_idx = 0;
    for (idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            h1 = Some((idx + 1, heading.trim().to_owned()));
            start_idx = idx + 1;
            break;
        }
    }

    let h1_title = match h1 {
        Some((line_num, title)) => {
            if h1_equals_stem && title != stem {
                report.errors.push(ValidationMessage {
                    file: file_name.to_owned(),
                    line: Some(line_num),
                    rule: "R04".to_owned(),
                    message: format!("H1 '{title}' must equal file stem '{stem}'"),
                });
            }
            title
        }
        None => {
            let message = if h1_equals_stem {
                format!("missing top-level H1 heading equal to '{stem}'")
            } else {
                "missing top-level H1 heading in folder specification README.md".to_owned()
            };
            report.errors.push(ValidationMessage {
                file: file_name.to_owned(),
                line: None,
                rule: "R04".to_owned(),
                message,
            });
            return None;
        }
    };

    // Parse H2 Sections (R05, R06)
    let sections = parse_sections(file_name, &lines[start_idx..], start_idx + 1, report);

    // Extract and parse embedded Lean 4 blocks & theorem statements
    let lean_blocks = extract_lean_blocks(text);
    let mut lean_modules = Vec::new();
    let mut theorems: Vec<LeanTheorem> = Vec::new();

    for (start_line, block_src) in lean_blocks {
        match parse_lean_source(&block_src, start_line, MAX_AST_RECURSION_DEPTH) {
            Ok(module) => {
                for th in module.theorems() {
                    theorems.push(th.clone());
                }
                lean_modules.push(module);
            }
            Err(err) => {
                report.errors.push(ValidationMessage {
                    file: file_name.to_owned(),
                    line: Some(start_line),
                    rule: "R15".to_owned(),
                    message: format!("Lean AST parse error: {err}"),
                });
            }
        }
    }

    // R14: Check theorem completeness in complete specifications
    if frontmatter.status == SpecStatus::Complete {
        for th in &theorems {
            if matches!(th.proof_status, ProofStatus::Sorry) {
                report.warnings.push(ValidationMessage {
                    file: file_name.to_owned(),
                    line: Some(th.line_number),
                    rule: "R14".to_owned(),
                    message: format!(
                        "theorem '{}' contains 'sorry' proof placeholder in a complete specification",
                        th.name
                    ),
                });
            }
        }
    }

    // Compute canonical BLAKE3 digest of source text
    let content_hash = compute_blake3_hash(text.as_bytes());

    Some(SpecDocument {
        stem,
        number,
        slug,
        h1: h1_title,
        frontmatter,
        sections,
        theorems,
        lean_modules,
        content_hash,
        line_count,
    })
}

fn parse_frontmatter(
    file_name: &str,
    lines: &[&str],
    report: &mut ValidationReport,
) -> Option<SpecFrontmatter> {
    if lines.is_empty() || lines[0].trim() != "---" {
        report.errors.push(ValidationMessage {
            file: file_name.to_owned(),
            line: Some(1),
            rule: "R02".to_owned(),
            message: "file must begin with frontmatter fence '---'".to_owned(),
        });
        return None;
    }

    let mut end_idx = None;
    for (idx, &line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end_idx = Some(idx);
            break;
        }
    }

    let end_idx = match end_idx {
        Some(idx) => idx,
        None => {
            report.errors.push(ValidationMessage {
                file: file_name.to_owned(),
                line: Some(1),
                rule: "R02".to_owned(),
                message: "unclosed frontmatter: missing closing '---'".to_owned(),
            });
            return None;
        }
    };

    let mut status = None;
    let mut created = None;
    let mut tags = Vec::new();
    let mut priority = None;
    let mut depends_on = Vec::new();
    let mut scope = None;

    for &line in &lines[1..end_idx] {
        let trimmed = line.trim();
        if let Some((key, val)) = trimmed.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            match key {
                "status" => status = SpecStatus::from_str_lossy(val),
                "created" => created = Some(val.trim_matches('"').to_owned()),
                "priority" => priority = SpecPriority::from_str_lossy(val),
                "scope" => scope = Some(val.trim_matches('"').to_owned()),
                "tags" => {
                    let cleaned = val.trim_matches(|c| c == '[' || c == ']' || c == '"');
                    tags = cleaned
                        .split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "depends_on" => {
                    let cleaned = val.trim_matches(|c| c == '[' || c == ']' || c == '"');
                    depends_on = cleaned
                        .split(',')
                        .map(|s| s.trim().to_owned())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
    }

    let status = match status {
        Some(s) => s,
        None => {
            report.errors.push(ValidationMessage {
                file: file_name.to_owned(),
                line: Some(1),
                rule: "R02".to_owned(),
                message: "frontmatter must include valid 'status'".to_owned(),
            });
            return None;
        }
    };

    let created = created.unwrap_or_else(|| "1970-01-01".to_owned());

    Some(SpecFrontmatter {
        status,
        created,
        tags,
        priority,
        depends_on,
        scope,
    })
}

fn parse_sections(
    file_name: &str,
    lines: &[&str],
    line_offset: usize,
    report: &mut ValidationReport,
) -> Vec<SpecSection> {
    let mut sections = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_start = line_offset;
    let mut current_body = Vec::new();

    for (idx, &line) in lines.iter().enumerate() {
        let abs_line = line_offset + idx;
        let trimmed = line.trim();

        if let Some(title) = trimmed.strip_prefix("## ") {
            if let Some(prev_title) = current_title.take() {
                sections.push(SpecSection {
                    title: prev_title,
                    start_line: current_start,
                    body: current_body.join("\n"),
                });
                current_body.clear();
            }
            current_title = Some(title.trim().to_owned());
            current_start = abs_line;
        } else if current_title.is_some() {
            current_body.push(line);
        }
    }

    if let Some(last_title) = current_title {
        sections.push(SpecSection {
            title: last_title,
            start_line: current_start,
            body: current_body.join("\n"),
        });
    }

    // Validate section order (R05)
    let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
    let is_modern = titles.contains(&"Requirements") || titles.contains(&"Tasks");
    let expected = if is_modern {
        MODERN_SECTIONS
    } else {
        LEGACY_SECTIONS
    };

    for title in &titles {
        if !expected.contains(title) {
            report.warnings.push(ValidationMessage {
                file: file_name.to_owned(),
                line: None,
                rule: "R05".to_owned(),
                message: format!("custom or unexpected H2 section: '{title}'"),
            });
        }
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_spec_parse_with_lean() {
        let raw = r#"---
status: complete
created: 2026-10-11
tags: [test, spec]
priority: high
depends_on: [0001-template-architecture]
---

# 0002-test-spec

## Requirements

Requirements text.

```lean
def double (x : Nat) : Nat := x + x

theorem double_zero : double 0 = 0 := by
  rfl
```

## Design

Design body.

## Tasks

Tasks body.

## Verification

```bash
cargo test
```

## Notes

Notes body.
"#;
        let mut report = ValidationReport::default();
        let doc = parse_spec("0002-test-spec.md", raw, &mut report);
        assert!(doc.is_some());
        assert!(report.is_clean());
        let doc = doc.unwrap();
        assert_eq!(doc.number, "0002");
        assert_eq!(doc.slug, "test-spec");
        assert_eq!(doc.frontmatter.status, SpecStatus::Complete);
        assert_eq!(doc.theorems.len(), 1);
        assert_eq!(doc.theorems[0].name, "double_zero");
        assert_eq!(doc.content_hash.len(), 64);
    }

    #[test]
    fn test_spec_name_with_spec_prefix() {
        assert_eq!(
            parse_file_name("SPEC-0001-leanspec-ast.md"),
            Some(("0001".to_owned(), "leanspec-ast".to_owned()))
        );
    }
}
