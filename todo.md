# tpt-fab — TODO

**Owner:** TPT Solutions
**License:** MIT OR Apache-2.0 (dual, matching `tpt-telos` / `tpt-protocol`)
**Scope:** RFC-002 — wafer-fabrication SECS/GEM control & process-simulation stack, plus the
bidirectional Manufacturing Outcome file exchange shared with the RFC-001 PCB track. Source of
truth: [`spec.txt`](./spec.txt). Repo is standalone (`tpt-solutions/tpt-fab`), no shared Cargo
workspace with `tpt-silicon` / `tpt-protocol` / `tpt-telos`. Cross-cutting deps: `tpt-telos`,
`tpt-ai`. Consumes output artifacts from `tpt-silicon` and (per RFC-001) `tpt-silicon-cam`.

**Status (2026-09-16):** Phases 1–4 implemented — all four crates in place, **103 tests green**,
CI gates (`fmt --check`, `clippy -D warnings`, `test`) clean. Remaining items below are the ones
genuinely gated on external parties (Phase 5, real MPW silicon, the PCB-repo wiring) or on real
contribution volume (dataset publication).

---

## Repo bootstrap (done)

- [x] `git init`
- [x] `LICENSE-MIT` + `LICENSE-APACHE` at root
- [x] Root `Cargo.toml` workspace — `resolver = "2"`, `edition = "2021"`, `rust-version` pin
      (1.75), members under `crates/`
- [x] `crates/` layout: `crates/tpt-fab`, `crates/tpt-fab-litho`, `crates/tpt-fab-process`,
      `crates/tpt-fab-aggregate`
