//! Lot tracking data model.
//!
//! Lots map onto the GEM300 substrate model: a lot names a set of substrates and records a
//! timestamped state history with the CEIDs a host would expect to see on the wire.

use crate::error::FabError;
use crate::gem300::ObjectId;
use std::time::SystemTime;

/// Lot lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LotState {
    /// Created, not yet released to the floor.
    Created,
    /// Queued at the tool.
    InQueue,
    /// Being processed (active process job in parentheses is carried separately).
    InProcessing,
    /// On hold (operator or auto).
    OnHold,
    /// Completed all steps.
    Completed,
    /// Scrapped.
    Scrapped,
}

/// One history entry in a lot's life.
#[derive(Debug, Clone)]
pub struct LotEvent {
    /// State before the transition.
    pub from: LotState,
    /// State after the transition.
    pub to: LotState,
    /// CEID a host would expect for this transition (0 = internal only).
    pub ceid: u32,
    /// Free-text reason.
    pub reason: String,
    /// When it happened.
    pub at: SystemTime,
}

/// A tracked lot.
#[derive(Debug, Clone)]
pub struct Lot {
    /// Lot ID.
    pub id: ObjectId,
    /// Substrates in this lot.
    pub substrates: Vec<ObjectId>,
    /// Current state.
    pub state: LotState,
    /// Full history, oldest first.
    pub history: Vec<LotEvent>,
}

/// CEIDs used for lot lifecycle events (aligned with the E40/E90 examples in `gem300`).
pub mod lot_ceids {
    /// Lot arrived at the tool / queued.
    pub const QUEUED: u32 = 7001;
    /// Lot processing started.
    pub const STARTED: u32 = 7002;
    /// Lot processing completed.
    pub const COMPLETED: u32 = 7003;
    /// Lot placed on hold.
    pub const HELD: u32 = 7004;
    /// Lot scrapped.
    pub const SCRAPPED: u32 = 7005;
}

impl Lot {
    /// Creates a lot in the `Created` state.
    pub fn new(id: impl Into<ObjectId>, substrates: Vec<ObjectId>) -> Self {
        Lot { id: id.into(), substrates, state: LotState::Created, history: Vec::new() }
    }

    /// Applies a state transition, appending to history; errors on illegal transitions.
    pub fn transition(&mut self, to: LotState, reason: impl Into<String>) -> Result<u32, FabError> {
        let ceid = match (self.state, to) {
            (LotState::Created, LotState::InQueue) => lot_ceids::QUEUED,
            (LotState::InQueue, LotState::InProcessing) => lot_ceids::STARTED,
            (LotState::InProcessing, LotState::Completed) => lot_ceids::COMPLETED,
            (LotState::InQueue, LotState::OnHold) | (LotState::InProcessing, LotState::OnHold) => {
                lot_ceids::HELD
            }
            (LotState::OnHold, LotState::InQueue) => lot_ceids::QUEUED,
            (_, LotState::Scrapped) => lot_ceids::SCRAPPED,
            (from, to) => {
                return Err(FabError::Lot(format!(
                    "illegal lot transition {from:?} -> {to:?} for {}",
                    self.id
                )))
            }
        };
        self.history.push(LotEvent {
            from: self.state,
            to,
            ceid,
            reason: reason.into(),
            at: SystemTime::now(),
        });
        self.state = to;
        Ok(ceid)
    }

    /// Whether the lot is finished (completed or scrapped).
    pub fn is_finished(&self) -> bool {
        matches!(self.state, LotState::Completed | LotState::Scrapped)
    }
}

/// Registry of lots.
#[derive(Debug, Default)]
pub struct LotTracker {
    lots: Vec<Lot>,
}

impl LotTracker {
    /// Empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new lot; errors on duplicate ID.
    pub fn register(&mut self, lot: Lot) -> Result<(), FabError> {
        if self.lots.iter().any(|l| l.id == lot.id) {
            return Err(FabError::Lot(format!("duplicate lot {}", lot.id)));
        }
        self.lots.push(lot);
        Ok(())
    }

    /// Fetches a lot mutably.
    pub fn get_mut(&mut self, id: &str) -> Result<&mut Lot, FabError> {
        self.lots
            .iter_mut()
            .find(|l| l.id == id)
            .ok_or_else(|| FabError::Lot(format!("unknown lot {id}")))
    }

    /// Fetches a lot.
    pub fn get(&self, id: &str) -> Result<&Lot, FabError> {
        self.lots
            .iter()
            .find(|l| l.id == id)
            .ok_or_else(|| FabError::Lot(format!("unknown lot {id}")))
    }

    /// All lots.
    pub fn lots(&self) -> &[Lot] {
        &self.lots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lot_full_cycle_with_history() {
        let mut lot = Lot::new("L1", vec!["W01".into(), "W02".into()]);
        assert_eq!(lot.transition(LotState::InQueue, "released").unwrap(), lot_ceids::QUEUED);
        assert_eq!(
            lot.transition(LotState::InProcessing, "PJ1 started").unwrap(),
            lot_ceids::STARTED
        );
        assert_eq!(
            lot.transition(LotState::Completed, "all wafers done").unwrap(),
            lot_ceids::COMPLETED
        );
        assert!(lot.is_finished());
        assert_eq!(lot.history.len(), 3);
        assert_eq!(lot.history[0].from, LotState::Created);
        assert_eq!(lot.history[2].to, LotState::Completed);
    }

    #[test]
    fn hold_and_resume() {
        let mut lot = Lot::new("L2", vec!["W03".into()]);
        lot.transition(LotState::InQueue, "").unwrap();
        lot.transition(LotState::OnHold, "misprocess risk").unwrap();
        lot.transition(LotState::InQueue, "released after review").unwrap();
        lot.transition(LotState::InProcessing, "").unwrap();
        assert!(lot.transition(LotState::Created, "backwards").is_err());
    }

    #[test]
    fn tracker_rejects_duplicates() {
        let mut t = LotTracker::new();
        t.register(Lot::new("L1", vec![])).unwrap();
        assert!(t.register(Lot::new("L1", vec![])).is_err());
        assert!(t.get("L1").is_ok());
        assert!(t.get_mut("L2").is_err());
    }
}
