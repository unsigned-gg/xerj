//! LeanSpec specification document models and validation reports.
//!
//! Pure data structures with zero I/O.

use crate::lean::{LeanModule, LeanTheorem};
use serde::{Deserialize, Serialize};

/// Recognized lifecycle statuses of an architectural specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpecStatus {
    /// Planned for future implementation.
    Planned,
    /// Currently being implemented.
    InProgress,
    /// Fully implemented and verified.
    Complete,
    /// Deprecated or superseded.
    Archived,
}

impl SpecStatus {
    /// Parse a status string into `SpecStatus`.
    ///
    /// Also accepts the alias words of other LeanSpec tools (SPEC-0001, "Status
    /// vocabulary"): `proposed` and `draft` as `Planned`, `accepted` as `InProgress`,
    /// and `implemented` as `Complete`.
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "planned" | "proposed" | "draft" => Some(Self::Planned),
            "in-progress" | "accepted" => Some(Self::InProgress),
            "complete" | "implemented" => Some(Self::Complete),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }

    /// Canonical string identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::InProgress => "in-progress",
            Self::Complete => "complete",
            Self::Archived => "archived",
        }
    }
}

/// Priority tier of an architectural specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SpecPriority {
    /// Low priority.
    Low,
    /// Medium priority.
    Medium,
    /// High priority.
    High,
    /// Critical architecture priority.
    Critical,
}

impl SpecPriority {
    /// Parse a priority string into `SpecPriority`.
    #[must_use]
    pub fn from_str_lossy(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

/// Parsed YAML frontmatter header of a LeanSpec document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecFrontmatter {
    /// Specification lifecycle status.
    pub status: SpecStatus,
    /// ISO 8601 creation date (YYYY-MM-DD).
    pub created: String,
    /// Categorical tags.
    pub tags: Vec<String>,
    /// Priority level.
    pub priority: Option<SpecPriority>,
    /// Dependency spec stems (e.g. `0001-template-architecture`).
    pub depends_on: Vec<String>,
    /// Architectural scope.
    pub scope: Option<String>,
}

/// H2 section in a LeanSpec document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecSection {
    /// Section heading text (e.g. `Overview`, `Requirements`, `Design`).
    pub title: String,
    /// 1-indexed starting line in the source file.
    pub start_line: usize,
    /// Section body text.
    pub body: String,
}

/// Full AST representation of a parsed LeanSpec document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDocument {
    /// File stem without `.md` extension (e.g. `0001-template-architecture`).
    pub stem: String,
    /// 4-digit specification number string.
    pub number: String,
    /// Kebab-case slug.
    pub slug: String,
    /// Top-level H1 heading text.
    pub h1: String,
    /// Parsed YAML frontmatter.
    pub frontmatter: SpecFrontmatter,
    /// Ordered H2 sections.
    pub sections: Vec<SpecSection>,
    /// Extracted formal theorem statements from embedded Lean 4 blocks.
    pub theorems: Vec<LeanTheorem>,
    /// Extracted parsed Lean 4 modules.
    pub lean_modules: Vec<LeanModule>,
    /// BLAKE3 digest of the complete specification source text.
    pub content_hash: String,
    /// Line count of the source file.
    pub line_count: usize,
}

/// Validation message resulting from checking LeanSpec conformance rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationMessage {
    /// File path or reference.
    pub file: String,
    /// Line number (if pinpointed).
    pub line: Option<usize>,
    /// Rule identifier (e.g. `R01`, `R04`, `R08`, `R14`).
    pub rule: String,
    /// Descriptive error or warning message.
    pub message: String,
}

/// Aggregated validation report for a LeanSpec repository.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Hard errors violating architectural rules.
    pub errors: Vec<ValidationMessage>,
    /// Advisory warnings (e.g. approaching line length thresholds or incomplete proofs).
    pub warnings: Vec<ValidationMessage>,
    /// Informational notices.
    pub notices: Vec<String>,
}

impl ValidationReport {
    /// Whether any hard errors occurred.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}
