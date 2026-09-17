//! The PCB track (RFC-001, `tpt-silicon-cam`) wired against the one shared outcome schema —
//! the reference integration this crate was built to serve.
//!
//! The wafer track (`tpt-fab`) and the PCB track produce **the same `OutcomeReport`** with the
//! same consent/cohort/DP protections; only `ManufacturingTrack` differs. This example is the
//! byte-exact contract the PCB exporter implements: build the report from
//! `FabricLink.id`/footprint-ID-keyed measurements, consent-tag it, sign it, write it to a
//! file — and on the `tpt-solutions` side, read, screen, aggregate, and gate it.
//!
//! Run with: `cargo run -p tpt-fab-aggregate --example pcb_track`

use std::path::PathBuf;
use std::rc::Rc;

use tpt_fab_aggregate::aggregate::{BucketKey, Contribution, IngestDecision};
use tpt_fab_aggregate::anomaly::{validate_report_basics, AnomalyDetector};
use tpt_fab_aggregate::outcome::{
    ConsentScope, ElectricalMeasurement, GeometryDeviation, ManufacturingTrack, OutcomeReport,
    SemanticId, YieldSummary,
};
use tpt_fab_aggregate::schema::SchemaVersion;
use tpt_fab_aggregate::signature::{sign_report, UnsignedPolicy};
use tpt_fab_aggregate::{AggregationEngine, JsonOutcomeFile, OutcomeFileReader, OutcomeFileWriter};

fn main() {
    // ---------------------------------------------------------------
    // PCB-fab side (runs on fab infrastructure; file-out only).
    // ---------------------------------------------------------------
    let mut report = OutcomeReport {
        // Ties back to the exact SMP payload the exporter sent to the fab.
        payload_manifest_id: "3f2a1b0c-1111-4222-8333-444455556666".into(),
        track: ManufacturingTrack::Pcb,
        measured_geometry: vec![
            GeometryDeviation {
                feature_id: SemanticId::new("fp-usb-c-0402-r1").unwrap(),
                designed_um: 500.0,
                measured_um: 502.5,
            },
            GeometryDeviation {
                feature_id: SemanticId::new("net-usb-dp-width").unwrap(),
                designed_um: 120.0,
                measured_um: 118.0,
            },
        ],
        // A predicted 85Ω trace that measured 87Ω is a directly usable calibration point
        // for the tpt-silicon-si-pi field solver — keyed by FabricLink.id, not coordinates.
        electrical_test: vec![ElectricalMeasurement {
            link_id: SemanticId::new("FL-usb-dp").unwrap(),
            quantity: "impedance".into(),
            designed: 85.0,
            measured: 87.0,
            unit: "ohm".into(),
        }],
        yield_outcome: YieldSummary {
            units_started: 120,
            units_good: 117,
            dominant_failure_mode: Some("solder bridge".into()),
        },
        process_notes: vec![],
        consent: ConsentScope::AggregatedContribution,
        schema_version: SchemaVersion::CURRENT,
        signature: None,
    };
    sign_report(&mut report, "pcb-fab-alpha", b"shared-secret-key");

    let out_path = PathBuf::from("pcb-outcome.json");
    JsonOutcomeFile.write_outcome_file(&report, &out_path).unwrap();
    // The writer has returned. Sending the file is the fab's separate, deliberate act.
    println!(
        "[fab ] wrote signed outcome file {} ({} bytes)",
        out_path.display(),
        std::fs::metadata(&out_path).unwrap().len()
    );

    // ---------------------------------------------------------------
    // tpt-solutions side (runs on the manually-delivered file).
    // ---------------------------------------------------------------
    let keys = |kid: &str| (kid == "pcb-fab-alpha").then(|| b"shared-secret-key".to_vec());
    let reader = JsonOutcomeFile::reader(UnsignedPolicy::RequireSignature, Rc::new(keys));
    let received = reader.read_outcome_file(&out_path).unwrap();
    println!("[recv] signature verified; track = {}", received.track.as_str());
    validate_report_basics(&received).unwrap();

    let screen = AnomalyDetector::new().screen(&received);
    println!("[recv] anomaly screen: flagged = {}", screen.flagged);

    // Aggregate into the PCB bucket; the N >= 5 minimum-cohort gate applies per bucket.
    let bucket = BucketKey { node_nm: 0, process: "pcb-fr4-2layer".into(), tech: "pcb".into() };
    let mut engine = AggregationEngine::new();
    for i in 0..4 {
        let mut peer = received.clone();
        peer.signature = None; // peer reports arrived through their own verified channels
        assert_eq!(
            engine
                .ingest_public_contribution(Contribution {
                    contributor_id: format!("pcb-fab-{i}"),
                    bucket: bucket.clone(),
                    report: peer,
                })
                .unwrap(),
            IngestDecision::Accepted
        );
    }

    // Four contributors: below the gate, only DP-noised statistics leave the engine.
    match engine.publish(&bucket, 1) {
        Some(pubn) if pubn.dp_protected => println!(
            "[agg ] cohort {}: below N=5, DP-protected publication only (yield shown as {:.3} after Laplace noise)",
            pubn.n_contributors, pubn.mean_yield_rate
        ),
        _ => unreachable!("cohort of 4 must take the DP path"),
    }

    // The fifth contributor crosses the gate: the plain aggregate becomes publishable.
    assert_eq!(
        engine
            .ingest_public_contribution(Contribution {
                contributor_id: "pcb-fab-alpha".into(),
                bucket: bucket.clone(),
                report: received,
            })
            .unwrap(),
        IngestDecision::Accepted
    );
    let publication = engine.publish(&bucket, 1).unwrap();
    assert!(!publication.dp_protected && publication.n_contributors == 5);
    println!(
        "[agg ] cohort 5: gate open, plain aggregate published (mean yield {:.3})",
        publication.mean_yield_rate
    );

    std::fs::remove_file(&out_path).ok();
}
