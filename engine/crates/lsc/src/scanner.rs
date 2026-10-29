use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use lsc_core::{
    compute_blake3_hash, parse_file_name, parse_folder_name, parse_folder_spec, parse_spec,
    CasEntry, CasEntryType, CasListing, SpecDocument, ValidationMessage, ValidationReport,
};

/// Where a discovered specification lives (SPEC-0005).
enum SpecSource {
    /// Flat layout: `NNNN-kebab-slug.md`.
    Flat(String),
    /// Folder layout: `NNN-kebab-slug/README.md`.
    Folder(String),
}

/// Result of scanning a specification directory.
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// Parsed specification documents.
    pub documents: Vec<SpecDocument>,
    /// Accumulated validation report.
    pub report: ValidationReport,
}

/// Scans a directory containing LeanSpec specifications (e.g. `specs/`).
///
/// # Errors
/// Returns an I/O error if the directory path cannot be read or traversed.
pub fn scan_specs_dir(dir: &Path) -> Result<ScanResult, std::io::Error> {
    let mut report = ValidationReport::default();
    let mut documents = Vec::new();

    if !dir.exists() {
        report.notices.push(format!(
            "directory '{}' does not exist; nothing to check",
            dir.display()
        ));
        return Ok(ScanResult { documents, report });
    }

    // Flat `.md` files, and folders whose name matches the folder layout (SPEC-0005).
    // Other directories, for example `formal/`, are ignored.
    let mut names: Vec<String> = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if file_type.is_file() && name.ends_with(".md") {
            names.push(name);
        } else if file_type.is_dir() && parse_folder_name(&name).is_some() {
            folders.push(name);
        }
    }
    names.sort();
    folders.sort();

    if names.is_empty() && folders.is_empty() {
        report.notices.push(format!(
            "no markdown specifications found in '{}'",
            dir.display()
        ));
        return Ok(ScanResult { documents, report });
    }

    // Keyed by numeric value, so `001` (folder) and `0001` (flat) collide.
    let mut numbers: BTreeMap<u32, String> = BTreeMap::new();
    let mut stems: BTreeSet<String> = BTreeSet::new();
    let mut sources: Vec<SpecSource> = Vec::new();

    // R01: Verify file names and number uniqueness
    for name in names.iter().filter(|n| n.as_str() != "README.md") {
        let rel_path = format!("{}/{}", dir.display(), name);
        let Some((number, _slug)) = parse_file_name(name) else {
            report.errors.push(ValidationMessage {
                file: rel_path,
                line: Some(1),
                rule: "R01".to_owned(),
                message: "file name must match NNNN-kebab-slug.md".to_owned(),
            });
            continue;
        };

        if !claim_number(&mut numbers, &number, name, &rel_path, &mut report) {
            continue;
        }
        let stem = name.strip_suffix(".md").unwrap_or(name).to_owned();
        stems.insert(stem);
        sources.push(SpecSource::Flat(name.clone()));
    }

    // R01 for the folder layout: README.md is required, numbers are shared with flat specs.
    for folder in &folders {
        let rel_path = format!("{}/{}", dir.display(), folder);
        if !dir.join(folder).join("README.md").is_file() {
            report.errors.push(ValidationMessage {
                file: rel_path,
                line: None,
                rule: "R01".to_owned(),
                message: "folder specification must contain README.md".to_owned(),
            });
            continue;
        }
        let Some((number, _slug)) = parse_folder_name(folder) else {
            continue;
        };
        let display = format!("{folder}/README.md");
        if !claim_number(&mut numbers, &number, &display, &rel_path, &mut report) {
            continue;
        }
        stems.insert(folder.clone());
        sources.push(SpecSource::Folder(folder.clone()));
    }

    // Parse each specification. Links resolve from the specification's own directory (R12).
    let mut doc_paths: BTreeMap<String, String> = BTreeMap::new();
    for source in &sources {
        let (rel_path, file_path, base_dir) = match source {
            SpecSource::Flat(name) => (
                format!("{}/{}", dir.display(), name),
                dir.join(name),
                dir.to_path_buf(),
            ),
            SpecSource::Folder(folder) => (
                format!("{}/{}/README.md", dir.display(), folder),
                dir.join(folder).join("README.md"),
                dir.join(folder),
            ),
        };
        match fs::read_to_string(&file_path) {
            Ok(content) => {
                let parsed = match source {
                    SpecSource::Flat(name) => parse_spec(name, &content, &mut report),
                    SpecSource::Folder(folder) => parse_folder_spec(folder, &content, &mut report),
                };
                if let Some(doc) = parsed {
                    // Check links in document (R12)
                    check_relative_links(&rel_path, &base_dir, &content, &mut report);
                    doc_paths.insert(doc.stem.clone(), rel_path);
                    documents.push(doc);
                }
            }
            Err(err) => {
                report.errors.push(ValidationMessage {
                    file: rel_path,
                    line: Some(1),
                    rule: "IO".to_owned(),
                    message: format!("cannot read file: {err}"),
                });
            }
        }
    }

    // R03: Verify depends_on references exist
    for doc in &documents {
        let rel_path = doc_paths
            .get(&doc.stem)
            .cloned()
            .unwrap_or_else(|| format!("{}/{}.md", dir.display(), doc.stem));
        for dep in &doc.frontmatter.depends_on {
            if !stems.contains(dep) {
                report.errors.push(ValidationMessage {
                    file: rel_path.clone(),
                    line: Some(1),
                    rule: "R03".to_owned(),
                    message: format!(
                        "depends_on stem '{dep}' does not exist in specification suite"
                    ),
                });
            }
        }
    }

    // R11, R13: Validate README.md index table if present
    let readme_path = dir.join("README.md");
    if readme_path.exists() {
        let rel_readme = format!("{}/README.md", dir.display());
        match fs::read_to_string(&readme_path) {
            Ok(readme_text) => {
                check_relative_links(&rel_readme, dir, &readme_text, &mut report);
                check_readme_index(&rel_readme, &readme_text, &documents, &mut report);
            }
            Err(err) => {
                report.errors.push(ValidationMessage {
                    file: rel_readme,
                    line: Some(1),
                    rule: "IO".to_owned(),
                    message: format!("cannot read README.md: {err}"),
                });
            }
        }
    } else if !sources.is_empty() {
        report.warnings.push(ValidationMessage {
            file: format!("{}/README.md", dir.display()),
            line: None,
            rule: "R11".to_owned(),
            message: "specification directory lacks README.md catalog index".to_owned(),
        });
    }

    Ok(ScanResult { documents, report })
}

