# tpt-fab

Wafer-fabrication SECS/GEM control & process-simulation stack, plus the bidirectional
Manufacturing Outcome file exchange shared with the RFC-001 PCB track.

**Status:** proposed/under construction — source of truth is [`spec.txt`](./spec.txt) (RFC-002),
build progress in [`TODO.md`](./TODO.md).

## What this is

`tpt-fab` covers two things neither `tpt-silicon` (RTL→GDSII) nor RFC-001
(PCB/system co-design) addresses:

1. **The wafer-fabrication layer** — the SECS/GEM-speaking (SEMI E4/E5/E30/E37/E39/E40/E90/E116,
   the GEM300 suite), lithography-technology-agnostic equipment-control and process-simulation
   software that sits between a finished layout and an actual wafer.
2. **A bidirectional manufacturing feedback loop** — a formal, privacy-respecting, *file-based*
   return channel that lets any fab (wafer or PCB) send real outcome data back, so the tools'
   simulators and yield models can be calibrated against ground truth — without `tpt-solutions`
   ever owning fab hardware, and without any network egress in this codebase at all.

## Positioning

This is not aimed at ASML, Applied Materials, or the other tier-one toolmakers, whose control
software is safety-certified and IP-locked around hardware we will never get API access to. It is
aimed at the tier below — NIL vendors and domestic litho programs still building their software
stack from scratch, and the long tail of mature-node, analog, power, and specialty fabs who
currently have nothing between "nothing" and enterprise EDA/MES licensing. The goal is to be good
enough to matter to them, not to match a $10M TCAD license.

## Crates

| Crate | Role |
|---|---|
| [`tpt-fab`](./crates/tpt-fab) | Core: HSMS/SECS-II/GEM300 message stack, recipe versioning + drift detection, lot tracking, equipment-simulator harness, opt-in vendor adapter seam |
| [`tpt-fab-litho`](./crates/tpt-fab-litho) | Pluggable `PatterningBackend` trait: DUV multi-patterning, EUV, nanoimprint (J-FIL-style), e-beam |
| [`tpt-fab-process`](./crates/tpt-fab-process) | Explicitly best-effort OPC suggestions + coarse etch/deposition/thermal simulation; sky130 MPW shuttle-run ingestion path |
| [`tpt-fab-aggregate`](./crates/tpt-fab-aggregate) | The Manufacturing Outcome file exchange: schema, consent, client-side aggregation, minimum-cohort + differential-privacy protection. **Zero dependencies, zero network code — auditable by construction.** |

## The outcome file exchange is a file exchange, not a service

Nothing here opens a network connection to send outcome data anywhere, on any schedule, under any
configuration. `OutcomeFileWriter` writes to a local path and returns. What happens next — email,
portal upload, USB drive, or nothing — is entirely the fab's decision, made after the file exists
and its contents are already known to them. Every file that reaches `tpt-solutions` reached it
because a human at the fab decided to send it. There is no "offline mode" to enable, because there
is no online mode to begin with.

## License

Dual-licensed, matching `tpt-telos` / `tpt-protocol`: [MIT](./LICENSE-MIT) OR
[Apache-2.0](./LICENSE-APACHE).
