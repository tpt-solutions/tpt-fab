//! # tpt-fab-intake
//!
//! The semi-automated intake queue for manually-delivered Manufacturing Outcome files
//! (spec Section 6's operational question: "someone at `tpt-solutions` has to receive it,
//! verify the signature, and run it through `OutcomeFileReader` by hand").
//!
//! The queue replaces the *mechanical* half of that manual work while keeping every judgment
//! human: point it at a directory of files that arrived out of band and it will, per file —
//!
//! 1. verify the signature ([`tpt_fab_aggregate::signature`], origin not transport),
//! 2. validate the schema version (migrate-or-reject, never guess),
//! 3. enforce consent (`NoSharing` reports are refused — the receiver must not process them),
//! 4. screen for anomalies against the *accumulated* distribution (history persists in the
//!    ledger across runs), flagging outliers for review rather than absorbing them,
//!
//! and move the file to `accepted/`, `flagged/`, or `rejected/`, recording every decision in
//! a JSON ledger next to them.
//!
//! It runs **entirely on local files** — like the rest of this stack there is no egress path,
//! no daemon, no schedule: one invocation, one directory, done. The human still decides what
//! happens to accepted and flagged files; the queue only guarantees that whatever reaches
//! them is authentic, current-schema, consent-checked, and pre-screened.
//!
//! Library use:
//!
//! ```no_run
//! use std::rc::Rc;
//! use tpt_fab_intake::{IntakeConfig, IntakeQueue};
//!
//! let keys = |kid: &str| (kid == "fab-alpha").then(|| b"key".to_vec());
//! let config = IntakeConfig {
//!     unsigned_policy: tpt_fab_aggregate::signature::UnsignedPolicy::RequireSignature,
//!     key_of: Rc::new(keys),
//!     z_threshold: 4.0,
//!     min_history: 8,
//! };
//! let mut queue = IntakeQueue::new(config);
//! let decision = queue.process_file(std::path::Path::new("outcome.json"));
//! println!("{} -> {:?}", decision.file, decision.disposition);
//! ```

#![forbid(unsafe_code)]

pub mod ledger;

pub use ledger::{DecisionRecord, IntakeLedger};

use std::path::Path;

use tpt_fab_aggregate::anomaly::{
    mean_abs_geometry_deviation_um, mean_electrical_ratio, validate_report_basics, AnomalyDetector,
};
use tpt_fab_aggregate::file::{JsonOutcomeFile, KeyLookup, OutcomeFileReader};
use tpt_fab_aggregate::outcome::ConsentScope;
use tpt_fab_aggregate::signature::UnsignedPolicy;

/// Queue configuration.
#[derive(Clone)]
pub struct IntakeConfig {
    /// What to do with unsigned files (pilot fabs may not have keys yet).
    pub unsigned_policy: UnsignedPolicy,
    /// Signature key lookup by key ID.
    pub key_of: KeyLookup,
    /// Anomaly flag threshold in MADs.
    pub z_threshold: f64,
    /// Minimum screening history before flagging is possible.
    pub min_history: usize,
}

/// What happened to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Authentic, current schema, consent-checked, unremarkable — passed to the human for
    /// the value judgment (ingest, aggregate, respond).
    Accepted,
    /// Processable but anomalous — parked for human review, never auto-fed to a model.
    Flagged,
    /// Unusable: unreadable, bad signature, unsupported schema, or consent forbids
    /// processing (`NoSharing`).
    Rejected,
}

impl Disposition {
    /// Directory name for this disposition.
    pub fn dir_name(self) -> &'static str {
        match self {
            Disposition::Accepted => "accepted",
            Disposition::Flagged => "flagged",
            Disposition::Rejected => "rejected",
        }
    }
}

/// The outcome of processing one file.
#[derive(Debug, Clone, PartialEq)]
pub struct IntakeDecision {
    /// File basename.
    pub file: String,
    /// Disposition applied.
    pub disposition: Disposition,
    /// Human-readable reason (screening findings, rejection cause).
    pub reason: String,
    /// Consent scope if the report parsed (drives what may legally happen next).
    pub consent: Option<ConsentScope>,
    /// The parsed report if the file was processable at all.
    pub report: Option<tpt_fab_aggregate::OutcomeReport>,
}

/// The intake queue: a reader + anomaly screener with persisted history.
pub struct IntakeQueue {
    config: IntakeConfig,
    detector: AnomalyDetector,
}

impl IntakeQueue {
    /// Queue with fresh screening history (first run, or history intentionally not loaded).
    pub fn new(config: IntakeConfig) -> Self {
        let mut detector = AnomalyDetector::new();
        detector.z_threshold = config.z_threshold;
        detector.min_history = config.min_history;
        IntakeQueue { config, detector }
    }

