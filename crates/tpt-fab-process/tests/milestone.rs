//! Phase 3 milestone (spec Section 5): *a sky130 design run through the simulator flags at
//! least one class of issue plain DRC would miss.*

use tpt_fab::lot::LotState;
use tpt_fab::sim::{EquipmentSimulator, HostClient, SimulatorConfig};
use tpt_fab::PatterningBackend;
use tpt_fab_aggregate::AggregatePublication;
use tpt_fab_litho::duv::{DuvConfig, DuvMultiPatternBackend};
use tpt_fab_process::opc::{DrawFeature, FeatureKind, OpcEngine};
use tpt_fab_process::sky130::{ingest_shuttle_result, parse_shuttle_result};
use tpt_fab_process::window::{
    simulate_deposition, simulate_etch, simulate_thermal, DepositionParams, EtchParams,
    ProcessWindow, ThermalParams,
};
use tpt_fab_process::FeatureKind as ReExportedKind;

/// Naive width/spacing DRC over the drawn layer (sky130 poly-ish numbers): the checks every
/// design rule deck already performs. Returns (design is DRC-clean?).
fn naive_drc(features: &[DrawFeature], min_width_nm: f64, min_spacing_nm: f64) -> bool {
    features.iter().all(|f| f.width_nm >= min_width_nm)
        && features
            .iter()
            .filter_map(|f| f.pitch_nm.map(|p| p - f.width_nm))
            .all(|space| space >= min_spacing_nm)
}

#[test]
fn milestone_simulator_flags_what_plain_drc_misses() {
    // A sky130-style poly layer: every feature passes width/spacing DRC.
    let layer = vec![
        DrawFeature {
            id: "gate-line".into(),
            kind: FeatureKind::Line,
            width_nm: 150.0,
            pitch_nm: Some(150.0 + 150.0),
        },
        DrawFeature {
            id: "gate-end".into(),
            kind: FeatureKind::LineEnd,
            width_nm: 150.0,
            pitch_nm: None,
        },
        DrawFeature {
            id: "gate-corner".into(),
            kind: FeatureKind::Corner,
            width_nm: 150.0,
            pitch_nm: None,
        },
        DrawFeature { id: "via".into(), kind: FeatureKind::Via, width_nm: 150.0, pitch_nm: None },
    ];
    assert!(
        naive_drc(&layer, 140.0, 140.0),
        "design must be DRC-clean for the milestone to mean anything"
    );

    // Run it through the OPC engine keyed to the (simulated) active backend.
    let report = OpcEngine::new(tpt_fab_litho::PatterningTech::DuvMultiPattern).analyze(&layer);
    let classes: Vec<&str> = report
        .suggestions
        .iter()
        .map(|s| s.rationale.as_str())
        .filter(|r| r.contains("DRC cannot see this"))
        .map(|_| "drc-invisible")
        .collect();
    assert!(
        classes.len() >= 2,
        "milestone: at least two DRC-invisible issue classes (line-end + corner) must be flagged"
    );
    let _ = ReExportedKind::Line; // re-export sanity

    // The same layer through the coarse etch simulator flags a process-window violation the
    // rule deck has no opinion about.
    let violations = simulate_etch(&EtchParams {
        target_depth_nm: 318.0, // window tops out at 315
        non_uniformity: 0.03,
        post_etch_cd_nm: 152.0,
        depth_window: ProcessWindow::new(285.0, 315.0),
        cd_window: ProcessWindow::new(125.0, 145.0),
    });
    assert!(
        violations.iter().any(|v| v.kind == tpt_fab_process::ViolationKind::EtchDepth),
        "etch window violation must be flagged"
    );

    // Thermal budget check also catches an over-budget sequence.
    let thermal = simulate_thermal(&ThermalParams {
        budget_c_min: 30_000.0,
        steps: vec![(450.0, 45.0), (400.0, 30.0)],
    });
    assert_eq!(thermal.len(), 1);

    // Deposition stays clean when in-window (the checks are not vacuously true).
    assert!(simulate_deposition(&DepositionParams {
        target_thickness_nm: 500.0,
        non_uniformity: 0.03,
        thickness_window: ProcessWindow::new(480.0, 520.0),
    })
    .is_empty());
}

#[test]
fn milestone_sky130_shuttle_round_trip_through_aggregate_protection() {
    // The software path of the Phase 4 milestone: a shuttle result is parsed, converted to
    // an outcome report keyed to semantic IDs, and ingested under cohort + DP protection.
    let sim_backend: Box<dyn PatterningBackend> =
        Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap());
    let _sim = EquipmentSimulator::bind(SimulatorConfig::default(), sim_backend).unwrap();

    let mut engine = tpt_fab_aggregate::AggregationEngine::new();
    let json = |designer: &str| {
        format!(
            r#"{{
                "run_id": "mpw-9",
                "project_id": "{designer}",
                "geometry": [{{"id": "via-chain", "designed_um": 8.0, "measured_um": 8.05}}],
                "electrical": [{{"id": "ro-freq", "quantity": "freq", "designed": 50.0, "measured": 48.0, "unit": "mhz"}}],
                "yield": {{"started": 3, "good": 2}}
            }}"#
        )
    };
    for i in 0..5 {
        let r = parse_shuttle_result(&json(&format!("designer-{i}"))).unwrap();
        ingest_shuttle_result(&mut engine, &r).unwrap();
    }
    let bucket = tpt_fab_aggregate::BucketKey {
        node_nm: 130,
        process: "sky130-mpw-9".into(),
        tech: "duv-multi-pattern".into(),
    };
    let publication: AggregatePublication = engine.publish(&bucket, 1).unwrap();
    assert!(!publication.dp_protected, "5 independent designers meet the cohort");
    assert!((publication.mean_yield_rate - 2.0 / 3.0).abs() < 1e-9);
}

/// The litho-backend swap story, end to end from the process crate's perspective: the same
/// staged lot runs to completion regardless of backend, and the process-sim layer above sees
/// identical outcomes.
#[test]
fn process_layer_sees_identical_lifecycle_across_backend_swap() {
    fn run_to_completion(backend: Box<dyn PatterningBackend>) -> LotState {
        let sim = std::sync::Arc::new(
            EquipmentSimulator::bind(SimulatorConfig::default(), backend).unwrap(),
        );
        sim.stage_lot_and_job("LX", &["WX1"], "PJX", "etch-std").unwrap();
        let addr = sim.spawn();
        let mut host = HostClient::connect(addr, 0, SimulatorConfig::default().timers).unwrap();
        host.establish_communications().unwrap();
        host.go_online().unwrap();
        host.upload_recipe("etch-std", b"b").unwrap();
        host.remote_command("START", vec![("PJID", "PJX")]).unwrap();
        loop {
            let ev = host.next_event(std::time::Duration::from_secs(5)).unwrap();
            let ceid = ev
                .item
                .as_ref()
                .and_then(|i| i.as_list().and_then(|l| l.get(1)))
                .and_then(|c| c.as_u4())
                .unwrap_or(0);
            if ceid == tpt_fab::sim::ceids::LOT_COMPLETED {
                break;
            }
        }
        let lot_state = sim.core().state.lock().unwrap().lots.get("LX").unwrap().state;
        lot_state
    }

    let duv =
        run_to_completion(Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()));
    let euv = run_to_completion(Box::new(
        tpt_fab_litho::euv::EuvBackend::new(tpt_fab_litho::euv::EuvConfig::default()).unwrap(),
    ));
    assert_eq!(duv, LotState::Completed);
    assert_eq!(euv, LotState::Completed);
}
