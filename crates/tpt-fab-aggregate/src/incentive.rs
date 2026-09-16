//! Incentive wiring (spec 4.7).
//!
//! The exchange is stated explicitly rather than assuming goodwill: contributing (even at
//! `AggregatedContribution` level) unlocks the real-time "shift-left" DRC/CAM turnaround from
//! RFC-001 Section 5, plus access to the yield-heatmap tooling in `tpt-ai`. Non-contributors
//! still get the full open-source tool; contributors get a better-calibrated one.
//!
//! The ledger here is the machine-checkable half of that promise: a *verified* contribution
//! (accepted after signature verification and anomaly screening) grants entitlements.

use crate::outcome::ConsentScope;
use std::collections::BTreeMap;

/// What a contributor unlocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Entitlement {
    /// RFC-001 §5 shift-left DRC/CAM turnaround.
    ShiftLeftDrc,
    /// `tpt-ai` yield-heatmap access.
    YieldHeatmap,
}

impl Entitlement {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Entitlement::ShiftLeftDrc => "shift-left-drc",
            Entitlement::YieldHeatmap => "yield-heatmap",
        }
    }
}

/// Entitlement ledger keyed by contributor.
#[derive(Debug, Default)]
pub struct EntitlementLedger {
    grants: BTreeMap<String, Vec<Entitlement>>,
}

impl EntitlementLedger {
    /// Empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a **verified** contribution and grants the contributor their entitlements.
    ///
    /// Any consent scope that transmits data at all (`PrivateBilateral` included — the fab
    /// did share with `tpt-solutions`, just privately) earns the incentive; `NoSharing`
    /// reports never left the fab's network, so they earn nothing.
    pub fn record_verified_contribution(&mut self, contributor_id: &str, consent: ConsentScope) {
        if !matches!(consent, ConsentScope::NoSharing) {
            let grants = self.grants.entry(contributor_id.to_string()).or_default();
            for e in [Entitlement::ShiftLeftDrc, Entitlement::YieldHeatmap] {
                if !grants.contains(&e) {
                    grants.push(e);
                }
            }
        }
    }

    /// Whether the contributor holds an entitlement.
    pub fn holds(&self, contributor_id: &str, entitlement: Entitlement) -> bool {
        self.grants.get(contributor_id).map(|g| g.contains(&entitlement)).unwrap_or(false)
    }

    /// All entitlements held by a contributor.
    pub fn entitlements(&self, contributor_id: &str) -> &[Entitlement] {
        self.grants.get(contributor_id).map(|g| g.as_slice()).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contribution_unlocks_both_entitlements() {
        let mut ledger = EntitlementLedger::new();
        ledger.record_verified_contribution("fab-a", ConsentScope::AggregatedContribution);
        assert!(ledger.holds("fab-a", Entitlement::ShiftLeftDrc));
        assert!(ledger.holds("fab-a", Entitlement::YieldHeatmap));
    }

    #[test]
    fn bilateral_sharing_also_counts() {
        let mut ledger = EntitlementLedger::new();
        ledger.record_verified_contribution("fab-b", ConsentScope::PrivateBilateral);
        assert_eq!(ledger.entitlements("fab-b").len(), 2);
    }

    #[test]
    fn no_sharing_earns_nothing() {
        let mut ledger = EntitlementLedger::new();
        ledger.record_verified_contribution("fab-c", ConsentScope::NoSharing);
        assert!(ledger.entitlements("fab-c").is_empty());
    }

    #[test]
    fn grants_are_idempotent() {
        let mut ledger = EntitlementLedger::new();
        ledger.record_verified_contribution("fab-d", ConsentScope::AggregatedContribution);
        ledger.record_verified_contribution("fab-d", ConsentScope::AggregatedContribution);
        assert_eq!(ledger.entitlements("fab-d").len(), 2);
    }
}