- [x] `[workspace.lints.clippy]` in root `Cargo.toml` (mirror `tpt-silicon`'s `all = "warn"`)
- [x] `rustfmt.toml` (mirror `tpt-silicon`'s)
- [x] `README.md` — project overview, positioning statement (Section 1), links to `spec.txt`
- [x] CI: `.github/workflows/ci.yml` (fmt/clippy/test), mirroring `tpt-protocol`/`tpt-telos`
- [x] `AGENTS.md` / `CLAUDE.md` agent-facing dev docs (sibling-repo convention)

---

## Phase 1 — `tpt-fab` core (done — 29 unit tests + milestone tests)

- [x] HSMS message-transport layer (TCP-based, per SEMI E37) — framing, select/linktest/separate,
      state machine, T3/T5/T6/T7/T8 timers (`crates/tpt-fab/src/hsms.rs`)
- [x] SECS-II message encode/decode (per SEMI E5) — all item formats, 10-byte header, CRC-32
      (`crates/tpt-fab/src/secs.rs`)
- [x] GEM state model & scenarios (per SEMI E30) — communication + control state models,
      SVID/ECID/CEID/ALID registries, alarms, terminal, S1/S2/S5/S6/S7/S10 handling
      (`crates/tpt-fab/src/gem.rs`)
- [x] GEM300 suite support: E39 (object services), E40 (processing management), E90 (substrate
      tracking), E116 (equipment performance) (`crates/tpt-fab/src/gem300.rs`)
- [x] Recipe data model with versioning — append-only version chain, body CRC, tolerance model
      (`crates/tpt-fab/src/recipe.rs`)
- [x] Recipe drift-detection logic — per-parameter absolute/relative tolerances, in-spec /
      tolerance / out-of-spec classification; wired into the simulator's START path
- [x] Lot tracking data model — CEID-annotated state history (`crates/tpt-fab/src/lot.rs`)
- [x] Equipment-simulator harness (end-to-end validation without real tool access) —
      passive-HSMS TCP equipment + `HostClient` validation client
      (`crates/tpt-fab/src/sim.rs`)
- [x] Explicitly exclude vendor-proprietary SECS/GEM extensions from core; design an opt-in
      adapter seam for later — `crates/tpt-fab/src/adapter.rs`, empty by default
- [x] **Milestone:** a simulated equipment session completes a full recipe cycle against
      `tpt-fab`'s own simulator, no real hardware involved —
      `crates/tpt-fab/tests/milestone.rs::milestone_simulated_session_completes_full_recipe_cycle`
      (select → establish comms → online → S7F3 recipe upload → S2F41 START → E40 job lifecycle
      → E90 substrate moves → lot completed; plus a drift-abort test proving the guard works)

## Phase 2 — `tpt-fab-litho` backends (done — 8 tests + swap milestone)

- [x] Define `PatterningBackend` trait — thin: `tech`/`name`/`validated_against_real_hardware`/
      `plan_exposure`/`health` (`crates/tpt-fab-litho/src/backend.rs`)
- [x] `DuvMultiPatternBackend` — generic DUV multi-patterning first (published process
      characteristics; SMEE-style domestic ArF immersion parameters deferred to an optional
      profile once a partner/validation opportunity exists — see Decisions). LELE/LELELE/SADP
      schemes, RSS overlay budgeting (`src/duv.rs`)
- [x] `EuvBackend` — projection-optics exposure sequencing, pellicle/dose bookkeeping
      (`src/euv.rs`)
- [x] `NilBackend` — J-FIL-style imprint/separation cycle control, template defect tracking
      (`src/nil.rs`)
- [x] `EbeamBackend` — vector-scan control (mask writing / low-volume direct-write)
      (`src/ebeam.rs`)
- [x] Each backend flagged in its own docs as unvalidated-against-real-hardware until a design
      partner adopts it — crate docs + `validated_against_real_hardware() = false` with a test
      asserting it stays false
- [x] **Milestone:** swapping the backend behind `tpt-fab`'s core requires no changes to
      recipe/lot/APC logic —
      `crates/tpt-fab/tests/milestone.rs::milestone_backend_swap_changes_nothing_in_core`
      (identical host-side script against all four backends → identical lifecycle outcomes)

## Phase 3 — `tpt-fab-process` simulation (done — 13 unit tests + milestone tests)

- [x] OPC (optical proximity correction) mask-correction suggestion engine, keyed to active
      `PatterningBackend` — per-feature bias rules (dense-line undersizing, line-end pullback,
      corner rounding, via bias) driven by the backend's illumination; NIL/E-beam correctly
      produce no optical suggestions (`crates/tpt-fab-process/src/opc.rs`)
- [x] Coarse etch/deposition/thermal process simulator flagging process-window violations —
      five-site wafer map, depth/CD/thickness windows, cumulative thermal budget
      (`crates/tpt-fab-process/src/window.rs`)
- [x] Explicit non-goal documentation in crate docs: not a TCAD replacement; target is "catches
      what mature-node/emerging fabs currently catch with nothing," not Sentaurus-level accuracy
- [x] Ingestion path for efabless/Skywater-style sky130 MPW shuttle results (build from day one
      per spec Section 2C / 4.4) — JSON shuttle-result parser → `OutcomeReport` conversion with
      semantic-ID keying → ingestion through the full aggregate pipeline
      (`crates/tpt-fab-process/src/sky130.rs`)
- [x] **Milestone:** a sky130 design run through the simulator flags at least one class of issue
      plain DRC would miss —
      `crates/tpt-fab-process/tests/milestone.rs::milestone_simulator_flags_what_plain_drc_misses`
      (DRC-clean layer still gets line-end + corner OPC flags and etch-window violations)

## Phase 4 — Bidirectional outcome file exchange (implemented; two items pending external wiring)

- [x] `OutcomeReport` struct (payload_manifest_id, track, measured_geometry, electrical_test,
      yield_outcome, process_notes, consent, schema_version, signature) — keyed to shared semantic
      IDs (`FabricLink.id`, footprint IDs, net names, recipe/lot IDs), not raw coordinates
      (`crates/tpt-fab-aggregate/src/outcome.rs`; canonical JSON serialization)
- [x] `OutcomeFileWriter` trait — file-out only, runs on fab infrastructure
      (`crates/tpt-fab-aggregate/src/file.rs`)
- [x] `OutcomeFileReader` trait — runs on `tpt-solutions` side against a manually-delivered file
      (reader enforces signature policy on receipt)
- [x] `ConsentScope` enum (`PrivateBilateral` / `AggregatedContribution` / `NoSharing`), enforced
      structurally by the ingestion pipeline — `ingest_public_contribution` refuses anything
      that does not permit public aggregation; tested from `NoSharing` reports
- [x] `tpt-fab-aggregate` crate: client-side aggregation/anonymization, published & auditable —
      **acceptance criterion met: zero dependencies in `Cargo.toml` (not merely
      network-free ones), no egress code path at all** (only `std::fs` read/write; JSON,
      SHA-256/HMAC, and the DP PRNG are all implemented in-crate so the audit surface is this
      one directory)
- [x] Minimum-cohort gating: withhold aggregate publication until **N ≥ 5** independent
      contributors per process/node/technology bucket (see Decisions) —
      `DEFAULT_MIN_COHORT = 5`, distinct-contributor counting (duplicate submissions don't
      inflate the cohort), DP-protected publication below the gate
- [x] Differential-privacy noise injection for smaller cohorts, **epsilon configurable per field
      sensitivity** — tighter (noisier) for yield/business-sensitive fields, looser for geometry
      deviations (see Decisions) — Laplace mechanism, `EpsilonPolicy` with
      Geometry/Yield/Process classes, deterministic SplitMix64 so publications are auditable
      (`crates/tpt-fab-aggregate/src/privacy.rs`)
- [x] Apply minimum-cohort + DP protection to the sky130 shuttle-run ingestion path too — the
      shuttle path ingests through the same `AggregationEngine`, so consent/cohort/DP apply
      unchanged (tested)
- [x] `SchemaVersion` semver field on both `SmpPayload` and `OutcomeReport`; ingestion pipeline
      rejects or best-effort-migrates unrecognized minor versions, never silently misinterprets
      changed fields — `OutcomeReport` done here (0.1→0.2 migration tested; 0.3+/1.x rejected);
      `SmpPayload` is RFC-001's type in `tpt-silicon-cam`, which consumes this same crate's
      `SchemaVersion` when that wiring happens
- [x] Anomaly/anti-poisoning detection against the existing aggregate distribution before any
      report feeds a model (`tpt-ai` yield heatmap, `tpt-silicon-si-pi` solver calibration,
      `tpt-fab-process` OPC/etch corrections) — MAD-based robust z-screening with
      minimum-history rule; "too good to be true" yields and wild geometry flagged for review,
      never silently absorbed (`crates/tpt-fab-aggregate/src/anomaly.rs`)
- [x] `Signature` verification on receipt (authenticates file origin, not transport) —
      HMAC-SHA256 over canonical JSON (signature field excluded), in-crate implementation with
      published test vectors; tamper/wrong-key/unknown-key all rejected
- [x] Incentive wiring: contribution (even `AggregatedContribution`) unlocks RFC-001 §5 shift-left
      DRC/CAM turnaround + `tpt-ai` yield-heatmap access — `EntitlementLedger`; `NoSharing`
      earns nothing (`crates/tpt-fab-aggregate/src/incentive.rs`)
- [ ] Wire into `tpt-silicon-cam` (PCB track) and `tpt-fab` (wafer track) simultaneously against the
      one shared schema — **wafer track wired here** (`tpt-fab` simulator outcomes and the
      sky130 shuttle path both produce/ingest this schema); the PCB-track wiring lands in the
      `tpt-silicon-cam` repository against this same shared schema (separate repo, separate RFC)
- [ ] Open aggregated dataset release — publish to a dedicated **`tpt-fab-data`** repo (see
      Decisions), with attribution to opted-in contributing fabs, no raw data — the release
      artifact is implemented and tested (`dataset.rs`: attribution-only, aggregate-only JSON,
      CC-BY-4.0 marker); the actual publication waits for real data and the repo's creation
- [ ] **Milestone:** a sky130 MPW shuttle-run result round-trips end to end — design exported, real
      silicon returned, outcome file generated locally, emailed, manually ingested — without a
      design partner — **the software path is complete and tested** (file → signature verify →
      screen → aggregate → gated publication → release artifact, in
      `tpt-fab-aggregate`'s e2e test); the milestone itself additionally needs a real shuttle
      submission to come back from the foundry

## Phase 5 — Design partner pilot (Month 6+)

- [ ] Approach a mature-node or NIL-adjacent fab (per Section 1 positioning: not tier-one
      toolmakers) to pilot the file exchange beyond the sky130 shuttle-run path
      — blocked on an external yes; not scheduled tighter than "when Phase 4 is solid"

---

## Decisions (spec.txt Section 6, resolved)

- **Minimum cohort size N = 5** — adopted from the spec's own placeholder as the starting value;
  revisit once real contribution volume exists rather than treating it as still-open.
  *Implemented as `DEFAULT_MIN_COHORT = 5` in `tpt-fab-aggregate`.*
- **Differential-privacy epsilon varies by field sensitivity** — yield numbers (business-sensitive)
  get a tighter epsilon than geometry deviations. Phase 4 includes a per-field epsilon config task,
  not a single global epsilon. *Implemented as `EpsilonPolicy` with Geometry/Yield/Process classes.*
- **Open dataset home = dedicated `tpt-fab-data` repo** — keeps large/versioned dataset artifacts
  out of the code repo's git history, same org/licensing umbrella. *`dataset.rs` emits exactly
  that repo's release artifact.*
- **DUV backend: generic-first** — build `DuvMultiPatternBackend` against published generic DUV
  multi-patterning characteristics first; add SMEE-style domestic ArF immersion parameters as an
  optional profile later, once a partner or validation opportunity around domestic tooling exists.
  Avoids over-fitting to one program before any real hardware validation. *`DuvProfile::GenericArFi`
  is the only profile; the enum is the future seam.*

## Still genuinely open (not resolved, flagged for later)

- [ ] Operational cost of manual signature verification / file ingestion at `tpt-solutions` as
      contribution volume grows — design a semi-automated intake queue once volume exceeds a
      handful of fabs/month. Operations question, not a technical blocker for Phases 1–4.
