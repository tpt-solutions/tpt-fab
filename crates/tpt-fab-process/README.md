# tpt-fab-process

**OPC mask-correction suggestions and coarse etch/deposition/thermal process simulation, keyed to the active patterning backend — plus the sky130 MPW shuttle-run ingestion path.**

The simulation layer of the [`tpt-fab`](../../README.md) stack (RFC-002 Section 2C, [`spec.txt`](../../spec.txt)).

## Explicit non-goal — read before trusting a number

**This is not a TCAD replacement.** Without calibration data from real fab runs, it cannot match the accuracy of Synopsys Sentaurus or equivalent. The models are deliberately coarse, rule-based, and built from published process characteristics. The target is *"catches the mistakes a mature-node or emerging fab currently catches with nothing"* — line-end shortening, corner rounding, obvious process-window violations — not "matches leading-edge TCAD." Treat every suggestion as worth an engineer's review, never as a verified correction. The Manufacturing Outcome exchange (`tpt-fab-aggregate`, fed here by the sky130 shuttle path) is what closes this gap over time with real ground truth.

## What's inside

| Module | Contents |
|---|---|
| `opc` | Per-feature mask-bias suggestions driven by the active backend's illumination: dense-line undersizing (k1-scaled), line-end pullback, corner serifs, via bias. NIL and e-beam correctly produce **no** optical suggestions — documented, with the equivalent control named (template QC / write-time proximity correction) |
| `window` | Five-site wafer-map etch, deposition, and thermal-budget simulation flagging process-window violations with signed margins |
| `sky130` | efabless/Skywater-style MPW shuttle-result JSON parsing → `OutcomeReport` conversion (semantic-ID keyed) → ingestion through the full cohort-protected aggregation pipeline |

## Usage

OPC keyed to the backend — same geometry, different backend, different suggestions:

```rust
use tpt_fab_process::opc::{DrawFeature, FeatureKind, OpcEngine};
use tpt_fab_litho::PatterningTech;

let layer = vec![
    DrawFeature { id: "end-1".into(), kind: FeatureKind::LineEnd, width_nm: 150.0, pitch_nm: None },
    DrawFeature { id: "corner-1".into(), kind: FeatureKind::Corner, width_nm: 150.0, pitch_nm: None },
];

// DUV immersion flags features EUV would consider comfortably printable, and vice versa:
let report = OpcEngine::new(PatterningTech::DuvMultiPattern).analyze(&layer);
for s in &report.suggestions {
    println!("{}: {:+.1} nm — {}", s.feature_id, s.suggested_bias_nm, s.rationale);
}
```

Coarse process-window simulation:

```rust
use tpt_fab_process::window::{simulate_etch, EtchParams, ProcessWindow};

let violations = simulate_etch(&EtchParams {
    target_depth_nm: 315.0,
    non_uniformity: 0.10,            // edge sites overshoot while center stays in-window
    post_etch_cd_nm: 130.0,
    depth_window: ProcessWindow::new(285.0, 315.0),
    cd_window: ProcessWindow::new(125.0, 135.0),
});
assert!(violations.iter().any(|v| v.location == "east"));
# Ok::<(), ()>(())
```

sky130 MPW shuttle ingestion — consent, the N ≥ 5 minimum-cohort gate, and differential-privacy fallback all apply from day one because the path ingests through the same `AggregationEngine` as direct fab contributions:

```rust
use tpt_fab_aggregate::AggregationEngine;
use tpt_fab_process::sky130::{ingest_shuttle_result, parse_shuttle_result};

let json = r#"{"run_id":"mpw-7","project_id":"designer-42",
               "geometry":[{"id":"via-chain-1","designed_um":8.0,"measured_um":8.06}],
               "yield":{"started":3,"good":2}}"#;
let result = parse_shuttle_result(json)?;
let mut engine = AggregationEngine::new();
ingest_shuttle_result(&mut engine, &result)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Runnable versions:

```sh
cargo run -p tpt-fab-process --example sky130_ingest
```

## Milestones carried here

- *Phase 3:* a sky130 design run through the simulator flags at least one class of issue plain DRC would miss — `tests/milestone.rs::milestone_simulator_flags_what_plain_drc_misses` (a layer that passes a naive width/spacing deck still gets line-end + corner OPC flags and etch-window violations).
- The backend-swap invariance is re-checked from this layer's perspective: the simulator completes identical lot lifecycles across backends (`tests/milestone.rs::process_layer_sees_identical_lifecycle_across_backend_swap`).

## Status

`0.1.0`, unpublished — see [`CHANGELOG.md`](./CHANGELOG.md). Source of truth: [`spec.txt`](../../spec.txt) (RFC-002). Dual-licensed MIT OR Apache-2.0.
