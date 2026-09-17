# Changelog

All notable changes to the `tpt-fab-aggregate` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `OutcomeReport` schema shared by the PCB track (RFC-001 `tpt-silicon-cam`)
  and the wafer track (`tpt-fab`), keyed to semantic IDs (never raw
  coordinates) with canonical, insertion-ordered JSON serialization.
- `ConsentScope` (`PrivateBilateral` / `AggregatedContribution` / `NoSharing`)
  traveling inside the report and enforced structurally by ingestion.
- `OutcomeFileWriter` / `OutcomeFileReader` traits (file-out / file-in only)
  with a canonical JSON implementation and signature policy on receipt.
- Minimum-cohort gating: no aggregate publication below N >= 5 independent
  contributors per process/node/technology bucket (`DEFAULT_MIN_COHORT`);
  duplicate submissions count once.
- Differential-privacy fallback for smaller cohorts: Laplace mechanism with
  per-field-sensitivity epsilon (`EpsilonPolicy`: yield tighter than geometry
  than process) and a deterministic SplitMix64 PRNG for auditability.
- `SchemaVersion` with migration-or-reject semantics: 0.1 -> 0.2 migration
  implemented and tested; unrecognized minors and major bumps rejected.
- Anomaly/anti-poisoning screening: MAD-based robust z-scores with a
  minimum-history rule, flagging (never silently absorbing) outliers, plus
  basic sanity validation (impossible yields rejected).
- HMAC-SHA256 origin signatures over canonical JSON, implemented in-crate
  (SHA-256 + HMAC with published test vectors); tamper, wrong-key, and
  unknown-key all reject. Signatures authenticate file origin, not transport.
- `EntitlementLedger` incentive wiring: verified contributions unlock
  shift-left DRC/CAM + yield-heatmap entitlements; `NoSharing` earns nothing.
- `DatasetRelease` builder: the attribution-only, aggregate-only artifact for
  the dedicated `tpt-fab-data` repo (CC-BY-4.0 marker, no raw data fields).
- Zero-dependency, zero-egress acceptance criterion: empty `[dependencies]`,
  `std::fs` the only I/O, JSON + hashing + PRNG implemented in-crate.
- End-to-end test: signed file -> verify -> screen -> aggregate -> gated
  publication -> release artifact.
- Example `pcb_track`: the PCB-track (RFC-001) reference integration against
  the one shared schema, including the N >= 5 gate opening on the fifth
  contributor.