    /// Queue whose screening history continues from a loaded ledger.
    pub fn from_ledger(config: IntakeConfig, ledger: &IntakeLedger) -> Self {
        let mut detector = AnomalyDetector::new().with_history(
            &ledger.history_yield,
            &ledger.history_geometry,
            &ledger.history_electrical,
        );
        detector.z_threshold = config.z_threshold;
        detector.min_history = config.min_history;
        IntakeQueue { config, detector }
    }

    /// Processes one file: verify → validate → consent → screen.
    ///
    /// The file itself is not moved by this method; use [`IntakeQueue::process_directory`]
    /// for the full move-and-record flow, or handle [`IntakeDecision`] yourself.
    pub fn process_file(&mut self, path: &Path) -> IntakeDecision {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        // 1-2. Signature verification + schema validation happen inside the reader.
        let keys = self.config.key_of.clone();
        let reader = JsonOutcomeFile::reader(self.config.unsigned_policy, keys);
        let report = match reader.read_outcome_file(path) {
            Ok(r) => r,
            Err(e) => {
                return IntakeDecision {
                    file,
                    disposition: Disposition::Rejected,
                    reason: format!("{e}"),
                    consent: None,
                    report: None,
                }
            }
        };

        // 3. Basic sanity (impossible yields etc.).
        if let Err(e) = validate_report_basics(&report) {
            return IntakeDecision {
                file,
                disposition: Disposition::Rejected,
                reason: format!("{e}"),
                consent: Some(report.consent),
                report: Some(report),
            };
        }

        // Consent: NoSharing reports must not be processed by the receiver at all.
        if report.consent == ConsentScope::NoSharing {
            return IntakeDecision {
                file,
                disposition: Disposition::Rejected,
                reason: "consent 'no-sharing': report must not be processed by the receiver".into(),
                consent: Some(report.consent),
                report: Some(report),
            };
        }

        // 4. Anomaly screening against accumulated history.
        let screen = self.detector.screen(&report);
        let disposition = if screen.flagged { Disposition::Flagged } else { Disposition::Accepted };
        let reason = if screen.flagged {
            let parts: Vec<String> = screen
                .findings
                .iter()
                .map(|(name, z)| format!("{name} at z={z:.1} MADs"))
                .collect();
            format!("flagged for review: {}", parts.join(", "))
        } else {
            "authentic (signature verified, or unsigned accepted by policy), schema current,              consent honored, screening unremarkable"
                .to_string()
        };

        IntakeDecision {
            file,
            disposition,
            reason,
            consent: Some(report.consent),
            report: Some(report),
        }
    }

    /// Processes every `*.json` in `incoming` (sorted by name), moves each file to
    /// `<root>/<disposition dir>/` (collision-safe), records the decision + screening
    /// metrics into `ledger`, and saves the ledger to `ledger_path` after every file.
    pub fn process_directory(
        &mut self,
        incoming: &Path,
        root: &Path,
        ledger: &mut IntakeLedger,
        ledger_path: &Path,
    ) -> Result<Vec<IntakeDecision>, tpt_fab_aggregate::AggregateError> {
        let mut names: Vec<String> = std::fs::read_dir(incoming)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();

        let mut decisions = Vec::new();
        for name in names {
            let src = incoming.join(&name);
            let decision = self.process_file(&src);
            let target_dir = root.join(decision.disposition.dir_name());
            std::fs::create_dir_all(&target_dir)?;
            let target = collision_safe(&target_dir, &name);
            move_file(&src, &target)?;
            ledger.record(DecisionRecord {
                file: decision.file.clone(),
                disposition: decision.disposition.dir_name().to_string(),
                reason: decision.reason.clone(),
                at_unix: ledger::unix_now(),
                consent: decision.consent.map(|c| c.as_str().to_string()).unwrap_or_default(),
            });
            if let Some(r) = &decision.report {
                ledger.observe_metrics(
                    r.yield_outcome.yield_rate(),
                    mean_abs_geometry_deviation_um(r),
                    mean_electrical_ratio(r),
                );
            }
            ledger.save(ledger_path)?;
            decisions.push(decision);
        }
        Ok(decisions)
    }

    /// Current screening history (for persisting into a ledger).
    pub fn history(&self) -> (&[f64], &[f64], &[f64]) {
        self.detector.history()
    }
}

fn collision_safe(dir: &Path, name: &str) -> std::path::PathBuf {
    let mut candidate = dir.join(name);
    let stem = name.strip_suffix(".json").unwrap_or(name);
    let mut n = 1u32;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}-dup{n}.json"));
        n += 1;
    }
    candidate
}

