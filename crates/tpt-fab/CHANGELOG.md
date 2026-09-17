# Changelog

All notable changes to the `tpt-fab` crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `hsms` (SEMI E37): length-prefixed framing, select/linktest/separate/reject
  control messages, `NOT_CONNECTED -> CONNECTED(NOT_SELECTED) -> SELECTED`
  state machine, T3/T5/T6/T7/T8 + linktest timers, TCP channel with idle and
  inter-byte timeouts.
- `secs` (SEMI E5): all 14 SECS-II item formats with round-trip-tested
  encode/decode, the 10-byte HSMS-shared header, CRC-32 for recipe bodies.
- `gem` (SEMI E30): communication and control state models, SVID/ECID/CEID/ALID
  registries, alarms (S5F1), event reports (S6F11), establish-communications
  (S1F13/S1F14), online/offline (S1F15-S1F18), status (S1F3/S1F4), equipment
  constants (S2F13-S2F16), date/time (S2F31/S2F32), remote commands
  (S2F41/S2F42), recipe transfer (S7F1-S7F6), terminal (S10F3/S10F4); side
  effects surface as typed `GemAction`s.
- `gem300`: E39 object registry (create/get/destroy), E40 process jobs with
  guarded lifecycle transitions and CEIDs, E90 substrate tracking with
  location-change events, E116 data-collection plans and trace samples.
- `recipe`: append-only `Recipe` version chain, CRC-checked bodies, per-parameter
  absolute/relative tolerance drift detection with in-spec / tolerance /
  out-of-spec classification.
- `lot`: lot lifecycle with CEID-annotated history and guarded transitions.
- `adapter`: the single opt-in seam for vendor-proprietary SECS/GEM extensions
  (`VendorAdapter` registry, empty by default; core stays pure-standard).
- `sim`: passive-HSMS TCP equipment simulator hosting the full GEM/GEM300/
  recipe/lot state (one lock, scenario staging helpers, drift-gated START path,
  event-report stream) plus a `HostClient` validation client with typed helpers.
- Milestone tests: full simulated recipe cycle, backend-swap invariance (all
  four `tpt-fab-litho` backends produce identical core outcomes), end-state
  verification, drift-abort with alarm.
- Example `full_session` running the narrated end-to-end cycle over TCP.
