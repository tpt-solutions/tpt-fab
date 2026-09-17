# tpt-fab-litho

**Pluggable patterning backends for `tpt-fab` — one implementation per lithography technology, behind one thin trait.**

Part of the [`tpt-fab`](../../README.md) wafer-fabrication stack (RFC-002, [`spec.txt`](../../spec.txt)). The [`PatterningBackend`] trait is deliberately thin so that `tpt-fab`'s recipe, lot, and APC logic depends on *nothing* technology-specific: swapping which backend is installed behind the core requires no changes to that logic (asserted by a milestone test in the core crate).

## Backends

| Backend | Technology | What it models |
|---|---|---|
| `DuvMultiPatternBackend` | DUV multi-patterning | LELE / LELELE / SADP split-and-cut sequences, RSS overlay budgeting across passes, per-pass dose |
| `EuvBackend` | EUV projection | Single-exposure sequencing, pellicle shot-life and cumulative-dose bookkeeping driving health status |
| `NilBackend` | Nanoimprint (J-FIL-style) | Imprint → UV cure → separation cycle, template defect density and imprint-life gating, separation-force window checks |
| `EbeamBackend` | Electron beam | Vector-scan write plans (field/subfield split, multipass), mask-write and direct-write modes |

## Validation status — read before trusting anything

**None of these backends is validated against real hardware.** Each is built and tested against the *published process characteristics* of its technology and against `tpt-fab`'s own equipment simulator. Every backend reports `validated_against_real_hardware() == false`, and a test asserts this stays true, until a design partner using that technology adopts it. Per the resolved spec decision, the DUV backend ships only the `DuvProfile::GenericArFi` profile (published generic ArF-immersion characteristics); SMEE-style domestic parameters are deliberately deferred to a future profile rather than over-fitted ahead of any hardware validation.

## Usage

```rust
use tpt_fab_litho::duv::{DuvConfig, DuvMultiPatternBackend};
use tpt_fab_litho::{ExposureRequest, LayerSpec, PatterningBackend, ProcessContext};

let backend = DuvMultiPatternBackend::new(DuvConfig::default())?;
let request = ExposureRequest {
    layer: LayerSpec::new("metal1", 40.0, 80.0, 0.4),
    context: ProcessContext::default(),
    overlay_budget_nm: 10.0,
    dose_budget_mj_cm2: 200.0,
};

// Core logic only ever calls this — never matches on the technology.
let plan = backend.plan_exposure(&request)?;
println!("{} steps, {:.1} mJ/cm² total", plan.steps.len(), plan.total_dose_mj_cm2);
# Ok::<(), tpt_fab_litho::LithoError>(())
```

A complete tour that plans one layer against all four backends and prints the differences:

```sh
cargo run -p tpt-fab-litho --example plan_layer
```

## Trait surface

```text
PatterningBackend
├── tech()                            -> PatterningTech          (duv-multi-pattern | euv | nil | ebeam)
├── name()                            -> &str                    (logs, reports)
├── validated_against_real_hardware() -> bool                    (false for every backend here)
├── plan_exposure(&ExposureRequest)   -> Result<ExposureStep>    (the one call core makes)
└── health()                          -> BackendHealth           (consumables, drift telemetry)
```

Backend-specific bookkeeping (EUV pellicle shots, NIL separation forces, e-beam subfield counters) lives as inherent methods on each backend type — core never needs it, so it never sees it. Plans are validated against the caller's overlay and dose budgets before they are returned; violations come back as typed errors (`OverlayBudgetExceeded`, `DoseBudgetExceeded`, `UnsupportedFeature`, `TemplateConditionUnacceptable`).

## Design notes

- **Technology neutrality is enforced, not aspirational.** The core-crate milestone test (`milestone_backend_swap_changes_nothing_in_core`) runs an identical host-side session against all four backends and asserts byte-identical recipe/lot/job outcomes.
- **Overlay math** is RSS across overlay-bearing steps; self-aligned steps (SADP spacers) contribute zero.
- **Printability gate:** pitches whose k1 factor falls below the printable limit are rejected at plan time rather than producing fantasy plans.
- `#![forbid(unsafe_code)]`; zero dependencies.

## Status

`0.1.0`, unpublished — see [`CHANGELOG.md`](./CHANGELOG.md). Source of truth: [`spec.txt`](../../spec.txt) (RFC-002). Dual-licensed MIT OR Apache-2.0.
