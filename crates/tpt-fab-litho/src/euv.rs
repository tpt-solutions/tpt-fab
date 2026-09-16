//! EUV backend — projection-optics exposure sequencing with pellicle/dose bookkeeping.
//!
//! Unvalidated against real hardware; see crate docs.

use crate::backend::{
    BackendHealth, ExposureRequest, ExposureStep, HealthStatus, PassKind, PatterningBackend,
    PatterningTech, PlanStep,
};
use crate::error::LithoError;

/// Configuration for [`EuvBackend`].
#[derive(Debug, Clone, PartialEq)]
pub struct EuvConfig {
    /// Exposure wavelength, nm (13.5 for EUV).
    pub wavelength_nm: f64,
    /// Projection optics numerical aperture.
    pub na: f64,
    /// Nominal single-exposure dose, mJ/cm².
    pub dose_mj_cm2: f64,
    /// Dose latitude margin, percent (+/-) applied to the nominal dose when budgeting.
    pub dose_latitude_pct: f64,
    /// Expected pellicle lifetime in exposures before replacement.
    pub pellicle_life_shots: u64,
    /// Source power at intermediate focus, W (affects throughput telemetry only).
    pub source_power_w: f64,
}

impl Default for EuvConfig {
    fn default() -> Self {
        Self {
            wavelength_nm: 13.5,
            na: 0.33,
            dose_mj_cm2: 45.0,
            dose_latitude_pct: 2.0,
            pellicle_life_shots: 50_000,
            source_power_w: 250.0,
        }
    }
}

/// EUV backend with pellicle and cumulative-dose bookkeeping.
///
/// Unvalidated against real hardware; see crate docs.
#[derive(Debug)]
pub struct EuvBackend {
    config: EuvConfig,
    pellicle_shots_used: u64,
    total_dose_delivered_mj_cm2: f64,
}

impl EuvBackend {
    /// Creates a backend; errors if the config is self-inconsistent.
    pub fn new(config: EuvConfig) -> Result<Self, LithoError> {
        if config.na <= 0.0 || config.na > 0.7 {
            return Err(LithoError::InvalidConfiguration(format!("NA {} out of range", config.na)));
        }
        if config.dose_mj_cm2 <= 0.0 || config.pellicle_life_shots == 0 {
            return Err(LithoError::InvalidConfiguration(
                "dose must be positive and pellicle life non-zero".into(),
            ));
        }
        Ok(Self { config, pellicle_shots_used: 0, total_dose_delivered_mj_cm2: 0.0 })
    }

    /// Bookkeeping for one executed exposure: charges the pellicle and the dose ledger.
    pub fn record_exposure(&mut self, dose_mj_cm2: f64) {
        self.pellicle_shots_used += 1;
        self.total_dose_delivered_mj_cm2 += dose_mj_cm2;
    }

    /// Shots remaining on the current pellicle.
    pub fn pellicle_shots_remaining(&self) -> u64 {
        self.config.pellicle_life_shots.saturating_sub(self.pellicle_shots_used)
    }
}

impl PatterningBackend for EuvBackend {
    fn tech(&self) -> PatterningTech {
        PatterningTech::Euv
    }

    fn name(&self) -> &str {
        "euv"
    }

    fn plan_exposure(&self, request: &ExposureRequest) -> Result<ExposureStep, LithoError> {
        let cfg = &self.config;
        let pitch_k1 = request.layer.min_pitch_nm * cfg.na / cfg.wavelength_nm;
        if pitch_k1 < 0.2 {
            return Err(LithoError::UnsupportedFeature(format!(
                "pitch {:.0} nm gives k1={pitch_k1:.2}, below single-exposure EUV limit",
                request.layer.min_pitch_nm
            )));
        }

        // Dose budgeted at nominal + latitude margin.
        let budgeted_dose = cfg.dose_mj_cm2 * (1.0 + cfg.dose_latitude_pct / 100.0);
        if budgeted_dose > request.dose_budget_mj_cm2 {
            return Err(LithoError::DoseBudgetExceeded {
                required_mj_cm2: budgeted_dose,
                budget_mj_cm2: request.dose_budget_mj_cm2,
            });
        }

        let steps = vec![PlanStep::new("euv-single", PassKind::Exposure, budgeted_dose, 0.0)];
        Ok(ExposureStep {
            steps,
            required_overlay_nm: 0.0,
            total_dose_mj_cm2: budgeted_dose,
            notes: vec![format!(
                "single exposure, k1={pitch_k1:.2}; pellicle has {} of {} shots left",
                self.pellicle_shots_remaining(),
                cfg.pellicle_life_shots
            )],
        })
    }

    fn health(&self) -> BackendHealth {
        let remaining = self.pellicle_shots_remaining() as f64;
        let life = self.config.pellicle_life_shots as f64;
        let frac = remaining / life;
        let status = if frac <= 0.05 {
            HealthStatus::Down
        } else if frac <= 0.2 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Nominal
        };
        let mut notes = Vec::new();
        if status != HealthStatus::Nominal {
            notes.push("pellicle nearing end of life — schedule replacement".into());
        }
        BackendHealth {
            status,
            notes,
            metrics: vec![
                ("pellicle_shots_remaining".into(), remaining),
                ("total_dose_mj_cm2".into(), self.total_dose_delivered_mj_cm2),
                ("source_power_w".into(), self.config.source_power_w),
            ],
        }
    }
}
