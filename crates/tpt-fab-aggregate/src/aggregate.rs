//! Minimum-cohort gating and client-side aggregation (spec 4.2 / 4.4).
//!
//! An aggregate of one data point is not private, so **no aggregate statistic is published
//! until N ≥ 5 independent contributors** exist for that specific
//! process/node/technology bucket (resolved decision; configurable). For smaller cohorts that
//! still want to contribute, differential-privacy noise is applied as a second layer.
//!
//! Consent is enforced **structurally**: only reports marked `AggregatedContribution` ever
//! enter the public aggregation path; `PrivateBilateral` reports stay in the bilateral
//! calibration store (useful to `tpt-solutions` for solver calibration, never published);
//! `NoSharing` reports are recorded as received and used for nothing else.

use crate::error::AggregateError;
use crate::json::Json;
use crate::outcome::{ConsentScope, OutcomeReport};
use crate::privacy::{noisify, EpsilonPolicy, FieldSensitivity, SplitMix64};
use crate::schema::SchemaVersion;

/// Default minimum cohort size, adopted from the spec's own placeholder as the resolved
/// starting value (spec Section 6 decision).
pub const DEFAULT_MIN_COHORT: usize = 5;

/// Which bucket a contribution lands in: process/node/technology.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BucketKey {
    /// Process node, nm (e.g. 130 for sky130-style shuttles).
    pub node_nm: u32,
    /// Process identifier (e.g. `"sky130-mpw"`, `"2lm-cmos"`).
    pub process: String,
    /// Technology tag (`"pcb"`, `"euv"`, `"duv-multi-pattern"`, ...).
    pub tech: String,
}

/// One ingested contribution.
#[derive(Debug, Clone)]
pub struct Contribution {
    /// Anonymous contributor identifier (one entry per *independent* contributor for cohort
    /// counting — the same fab sending twice still counts once).
    pub contributor_id: String,
    /// Bucket this contribution belongs to.
    pub bucket: BucketKey,
    /// The report itself.
    pub report: OutcomeReport,
}

/// Aggregate statistic for one bucket.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregatePublication {
    /// Bucket published.
    pub bucket: BucketKey,
    /// Independent contributors behind this statistic (after gating).
    pub n_contributors: usize,
    /// Mean yield rate across contributions.
    pub mean_yield_rate: f64,
    /// Mean absolute geometry deviation (µm) across all geometry entries.
    pub mean_abs_geometry_deviation_um: f64,
    /// Mean electrical deviation ratio (measured/designed) across all electrical entries.
    pub mean_electrical_ratio: f64,
    /// Whether DP noise was applied (small-cohort fallback path).
    pub dp_protected: bool,
}

impl AggregatePublication {
    /// Serializes for publication (the exact JSON that would land in `tpt-fab-data`).
    pub fn to_json(&self) -> Json {
        crate::jobj! {
            "bucket" => Json::Obj(vec![
                ("node_nm".into(), Json::Num(self.bucket.node_nm as f64)),
                ("process".into(), Json::Str(self.bucket.process.clone())),
                ("tech".into(), Json::Str(self.bucket.tech.clone())),
            ]),
            "n_contributors" => Json::Num(self.n_contributors as f64),
            "mean_yield_rate" => Json::Num(self.mean_yield_rate),
            "mean_abs_geometry_deviation_um" => Json::Num(self.mean_abs_geometry_deviation_um),
            "mean_electrical_ratio" => Json::Num(self.mean_electrical_ratio),
            "dp_protected" => Json::Bool(self.dp_protected),
            "schema_version" => Json::Str(SchemaVersion::CURRENT.to_string()),
        }
    }
}

/// Client-side aggregation engine. Runs on the fab's own infrastructure; produces files and
/// stops. No egress path exists in this crate.
pub struct AggregationEngine {
    /// Minimum cohort size for ungated publication (default 5).
    pub min_cohort: usize,
    /// Epsilon policy for the DP fallback layer.
    pub epsilon_policy: EpsilonPolicy,
    contributions: Vec<Contribution>,
}

impl AggregationEngine {
    /// Engine with the default minimum cohort (N = 5).
    pub fn new() -> Self {
        AggregationEngine {
            min_cohort: DEFAULT_MIN_COHORT,
            epsilon_policy: EpsilonPolicy::defaults(),
            contributions: Vec::new(),
        }
    }

    /// Ingests one report. Consent is enforced here, structurally: `NoSharing` and
    /// `PrivateBilateral` reports are never added to the public contribution set.
    pub fn ingest_public_contribution(
        &mut self,
        contribution: Contribution,
    ) -> Result<IngestDecision, AggregateError> {
        if !contribution.report.consent.permits_public_aggregation() {
            return Ok(IngestDecision::ExcludedByConsent(contribution.report.consent));
        }
        self.contributions.push(contribution);
        Ok(IngestDecision::Accepted)
    }

