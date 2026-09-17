# Changelog

All notable changes to the `tpt-fab-process` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- OPC suggestion engine keyed to the active `PatterningBackend`'s illumination:
  k1-scaled dense-line bias, line-end pullback, corner serifs, via bias, and
  isolated-feature bias, each with a review-facing rationale string. NIL and
  e-beam produce no optical suggestions (documented why, with the equivalent
  control named).
- Coarse process simulation across a five-site wafer map: etch depth + post-etch
  CD windows, deposition thickness window, cumulative thermal budget — all
  returning typed violations with signed margins.
- Explicit non-goal documentation: not a TCAD replacement; the target is
  "catches what mature-node/emerging fabs currently catch with nothing."
- sky130 MPW shuttle-result ingestion: JSON parser, semantic-ID-keyed
  `OutcomeReport` conversion with deterministic manifest IDs, and ingestion
  through the cohort-protected `tpt-fab-aggregate` pipeline (consent, N >= 5
  gate, DP fallback all apply from day one).
- Milestone tests: a DRC-clean sky130-style layer still flagged by OPC (line-end
  + corner classes) and the etch window check; identical lot lifecycle across
  backend swaps; shuttle results gating correctly at the cohort threshold.
- Example `sky130_ingest` walking the shuttle path through the cohort gate.
