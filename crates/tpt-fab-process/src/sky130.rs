//! efabless/Skywater-style sky130 MPW shuttle-run ingestion (spec 2C / 4.4).
//!
//! MPW submissions from many independent designers are a natural, already-anonymous-by-default
//! cohort — worth building the ingestion path for from day one. Shuttle results arrive as
//! JSON (measured geometry, electrical tests, yield) and are converted into
//! [`OutcomeReport`]s that flow through the full `tpt-fab-aggregate` pipeline: **the
//! minimum-cohort gate (N ≥ 5) and differential-privacy fallback apply to this path from day
//! one**, exactly as they do for direct fab contributions.

use tpt_fab_aggregate::aggregate::{BucketKey, Contribution, IngestDecision};
use tpt_fab_aggregate::json::Json;
use tpt_fab_aggregate::outcome::{
    ConsentScope, ElectricalMeasurement, GeometryDeviation, ManufacturingTrack, OutcomeReport,
    SemanticId, WaferTech, YieldSummary,
};
use tpt_fab_aggregate::AggregateError;
use tpt_fab_aggregate::AggregationEngine;
use tpt_fab_aggregate::SchemaVersion;

/// A parsed MPW shuttle result.
#[derive(Debug, Clone, PartialEq)]
pub struct MpwShuttleResult {
    /// Shuttle run identifier (e.g. `"mpw-7"`).
    pub run_id: String,
    /// Designer/project identifier (anonymized upstream; used as the cohort contributor key).
    pub project_id: String,
    /// As-built vs. as-designed geometry entries.
    pub geometry: Vec<GeometryDeviation>,
    /// Electrical test entries.
    pub electrical: Vec<ElectricalMeasurement>,
    /// Yield across the submitted dies.
    pub yield_summary: YieldSummary,
    /// Consent selected by the submitting designer (defaults to `AggregatedContribution`).
    pub consent: ConsentScope,
}

/// Parses a shuttle-result JSON document.
///
/// Expected shape (all measured values optional, zero-entry arrays fine):
/// ```json
/// {
///   "run_id": "mpw-7",
///   "project_id": "designer-42",
///   "geometry": [{"id": "via-chain-1", "designed_um": 8.0, "measured_um": 8.06}],
///   "electrical": [{"id": "ringosc-1", "quantity": "freq_mhz", "designed": 50, "measured": 47}],
///   "yield": {"started": 3, "good": 2},
///   "consent": "aggregated-contribution"
/// }
/// ```
pub fn parse_shuttle_result(text: &str) -> Result<MpwShuttleResult, AggregateError> {
    let v = Json::parse(text)?;
    let need = |key: &str| -> Result<&Json, AggregateError> {
        v.get(key).ok_or_else(|| AggregateError::Schema(format!("shuttle result missing '{key}'")))
    };
    let run_id = need("run_id")?
        .as_str()
        .ok_or_else(|| AggregateError::Schema("run_id not a string".into()))?
        .to_string();
    let project_id = need("project_id")?
        .as_str()
        .ok_or_else(|| AggregateError::Schema("project_id not a string".into()))?
        .to_string();
    let mut geometry = Vec::new();
    if let Some(items) = need("geometry")?.as_arr() {
        for g in items {
            geometry.push(GeometryDeviation {
                feature_id: SemanticId::new(
                    g.get("id").and_then(Json::as_str).ok_or_else(|| {
                        AggregateError::Schema("geometry entry missing id".into())
                    })?,
                )?,
                designed_um: g.get("designed_um").and_then(Json::as_f64).unwrap_or(0.0),
                measured_um: g.get("measured_um").and_then(Json::as_f64).unwrap_or(0.0),
            });
        }
    }
    let mut electrical = Vec::new();
    if let Some(items) = need("electrical")?.as_arr() {
        for e in items {
            electrical.push(ElectricalMeasurement {
                link_id: SemanticId::new(e.get("id").and_then(Json::as_str).ok_or_else(|| {
                    AggregateError::Schema("electrical entry missing id".into())
                })?)?,
                quantity: e.get("quantity").and_then(Json::as_str).unwrap_or_default().to_string(),
                designed: e.get("designed").and_then(Json::as_f64).unwrap_or(0.0),
                measured: e.get("measured").and_then(Json::as_f64).unwrap_or(0.0),
                unit: e.get("unit").and_then(Json::as_str).unwrap_or_default().to_string(),
            });
        }
    }
    let y = need("yield")?;
    let yield_summary = YieldSummary {
        units_started: y.get("started").and_then(Json::as_f64).unwrap_or(0.0) as u64,
        units_good: y.get("good").and_then(Json::as_f64).unwrap_or(0.0) as u64,
        dominant_failure_mode: y.get("failure_mode").and_then(Json::as_str).map(str::to_string),
    };
    let consent = match v.get("consent").and_then(Json::as_str) {
        Some(s) => ConsentScope::parse(s)?,
        None => ConsentScope::AggregatedContribution, // MPW default: anonymous cohort data
    };
    Ok(MpwShuttleResult { run_id, project_id, geometry, electrical, yield_summary, consent })
}

