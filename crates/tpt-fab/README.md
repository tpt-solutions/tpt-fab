# tpt-fab

**Wafer-fab SECS/GEM core — HSMS transport, SECS-II codec, GEM/GEM300 state models, recipe versioning + drift detection, lot tracking, and an equipment simulator that runs the whole stack without real hardware.**

The operational layer of the [`tpt-fab`](../../README.md) stack (RFC-002, [`spec.txt`](../../spec.txt)): everything between a finished layout and an actual wafer that is software rather than photons. Implements the published SEMI standards — E37 (HSMS), E5 (SECS-II), E30 (GEM), and the GEM300 suite E39/E40/E90/E116 — with stream/function numbers exactly as published.

## What's inside

| Module | Standard | Contents |
|---|---|---|
| `hsms` | SEMI E37 | Length-prefixed framing, select/linktest/separate/reject control transactions, `NOT_CONNECTED → CONNECTED(NOT_SELECTED) → SELECTED` state machine, T3/T5/T6/T7/T8 + linktest timers, TCP channel with per-frame timeouts |
| `secs` | SEMI E5 | All 14 SECS-II item formats with round-trip-tested encode/decode, the shared 10-byte header, CRC-32 for recipe bodies |
| `gem` | SEMI E30 | Communication + control state models, SVID/ECID/CEID/ALID registries, alarms (S5), event reports (S6), remote commands (S2F41), terminal (S10), recipe transfer (S7) |
| `gem300` | E39/E40/E90/E116 | Object registry (create/get/destroy), process-job lifecycle with guarded transitions, substrate tracking with location-change events, E116 data-collection plans |
| `recipe` | — | Append-only version chain, CRC-checked bodies, per-parameter absolute/relative tolerance drift detection |
| `lot` | — | Lot lifecycle with CEID-annotated history (queued/started/completed/held/scrapped) |
| `adapter` | — | The **only** seam for vendor-proprietary extensions: opt-in `VendorAdapter` registry, empty by default |
| `sim` | — | A TCP-speaking passive-HSMS GEM equipment hosting the full state machine, plus a `HostClient` validation client |

## Usage

The simulator is the intended entry point — it is how fabs validate host integrations, and how this repo validates itself:

```rust
use std::time::Duration;
use tpt_fab::sim::{EquipmentSimulator, HostClient, SimulatorConfig, ceids};
use tpt_fab_litho::duv::{DuvConfig, DuvMultiPatternBackend};

let sim = EquipmentSimulator::bind(
    SimulatorConfig::default(),
    Box::new(DuvMultiPatternBackend::new(DuvConfig::default()).unwrap()),
).unwrap();
sim.stage_lot_and_job("L1", &["W1", "W2"], "PJ1", "etch-std").unwrap();
let addr = sim.spawn(); // passive HSMS listener on 127.0.0.1

let mut host = HostClient::connect(addr, 0, SimulatorConfig::default().timers).unwrap();
assert_eq!(host.establish_communications().unwrap(), 0);       // S1F13/S1F14
assert_eq!(host.go_online().unwrap(), 0);                      // S1F17/S1F18
assert_eq!(host.upload_recipe("etch-std", b"body").unwrap(), 0); // S7F3/S7F4
assert_eq!(host.remote_command("START", vec![("PJID", "PJ1")]).unwrap(), 0); // S2F41

// The E40 job runs; S6F11 event reports stream back until the lot completes.
let ev = host.next_event(Duration::from_secs(5)).unwrap();
# let _ = ev;
```

A narrated, runnable version of the full cycle:

```sh
cargo run -p tpt-fab --example full_session
```

## Design invariants

- **Technology neutrality is structural.** Core never matches on lithography technology; the simulator invokes the installed `PatterningBackend` per substrate. The milestone test `milestone_backend_swap_changes_nothing_in_core` proves swapping DUV ↔ EUV ↔ NIL ↔ e-beam changes nothing in recipe/lot/job logic.
- **Vendor quirks live outside core.** The only sanctioned seam is `adapter::VendorAdapter`, installed explicitly at runtime; with an empty registry (the default) the stack speaks pure SEMI-standard GEM.
- **Drift is a hard gate.** Before a job starts, the simulator computes effective parameters (recipe setpoints plus equipment-constant offsets) and runs drift detection; out-of-spec jobs abort with alarm ALID 9001 and the lot goes on hold.
- `#![forbid(unsafe_code)]`; the only dependency is `tpt-fab-litho` (for the backend trait).

## Milestones carried here

- *Phase 1:* a simulated equipment session completes a full recipe cycle against `tpt-fab`'s own simulator — `tests/milestone.rs`.
- *Phase 2:* backend swap requires no core changes — same file.
- Drift abort path: `tests/milestone.rs::drift_out_of_spec_aborts_job_and_raises_alarm`.

## Status

`0.1.0`, unpublished — see [`CHANGELOG.md`](./CHANGELOG.md). Source of truth: [`spec.txt`](../../spec.txt) (RFC-002). Dual-licensed MIT OR Apache-2.0.