/// Strip inline code spans enclosed in backticks (`).
fn strip_inline_code(line: &str) -> String {
    let mut stripped = String::with_capacity(line.len());
    let mut in_code = false;
    for c in line.chars() {
        if c == '`' {
            in_code = !in_code;
        } else if !in_code {
            stripped.push(c);
        }
    }
    stripped
}

/// R12: Relative links must resolve to existing files.
fn check_relative_links(file_rel: &str, dir: &Path, content: &str, report: &mut ValidationReport) {
    let mut in_code_block = false;
    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        // Strip inline code spans
        let clean_line = strip_inline_code(line);

        // Extract markdown links [text](target)
        let mut rest = clean_line.as_str();
        while let Some(open) = rest.find("](") {
            let target_start = open + 2;
            rest = &rest[target_start..];
            if let Some(close) = rest.find(')') {
                let target = rest[..close].trim();
                rest = &rest[close + 1..];

                // Skip web links, email, or anchor-only links
                if target.starts_with("http://")
                    || target.starts_with("https://")
                    || target.starts_with("mailto:")
                    || target.starts_with('#')
                {
                    continue;
                }

                // Strip anchor if present
                let target_path = target.split('#').next().unwrap_or("");
                if target_path.is_empty() {
                    continue;
                }

                let resolved = dir.join(target_path);
                if !resolved.exists() {
                    report.errors.push(ValidationMessage {
                        file: file_rel.to_owned(),
                        line: Some(line_num),
                        rule: "R12".to_owned(),
                        message: format!("broken relative link target '{target}'"),
                    });
                }
            }
        }
    }
}

/// R11 & R13: Catalog index in README.md must match parsed specifications.
fn check_readme_index(
    readme_rel: &str,
    content: &str,
    docs: &[SpecDocument],
    report: &mut ValidationReport,
) {
    let mut listed_stems: BTreeSet<String> = BTreeSet::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }

        // Search for markdown link to spec
        let mut rest = trimmed;
        while let Some(open) = rest.find("](") {
            rest = &rest[open + 2..];
            if let Some(close) = rest.find(')') {
                let target = rest[..close].trim();
                rest = &rest[close + 1..];

                let target_file = target.split('#').next().unwrap_or("");
                if let Some(stem) = catalog_stem(target_file) {
                    listed_stems.insert(stem);
                }
            }
        }
    }

    for doc in docs {
        if !listed_stems.contains(&doc.stem) {
            report.warnings.push(ValidationMessage {
                file: readme_rel.to_owned(),
                line: None,
                rule: "R11".to_owned(),
                message: format!("spec '{}' is not indexed in README.md catalog", doc.stem),
            });
        }
    }
}

