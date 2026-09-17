# Changelog

All notable changes to the `tpt-fab-litho` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `PatterningBackend` trait — the thin seam `tpt-fab`'s core depends on:
  `tech`, `name`, `validated_against_real_hardware` (defaults to `false` and a test
  keeps it false), `plan_exposure`, and `health`.
- `DuvMultiPatternBackend`: LELE / LELELE / SADP schemes over published generic ArF
  immersion characteristics (`DuvProfile::GenericArFi` only, per the resolved
  spec decision to defer SMEE-style parameters), RSS overlay budgeting across
  overlay-bearing passes, k1-based printability gating, dose budgets.
- `EuvBackend`: single-exposure sequencing with pellicle shot-life and
  cumulative-dose bookkeeping driving `BackendHealth` (Nominal / Degraded / Down).
- `NilBackend`: J-FIL-style imprint / UV cure / separation cycle plans with
  template defect-density, imprint-life, and separation-force-window gating.
- `EbeamBackend`: vector-scan write plans (field/subfield split, multipass dose
  scaling) for mask-write and direct-write modes.
- Typed errors: `OverlayBudgetExceeded`, `DoseBudgetExceeded`,
  `UnsupportedFeature`, `TemplateConditionUnacceptable`, `InvalidConfiguration`.
- Example `plan_layer` planning one layer against all four backends.
- All backends documented unvalidated-against-real-hardware.