impl MpwShuttleResult {
    /// Converts the shuttle result into an outcome report keyed to the wafer track.
    pub fn to_outcome_report(&self) -> Result<OutcomeReport, AggregateError> {
        Ok(OutcomeReport {
            payload_manifest_id: tpt_fab_aggregate::outcome::new_manifest_id(
                // Deterministic per project+run so re-ingesting the same result is idempotent.
                fnv1a(format!("{}:{}", self.run_id, self.project_id).as_bytes()),
            ),
            track: ManufacturingTrack::WaferLitho(WaferTech::DuvMultiPattern),
            measured_geometry: self.geometry.clone(),
            electrical_test: self.electrical.clone(),
            yield_outcome: self.yield_summary.clone(),
            process_notes: Vec::new(),
            consent: self.consent,
            schema_version: SchemaVersion::CURRENT,
            signature: None, // shuttle results are authenticated by the shuttle program
        })
    }

    /// The bucket this shuttle run reports into.
    pub fn bucket(&self) -> BucketKey {
        BucketKey {
            node_nm: 130,
            process: format!("sky130-{}", self.run_id),
            tech: "duv-multi-pattern".into(),
        }
    }
}

/// Ingests a shuttle result into the aggregation engine.
///
/// Protection is inherited from the engine: consent enforcement, the N ≥ 5 minimum-cohort
/// gate, and DP fallback all apply before anything is published.
pub fn ingest_shuttle_result(
    engine: &mut AggregationEngine,
    result: &MpwShuttleResult,
) -> Result<IngestDecision, AggregateError> {
    let report = result.to_outcome_report()?;
    tpt_fab_aggregate::anomaly::validate_report_basics(&report)?;
    engine.ingest_public_contribution(Contribution {
        contributor_id: result.project_id.clone(),
        bucket: result.bucket(),
        report,
    })
}

/// FNV-1a hash (for deterministic manifest IDs; not a security primitive).
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "run_id": "mpw-7",
        "project_id": "designer-42",
        "geometry": [{"id": "via-chain-1", "designed_um": 8.0, "measured_um": 8.06}],
        "electrical": [{"id": "ringosc-1", "quantity": "freq_mhz", "designed": 50.0, "measured": 47.5, "unit": "mhz"}],
        "yield": {"started": 3, "good": 2},
        "consent": "aggregated-contribution"
    }"#;

    #[test]
    fn parses_shuttle_result() {
        let r = parse_shuttle_result(SAMPLE).unwrap();
        assert_eq!(r.run_id, "mpw-7");
        assert_eq!(r.geometry.len(), 1);
        assert_eq!(r.electrical.len(), 1);
        assert_eq!(r.yield_summary.units_good, 2);
        assert_eq!(r.consent, ConsentScope::AggregatedContribution);
    }

    #[test]
    fn report_conversion_is_keyed_to_semantic_ids() {
        let r = parse_shuttle_result(SAMPLE).unwrap();
        let report = r.to_outcome_report().unwrap();
        assert_eq!(report.track, ManufacturingTrack::WaferLitho(WaferTech::DuvMultiPattern));
        assert_eq!(report.measured_geometry[0].feature_id.0, "via-chain-1");
        assert_eq!(report.electrical_test[0].link_id.0, "ringosc-1");
        // Deterministic manifest id (idempotent re-ingestion).
        assert_eq!(report.payload_manifest_id, r.to_outcome_report().unwrap().payload_manifest_id);
    }

    #[test]
    fn malformed_shuttle_results_are_rejected() {
        assert!(parse_shuttle_result("{}").is_err());
        assert!(parse_shuttle_result(r#"{"run_id": "x"}"#).is_err());
        assert!(parse_shuttle_result("not json").is_err());
    }

    #[test]
    fn shuttle_results_flow_through_cohort_gated_aggregation() {
        let mut engine = AggregationEngine::new();
        // Four designers submit to the same shuttle run: below N=5, DP-protected only.
        for i in 0..4 {
            let text = SAMPLE.replace("designer-42", &format!("designer-{i}"));
            let r = parse_shuttle_result(&text).unwrap();
            assert_eq!(ingest_shuttle_result(&mut engine, &r).unwrap(), IngestDecision::Accepted);
        }
        let bucket = BucketKey {
            node_nm: 130,
            process: "sky130-mpw-7".into(),
            tech: "duv-multi-pattern".into(),
        };
        let small = engine.publish(&bucket, 1).unwrap();
        assert!(small.dp_protected);

        // The fifth designer crosses the cohort threshold: plain aggregate.
        let r = parse_shuttle_result(SAMPLE).unwrap();
        ingest_shuttle_result(&mut engine, &r).unwrap();
        let full = engine.publish(&bucket, 1).unwrap();
        assert!(!full.dp_protected);
        assert_eq!(full.n_contributors, 5);
    }

    #[test]
    fn designer_cannot_cheat_the_cohort_by_submitting_twice() {
        let mut engine = AggregationEngine::new();
        for _ in 0..6 {
            let r = parse_shuttle_result(SAMPLE).unwrap();
            ingest_shuttle_result(&mut engine, &r).unwrap();
        }
        let bucket = BucketKey {
            node_nm: 130,
            process: "sky130-mpw-7".into(),
            tech: "duv-multi-pattern".into(),
        };
        assert_eq!(engine.cohort_size(&bucket), 1, "same project id counts once");
        assert!(engine.publish(&bucket, 1).unwrap().dp_protected);
    }
}
