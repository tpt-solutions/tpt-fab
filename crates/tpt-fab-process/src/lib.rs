//! # tpt-fab-process
//!
//! OPC (optical proximity correction) mask-correction suggestions and a coarse
//! etch/deposition/thermal process simulator, keyed to the active
//! [`PatterningBackend`](tpt_fab_litho::PatterningBackend), plus the sky130 MPW shuttle-run
//! ingestion path.
//!
//! ## Explicit non-goal — read this before trusting a number
//!
//! **This crate is not a TCAD replacement.** Without calibration data from real fab runs, it
//! cannot match the accuracy of Synopsys Sentaurus or equivalent commercial tools. The models
//! here are deliberately coarse, rule-based, and built from published process characteristics;
//! they exist to catch the mistakes a mature-node or emerging fab currently catches *with
//! nothing* — line-end shortening, corner rounding, obvious process-window violations — not to
//! match leading-edge TCAD. Treat every suggestion as "worth an engineer's review", never as a
//! verified correction. The Manufacturing Outcome exchange (spec Section 4, see [`sky130`])
//! is what closes this gap over time: real shuttle-run outcomes calibrate the models against
//! ground truth.
//!
//! ## Modules
//!
//! * [`opc`] — mask-correction suggestions per feature, keyed to the active backend's
//!   illumination (no suggestions for NIL/E-beam, which pattern differently — documented).
//! * [`window`] — coarse etch/deposition/thermal simulation flagging process-window
//!   violations across a five-site wafer map.
//! * [`sky130`] — ingestion of efabless/Skywater-style MPW shuttle results, protected from
//!   day one by the `tpt-fab-aggregate` min-cohort (N ≥ 5) + differential-privacy machinery.

#![forbid(unsafe_code)]

pub mod opc;
pub mod sky130;
pub mod window;

pub use opc::{DrawFeature, FeatureKind, OpcEngine, OpcReport, OpcSuggestion};
pub use window::{
    DepositionParams, EtchParams, ProcessWindow, ThermalParams, Violation, ViolationKind,
};
