//! Retained-run custody state machine ported from `sbx-core::settle`.
//!
//! A sandboxed run that exits leaves a copy-on-write world on disk until a
//! settlement decision is taken. [`RetainedRun`] makes invalid transitions
//! unrepresentable at the type level by consuming `self` on every terminal
//! operation (`select`, `release`, `discard`, `apply`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Opaque identifier of a retained sandbox world.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorldId(pub String);

impl WorldId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorldId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Terminal disposition of a [`RetainedRun`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Held as the winning candidate of a selection batch; eligible for a later
    /// `apply` or `release`, not destroyed yet.
    Selected,
    /// Explicitly released by a caller after inspection; world is destroyed.
    Released,
    /// Promoted into the base workspace (`apply`) and then destroyed.
    Applied,
    /// Destroyed as a non-winner of a selection batch (`select` loser) or direct
    /// discard.
    Discarded,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettleError {
    #[error("selection batch is empty; at least one RetainedRun is required")]
    EmptyBatch,
    #[error("winner `{0}` is not a member of the retained run batch")]
    UnknownWinner(WorldId),
    #[error("duplicate world id `{0}` in retained run batch")]
    DuplicateWorld(WorldId),
}

/// An unsettled, retained sandbox run holding custody of a live world.
///
/// Intentionally **not** `Clone` or `Copy`: every settlement operation consumes
/// `self` by value and returns a [`Settlement`] receipt, so the compiler rejects
/// any attempt to settle the same world twice.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a RetainedRun holds a live world on disk and must be settled via select, apply, release, or discard"]
pub struct RetainedRun {
    world_id: WorldId,
}

/// Immutable receipt produced when a [`RetainedRun`] is settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settlement {
    pub world_id: WorldId,
    pub outcome: Outcome,
    /// True iff the backing copy-on-write world must be destroyed as part of
    /// committing this settlement.
    pub destroy_world: bool,
}

impl RetainedRun {
    /// Enter the `Retained` state for `world_id`.
    pub fn new(world_id: impl Into<WorldId>) -> Self {
        Self {
            world_id: world_id.into(),
        }
    }

    pub fn world_id(&self) -> &WorldId {
        &self.world_id
    }

    /// Transition `Retained -> Released` (world destroyed).
    pub fn release(self) -> Settlement {
        Settlement {
            world_id: self.world_id,
            outcome: Outcome::Released,
            destroy_world: true,
        }
    }

    /// Transition `Retained -> Discarded` (world destroyed).
    pub fn discard(self) -> Settlement {
        Settlement {
            world_id: self.world_id,
            outcome: Outcome::Discarded,
            destroy_world: true,
        }
    }

    /// Transition `Retained -> Applied` (changes promoted, then world destroyed).
    pub fn apply(self) -> Settlement {
        Settlement {
            world_id: self.world_id,
            outcome: Outcome::Applied,
            destroy_world: true,
        }
    }

    /// Settle a batch of retained runs by selecting `winner`: the winning run
    /// transitions to [`Outcome::Selected`] (kept alive for inspection/apply)
    /// and every other run in `batch` transitions to [`Outcome::Discarded`]
    /// (`destroy_world = true`).
    pub fn select(
        batch: Vec<RetainedRun>,
        winner: &WorldId,
    ) -> Result<(RetainedRun, Vec<Settlement>), SettleError> {
        if batch.is_empty() {
            return Err(SettleError::EmptyBatch);
        }

        for (i, a) in batch.iter().enumerate() {
            for b in &batch[i + 1..] {
                if a.world_id == b.world_id {
                    return Err(SettleError::DuplicateWorld(a.world_id.clone()));
                }
            }
        }

        if !batch.iter().any(|r| &r.world_id == winner) {
            return Err(SettleError::UnknownWinner(winner.clone()));
        }

        let mut kept: Option<RetainedRun> = None;
        let mut settlements = Vec::with_capacity(batch.len());

        for run in batch {
            if &run.world_id == winner {
                settlements.push(Settlement {
                    world_id: run.world_id.clone(),
                    outcome: Outcome::Selected,
                    destroy_world: false,
                });
                kept = Some(run);
            } else {
                settlements.push(run.discard());
            }
        }

        Ok((kept.expect("winner membership verified above"), settlements))
    }
}

impl From<&str> for WorldId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for WorldId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_run_transitions_produce_expected_outcomes() {
        let s_rel = RetainedRun::new("w-rel").release();
        assert_eq!(s_rel.outcome, Outcome::Released);
        assert!(s_rel.destroy_world);

        let s_dis = RetainedRun::new("w-dis").discard();
        assert_eq!(s_dis.outcome, Outcome::Discarded);
        assert!(s_dis.destroy_world);

        let s_app = RetainedRun::new("w-app").apply();
        assert_eq!(s_app.outcome, Outcome::Applied);
        assert!(s_app.destroy_world);
    }

    #[test]
    fn select_keeps_winner_alive_and_discards_losers() {
        let batch = vec![
            RetainedRun::new("w-1"),
            RetainedRun::new("w-2"),
            RetainedRun::new("w-3"),
        ];
        let winner_id = WorldId::new("w-2");

        let (winner_run, settlements) = RetainedRun::select(batch, &winner_id).unwrap();
        assert_eq!(winner_run.world_id(), &winner_id);
        assert_eq!(settlements.len(), 3);

        let final_settlement = winner_run.apply();
        assert_eq!(final_settlement.outcome, Outcome::Applied);
        assert!(final_settlement.destroy_world);
    }
}
