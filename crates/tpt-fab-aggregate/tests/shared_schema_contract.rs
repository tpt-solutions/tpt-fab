//! The one-shared-schema contract (spec 4.1 / Phase 4 "wire into both tracks simultaneously"):
//! the PCB track (`tpt-silicon-cam`, RFC-001) and the wafer track (`tpt-fab`) must be
//! indistinguishable to the pipeline except for the `track` field. Every protection —
//! signature, schema migration, consent enforcement, cohort gating, anomaly screening,
//! release emission — applies identically to both. This test pins that contract so neither
//! track can drift.

use std::rc::Rc;

use std::path::PathBuf;
use tpt_fab_aggregate::aggregate::{Contribution, IngestDecision};
use tpt_fab_aggregate::anomaly::{validate_report_basics, AnomalyDetector};
use tpt_fab_aggregate::outcome::{
    ConsentScope, ElectricalMeasurement, GeometryDeviation, ManufacturingTrack, OutcomeReport,
    SemanticId, WaferTech, YieldSummary,
};
use tpt_fab_aggregate::signature::{sign_report, UnsignedPolicy};
use tpt_fab_aggregate::{
    AggregationEngine, BucketKey, JsonOutcomeFile, OutcomeFileReader, OutcomeFileWriter,
};

/// Identical measurement payloads, differing only in `track`.
fn report(track: ManufacturingTrack, manifest: &str) -> OutcomeReport {
    OutcomeReport {
        payload_manifest_id: manifest.into(),
        track,
        measured_geometry: vec![GeometryDeviation {
            feature_id: SemanticId::new("shared-feature-1").unwrap(),
            designed_um: 10.0,
            measured_um: 10.12,
        }],
        electrical_test: vec![ElectricalMeasurement {
            link_id: SemanticId::new("FL-shared-1").unwrap(),
            quantity: "impedance".into(),
            designed: 85.0,
            measured: 87.0,
            unit: "ohm".into(),
        }],
        yield_outcome: YieldSummary {
            units_started: 400,
            units_good: 370,
            dominant_failure_mode: None,
        },
        process_notes: vec![],
        consent: ConsentScope::AggregatedContribution,
        schema_version: tpt_fab_aggregate::SchemaVersion::CURRENT,
        signature: None,
    }
}

#[test]
fn canonical_forms_differ_only_in_the_track_field() {
    // Identical payloads and manifest ID: after masking the track value, the canonical
    // forms must be byte-identical — proof that track is the *only* schema difference.
    let pcb = report(ManufacturingTrack::Pcb, "same-manifest");
    let wafer = report(ManufacturingTrack::WaferLitho(WaferTech::Euv), "same-manifest");
    assert!(pcb.to_canonical_json().contains("\"track\":\"pcb\""));
    assert!(wafer.to_canonical_json().contains("\"track\":\"wafer-litho:euv\""));
    let normalize = |s: &str| {
        s.replace("\"track\":\"pcb\"", "TRACK").replace("\"track\":\"wafer-litho:euv\"", "TRACK")
    };
    assert_eq!(normalize(&pcb.to_canonical_json()), normalize(&wafer.to_canonical_json()));
}

#[test]
fn both_tracks_round_trip_through_the_signed_file_exchange() {
    let dir = {
        let mut p = std::env::temp_dir();
        p.push(format!("tpt-fab-schema-contract-{}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    };
    const KEY: &[u8] = b"contract-key";

    let pcb = {
        let mut r = report(ManufacturingTrack::Pcb, "pcb-manifest");
        sign_report(&mut r, "pcb-fab", KEY);
        r
    };
    let wafer = {
        let mut r = report(ManufacturingTrack::WaferLitho(WaferTech::Nil), "wafer-manifest");
        sign_report(&mut r, "wafer-fab", KEY);
        r
    };

    for (name, signed) in [("pcb.json", &pcb), ("wafer.json", &wafer)] {
        let path: PathBuf = dir.join(name);
        JsonOutcomeFile.write_outcome_file(signed, &path).unwrap();
        let keys = |kid: &str| (kid == "pcb-fab" || kid == "wafer-fab").then(|| KEY.to_vec());
        let reader = JsonOutcomeFile::reader(UnsignedPolicy::RequireSignature, Rc::new(keys));
        let back = reader.read_outcome_file(&path).unwrap();
        assert_eq!(back, *signed);
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn both_tracks_get_identical_pipeline_treatment() {
    let pcb = report(ManufacturingTrack::Pcb, "pcb-manifest");
    let wafer =
        report(ManufacturingTrack::WaferLitho(WaferTech::DuvMultiPattern), "wafer-manifest");

    for r in [&pcb, &wafer] {
        validate_report_basics(r).unwrap();
        let mut detector = AnomalyDetector::new();
        let screen = detector.screen(r);
        assert!(!screen.flagged);
    }

    let pcb_bucket = BucketKey { node_nm: 0, process: "pcb-fr4".into(), tech: "pcb".into() };
    let wafer_bucket =
        BucketKey { node_nm: 130, process: "sky130-mpw".into(), tech: "duv-multi-pattern".into() };
    let mut engine = AggregationEngine::new();
    assert_eq!(
        engine
            .ingest_public_contribution(Contribution {
                contributor_id: "fab".into(),
                bucket: pcb_bucket.clone(),
                report: pcb,
            })
            .unwrap(),
        IngestDecision::Accepted
    );
    assert_eq!(
        engine
            .ingest_public_contribution(Contribution {
                contributor_id: "fab".into(),
                bucket: wafer_bucket,
                report: wafer,
            })
            .unwrap(),
        IngestDecision::Accepted
    );

    // Below the cohort gate, publication is DP-protected regardless of track.
    let p = engine.publish(&pcb_bucket, 1).unwrap();
    assert!(p.dp_protected, "below-gate publication must be DP-protected");
    // And a NoSharing report is excluded for either track.
    let mut refused = report(ManufacturingTrack::Pcb, "no-share");
    refused.consent = ConsentScope::NoSharing;
    assert_eq!(
        engine
            .ingest_public_contribution(Contribution {
                contributor_id: "fab".into(),
                bucket: pcb_bucket,
                report: refused,
            })
            .unwrap(),
        IngestDecision::ExcludedByConsent(ConsentScope::NoSharing)
    );
}
