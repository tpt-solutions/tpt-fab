//! Errors returned by patterning backends.

use std::fmt;

/// Error type for [`crate::PatterningBackend`] operations.
#[derive(Debug, Clone, PartialEq)]
pub enum LithoError {
    /// The backend cannot pattern this layer/feature combination.
    UnsupportedFeature(String),
    /// Accumulated overlay across passes would exceed the layer's overlay budget.
    OverlayBudgetExceeded {
        /// Overlay requirement computed from the pass sequence, in nm.
        required_nm: f64,
        /// Overlay budget available, in nm.
        budget_nm: f64,
    },
    /// Total accumulated dose would exceed the dose budget for the pass sequence.
    DoseBudgetExceeded {
        /// Required dose, in mJ/cm².
        required_mj_cm2: f64,
        /// Budgeted dose, in mJ/cm².
        budget_mj_cm2: f64,
    },
    /// Template defect density or wear exceeds the configured limit (NIL).
    TemplateConditionUnacceptable(String),
    /// The backend configuration itself is invalid.
    InvalidConfiguration(String),
}

impl fmt::Display for LithoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LithoError::UnsupportedFeature(s) => write!(f, "unsupported feature: {s}"),
            LithoError::OverlayBudgetExceeded { required_nm, budget_nm } => {
                write!(
                    f,
                    "overlay budget exceeded: need {required_nm:.1} nm, budget {budget_nm:.1} nm"
                )
            }
            LithoError::DoseBudgetExceeded { required_mj_cm2, budget_mj_cm2 } => {
                write!(f, "dose budget exceeded: need {required_mj_cm2:.1} mJ/cm2, budget {budget_mj_cm2:.1} mJ/cm2")
            }
            LithoError::TemplateConditionUnacceptable(s) => {
                write!(f, "template condition unacceptable: {s}")
            }
            LithoError::InvalidConfiguration(s) => write!(f, "invalid backend configuration: {s}"),
        }
    }
}

impl std::error::Error for LithoError {}
