//! Status vocabulary (SPEC-0001, "Status vocabulary"; CER-2936): aliases from other
//! LeanSpec tools parse to the canonical `SpecStatus` without rewriting the source.

use lsc_core::{parse_spec, SpecStatus, ValidationReport};

fn spec_with_status(status: &str, body: &str) -> String {
    format!(
        "---\nstatus: {status}\ncreated: 2026-10-18\n---\n\n# 0001-alias-probe\n\n\
         ## Requirements\n\nThe parser shall accept the status.\n{body}\n\
         ## Design\n\nDesign.\n\n## Tasks\n\nTasks.\n\n## Verification\n\n\
         ```bash\ncargo test\n```\n\n## Notes\n\nNotes.\n"
    )
}

fn parse(status: &str, body: &str) -> (Option<SpecStatus>, ValidationReport) {
    let mut report = ValidationReport::default();
    let doc = parse_spec(
        "0001-alias-probe.md",
        &spec_with_status(status, body),
        &mut report,
    );
    (doc.map(|d| d.frontmatter.status), report)
}

#[test]
fn canonical_words_still_parse() {
    for (word, expected) in [
        ("planned", SpecStatus::Planned),
        ("in-progress", SpecStatus::InProgress),
        ("complete", SpecStatus::Complete),
        ("archived", SpecStatus::Archived),
    ] {
        assert_eq!(SpecStatus::from_str_lossy(word), Some(expected), "{word}");
    }
}

#[test]
fn aliases_map_to_canonical_statuses() {
    for (word, expected) in [
        ("proposed", SpecStatus::Planned),
        ("draft", SpecStatus::Planned),
        ("accepted", SpecStatus::InProgress),
        ("implemented", SpecStatus::Complete),
        ("  Draft ", SpecStatus::Planned),
        ("IMPLEMENTED", SpecStatus::Complete),
    ] {
        assert_eq!(SpecStatus::from_str_lossy(word), Some(expected), "{word:?}");
    }
}

#[test]
fn alias_specs_parse_clean_with_canonical_status() {
    for (word, expected) in [
        ("proposed", SpecStatus::Planned),
        ("draft", SpecStatus::Planned),
        ("accepted", SpecStatus::InProgress),
        ("implemented", SpecStatus::Complete),
    ] {
        let (status, report) = parse(word, "");
        assert_eq!(status, Some(expected), "{word}");
        assert!(report.is_clean(), "{word}: {:?}", report.errors);
    }
}

#[test]
fn implemented_alias_gets_the_complete_spec_r14_check() {
    let lean = "\n```lean\ntheorem open_goal : 1 = 1 := by\n  sorry\n```\n";
    let (status, report) = parse("implemented", lean);
    assert_eq!(status, Some(SpecStatus::Complete));
    assert!(
        report.warnings.iter().any(|w| w.rule == "R14"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn unknown_status_words_are_still_r02_errors() {
    for word in ["wip", "done", "rejected", ""] {
        let (status, report) = parse(word, "");
        assert_eq!(status, None, "{word:?}");
        assert!(
            report.errors.iter().any(|e| e.rule == "R02"),
            "{word:?}: {:?}",
            report.errors
        );
    }
}