/// R01: record a specification number, or report it as a duplicate. Numbers compare by
/// numeric value across both layouts (SPEC-0005). Returns whether the number was free.
fn claim_number(
    numbers: &mut BTreeMap<u32, String>,
    number: &str,
    display: &str,
    rel_path: &str,
    report: &mut ValidationReport,
) -> bool {
    let Ok(value) = number.parse::<u32>() else {
        return false;
    };
    if let Some(existing) = numbers.get(&value) {
        report.errors.push(ValidationMessage {
            file: rel_path.to_owned(),
            line: Some(1),
            rule: "R01".to_owned(),
            message: format!("specification number '{number}' already used by '{existing}'"),
        });
        return false;
    }
    numbers.insert(value, display.to_owned());
    true
}

/// Stem that a catalog link points at (R11): a flat `NNNN-slug.md`, or a folder
/// specification as `NNN-slug/README.md` or `NNN-slug/` (SPEC-0005).
fn catalog_stem(target: &str) -> Option<String> {
    let target = target.strip_prefix("./").unwrap_or(target);
    if let Some(folder) = target
        .strip_suffix("/README.md")
        .or_else(|| target.strip_suffix('/'))
    {
        return parse_folder_name(folder).map(|_| folder.to_owned());
    }
    let stem = target.strip_suffix(".md").unwrap_or(target);
    parse_file_name(&format!("{stem}.md")).map(|_| stem.to_owned())
}

/// Scans a path and computes its content-addressed BLAKE3 digest.
/// If `path` is a file, returns its file digest.
/// If `path` is a directory, returns the CAS Merkle tree root hash.
///
/// # Errors
/// Returns an I/O error if files cannot be read.
pub fn scan_cas_path(path: &Path) -> Result<String, std::io::Error> {
    if path.is_file() {
        let bytes = fs::read(path)?;
        Ok(compute_blake3_hash(&bytes))
    } else if path.is_dir() {
        let listing = scan_cas_directory(path)?;
        Ok(listing.root_hash)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("path '{}' not found", path.display()),
        ))
    }
}

/// Recursively scans a directory into a deterministic `CasListing`.
///
/// Ignores `.git` and `target` directories.
///
/// # Errors
/// Returns an I/O error if directory traversal fails.
pub fn scan_cas_directory(dir: &Path) -> Result<CasListing, std::io::Error> {
    let mut entries = Vec::new();
    collect_cas_entries(dir, dir, &mut entries)?;
    Ok(CasListing::from_entries(entries))
}

fn collect_cas_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<CasEntry>,
) -> Result<(), std::io::Error> {
    if !current.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if name_str == ".git" || name_str == "target" || name_str == ".cargo" {
            continue;
        }

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_cas_entries(root, &path, entries)?;
        } else if file_type.is_file() {
            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path)?;
            let hash = compute_blake3_hash(&bytes);
            let size = bytes.len() as u64;
            entries.push(CasEntry::new(hash, rel_path, size, CasEntryType::Blob));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;

    struct TestDir {
        path: std::path::PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "ls_test_{}_{}",
                name,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write_file(&self, name: &str, content: &str) {
            fs::write(self.path.join(name), content).unwrap();
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_scan_valid_specs() {
        let td = TestDir::new("valid");
        td.write_file(
            "0001-template.md",
            r#"---
status: complete
created: 2026-10-11
---

# 0001-template

## Requirements

The system shall initialize cleanly.

## Design

Design notes.

## Tasks

Tasks list.

## Verification

```bash
cargo test
```

## Notes

Final notes.
"#,
        );
        td.write_file(
            "README.md",
            r#"# Specifications Index

| Spec | Title | Status |
|---|---|---|
| [0001-template.md](./0001-template.md) | Template Architecture | complete |
"#,
        );

        let res = scan_specs_dir(&td.path).unwrap();
        assert_eq!(res.documents.len(), 1);
        assert!(res.report.is_clean());
        assert_eq!(res.documents[0].number, "0001");
    }

    #[test]
    fn test_scan_broken_link_detected() {
        let td = TestDir::new("broken_link");
        td.write_file(
            "0001-first.md",
            r#"---
status: complete
created: 2026-10-11
---

# 0001-first

## Requirements

The system shall link to [nonexistent](./nonexistent.md).

## Design

Design.

## Tasks

Tasks.

## Verification

```bash
cargo test
```

## Notes

Notes.
"#,
        );

        let res = scan_specs_dir(&td.path).unwrap();
        assert!(!res.report.is_clean());
        assert!(res.report.errors.iter().any(|e| e.rule == "R12"));
    }
}