fn move_file(from: &Path, to: &Path) -> Result<(), tpt_fab_aggregate::AggregateError> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Cross-device rename fallback: copy then remove.
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger::DecisionRecord;
    use std::path::PathBuf;
    use std::rc::Rc;
    use tpt_fab_aggregate::outcome::{
        ConsentScope, ManufacturingTrack, OutcomeReport, SemanticId, WaferTech, YieldSummary,
    };
    use tpt_fab_aggregate::schema::SchemaVersion;
    use tpt_fab_aggregate::signature::sign_report;
    use tpt_fab_aggregate::{JsonOutcomeFile, OutcomeFileWriter};

    const KEY: &[u8] = b"intake-test-key";

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tpt-fab-intake-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("incoming")).unwrap();
        p
    }

    fn config(unsigned: UnsignedPolicy) -> IntakeConfig {
        IntakeConfig {
            unsigned_policy: unsigned,
            key_of: Rc::new(|kid: &str| (kid == "k1").then(|| KEY.to_vec())),
            z_threshold: 4.0,
            min_history: 8,
        }
    }

    fn report(
        seed: u64,
        consent: ConsentScope,
        yield_rate: f64,
        geom_dev_um: f64,
    ) -> OutcomeReport {
        OutcomeReport {
            payload_manifest_id: tpt_fab_aggregate::outcome::new_manifest_id(seed),
            track: ManufacturingTrack::WaferLitho(WaferTech::Euv),
            measured_geometry: vec![tpt_fab_aggregate::outcome::GeometryDeviation {
                feature_id: SemanticId::new("via-chain").unwrap(),
                designed_um: 8.0,
                measured_um: 8.0 + geom_dev_um,
            }],
            electrical_test: vec![],
            yield_outcome: YieldSummary {
                units_started: 1000,
                units_good: (yield_rate * 1000.0) as u64,
                dominant_failure_mode: None,
            },
            process_notes: vec![],
            consent,
            schema_version: SchemaVersion::CURRENT,
            signature: None,
        }
    }

    fn write(dir: &Path, name: &str, r: &OutcomeReport) -> PathBuf {
        let p = dir.join(name);
        JsonOutcomeFile.write_outcome_file(r, &p).unwrap();
        p
    }

    #[test]
    fn signed_clean_report_is_accepted() {
        let dir = scratch("accepted");
        let mut r = report(1, ConsentScope::AggregatedContribution, 0.9, 0.05);
        sign_report(&mut r, "k1", KEY);
        let p = write(&dir.join("incoming"), "good.json", &r);

        let mut queue = IntakeQueue::new(config(UnsignedPolicy::RequireSignature));
        let d = queue.process_file(&p);
        assert_eq!(d.disposition, Disposition::Accepted, "reason: {}", d.reason);
        assert_eq!(d.consent, Some(ConsentScope::AggregatedContribution));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tampered_and_unsupported_schema_files_are_rejected() {
        let dir = scratch("rejected");
        let incoming = dir.join("incoming");

        let mut r = report(2, ConsentScope::AggregatedContribution, 0.9, 0.05);
        sign_report(&mut r, "k1", KEY);
        r.yield_outcome.units_good += 400; // tamper after signing
        let tampered = write(&incoming, "tampered.json", &r);

        let future = jobj_str(&tpt_fab_aggregate::jobj! {
            "payload_manifest_id" => tpt_fab_aggregate::json::Json::Str("x".into()),
            "schema_version" => tpt_fab_aggregate::json::Json::Str("9.9".into()),
        });
        let future_path = incoming.join("future.json");
        std::fs::write(&future_path, future).unwrap();

        // Signed, so it passes the signature gate and hits the consent gate.
        let mut no_sharing = report(3, ConsentScope::NoSharing, 0.9, 0.05);
        sign_report(&mut no_sharing, "k1", KEY);
        let no_sharing = write(&incoming, "no-sharing.json", &no_sharing);

        let mut queue = IntakeQueue::new(config(UnsignedPolicy::RequireSignature));
        let d1 = queue.process_file(&tampered);
        assert_eq!(d1.disposition, Disposition::Rejected);
        assert!(d1.reason.contains("signature"), "reason: {}", d1.reason);

        let d2 = queue.process_file(&future_path);
        assert_eq!(d2.disposition, Disposition::Rejected);
        assert!(d2.reason.contains("schema version"), "reason: {}", d2.reason);

        // NoSharing parses fine but must not be processed.
        let d3 = queue.process_file(&no_sharing);
        assert_eq!(d3.disposition, Disposition::Rejected);
        assert!(d3.reason.contains("no-sharing"), "reason: {}", d3.reason);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unsigned_policy_controls_gate() {
        let dir = scratch("unsigned");
        let p = write(
            &dir.join("incoming"),
            "unsigned.json",
            &report(4, ConsentScope::PrivateBilateral, 0.9, 0.05),
        );
        let strict = {
            let mut q = IntakeQueue::new(config(UnsignedPolicy::RequireSignature));
            q.process_file(&p)
        };
        assert_eq!(strict.disposition, Disposition::Rejected);
        let lenient = {
            let mut q = IntakeQueue::new(config(UnsignedPolicy::AcceptUnsigned));
            q.process_file(&p)
        };
        assert_eq!(lenient.disposition, Disposition::Accepted);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn history_persists_across_ledger_reload_and_flags_outliers() {
        let dir = scratch("history");
        let incoming = dir.join("incoming");
        let ledger_path = dir.join("ledger.json");

        // Batch 1: eight clean reports; the ninth is wildly "too good to be true".
        let mut queue = IntakeQueue::new(config(UnsignedPolicy::AcceptUnsigned));
        for i in 0..8 {
            let p = write(
                &incoming,
                &format!("clean-{i}.json"),
                &report(100 + i, ConsentScope::AggregatedContribution, 0.90, 0.05),
            );
            let d = queue.process_file(&p);
            assert_eq!(d.disposition, Disposition::Accepted);
        }
        let outlier = write(
            &incoming,
            "outlier.json",
            &report(200, ConsentScope::AggregatedContribution, 0.999, 6.0),
        );
        let d = queue.process_file(&outlier);
        assert_eq!(d.disposition, Disposition::Flagged, "reason: {}", d.reason);

        // Persist through a ledger and prove a fresh process (rebuilt from the ledger)
        // screens with the same accumulated history — the point of the ledger.
        let mut ledger = IntakeLedger::default();
        ledger.record(DecisionRecord {
            file: "history-marker".into(),
            disposition: "accepted".into(),
            reason: String::new(),
            at_unix: 0,
            consent: String::new(),
        });
        let (y, g, e) = queue.history();
        ledger.history_yield = y.to_vec();
        ledger.history_geometry = g.to_vec();
        ledger.history_electrical = e.to_vec();
        ledger.save(&ledger_path).unwrap();
        let reloaded = IntakeLedger::load(&ledger_path).unwrap();
        assert_eq!(reloaded.history_yield.len(), y.len());

        let later_outlier = write(
            &incoming,
            "later-outlier.json",
            &report(300, ConsentScope::AggregatedContribution, 0.999, 6.0),
        );
        let mut fresh_queue =
            IntakeQueue::from_ledger(config(UnsignedPolicy::AcceptUnsigned), &reloaded);
        let d = fresh_queue.process_file(&later_outlier);
        assert_eq!(d.disposition, Disposition::Flagged, "history did not persist");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn directory_flow_moves_files_and_records_decisions() {
        let dir = scratch("flow");
        let incoming = dir.join("incoming");
        let mut ledger = IntakeLedger::default();
        let ledger_path = dir.join("ledger.json");

        let mut good = report(400, ConsentScope::AggregatedContribution, 0.9, 0.05);
        sign_report(&mut good, "k1", KEY);
        write(&incoming, "a-good.json", &good);
        write(&incoming, "b-bad.json", &report(401, ConsentScope::NoSharing, 0.9, 0.05));
        // A non-JSON file that must be ignored entirely.
        std::fs::write(incoming.join("notes.txt"), "not an outcome file").unwrap();

        let mut queue = IntakeQueue::new(config(UnsignedPolicy::RequireSignature));
        let decisions =
            queue.process_directory(&incoming, &dir, &mut ledger, &ledger_path).unwrap();
        assert_eq!(decisions.len(), 2, "only *.json files are processed");

        assert!(dir.join("accepted").join("a-good.json").exists());
        assert!(dir.join("rejected").join("b-bad.json").exists());
        assert!(incoming.join("notes.txt").exists(), "non-JSON files stay put");
        assert!(!incoming.join("a-good.json").exists());

        let reloaded = IntakeLedger::load(&ledger_path).unwrap();
        assert_eq!(reloaded.decisions.len(), 2);
        assert_eq!(reloaded.decisions[0].disposition, "accepted");
        assert_eq!(reloaded.decisions[1].disposition, "rejected");

        // A name collision gets a -dupN suffix instead of clobbering.
        let mut again = IntakeQueue::new(config(UnsignedPolicy::RequireSignature));
        write(&incoming, "a-good.json", &good);
        again.process_directory(&incoming, &dir, &mut ledger, &ledger_path).unwrap();
        assert!(dir.join("accepted").join("a-good-dup1.json").exists());
        assert!(dir.join("accepted").join("a-good.json").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    fn jobj_str(v: &tpt_fab_aggregate::json::Json) -> String {
        v.serialize()
    }
}
