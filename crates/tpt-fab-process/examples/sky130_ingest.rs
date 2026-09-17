//! Ingests sky130 MPW shuttle results through the cohort-protected aggregation pipeline and
//! prints what would (or would not) be published.
//!
//! Run with: `cargo run -p tpt-fab-process --example sky130_ingest`

use tpt_fab_aggregate::AggregationEngine;
use tpt_fab_process::sky130::{ingest_shuttle_result, parse_shuttle_result};

const SHUTTLE_JSON: &str = r#"{
    "run_id": "mpw-7",
    "project_id": "REPLACED-BY-LOOP",
    "geometry": [
        {"id": "via-chain-1", "designed_um": 8.0, "measured_um": 8.06},
        {"id": "poly-cd", "designed_um": 0.15, "measured_um": 0.148}
    ],
    "electrical": [
        {"id": "ringosc-1", "quantity": "freq", "designed": 50.0, "measured": 47.5, "unit": "mhz"}
    ],
    "yield": {"started": 3, "good": 2, "failure_mode": "metal open"}
}"#;

fn main() {
    let mut engine = AggregationEngine::new();
    let bucket = tpt_fab_aggregate::BucketKey {
        node_nm: 130,
        process: "sky130-mpw-7".into(),
        tech: "duv-multi-pattern".into(),
    };

    // Four designers submit: the cohort gate withholds the plain aggregate.
    for i in 0..4 {
        let text = SHUTTLE_JSON.replace("REPLACED-BY-LOOP", &format!("designer-{i}"));
        let result = parse_shuttle_result(&text).unwrap();
        ingest_shuttle_result(&mut engine, &result).unwrap();
    }
    match engine.publish(&bucket, 1) {
        Some(p) if p.dp_protected => println!(
            "cohort {}: only DP-noised statistics are publishable (yield rate shown as {:.3})",
            p.n_contributors, p.mean_yield_rate
        ),
        other => panic!("expected DP-protected publication, got {other:?}"),
    }

    // The fifth submission opens the gate for this shuttle bucket.
    let text = SHUTTLE_JSON.replace("REPLACED-BY-LOOP", "designer-4");
    let result = parse_shuttle_result(&text).unwrap();
    ingest_shuttle_result(&mut engine, &result).unwrap();

    let p = engine.publish(&bucket, 1).unwrap();
    println!(
        "cohort {}: plain aggregate published -- mean yield {:.3}, mean |geometry deviation| {:.3} um",
        p.n_contributors, p.mean_yield_rate, p.mean_abs_geometry_deviation_um
    );

    // The same designer submitting twice does not inflate the cohort.
    ingest_shuttle_result(&mut engine, &parse_shuttle_result(&text).unwrap()).unwrap();
    println!(
        "after a duplicate submission the cohort is still {} (independent contributors only)",
        engine.cohort_size(&bucket)
    );
}
