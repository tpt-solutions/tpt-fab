# tpt-fab-intake

**Semi-automated local intake queue for manually-delivered Manufacturing Outcome files — the receiver-side answer to "someone at `tpt-solutions` has to receive it, verify the signature, and run it through `OutcomeFileReader` by hand."**

Part of the [`tpt-fab`](../../README.md) stack (RFC-002, [`spec.txt`](../../spec.txt)). This crate automates the *mechanical* half of outcome-file intake while keeping every judgment human.

## What it does per file

1. **Verifies the signature** (HMAC-SHA256, `tpt-fab-aggregate`) — the file's *origin*, not its transport. Tampered, wrong-key, and unknown-key files are rejected with the reason recorded.
2. **Validates the schema version** — migrate-or-reject; an unknown minor or major version is never silently misinterpreted.
3. **Enforces consent** — a `NoSharing` report parses but is *refused*: the receiver must not process it. `PrivateBilateral` and `AggregatedContribution` proceed (recorded in the ledger so downstream humans know what they may do with the data).
4. **Screens for anomalies** — MAD-based robust z-screening against the *accumulated* distribution; the screening history persists in the ledger, so this month's files are screened against everything ever seen, not just this batch. Flagged files are parked for review, never auto-fed to a model.

Each file lands in `accepted/`, `flagged/`, or `rejected/`, and every decision is appended to a JSON **ledger** (decisions + screening history) saved after every file, so a run can be interrupted and resumed without losing bookkeeping.

## What it does not do

- **No egress, ever.** Like the rest of this stack: local files in, local files out, nothing sent anywhere, no daemon, no schedule, no network code. The tool only sorts files a human already chose to deliver.
- **No value judgments.** Accepted and flagged files still wait for a human to decide about ingestion, aggregation, and response. The queue guarantees that whatever reaches them is authentic, current-schema, consent-checked, and pre-screened.

## Usage

```sh
# First run (keys file: {"fab-alpha": "6865782d6b6579"}):
tpt-fab-intake --incoming ./incoming --root ./intake --keys ./keys.json

# A pilot fab without a key yet:
tpt-fab-intake --incoming ./incoming --root ./intake --unsigned accept

# After processing:
#   intake/accepted/…   — clean, waiting for a human decision
#   intake/flagged/…    — anomalous, waiting for review
#   intake/rejected/…   — bad signature / unsupported schema / NoSharing consent
#   intake/ledger.json  — every decision + the screening history
```

Library use (the CLI is a thin wrapper over this):

```rust
use std::rc::Rc;
use tpt_fab_intake::{IntakeConfig, IntakeQueue};

let config = IntakeConfig {
    unsigned_policy: tpt_fab_aggregate::signature::UnsignedPolicy::RequireSignature,
    key_of: Rc::new(|kid: &str| (kid == "fab-alpha").then(|| b"key".to_vec())),
    z_threshold: 4.0,
    min_history: 8,
};
let mut queue = IntakeQueue::new(config);
let decision = queue.process_file(std::path::Path::new("outcome.json"));
println!("{} -> {:?}", decision.file, decision.disposition);
```

Runnable tour:

```sh
cargo run -p tpt-fab-intake --example intake_api
```

## Design notes

- **History persistence is the point.** Anti-poisoning screening (`tpt-fab-aggregate::anomaly`) is only as good as its baseline; [`IntakeLedger`] persists the yield/geometry/electrical distributions across runs and [`IntakeQueue::from_ledger`] resumes with them, so a fresh process screens immediately instead of re-warming.
- **Nothing is deleted.** Files move; collisions get `-dupN` suffixes; non-`*.json` files in the incoming directory are left untouched.
- **Depends only on `tpt-fab-aggregate`** (which is dependency-free). `#![forbid(unsafe_code)]`.

## Status

`0.1.0`, unpublished — see [`CHANGELOG.md`](./CHANGELOG.md). Source of truth: [`spec.txt`](../../spec.txt) (RFC-002). Dual-licensed MIT OR Apache-2.0.
