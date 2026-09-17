# Changelog

All notable changes to the `tpt-fab-intake` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `IntakeQueue`: per-file intake pipeline — signature verification, schema
  validation (migrate-or-reject), consent enforcement (`NoSharing` refused),
  and MAD-based anomaly screening — producing typed `IntakeDecision`s
  (accepted / flagged / rejected with reasons).
- `IntakeLedger`: JSON persistence for decision records and the
  anomaly-screening history, saved after every file; `IntakeQueue::from_ledger`
  resumes screening with the accumulated distributions so history spans runs.
- `process_directory`: sorted batch processing of `*.json` deliveries, moving
  files into `accepted/` / `flagged/` / `rejected/` (collision-safe `-dupN`
  names, cross-device move fallback) while leaving non-JSON files untouched.
- `tpt-fab-intake` CLI: `--incoming`, `--root`, `--ledger`, `--keys`
  (hex-key JSON), `--unsigned accept|reject`, `--z-threshold`, with a
  per-file disposition summary on completion.
- Example `intake_api` demonstrating the library API over a scratch directory.
- Entirely local operation: no egress path, no daemon, no schedule — the crate
  depends only on the zero-dependency `tpt-fab-aggregate`.
