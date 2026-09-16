//! E-beam backend — vector-scan control for mask writing and low-volume direct-write.
//!
//! Unvalidated against real hardware; see crate docs.

use crate::backend::{
    BackendHealth, ExposureRequest, ExposureStep, PassKind, PatterningBackend, PatterningTech,
    PlanStep,
};
use crate::error::LithoError;

/// Operating mode of the e-beam backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EbeamMode {
    /// Mask writing (photomask/reticle production).
    MaskWrite,
    /// Low-volume direct write onto wafers.
    DirectWrite,
}

/// Configuration for [`EbeamBackend`].
#[derive(Debug, Clone, PartialEq)]
pub struct EbeamConfig {
    /// Operating mode.
    pub mode: EbeamMode,
    /// Beam current, nA.
    pub beam_current_na: f64,
    /// Write field size, µm.
    pub field_size_um: f64,
    /// Subfield size, µm (deflection-tile size; must divide into the field).
    pub subfield_size_um: f64,
    /// Number of shape-repeating passes used to average shot noise.
    pub multipass_count: u32,
}

impl Default for EbeamConfig {
    fn default() -> Self {
        Self {
            mode: EbeamMode::MaskWrite,
            beam_current_na: 10.0,
            field_size_um: 500.0,
            subfield_size_um: 10.0,
            multipass_count: 4,
        }
    }
}

/// E-beam vector-scan backend.
///
/// Unvalidated against real hardware; see crate docs.
#[derive(Debug, Clone)]
pub struct EbeamBackend {
    config: EbeamConfig,
    subfields_written: u64,
}

impl EbeamBackend {
    /// Creates a backend; errors if the config is self-inconsistent.
    pub fn new(config: EbeamConfig) -> Result<Self, LithoError> {
        if config.field_size_um <= 0.0 || config.subfield_size_um <= 0.0 {
            return Err(LithoError::InvalidConfiguration(
                "field and subfield sizes must be positive".into(),
            ));
        }
        if config.subfield_size_um > config.field_size_um
            || (config.field_size_um / config.subfield_size_um).fract().abs() > 1e-9
        {
            return Err(LithoError::InvalidConfiguration(
                "subfield size must divide the field size evenly".into(),
            ));
        }
        if config.multipass_count == 0 || config.beam_current_na <= 0.0 {
            return Err(LithoError::InvalidConfiguration(
                "multipass count and beam current must be positive".into(),
            ));
        }
        Ok(Self { config, subfields_written: 0 })
    }

    /// Subfields per full field.
    pub fn subfields_per_field(&self) -> u32 {
        let per_axis = (self.config.field_size_um / self.config.subfield_size_um) as u32;
        per_axis * per_axis
    }

    /// Telemetry: records that a write pass covered `subfields` subfields.
    pub fn record_write(&mut self, subfields: u64) {
        self.subfields_written += subfields;
    }
}

impl PatterningBackend for EbeamBackend {
    fn tech(&self) -> PatterningTech {
        PatterningTech::Ebeam
    }

    fn name(&self) -> &str {
        "ebeam"
    }

    fn plan_exposure(&self, request: &ExposureRequest) -> Result<ExposureStep, LithoError> {
        // Charge-based dose scales inversely with beam current relative to the 10 nA reference;
        // multipass divides the per-pass dose.
        if self.config.beam_current_na <= 0.0 {
            return Err(LithoError::InvalidConfiguration("beam current must be positive".into()));
        }
        let per_pass_dose =
            80.0 * (10.0 / self.config.beam_current_na) / f64::from(self.config.multipass_count);
        if per_pass_dose > request.dose_budget_mj_cm2 {
            return Err(LithoError::DoseBudgetExceeded {
                required_mj_cm2: per_pass_dose,
                budget_mj_cm2: request.dose_budget_mj_cm2,
            });
        }

        let mode_note = match self.config.mode {
            EbeamMode::MaskWrite => "mask write",
            EbeamMode::DirectWrite => "direct write",
        };
        let subfields = self.subfields_per_field();
        let steps = vec![PlanStep::new(
            format!("vector-scan x{}", self.config.multipass_count),
            PassKind::Write,
            per_pass_dose,
            0.0,
        )];
        Ok(ExposureStep {
            steps,
            required_overlay_nm: 0.0,
            total_dose_mj_cm2: per_pass_dose,
            notes: vec![
                format!(
                    "{mode_note}: {} subfields/field, {}-pass strategy",
                    subfields, self.config.multipass_count
                ),
                "proximity-effect correction assumed applied upstream (OPC engine)".into(),
            ],
        })
    }

    fn health(&self) -> BackendHealth {
        BackendHealth::nominal(vec![("subfields_written".into(), self.subfields_written as f64)])
    }
}
