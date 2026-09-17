//! Uses the intake-queue library API on a scratch directory: three delivered files with
//! different fates (accepted, rejected for tampering, accepted via lenient unsigned policy),
//! with the ledger persisted between runs so screening history accumulates.
//!
//! Run with: `cargo run -p tpt-fab-intake --example intake_api`

use std::path::{Path, PathBuf};
use std::rc::Rc;

use tpt_fab_aggregate::outcome::{
    ConsentScope, ManufacturingTrack, OutcomeReport, SemanticId, WaferTech, YieldSummary,
};
use tpt_fab_aggregate::schema::SchemaVersion;
use tpt_fab_aggregate::signature::{sign_report, UnsignedPolicy};
use tpt_fab_aggregate::{JsonOutcomeFile, OutcomeFileWriter};
use tpt_fab_intake::{IntakeConfig, IntakeLedger, IntakeQueue};

const KEY: &[u8] = b"demo-key";

fn report(seed: u64, yield_rate: f64) -> OutcomeReport {
    OutcomeReport {
        payload_manifest_id: tpt_fab_aggregate::outcome::new_manifest_id(seed),
        track: ManufacturingTrack::WaferLitho(WaferTech::Euv),
        measured_geometry: vec![tpt_fab_aggregate::outcome::GeometryDeviation {
            feature_id: SemanticId::new("via-chain").unwrap(),
            designed_um: 8.0,
            measured_um: 8.05,
        }],
        electrical_test: vec![],
        yield_outcome: YieldSummary {
            units_started: 1000,
            units_good: (yield_rate * 1000.0) as u64,
            dominant_failure_mode: None,
        },
        process_notes: vec![],
        consent: ConsentScope::AggregatedContribution,
        schema_version: SchemaVersion::CURRENT,
        signature: None,
    }
}

fn write(dir: &Path, name: &str, r: &OutcomeReport) -> PathBuf {
    let p = dir.join(name);
    JsonOutcomeFile.write_outcome_file(r, &p).unwrap();
    p
}

fn main() {
    let root = std::env::temp_dir().join(format!("tpt-fab-intake-demo-{}", std::process::id()));
    let incoming = root.join("incoming");
    std::fs::create_dir_all(&incoming).unwrap();

    // What a fab emailed: one clean signed report, one tampered in transit.
    let mut clean = report(1, 0.92);
    sign_report(&mut clean, "fab-alpha", KEY);
    write(&incoming, "fab-alpha-clean.json", &clean);

    let mut tampered = report(2, 0.90);
    sign_report(&mut tampered, "fab-alpha", KEY);
    tampered.yield_outcome.units_good += 300;
    write(&incoming, "tampered.json", &tampered);

    // A pilot fab with no key yet; the operator chose to accept unsigned files.
    write(&incoming, "pilot-unsigned.json", &report(3, 0.88));

    let config = IntakeConfig {
        unsigned_policy: UnsignedPolicy::AcceptUnsigned,
        key_of: Rc::new(|kid: &str| (kid == "fab-alpha").then(|| KEY.to_vec())),
        z_threshold: 4.0,
        min_history: 8,
    };
    let ledger_path = root.join("ledger.json");
    let mut ledger = IntakeLedger::load(&ledger_path).unwrap();
    let mut queue = IntakeQueue::from_ledger(config, &ledger);

    for d in queue.process_directory(&incoming, &root, &mut ledger, &ledger_path).unwrap() {
        println!("{:>8}  {}  {}", d.disposition.dir_name(), d.file, d.reason);
    }
    println!("ledger: {} decision(s) at {}", ledger.decisions.len(), ledger_path.display());

    std::fs::remove_dir_all(&root).ok();
}
