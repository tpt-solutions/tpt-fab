//! NIL backend — J-FIL-style imprint/separation cycle control with template defect tracking.
//!
//! Unvalidated against real hardware; see crate docs.

use crate::backend::{
    BackendHealth, ExposureRequest, ExposureStep, HealthStatus, PassKind, PatterningBackend,
    PatterningTech, PlanStep,
};
use crate::error::LithoError;

/// Configuration for [`NilBackend`].
#[derive(Debug, Clone, PartialEq)]
pub struct NilConfig {
    /// UV cure dose per imprint, mJ/cm².
    pub cure_dose_mj_cm2: f64,
    /// Acceptable separation force window, mN (imprint fails or damage occurs outside it).
    pub separation_force_window_mn: (f64, f64),
    /// Template imprints before refurbishment/replacement.
    pub template_life_imprints: u64,
    /// Maximum acceptable template defect density, defects/cm².
    pub max_template_defect_density_per_cm2: f64,
}

impl Default for NilConfig {
    fn default() -> Self {
        Self {
            cure_dose_mj_cm2: 20.0,
            separation_force_window_mn: (5.0, 25.0),
            template_life_imprints: 2_000,
            max_template_defect_density_per_cm2: 0.1,
        }
    }
}

/// NIL backend tracking template wear and defect density.
///
/// Unvalidated against real hardware; see crate docs.
#[derive(Debug)]
pub struct NilBackend {
    config: NilConfig,
    imprints_done: u64,
    template_defect_density_per_cm2: f64,
    last_separation_force_mn: Option<f64>,
}

impl NilBackend {
    /// Creates a backend; errors if the config is self-inconsistent.
    pub fn new(config: NilConfig) -> Result<Self, LithoError> {
        let (lo, hi) = config.separation_force_window_mn;
        if !(lo > 0.0 && lo < hi) {
            return Err(LithoError::InvalidConfiguration(format!(
                "separation force window ({lo}, {hi}) invalid"
            )));
        }
        if config.cure_dose_mj_cm2 <= 0.0 || config.template_life_imprints == 0 {
            return Err(LithoError::InvalidConfiguration(
                "cure dose must be positive and template life non-zero".into(),
            ));
        }
        Ok(Self {
            config,
            imprints_done: 0,
            template_defect_density_per_cm2: 0.0,
            last_separation_force_mn: None,
        })
    }

    /// Bookkeeping after one imprint cycle: records separation force and observed defect growth.
    pub fn record_cycle(&mut self, separation_force_mn: f64, new_defect_density_per_cm2: f64) {
        self.imprints_done += 1;
        self.last_separation_force_mn = Some(separation_force_mn);
        self.template_defect_density_per_cm2 = new_defect_density_per_cm2;
    }

    /// Imprints remaining on the template.
    pub fn template_imprints_remaining(&self) -> u64 {
        self.config.template_life_imprints.saturating_sub(self.imprints_done)
    }
}

impl PatterningBackend for NilBackend {
    fn tech(&self) -> PatterningTech {
        PatterningTech::Nil
    }

    fn name(&self) -> &str {
        "nil"
    }

    fn plan_exposure(&self, request: &ExposureRequest) -> Result<ExposureStep, LithoError> {
        if self.template_defect_density_per_cm2 > self.config.max_template_defect_density_per_cm2 {
            return Err(LithoError::TemplateConditionUnacceptable(format!(
                "template defect density {:.3}/cm2 exceeds limit {:.3}/cm2",
                self.template_defect_density_per_cm2,
                self.config.max_template_defect_density_per_cm2
            )));
        }
        if let Some(force) = self.last_separation_force_mn {
            let (lo, hi) = self.config.separation_force_window_mn;
            if !(lo..=hi).contains(&force) {
                return Err(LithoError::TemplateConditionUnacceptable(format!(
                    "last separation force {force:.1} mN outside window ({lo:.1}, {hi:.1}) mN"
                )));
            }
        }
        if self.template_imprints_remaining() == 0 {
            return Err(LithoError::TemplateConditionUnacceptable(
                "template has exhausted its imprint life".into(),
            ));
        }

        let steps = vec![
            PlanStep::new("imprint", PassKind::Imprint, 0.0, 0.0),
            PlanStep::new("uv-cure", PassKind::Cure, self.config.cure_dose_mj_cm2, 0.0),
            PlanStep::new("separate", PassKind::Separation, 0.0, 0.0),
        ];
        if self.config.cure_dose_mj_cm2 > request.dose_budget_mj_cm2 {
            return Err(LithoError::DoseBudgetExceeded {
                required_mj_cm2: self.config.cure_dose_mj_cm2,
                budget_mj_cm2: request.dose_budget_mj_cm2,
            });
        }
        Ok(ExposureStep {
            steps,
            required_overlay_nm: 0.0,
            total_dose_mj_cm2: self.config.cure_dose_mj_cm2,
            notes: vec![format!(
                "J-FIL cycle; template has {} of {} imprints left",
                self.template_imprints_remaining(),
                self.config.template_life_imprints
            )],
        })
    }

    fn health(&self) -> BackendHealth {
        let remaining = self.template_imprints_remaining() as f64;
        let life = self.config.template_life_imprints as f64;
        let status = if remaining == 0.0
            || self.template_defect_density_per_cm2
                > self.config.max_template_defect_density_per_cm2
        {
            HealthStatus::Down
        } else if remaining / life <= 0.1 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Nominal
        };
        BackendHealth {
            status,
            notes: Vec::new(),
            metrics: vec![
                ("template_imprints_remaining".into(), remaining),
                ("template_defect_density_per_cm2".into(), self.template_defect_density_per_cm2),
                ("last_separation_force_mn".into(), self.last_separation_force_mn.unwrap_or(0.0)),
            ],
        }
    }
}
