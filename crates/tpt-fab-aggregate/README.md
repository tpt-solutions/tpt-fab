# tpt-fab-aggregate

**The Manufacturing Outcome file exchange — schema, consent, client-side aggregation, and minimum-cohort + differential-privacy protection. Zero dependencies. Zero network code.**

The return leg of the [`tpt-fab`](../../README.md) loop (RFC-002 Section 4, [`spec.txt`](../../spec.txt)): a formal, privacy-respecting, **file-based** channel that lets any fab — wafer or PCB — send real outcome data back so the tools' simulators and yield models can be calibrated against ground truth.

## The load-bearing constraint: there is no egress path

`tpt-fab` never opens a network connection to send outcome data anywhere, on any schedule, under any configuration. [`OutcomeFileWriter`] writes to a local path and returns. There is no "offline mode" to enable, because there is no online mode to begin with.

The proof is mechanical, not documentary:

- This crate's `Cargo.toml` declares **zero dependencies** — nothing network-capable can even be linked.
- The only I/O primitives anywhere in the crate are `std::fs::write` and `std::fs::read_to_string`.
- JSON parsing/serialization, SHA-256/HMAC-SHA256, and the differential-privacy PRNG are all implemented **in this crate**, so the entire audit surface is this one directory. `grep` it for `TcpStream`, `UdpSocket`, `process::Command` — no send step exists.

Because aggregation runs *on the fab's own infrastructure* using this published code, the code is the trust mechanism: "verify it yourself, and verify there's no send step" replaces "trust our privacy policy." What happens to the resulting file — email, portal, USB drive, or nothing — is entirely the fab's decision, made after the file exists and its contents are known to them.

## Protections, in the order the pipeline applies them

1. **Signature on receipt** — HMAC-SHA256 (in-crate) over the canonical JSON; authenticates the file's *origin*, not its transport. Tamper, wrong-key, and unknown-key all reject.
2. **Schema versioning** — `SchemaVersion` on every report: recognized older minors are best-effort migrated (0.1 → 0.2 tested), anything unrecognized is *rejected*, never silently misinterpreted.
3. **Consent, enforced structurally** — `ConsentScope` (`PrivateBilateral` / `AggregatedContribution` / `NoSharing`) travels inside the report; the ingestion pipeline itself refuses anything that does not permit public aggregation. `NoSharing` reports cannot reach the public path by construction.
4. **Minimum-cohort gating** — no aggregate statistic is published until **N ≥ 5** independent contributors exist for the process/node/technology bucket; duplicate submissions from one contributor don't inflate the cohort.
5. **Differential-privacy fallback** — smaller cohorts that still want to contribute get Laplace-noised statistics with **epsilon per field sensitivity** (yield tighter than geometry; `EpsilonPolicy` is fully configurable) and a deterministic PRNG so publications are auditable.
6. **Anti-poisoning screening** — MAD-based robust z-score screening against the accumulated distribution before any report feeds a model; "too good to be true" yields and wild geometry are flagged for review, never silently absorbed.

## Usage

The fab side — build a report keyed to semantic IDs, sign it, write a file:

```rust
use tpt_fab_aggregate::outcome::{ConsentScope, ManufacturingTrack, OutcomeReport, SemanticId, WaferTech};
use tpt_fab_aggregate::signature::sign_report;
use tpt_fab_aggregate::{JsonOutcomeFile, OutcomeFileWriter};

let mut report = OutcomeReport {
    payload_manifest_id: "3f2a1b0c-1111-4222-8333-444455556666".into(),
    track: ManufacturingTrack::WaferLitho(WaferTech::Euv),
    measured_geometry: vec![],   // keyed by footprint/net IDs, never raw coordinates
    electrical_test: vec![],
    yield_outcome: tpt_fab_aggregate::outcome::YieldSummary {
        units_started: 500, units_good: 461, dominant_failure_mode: None,
    },
    process_notes: vec![],
    consent: ConsentScope::AggregatedContribution,
    schema_version: tpt_fab_aggregate::SchemaVersion::CURRENT,
    signature: None,
};
sign_report(&mut report, "fab-alpha", b"key");
JsonOutcomeFile.write_outcome_file(&report, std::path::Path::new("outcome.json")).unwrap();
// Writer returned. Sending the file is the fab's separate, deliberate act.
```

The receiver side — read the manually-delivered file, screen, aggregate, gate:

```rust
# use tpt_fab_aggregate::{AggregationEngine, BucketKey, Contribution, JsonOutcomeFile, OutcomeFileReader};
# fn run() -> Result<(), Box<dyn std::error::Error>> {
let keys = |kid: &str| (kid == "fab-alpha").then(|| b"key".to_vec());
let reader = tpt_fab_aggregate::JsonOutcomeFile::reader(
    tpt_fab_aggregate::signature::UnsignedPolicy::RequireSignature,
    std::rc::Rc::new(keys),
);
let report = reader.read_outcome_file(std::path::Path::new("outcome.json"))?; // verifies signature + schema
let bucket = BucketKey { node_nm: 130, process: "sky130-mpw".into(), tech: "euv".into() };
let mut engine = AggregationEngine::new();
engine.ingest_public_contribution(Contribution {
    contributor_id: "fab-alpha".into(),
    bucket: bucket.clone(),
    report,
})?;
// Below N=5: only DP-noised statistics; at N=5: the plain aggregate.
let publication = engine.publish(&bucket, 1).unwrap();
println!("dp_protected = {}", publication.dp_protected);
# Ok(())
# }
# run().unwrap();
```

**Both manufacturing tracks use this one schema.** The wafer track is wired through `tpt-fab`'s simulator and the sky130 shuttle path; the PCB track (`tpt-silicon-cam`, RFC-001) implements the same contract — see the reference integration:

```sh
cargo run -p tpt-fab-aggregate --example pcb_track
```

## Module map

| Module | Role |
|---|---|
| `outcome` | `OutcomeReport` + subtypes, canonical JSON (de)serialization, semantic-ID types |
| `schema` | `SchemaVersion`, migration-or-reject semantics |
| `file` | `OutcomeFileWriter` / `OutcomeFileReader` traits + the canonical JSON implementation |
| `signature`, `hash` | HMAC-SHA256 origin signatures; SHA-256/HMAC implemented in-crate (published test vectors) |
| `aggregate` | Bucketed `AggregationEngine`, N ≥ 5 gate, DP fallback publication |
| `privacy` | Per-sensitivity epsilon policy, Laplace mechanism, deterministic SplitMix64 |
| `anomaly` | MAD-based robust screening + basic sanity validation |
| `incentive` | `EntitlementLedger`: verified contribution unlocks shift-left DRC/CAM + yield-heatmap access; `NoSharing` earns nothing |
| `dataset` | Attribution-only, aggregate-only release artifact for the dedicated `tpt-fab-data` repo (CC-BY-4.0 marker) |
| `json` | The minimal in-crate JSON stack (insertion-ordered objects → deterministic canonical bytes) |

## Status

`0.1.0`, unpublished — see [`CHANGELOG.md`](./CHANGELOG.md). Source of truth: [`spec.txt`](../../spec.txt) (RFC-002). Dual-licensed MIT OR Apache-2.0.
