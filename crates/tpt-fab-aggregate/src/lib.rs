//! # tpt-fab-aggregate
//!
//! The Manufacturing Outcome file exchange: schema, consent, client-side aggregation, and the
//! minimum-cohort + differential-privacy protections around the shared fine-tuning signal.
//!
//! ## This crate has no egress path — by construction, not by configuration
//!
//! This is the load-bearing design constraint of the whole loop (spec 4.8): `tpt-fab` never
//! opens a network connection to send outcome data anywhere, on any schedule, under any
//! configuration. [`OutcomeFileWriter`](file::OutcomeFileWriter) writes to a local path and
//! returns. There is no "offline mode" to enable, because there is no online mode to begin
//! with.
//!
//! The proof is mechanical and requires no trust in this document:
//!
//! * `Cargo.toml` of this crate declares **zero dependencies** — nothing network-capable can
//!   even be linked, and there is nothing to audit beyond this one directory.
//! * The only I/O primitives in the crate are `std::fs::write` / `std::fs::read_to_string`.
//! * `grep` the sources for `TcpStream`, `UdpSocket`, `process::Command` — there are no send
//!   paths, background tasks, or schedulers at all.
//!
//! Because the aggregation/anonymization step runs *on the fab's own infrastructure* using
//! this published code, the code is the trust mechanism: "verify it yourself, and verify
//! there's no send step" replaces "trust our privacy policy."
//!
//! ## What lives here
//!
//! * [`outcome`] — the `OutcomeReport` schema shared by the PCB track (RFC-001,
//!   `tpt-silicon-cam`) and the wafer track (`tpt-fab`), keyed to semantic IDs, never raw
//!   coordinates.
//! * [`schema`] — `SchemaVersion` with migration-or-reject semantics: unrecognized minor
//!   versions are refused, never silently misinterpreted.
//! * [`aggregate`] — `ConsentScope` travels inside the report and the pipeline enforces it
//!   structurally; minimum cohort N ≥ 5 gates publication; smaller cohorts fall back to
//!   per-field-sensitivity differential privacy ([`privacy`]).
//! * [`anomaly`] — anti-poisoning screening before any report feeds a model.
//! * [`signature`] — HMAC-SHA256 origin signatures ([`hash`], implemented in-crate).
//! * [`incentive`] — contribution unlocks shift-left DRC/CAM + yield-heatmap access.
//! * [`dataset`] — the attribution-only release artifact for the dedicated `tpt-fab-data`
//!   repo.

#![forbid(unsafe_code)]

pub mod aggregate;
pub mod anomaly;
pub mod dataset;
pub mod error;
pub mod file;
pub mod hash;
pub mod incentive;
pub mod json;
pub mod outcome;
pub mod privacy;
pub mod schema;
pub mod signature;

pub use aggregate::{
    AggregatePublication, AggregationEngine, BucketKey, Contribution, DEFAULT_MIN_COHORT,
};
pub use error::AggregateError;
pub use file::{JsonOutcomeFile, OutcomeFileReader, OutcomeFileWriter};
pub use incentive::{Entitlement, EntitlementLedger};
pub use outcome::{ConsentScope, ManufacturingTrack, OutcomeReport, WaferTech};
pub use schema::SchemaVersion;

#[cfg(test)]
mod tests {
    use super::*;
    use aggregate::IngestDecision;
    use anomaly::{validate_report_basics, AnomalyDetector};
    use json::Json;
    use outcome::{
        ConsentScope, ElectricalMeasurement, GeometryDeviation, ManufacturingTrack, SemanticId,
        Signature, WaferTech, YieldSummary,
    };
    use privacy::SplitMix64;
    use std::path::PathBuf;
    use std::rc::Rc;

    const FAB_KEY: &[u8] = b"fab-alpha-hmac-key";

