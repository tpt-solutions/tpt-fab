//! # tpt-fab-litho
//!
//! Pluggable patterning backends for [`tpt-fab`]'s core.
//!
//! The [`PatterningBackend`] trait is deliberately thin: `tpt-fab`'s recipe, lot, and APC
//! logic depends only on it, so swapping which lithography technology sits underneath never
//! requires changes to that core logic. One implementation per technology lives in this crate:
//!
//! * [`duv::DuvMultiPatternBackend`] — DUV multi-patterning (LELE / SADP-style), built against
//!   published generic process characteristics. SMEE-style domestic ArF-immersion parameters are
//!   deferred to an optional profile once a partner/validation opportunity exists (see
//!   [`duv::DuvProfile`]).
//! * [`euv::EuvBackend`] — projection-optics exposure sequencing with pellicle/dose bookkeeping.
//! * [`nil::NilBackend`] — J-FIL-style imprint/separation cycle control with template defect
//!   tracking.
//! * [`ebeam::EbeamBackend`] — vector-scan control for mask writing and low-volume direct-write.
//!
//! ## Validation status
//!
//! None of these backends has been validated against real hardware. Each is built and tested
//! against the *published process characteristics* of its technology and against `tpt-fab`'s own
//! equipment simulator, and is explicitly flagged as
//! [`PatterningBackend::validated_against_real_hardware`] = `false` until a design partner using
//! that technology adopts it.

#![forbid(unsafe_code)]

pub mod backend;
pub mod duv;
pub mod ebeam;
pub mod error;
pub mod euv;
pub mod nil;

pub use backend::{
    BackendHealth, ExposureRequest, ExposureStep, HealthStatus, LayerSpec, PassKind,
    PatterningBackend, PatterningTech, PlanStep, ProcessContext,
};
pub use error::LithoError;

#[cfg(test)]
mod tests {
    use super::*;
    use duv::{DuvConfig, DuvMultiPatternBackend, MultiPatternScheme};
    use ebeam::{EbeamBackend, EbeamConfig, EbeamMode};
    use euv::{EuvBackend, EuvConfig};
    use nil::{NilBackend, NilConfig};

    fn request(overlay_budget: f64, dose_budget: f64) -> ExposureRequest {
        ExposureRequest {
            layer: LayerSpec::new("metal1", 40.0, 80.0, 0.4),
            context: ProcessContext::default(),
            overlay_budget_nm: overlay_budget,
            dose_budget_mj_cm2: dose_budget,
        }
    }

    #[test]
    fn all_backends_report_unvalidated() {
        let duv = DuvMultiPatternBackend::new(DuvConfig::default()).unwrap();
        let euv = EuvBackend::new(EuvConfig::default()).unwrap();
        let nil = NilBackend::new(NilConfig::default()).unwrap();
        let ebeam = EbeamBackend::new(EbeamConfig::default()).unwrap();
        for b in [&duv as &dyn PatterningBackend, &euv, &nil, &ebeam] {
            assert!(!b.validated_against_real_hardware(), "{} claims validated", b.name());
        }
        // Distinct technologies.
        let mut techs = vec![duv.tech(), euv.tech(), nil.tech(), ebeam.tech()];
        techs.sort();
        techs.dedup();
        assert_eq!(techs.len(), 4);
    }

    #[test]
    fn duv_lele_plan_respects_budgets() {
        let duv = DuvMultiPatternBackend::new(DuvConfig::default()).unwrap();
        let plan = duv.plan_exposure(&request(10.0, 200.0)).unwrap();
        // LELE: 2 exposures + 1 cut at default corner density.
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.total_dose_mj_cm2, 30.0 * 2.0 + 30.0 * 0.8);
        // RSS of three 2.5 nm passes.
        assert!((plan.required_overlay_nm - (3.0f64 * 2.5 * 2.5).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn duv_rejects_unprintable_pitch() {
        let duv = DuvMultiPatternBackend::new(DuvConfig::default()).unwrap();
        let mut req = request(10.0, 200.0);
        req.layer.min_pitch_nm = 5.0; // k1 way below 0.25
        assert!(matches!(duv.plan_exposure(&req), Err(LithoError::UnsupportedFeature(_))));
    }

    #[test]
    fn duv_overlay_budget_enforced() {
        let duv = DuvMultiPatternBackend::new(DuvConfig::default()).unwrap();
        // RSS of 3 x 2.5nm is ~4.33; budget of 3 must fail.
        assert!(matches!(
            duv.plan_exposure(&request(3.0, 200.0)),
            Err(LithoError::OverlayBudgetExceeded { .. })
        ));
    }

    #[test]
    fn duv_sadp_absorbs_pass2_overlay() {
        let cfg = DuvConfig { scheme: MultiPatternScheme::Sadp, ..DuvConfig::default() };
        let duv = DuvMultiPatternBackend::new(cfg).unwrap();
        let plan = duv.plan_exposure(&request(10.0, 200.0)).unwrap();
        // Mandrel exposure + spacer-definition (SADP's single litho pass means no cuts).
        assert_eq!(plan.steps.len(), 2);
        // The spacer step is self-aligned (zero overlay); only the mandrel bears overlay.
        let bearing: Vec<_> = plan.steps.iter().filter(|s| s.overlay_tolerance_nm > 0.0).collect();
        assert_eq!(bearing.len(), 1);
        assert!(plan.notes.iter().any(|n| n.contains("absorb pass-2 overlay")));
    }

    #[test]
    fn euv_pellicle_bookkeeping_drives_health() {
        let mut euv = EuvBackend::new(EuvConfig::default()).unwrap();
        assert_eq!(euv.health().status, HealthStatus::Nominal);
        for _ in 0..(50_000 - 5_000) {
            euv.record_exposure(45.0);
        }
        assert_eq!(euv.health().status, HealthStatus::Degraded);
        for _ in 0..4_500 {
            euv.record_exposure(45.0);
        }
        assert_eq!(euv.health().status, HealthStatus::Down);
        assert_eq!(euv.pellicle_shots_remaining(), 500);
    }

    #[test]
    fn nil_template_rejects_bad_condition() {
        let mut nil = NilBackend::new(NilConfig::default()).unwrap();
        nil.record_cycle(12.0, 0.05);
        let plan = nil.plan_exposure(&request(10.0, 200.0));
        assert!(plan.is_ok());
        nil.record_cycle(40.0, 0.05); // outside separation window (5..25)
        assert!(matches!(
            nil.plan_exposure(&request(10.0, 200.0)),
            Err(LithoError::TemplateConditionUnacceptable(_))
        ));
    }

    #[test]
    fn ebeam_field_must_divide_evenly() {
        let cfg = EbeamConfig { subfield_size_um: 7.0, ..EbeamConfig::default() };
        assert!(matches!(EbeamBackend::new(cfg), Err(LithoError::InvalidConfiguration(_))));
        let ebeam = EbeamBackend::new(EbeamConfig {
            mode: EbeamMode::DirectWrite,
            ..EbeamConfig::default()
        })
        .unwrap();
        assert_eq!(ebeam.subfields_per_field(), 2_500); // 50 x 50
        let plan = ebeam.plan_exposure(&request(10.0, 200.0)).unwrap();
        assert_eq!(plan.steps[0].kind, PassKind::Write);
        assert!((plan.total_dose_mj_cm2 - 20.0).abs() < 1e-9); // 80 * 10/10 / 4
    }
}
