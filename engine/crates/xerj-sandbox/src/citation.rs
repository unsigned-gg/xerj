//! Output citation record emitted when a sandbox run settles, ported from `sbx-core::citation`.

use serde::{Deserialize, Serialize};

use crate::settle::{Outcome, Settlement};

/// Backing version-control / copy-on-write core that produced a sandbox run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VcsCore {
    /// Filesystem copy-on-write / reflink world store.
    Reflink,
    /// Git worktree / commit graph backend.
    Git,
    /// Jujutsu (`jj`) change-graph backend.
    Jj,
}

/// Immutable citation linking a settled sandbox output back to the run, world,
/// VCS core, base revision, and diff hash that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputCitation {
    /// Identifier of the sandbox run (`run_id`).
    pub run_id: String,
    /// Identifier of the copy-on-write world (`world_id`).
    pub world_id: String,
    /// Backing VCS / CoW core.
    pub vcs_core: VcsCore,
    /// Base workspace revision or snapshot digest the world forked from.
    pub base_rev: String,
    /// Content digest (e.g. BLAKE3 / SHA-256 hex) of the world's filesystem diff
    /// at settlement time.
    pub diff_hash: String,
    /// Terminal disposition taken when the run was settled.
    pub outcome: Outcome,
}

impl OutputCitation {
    /// Construct a citation directly from a [`Settlement`] receipt and the run's
    /// provenance metadata.
    pub fn from_settlement(
        run_id: impl Into<String>,
        settlement: &Settlement,
        vcs_core: VcsCore,
        base_rev: impl Into<String>,
        diff_hash: impl Into<String>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            world_id: settlement.world_id.as_str().to_string(),
            vcs_core,
            base_rev: base_rev.into(),
            diff_hash: diff_hash.into(),
            outcome: settlement.outcome,
        }
    }

    /// Deterministic compact reference string (`<vcs_core>:<world_id>@<base_rev>#<diff_hash>`).
    pub fn compact_ref(&self) -> String {
        let core = match self.vcs_core {
            VcsCore::Reflink => "reflink",
            VcsCore::Git => "git",
            VcsCore::Jj => "jj",
        };
        format!(
            "{core}:{}@{}#{}",
            self.world_id, self.base_rev, self.diff_hash
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settle::RetainedRun;

    #[test]
    fn builds_citation_from_settlement_and_roundtrips_json() {
        let settlement = RetainedRun::new("world-7f3a").apply();
        let citation = OutputCitation::from_settlement(
            "run-0042",
            &settlement,
            VcsCore::Reflink,
            "main@9f8bc12",
            "blake3:0123456789abcdef",
        );

        assert_eq!(citation.outcome, Outcome::Applied);
        assert_eq!(
            citation.compact_ref(),
            "reflink:world-7f3a@main@9f8bc12#blake3:0123456789abcdef"
        );

        let json = serde_json::to_string(&citation).unwrap();
        let decoded: OutputCitation = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, citation);
    }
}
