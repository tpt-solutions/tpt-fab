//! Plans the same layer against every patterning backend and prints the differences.
//!
//! Run with: `cargo run -p tpt-fab-litho --example plan_layer`

use tpt_fab_litho::duv::{DuvConfig, DuvMultiPatternBackend, MultiPatternScheme};
use tpt_fab_litho::ebeam::{EbeamBackend, EbeamConfig};
use tpt_fab_litho::euv::{EuvBackend, EuvConfig};
use tpt_fab_litho::nil::{NilBackend, NilConfig};
use tpt_fab_litho::{ExposureRequest, LayerSpec, PatterningBackend, ProcessContext};

fn main() {
    let request = ExposureRequest {
        layer: LayerSpec::new("metal1", 40.0, 80.0, 0.4),
        context: ProcessContext::default(),
        overlay_budget_nm: 10.0,
        dose_budget_mj_cm2: 200.0,
    };

    let backends: Vec<Box<dyn PatterningBackend>> = vec![
        Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()),
        // SADP absorbs pass-2 overlay into self-aligned spacers — same trait, different plan.
        Box::new(
            DuvMultiPatternBackend::new(DuvConfig {
                scheme: MultiPatternScheme::Sadp,
                ..DuvConfig::default()
            })
            .unwrap(),
        ),
        Box::new(EuvBackend::new(EuvConfig::default()).unwrap()),
        Box::new(NilBackend::new(NilConfig::default()).unwrap()),
        Box::new(EbeamBackend::new(EbeamConfig::default()).unwrap()),
    ];

    for backend in &backends {
        println!(
            "== {} (tech: {}, validated against real hardware: {}) ==",
            backend.name(),
            backend.tech().as_str(),
            backend.validated_against_real_hardware()
        );
        match backend.plan_exposure(&request) {
            Ok(plan) => {
                for step in &plan.steps {
                    println!(
                        "  {:>12}  dose {:>6.1} mJ/cm2  overlay tol {:.1} nm",
                        step.label, step.dose_mj_cm2, step.overlay_tolerance_nm
                    );
                }
                println!(
                    "  -> total dose {:.1} mJ/cm2, RSS overlay {:.2} nm",
                    plan.total_dose_mj_cm2, plan.required_overlay_nm
                );
                for note in &plan.notes {
                    println!("  note: {note}");
                }
            }
            Err(e) => println!("  plan rejected: {e}"),
        }
        println!("  health: {:?}", backend.health().status);
        println!();
    }
}
