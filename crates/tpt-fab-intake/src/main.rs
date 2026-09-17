//! `tpt-fab-intake` — the semi-automated intake queue CLI.
//!
//! One invocation processes every `*.json` in the incoming directory: signature-verify,
//! schema-check, consent-enforce, anomaly-screen, then file each report into
//! `accepted/`, `flagged/`, or `rejected/` and record the decision in the ledger.
//! Entirely local — the tool never sends anything anywhere; it only sorts files that a
//! human already chose to deliver.
//!
//! ```text
//! tpt-fab-intake --incoming ./incoming --root ./intake \
//!     [--ledger ./intake/ledger.json] [--keys ./keys.json] \
//!     [--unsigned accept|reject] [--z-threshold 4.0]
//! ```
//!
//! Keys file format: `{"key-id": "hex-encoded-key"}`.

use std::path::PathBuf;
use std::rc::Rc;

use tpt_fab_aggregate::json::Json;
use tpt_fab_aggregate::signature::UnsignedPolicy;
use tpt_fab_aggregate::AggregateError;
use tpt_fab_intake::{IntakeConfig, IntakeLedger, IntakeQueue};

struct Args {
    incoming: PathBuf,
    root: PathBuf,
    ledger: Option<PathBuf>,
    keys: Option<PathBuf>,
    unsigned: UnsignedPolicy,
    z_threshold: f64,
}

fn usage() -> &'static str {
    "usage: tpt-fab-intake --incoming DIR --root DIR [options]\n\
     \x20 --incoming DIR   directory of manually-delivered *.json outcome files\n\
     \x20 --root DIR       intake root; dispositions land in root/{accepted,flagged,rejected}\n\
     \x20 --ledger FILE    decision ledger (default: root/ledger.json)\n\
     \x20 --keys FILE      JSON object {\"key-id\": \"hex-key\"} for signature verification\n\
     \x20 --unsigned MODE  accept | reject   (default: reject)\n\
     \x20 --z-threshold N  anomaly flag threshold in MADs (default: 4)"
}

fn parse_args() -> Option<Args> {
    let mut incoming = None;
    let mut root = None;
    let mut ledger = None;
    let mut keys = None;
    let mut unsigned = UnsignedPolicy::RequireSignature;
    let mut z_threshold = 4.0;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || it.next().unwrap_or_default();
        match a.as_str() {
            "--incoming" => incoming = Some(PathBuf::from(value())),
            "--root" => root = Some(PathBuf::from(value())),
            "--ledger" => ledger = Some(PathBuf::from(value())),
            "--keys" => keys = Some(PathBuf::from(value())),
            "--unsigned" => match value().as_str() {
                "accept" => unsigned = UnsignedPolicy::AcceptUnsigned,
                "reject" => unsigned = UnsignedPolicy::RequireSignature,
                other => {
                    eprintln!("unknown --unsigned mode '{other}'");
                    return None;
                }
            },
            "--z-threshold" => {
                z_threshold = value().parse().ok()?;
            }
            "--help" | "-h" => return None,
            other => {
                eprintln!("unknown argument '{other}'");
                return None;
            }
        }
    }
    Some(Args { incoming: incoming?, root: root?, ledger, keys, unsigned, z_threshold })
}

fn load_keys(path: &PathBuf) -> Result<Vec<(String, Vec<u8>)>, AggregateError> {
    let text = std::fs::read_to_string(path)?;
    let v = Json::parse(&text)?;
    let mut keys = Vec::new();
    if let Json::Obj(entries) = v {
        for (kid, hex_value) in entries {
            let hex_str = hex_value
                .as_str()
                .ok_or_else(|| AggregateError::InvalidInput(format!("key '{kid}' not a string")))?;
            let bytes = tpt_fab_aggregate::hash::unhex(hex_str)
                .map_err(|e| AggregateError::InvalidInput(format!("key '{kid}': {e}")))?;
            keys.push((kid, bytes));
        }
    }
    Ok(keys)
}

fn main() {
    let Some(args) = parse_args() else {
        eprintln!("{}", usage());
        std::process::exit(2);
    };

    let keys: Vec<(String, Vec<u8>)> = match &args.keys {
        Some(p) => match load_keys(p) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("error loading keys: {e}");
                std::process::exit(2);
            }
        },
        None => {
            eprintln!(
                "note: no --keys file given; signed files will be rejected as unknown-key\n\
                 (pilot fabs without keys need --unsigned accept)"
            );
            Vec::new()
        }
    };

    let key_of = Rc::new(move |kid: &str| {
        keys.iter().find(|(id, _)| id == kid).map(|(_, bytes)| bytes.clone())
    });
    let config = IntakeConfig {
        unsigned_policy: args.unsigned,
        key_of,
        z_threshold: args.z_threshold,
        min_history: 8,
    };

    let ledger_path = args.ledger.clone().unwrap_or_else(|| args.root.join("ledger.json"));
    let mut ledger = match IntakeLedger::load(&ledger_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error loading ledger {}: {e}", ledger_path.display());
            std::process::exit(2);
        }
    };

    let mut queue = IntakeQueue::from_ledger(config, &ledger);

    if !args.incoming.exists() {
        eprintln!("incoming directory {} does not exist", args.incoming.display());
        std::process::exit(2);
    }
    let decisions =
        match queue.process_directory(&args.incoming, &args.root, &mut ledger, &ledger_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("intake failed: {e}");
                std::process::exit(1);
            }
        };

    if decisions.is_empty() {
        println!("no outcome files in {}", args.incoming.display());
        return;
    }

    let (mut accepted, mut flagged, mut rejected) = (0usize, 0usize, 0usize);
    for d in &decisions {
        match d.disposition {
            tpt_fab_intake::Disposition::Accepted => accepted += 1,
            tpt_fab_intake::Disposition::Flagged => flagged += 1,
            tpt_fab_intake::Disposition::Rejected => rejected += 1,
        }
        println!("{:>8}  {}  {}", d.disposition.dir_name(), d.file, d.reason);
    }
    println!(
        "{} processed: {accepted} accepted, {flagged} flagged, {rejected} rejected; ledger: {}",
        decisions.len(),
        ledger_path.display()
    );
    println!(
        "screening history: {} yield / {} geometry / {} electrical observations",
        ledger.history_yield.len(),
        ledger.history_geometry.len(),
        ledger.history_electrical.len()
    );
}