    /// Direct bilateral-side ingestion (runs on `tpt-solutions` infrastructure): accepts any
    /// consent level into the appropriate store. This is the path a manually-delivered file
    /// takes after `OutcomeFileReader`.
    pub fn ingest_any(&mut self, contribution: Contribution) -> IngestDecision {
        match contribution.report.consent {
            ConsentScope::AggregatedContribution => {
                self.contributions.push(contribution);
                IngestDecision::Accepted
            }
            other => IngestDecision::ExcludedByConsent(other),
        }
    }

    /// Distinct contributors currently eligible for the bucket.
    pub fn cohort_size(&self, bucket: &BucketKey) -> usize {
        let mut ids: Vec<&str> = self
            .contributions
            .iter()
            .filter(|c| &c.bucket == bucket)
            .map(|c| c.contributor_id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    /// Attempts to publish the bucket.
    ///
    /// * `cohort >= min_cohort` → plain aggregate statistics.
    /// * `0 < cohort < min_cohort` → DP-noised statistics (second protection layer), flagged
    ///   `dp_protected`.
    /// * `cohort == 0` → `None` (nothing to publish).
    pub fn publish(&self, bucket: &BucketKey, seed: u64) -> Option<AggregatePublication> {
        let cohort = self.cohort_size(bucket);
        if cohort == 0 {
            return None;
        }
        let eligible: Vec<&Contribution> = self
            .contributions
            .iter()
            .filter(|c| &c.bucket == bucket && c.report.consent.permits_public_aggregation())
            .collect();

        let n = eligible.len() as f64;
        let mean_yield =
            eligible.iter().map(|c| c.report.yield_outcome.yield_rate()).sum::<f64>() / n;

        let mut geom_sum = 0.0;
        let mut geom_count = 0.0;
        let mut elec_sum = 0.0;
        let mut elec_count = 0.0;
        for c in &eligible {
            for g in &c.report.measured_geometry {
                if g.designed_um != 0.0 {
                    geom_sum += (g.measured_um - g.designed_um).abs();
                    geom_count += 1.0;
                }
            }
            for e in &c.report.electrical_test {
                if e.designed != 0.0 {
                    elec_sum += e.measured / e.designed;
                    elec_count += 1.0;
                }
            }
        }
        let mean_geom = if geom_count > 0.0 { geom_sum / geom_count } else { 0.0 };
        let mean_elec = if elec_count > 0.0 { elec_sum / elec_count } else { 0.0 };

        let dp = cohort < self.min_cohort;
        if dp {
            let mut rng = SplitMix64::new(seed);
            Some(AggregatePublication {
                bucket: bucket.clone(),
                n_contributors: cohort,
                mean_yield_rate: noisify(
                    &mut rng,
                    mean_yield,
                    FieldSensitivity::Yield,
                    &self.epsilon_policy,
                )
                .clamp(0.0, 1.0),
                mean_abs_geometry_deviation_um: noisify(
                    &mut rng,
                    mean_geom,
                    FieldSensitivity::Geometry,
                    &self.epsilon_policy,
                )
                .max(0.0),
                mean_electrical_ratio: noisify(
                    &mut rng,
                    mean_elec,
                    FieldSensitivity::Geometry,
                    &self.epsilon_policy,
                ),
                dp_protected: true,
            })
        } else {
            Some(AggregatePublication {
                bucket: bucket.clone(),
                n_contributors: cohort,
                mean_yield_rate: mean_yield,
                mean_abs_geometry_deviation_um: mean_geom,
                mean_electrical_ratio: mean_elec,
                dp_protected: false,
            })
        }
    }

    /// All ingested public contributions (for the release builder).
    pub fn contributions(&self) -> &[Contribution] {
        &self.contributions
    }
}

impl Default for AggregationEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of an ingestion attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestDecision {
    /// Added to the public contribution set.
    Accepted,
    /// Excluded because its consent scope does not permit public aggregation.
    ExcludedByConsent(ConsentScope),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{
        ElectricalMeasurement, GeometryDeviation, ManufacturingTrack, SemanticId, WaferTech,
        YieldSummary,
    };
    use crate::schema::SchemaVersion;

    fn bucket() -> BucketKey {
        BucketKey { node_nm: 130, process: "sky130-mpw".into(), tech: "duv-multi-pattern".into() }
    }

    fn report(yield_rate: f64) -> OutcomeReport {
        OutcomeReport {
            payload_manifest_id: crate::outcome::new_manifest_id(1),
            track: ManufacturingTrack::WaferLitho(WaferTech::DuvMultiPattern),
            measured_geometry: vec![GeometryDeviation {
                feature_id: SemanticId::new("fp-x").unwrap(),
                designed_um: 10.0,
                measured_um: 10.1,
            }],
            electrical_test: vec![ElectricalMeasurement {
                link_id: SemanticId::new("FL-1").unwrap(),
                quantity: "impedance".into(),
                designed: 50.0,
                measured: 51.0,
                unit: "ohm".into(),
            }],
            yield_outcome: YieldSummary {
                units_started: 100,
                units_good: (yield_rate * 100.0) as u64,
                dominant_failure_mode: None,
            },
            process_notes: vec![],
            consent: ConsentScope::AggregatedContribution,
            schema_version: SchemaVersion::CURRENT,
            signature: None,
        }
    }

    fn contrib(id: &str, y: f64) -> Contribution {
        Contribution { contributor_id: id.into(), bucket: bucket(), report: report(y) }
    }

    #[test]
    fn consent_enforced_structurally() {
        let mut engine = AggregationEngine::new();
        let mut no_share = report(0.9);
        no_share.consent = ConsentScope::NoSharing;
        let d1 = engine
            .ingest_public_contribution(Contribution {
                contributor_id: "fab-a".into(),
                bucket: bucket(),
                report: no_share.clone(),
            })
            .unwrap();
        assert_eq!(d1, IngestDecision::ExcludedByConsent(ConsentScope::NoSharing));

        let mut bilateral = report(0.9);
        bilateral.consent = ConsentScope::PrivateBilateral;
        let d2 = engine
            .ingest_public_contribution(Contribution {
                contributor_id: "fab-a".into(),
                bucket: bucket(),
                report: bilateral,
            })
            .unwrap();
        assert_eq!(d2, IngestDecision::ExcludedByConsent(ConsentScope::PrivateBilateral));

        // Nothing entered the public path.
        assert_eq!(engine.cohort_size(&bucket()), 0);
        assert!(engine.publish(&bucket(), 1).is_none());

        // The bilateral path accepts it but still keeps it out of the public bucket.
        assert_eq!(
            engine.ingest_any(Contribution {
                contributor_id: "fab-a".into(),
                bucket: bucket(),
                report: no_share
            }),
            IngestDecision::ExcludedByConsent(ConsentScope::NoSharing)
        );
    }

    #[test]
    fn minimum_cohort_gates_publication() {
        let mut engine = AggregationEngine::new();
        for i in 0..4 {
            engine.ingest_public_contribution(contrib(&format!("fab-{i}"), 0.9)).unwrap();
        }
        assert_eq!(engine.cohort_size(&bucket()), 4);
        // Below N=5: DP-protected only, and flagged as such.
        let small = engine.publish(&bucket(), 42).unwrap();
        assert!(small.dp_protected);

        engine.ingest_public_contribution(contrib("fab-4", 0.9)).unwrap();
        let full = engine.publish(&bucket(), 42).unwrap();
        assert!(!full.dp_protected);
        assert_eq!(full.n_contributors, 5);
        assert!((full.mean_yield_rate - 0.9).abs() < 1e-9);
    }

    #[test]
    fn duplicate_contributor_counts_once() {
        let mut engine = AggregationEngine::new();
        for _ in 0..6 {
            engine.ingest_public_contribution(contrib("fab-a", 0.8)).unwrap();
        }
        assert_eq!(engine.cohort_size(&bucket()), 1);
        assert!(engine.publish(&bucket(), 1).unwrap().dp_protected);
    }

    #[test]
    fn dp_fallback_adds_noise_but_stays_bounded() {
        let mut engine = AggregationEngine::new();
        for i in 0..3 {
            engine.ingest_public_contribution(contrib(&format!("fab-{i}"), 0.95)).unwrap();
        }
        let pub1 = engine.publish(&bucket(), 7).unwrap();
        assert!(pub1.dp_protected);
        assert!((0.0..=1.0).contains(&pub1.mean_yield_rate));
        // Reproducible for the same seed (auditable).
        let pub2 = engine.publish(&bucket(), 7).unwrap();
        assert_eq!(pub1, pub2);
    }

    #[test]
    fn publication_json_is_stable() {
        let mut engine = AggregationEngine::new();
        for i in 0..5 {
            engine.ingest_public_contribution(contrib(&format!("fab-{i}"), 0.9)).unwrap();
        }
        let p = engine.publish(&bucket(), 1).unwrap();
        let text = p.to_json().serialize();
        assert!(text.contains("\"n_contributors\":5"));
        assert!(text.contains("\"dp_protected\":false"));
        assert!(text.contains("\"schema_version\":\"0.2\""));
    }
}
