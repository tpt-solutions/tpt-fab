//! DUV multi-patterning backend.
//!
//! Built against *published generic* DUV multi-patterning process characteristics (ArF
//! immersion, LELE/SADP-style split-and-cut sequences).
//!
//! **Deferred by decision (spec Section 6):** SMEE-style domestic ArF-immersion parameters are
//! intentionally not encoded here. [`DuvProfile::GenericArFi`] is the only profile until a
//! partner or validation opportunity around domestic tooling exists; the enum exists so a
//! profile can be added later without changing callers.

use crate::backend::{
    BackendHealth, ExposureRequest, ExposureStep, PassKind, PatterningBackend, PatterningTech,
    PlanStep,
};
use crate::error::LithoError;

/// Which multi-patterning scheme the backend plans for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiPatternScheme {
    /// Litho–etch–litho–etch: two full exposure/etch passes.
    Lele,
    /// Litho–etch–litho–etch–litho–etch: three full exposure/etch passes.
    Lelele,
    /// Self-aligned double patterning: one litho pass, spacer-defined second line set, one cut pass.
    Sadp,
}

impl MultiPatternScheme {
    /// Number of litho exposures the scheme requires per layer.
    pub fn litho_passes(self) -> usize {
        match self {
            MultiPatternScheme::Lele => 2,
            MultiPatternScheme::Lelele => 3,
            MultiPatternScheme::Sadp => 1,
        }
    }
}

/// Process profile for the DUV backend.
///
/// Only the generic published-characteristics profile exists today; see the crate-level docs for
/// the deferral decision around domestic ArF-immersion parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuvProfile {
    /// Generic 193 nm ArF immersion characteristics from published literature.
    GenericArFi,
}

/// Configuration for [`DuvMultiPatternBackend`].
#[derive(Debug, Clone, PartialEq)]
pub struct DuvConfig {
    /// Multi-patterning scheme to plan.
    pub scheme: MultiPatternScheme,
    /// Process profile.
    pub profile: DuvProfile,
    /// Exposure wavelength, nm (193 for ArF).
    pub wavelength_nm: f64,
    /// Lens numerical aperture.
    pub na: f64,
    /// Per-pass single-machine overlay tolerance, nm (used for RSS combination).
    pub per_pass_overlay_nm: f64,
    /// Nominal dose per litho pass, mJ/cm².
    pub dose_mj_cm2: f64,
}

impl Default for DuvConfig {
    fn default() -> Self {
        Self {
            scheme: MultiPatternScheme::Lele,
            profile: DuvProfile::GenericArFi,
            wavelength_nm: 193.0,
            na: 1.35,
            per_pass_overlay_nm: 2.5,
            dose_mj_cm2: 30.0,
        }
    }
}

/// DUV multi-patterning backend. Unvalidated against real hardware; see crate docs.
#[derive(Debug, Clone)]
pub struct DuvMultiPatternBackend {
    config: DuvConfig,
    /// Passes planned since reset, for telemetry.
    passes_planned: u64,
}

impl DuvMultiPatternBackend {
    /// Creates a backend; errors if the config is self-inconsistent.
    pub fn new(config: DuvConfig) -> Result<Self, LithoError> {
        if config.na <= 0.0 || config.na > 1.6 {
            return Err(LithoError::InvalidConfiguration(format!("NA {} out of range", config.na)));
        }
        if config.per_pass_overlay_nm <= 0.0 || config.dose_mj_cm2 <= 0.0 {
            return Err(LithoError::InvalidConfiguration(
                "overlay tolerance and dose must be positive".into(),
            ));
        }
        Ok(Self { config, passes_planned: 0 })
    }

    /// k1 factor implied by the config for the given pitch (informational).
    pub fn k1_for_pitch(&self, pitch_nm: f64) -> f64 {
        pitch_nm * self.config.na / self.config.wavelength_nm
    }
}

impl PatterningBackend for DuvMultiPatternBackend {
    fn tech(&self) -> PatterningTech {
        PatterningTech::DuvMultiPattern
    }

    fn name(&self) -> &str {
        "duv-multi-pattern"
    }

    fn plan_exposure(&self, request: &ExposureRequest) -> Result<ExposureStep, LithoError> {
        let cfg = &self.config;
        let mut steps = Vec::new();
        let mut notes = Vec::new();

        let k1 = self.k1_for_pitch(request.layer.min_pitch_nm);
        let litho_passes = cfg.scheme.litho_passes();
        notes.push(format!(
            "{:?}: {} litho passes, k1={k1:.2} at pitch {:.0} nm",
            cfg.scheme, litho_passes, request.layer.min_pitch_nm
        ));

        if k1 < 0.25 {
            return Err(LithoError::UnsupportedFeature(format!(
                "pitch {:.0} nm gives k1={k1:.2}, below printable limit",
                request.layer.min_pitch_nm
            )));
        }

        match cfg.scheme {
            MultiPatternScheme::Sadp => {
                steps.push(PlanStep::new(
                    "mandrel",
                    PassKind::Exposure,
                    cfg.dose_mj_cm2,
                    cfg.per_pass_overlay_nm,
                ));
                steps.push(PlanStep::new("spacer-def", PassKind::Spacing, 0.0, 0.0));
                notes.push("self-aligned spacers absorb pass-2 overlay".into());
            }
            _ => {
                for p in 1..=litho_passes {
                    steps.push(PlanStep::new(
                        format!("pass{p}"),
                        PassKind::Exposure,
                        cfg.dose_mj_cm2,
                        cfg.per_pass_overlay_nm,
                    ));
                }
            }
        }

        // Cut passes: corner-dense layers need cuts to break the split lines; one cut per
        // extra pass beyond the first.
        let cuts = if request.layer.corner_density > 0.0 { litho_passes - 1 } else { 0 };
        for c in 1..=cuts {
            steps.push(PlanStep::new(
                format!("cut{c}"),
                PassKind::Cut,
                cfg.dose_mj_cm2 * 0.8,
                cfg.per_pass_overlay_nm,
            ));
        }

        // RSS overlay across the overlay-contributing steps.
        let sq_sum: f64 =
            steps.iter().map(|s| s.overlay_tolerance_nm * s.overlay_tolerance_nm).sum();
        let required_overlay = sq_sum.sqrt();
        if required_overlay > request.overlay_budget_nm {
            return Err(LithoError::OverlayBudgetExceeded {
                required_nm: required_overlay,
                budget_nm: request.overlay_budget_nm,
            });
        }

        let total_dose: f64 = steps.iter().map(|s| s.dose_mj_cm2).sum();
        if total_dose > request.dose_budget_mj_cm2 {
            return Err(LithoError::DoseBudgetExceeded {
                required_mj_cm2: total_dose,
                budget_mj_cm2: request.dose_budget_mj_cm2,
            });
        }

        Ok(ExposureStep {
            steps,
            required_overlay_nm: required_overlay,
            total_dose_mj_cm2: total_dose,
            notes,
        })
    }

    fn health(&self) -> BackendHealth {
        BackendHealth::nominal(vec![("passes_planned".into(), self.passes_planned as f64)])
    }
}
