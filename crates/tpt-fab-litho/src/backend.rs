//! The [`PatterningBackend`] trait and the technology-neutral plan types shared by all backends.

use crate::error::LithoError;

/// Lithography technology of a [`PatterningBackend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PatterningTech {
    /// DUV multi-patterning (LELE / SADP-style split-and-cut sequences).
    DuvMultiPattern,
    /// Extreme ultraviolet projection lithography.
    Euv,
    /// Nanoimprint lithography (J-FIL-style jet-and-flash imprint).
    Nil,
    /// Electron-beam (vector-scan) lithography.
    Ebeam,
}

impl PatterningTech {
    /// Short human-readable name, e.g. for logs and event reports.
    pub fn as_str(self) -> &'static str {
        match self {
            PatterningTech::DuvMultiPattern => "duv-multi-pattern",
            PatterningTech::Euv => "euv",
            PatterningTech::Nil => "nil",
            PatterningTech::Ebeam => "ebeam",
        }
    }
}

/// A layer to be patterned, described only by the quantities every backend needs.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSpec {
    /// Layer name (e.g. `"poly"`, `"metal1"`).
    pub name: String,
    /// Target critical dimension after development/etch, in nm.
    pub target_cd_nm: f64,
    /// Minimum pitch on the layer, in nm.
    pub min_pitch_nm: f64,
    /// Fraction of edges that are line-ends or corners (0.0–1.0), used for overlay/OPC pressure.
    pub corner_density: f64,
}

impl LayerSpec {
    /// Creates a layer spec.
    pub fn new(
        name: impl Into<String>,
        target_cd_nm: f64,
        min_pitch_nm: f64,
        corner_density: f64,
    ) -> Self {
        Self { name: name.into(), target_cd_nm, min_pitch_nm, corner_density }
    }
}

/// Process context shared by all backends. Technology-specific parameters live in each
/// backend's own configuration type, not here — this is what keeps the trait thin.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessContext {
    /// Target node, in nm (nominally; e.g. 130 for sky130-style work).
    pub node_nm: u32,
    /// Full-wafer throughput hint in wafers/hour, if the caller wants scheduling to see it.
    pub wafers_per_hour_hint: Option<f64>,
}

impl Default for ProcessContext {
    fn default() -> Self {
        Self { node_nm: 130, wafers_per_hour_hint: None }
    }
}

/// Request handed to [`PatterningBackend::plan_exposure`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExposureRequest {
    /// The layer to pattern.
    pub layer: LayerSpec,
    /// Process context (node, throughput hints).
    pub context: ProcessContext,
    /// Overlay budget available for this layer, in nm (RSS across all passes must fit).
    pub overlay_budget_nm: f64,
    /// Dose budget available for this layer, in mJ/cm².
    pub dose_budget_mj_cm2: f64,
}

/// Kind of a single step in a patterning plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    /// A primary exposure pass.
    Exposure,
    /// A cut/block pass (multi-patterning).
    Cut,
    /// A mandrel/spacing-definition pass (SADP-style).
    Spacing,
    /// A trim pass (e-beam-assisted or dry trim after a split).
    Trim,
    /// Imprint step (NIL dispense + align + fill).
    Imprint,
    /// UV cure step (NIL).
    Cure,
    /// Template separation step (NIL).
    Separation,
    /// Vector-scan write step (e-beam).
    Write,
}

impl PassKind {
    /// Whether this step exposes resist with photons or electrons.
    pub fn is_exposure(self) -> bool {
        matches!(
            self,
            PassKind::Exposure
                | PassKind::Cut
                | PassKind::Spacing
                | PassKind::Trim
                | PassKind::Write
        )
    }
}

/// One step in a patterning plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanStep {
    /// Stable label (e.g. `"pass1-line"`, `"cut-a"`, `"imprint"`).
    pub label: String,
    /// Kind of step.
    pub kind: PassKind,
    /// Dose for this step in mJ/cm² (cure dose for NIL, charge dose for e-beam).
    pub dose_mj_cm2: f64,
    /// Per-step overlay tolerance in nm; steps that are self-aligned may set this to 0.
    pub overlay_tolerance_nm: f64,
}

impl PlanStep {
    /// Creates a step.
    pub fn new(
        label: impl Into<String>,
        kind: PassKind,
        dose_mj_cm2: f64,
        overlay_tolerance_nm: f64,
    ) -> Self {
        Self { label: label.into(), kind, dose_mj_cm2, overlay_tolerance_nm }
    }
}

/// The plan a backend produces for one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ExposureStep {
    /// Steps in execution order.
    pub steps: Vec<PlanStep>,
    /// RSS-combined overlay the sequence actually requires, in nm.
    pub required_overlay_nm: f64,
    /// Total dose across all steps, in mJ/cm².
    pub total_dose_mj_cm2: f64,
    /// Technology-specific notes worth surfacing in logs (e.g. "self-aligned spacers absorb pass 2").
    pub notes: Vec<String>,
}

/// Backend telemetry snapshot, backend-specific metrics go in `metrics` as name/value pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendHealth {
    /// Overall status.
    pub status: HealthStatus,
    /// Free-form human-readable notes.
    pub notes: Vec<String>,
    /// Named metrics (e.g. `("pellicle_shots_remaining", 12345.0)`).
    pub metrics: Vec<(String, f64)>,
}

impl BackendHealth {
    /// Nominal health with the given metrics.
    pub fn nominal(metrics: Vec<(String, f64)>) -> Self {
        Self { status: HealthStatus::Nominal, notes: Vec::new(), metrics }
    }
}

/// Overall backend status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Operating normally.
    Nominal,
    /// Consumable nearing end of life or drift approaching limits.
    Degraded,
    /// Not usable until intervention.
    Down,
}

/// A pluggable patterning backend.
///
/// `tpt-fab`'s core (recipe, lot, APC logic) depends only on this trait; nothing in the core
/// matches on [`PatterningTech`]. Swapping which backend is installed behind the core therefore
/// requires no changes to recipe/lot/APC code.
pub trait PatterningBackend: Send + Sync {
    /// Which technology this backend patterns with.
    fn tech(&self) -> PatterningTech;

    /// Backend name for logs and reports.
    fn name(&self) -> &str;

    /// Whether this backend has been validated against real hardware. Every backend in
    /// `tpt-fab-litho` returns `false` until a design partner using that technology adopts it.
    fn validated_against_real_hardware(&self) -> bool {
        false
    }

    /// Produce the patterning plan for one layer and validate it against the layer's budgets.
    fn plan_exposure(&self, request: &ExposureRequest) -> Result<ExposureStep, LithoError>;

    /// Current backend telemetry (consumables, drift counters).
    fn health(&self) -> BackendHealth;
}