    /// A representative wafer-track outcome report.
    fn report(seed: u64, consent: ConsentScope, yield_rate: f64) -> OutcomeReport {
        OutcomeReport {
            payload_manifest_id: outcome::new_manifest_id(seed),
            track: ManufacturingTrack::WaferLitho(WaferTech::DuvMultiPattern),
            measured_geometry: vec![GeometryDeviation {
                feature_id: SemanticId::new("via-chain-1").unwrap(),
                designed_um: 8.0,
                measured_um: 8.06,
            }],
            electrical_test: vec![ElectricalMeasurement {
                link_id: SemanticId::new("FL-clk-1").unwrap(),
                quantity: "impedance".into(),
                designed: 50.0,
                measured: 50.75,
                unit: "ohm".into(),
            }],
            yield_outcome: YieldSummary {
                units_started: 1000,
                units_good: (yield_rate * 1000.0) as u64,
                dominant_failure_mode: Some("metal open".into()),
            },
            process_notes: vec![],
            consent,
            schema_version: SchemaVersion::CURRENT,
            signature: None,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tpt-fab-aggregate-e2e-{name}.json"));
        p
    }

    /// The full exchange, end to end: the fab writes a signed file locally; the receiver
    /// ingests the manually-delivered file; the report is screened, aggregated, and released
    /// as an attribution-only dataset artifact.
    #[test]
    fn outcome_file_round_trips_through_pipeline() {
        // --- Fab side (runs on fab infrastructure) ---
        let mut fab_report = report(11, ConsentScope::AggregatedContribution, 0.92);
        signature::sign_report(&mut fab_report, "fab-alpha", FAB_KEY);
        let fab_file = tmp("fab");
        JsonOutcomeFile.write_outcome_file(&fab_report, &fab_file).unwrap();
        // The fab has seen exactly what the file contains; sending it is their decision.

        // --- Receiver side (runs on tpt-solutions infrastructure) ---
        let keys = |kid: &str| (kid == "fab-alpha").then(|| FAB_KEY.to_vec());
        let reader =
            JsonOutcomeFile::reader(signature::UnsignedPolicy::RequireSignature, Rc::new(keys));
        let received = reader.read_outcome_file(&fab_file).unwrap();
        assert_eq!(received, fab_report);
        validate_report_basics(&received).unwrap();

        // Screen before feeding any model.
        let mut detector = AnomalyDetector::new();
        let screen = detector.screen(&received);
        assert!(!screen.flagged);

        // Ingest and aggregate.
        let mut engine = AggregationEngine::new();
        let bucket = BucketKey {
            node_nm: 130,
            process: "sky130-mpw".into(),
            tech: "duv-multi-pattern".into(),
        };
        for i in 0..4 {
            let mut r = report(100 + i, ConsentScope::AggregatedContribution, 0.9);
            r.signature = Some(Signature {
                algorithm: "hmac-sha256".into(),
                key_id: format!("k{i}"),
                mac_hex: "00".repeat(32),
            });
            assert_eq!(
                engine
                    .ingest_public_contribution(Contribution {
                        contributor_id: format!("fab-{i}"),
                        bucket: bucket.clone(),
                        report: r
                    })
                    .unwrap(),
                IngestDecision::Accepted
            );
        }
        assert_eq!(
            engine
                .ingest_public_contribution(Contribution {
                    contributor_id: "fab-alpha".into(),
                    bucket: bucket.clone(),
                    report: received,
                })
                .unwrap(),
            IngestDecision::Accepted
        );
        assert_eq!(engine.cohort_size(&bucket), 5, "four test fabs plus the real one");
        let publication = engine.publish(&bucket, 1).unwrap();
        assert!(!publication.dp_protected, "cohort of 5 meets the minimum");

        // Release to the tpt-fab-data repo as an attribution-only artifact.
        let release = dataset::DatasetRelease {
            title: "sky130-mpw outcomes, 2026-Q3".into(),
            publications: vec![publication],
            attributions: vec![dataset::Attribution {
                name: "Fab Alpha (pilot)".into(),
                buckets: vec![bucket],
            }],
        };
        let release_file = tmp("release");
        release.write_to(&release_file).unwrap();
        let text = std::fs::read_to_string(&release_file).unwrap();
        assert!(!text.contains("payload_manifest_id"), "no raw data in the release");
        assert!(text.contains("Fab Alpha (pilot)"));

        std::fs::remove_file(&fab_file).ok();
        std::fs::remove_file(&release_file).ok();
    }

    /// A `NoSharing` report can be written to a file (it is the fab's own data), but the
    /// pipeline structurally refuses to publish anything from it.
    #[test]
    fn no_sharing_report_cannot_reach_the_public_path() {
        let mut engine = AggregationEngine::new();
        let r = report(22, ConsentScope::NoSharing, 0.9);
        let decision = engine
            .ingest_public_contribution(Contribution {
                contributor_id: "fab-local".into(),
                bucket: BucketKey { node_nm: 130, process: "p".into(), tech: "euv".into() },
                report: r,
            })
            .unwrap();
        assert_eq!(decision, IngestDecision::ExcludedByConsent(ConsentScope::NoSharing));
        let bucket = BucketKey { node_nm: 130, process: "p".into(), tech: "euv".into() };
        assert_eq!(engine.cohort_size(&bucket), 0);
        assert!(engine.publish(&bucket, 1).is_none());
    }

    /// Deterministic PRNG reproducibility audit (the noise the fab would see).
    #[test]
    fn dp_noise_is_reproducible_for_audit() {
        let mut a = SplitMix64::new(5);
        let mut b = SplitMix64::new(5);
        assert_eq!(a.next_f64(), b.next_f64());
    }

    /// The report JSON survives a parse/serialize cycle byte-for-byte (canonical form).
    #[test]
    fn canonical_json_is_stable() {
        let r = report(33, ConsentScope::PrivateBilateral, 0.5);
        let once = r.to_canonical_json();
        let reparsed = Json::parse(&once).unwrap();
        let twice = OutcomeReport::from_json(&reparsed).unwrap().to_canonical_json();
        assert_eq!(once, twice);
    }
}
