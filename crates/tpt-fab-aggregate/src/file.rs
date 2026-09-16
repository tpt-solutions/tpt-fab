//! The file-only I/O boundary (spec 4.1/4.8).
//!
//! `OutcomeFileWriter` runs entirely on the fab's own infrastructure: it produces a local file
//! and **sends nothing** — there is no network egress path in this crate at all, only file
//! generation. `OutcomeFileReader` runs entirely on the `tpt-solutions` side, on a file that
//! arrived by whatever manual means the fab chose (email attachment, portal, physical media —
//! all out of scope). What happens to the file after the writer returns is outside this
//! codebase's control by design.

use crate::error::AggregateError;
use crate::json::Json;
use crate::outcome::OutcomeReport;
use crate::schema::ensure_current;
use crate::signature::{verify_report, UnsignedPolicy};
use std::path::Path;
use std::rc::Rc;

/// Key-lookup callback: maps a signature's key ID to the verification key bytes.
pub type KeyLookup = Rc<dyn Fn(&str) -> Option<Vec<u8>>>;

/// Produces an outcome file at `path`. File-out only: writes to disk and returns.
pub trait OutcomeFileWriter {
    /// Writes `report` to `path` in this implementation's file format.
    fn write_outcome_file(&self, report: &OutcomeReport, path: &Path)
        -> Result<(), AggregateError>;
}

/// Reads an outcome file from `path`. Runs on the receiving side.
pub trait OutcomeFileReader {
    /// Reads and parses the report at `path`, validating schema version and signature policy.
    fn read_outcome_file(&self, path: &Path) -> Result<OutcomeReport, AggregateError>;
}

/// Canonical JSON file format (`.json`), signed with HMAC-SHA256.
pub struct JsonOutcomeFile;

impl JsonOutcomeFile {
    /// Reader with a signature policy and key lookup.
    pub fn reader(unsigned_policy: UnsignedPolicy, key_of: KeyLookup) -> JsonOutcomeFileReader {
        JsonOutcomeFileReader { unsigned_policy, key_of }
    }
}

impl OutcomeFileWriter for JsonOutcomeFile {
    fn write_outcome_file(
        &self,
        report: &OutcomeReport,
        path: &Path,
    ) -> Result<(), AggregateError> {
        let text = report.to_canonical_json();
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// Reader produced by [`JsonOutcomeFile::reader`].
pub struct JsonOutcomeFileReader {
    unsigned_policy: UnsignedPolicy,
    key_of: KeyLookup,
}

impl OutcomeFileReader for JsonOutcomeFileReader {
    fn read_outcome_file(&self, path: &Path) -> Result<OutcomeReport, AggregateError> {
        let text = std::fs::read_to_string(path)?;
        let value = Json::parse(&text)?;
        let value = ensure_current(value)?;
        let report = OutcomeReport::from_json(&value)?;
        verify_report(&report, self.key_of.as_ref(), self.unsigned_policy)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{ConsentScope, ManufacturingTrack, WaferTech, YieldSummary};
    use crate::schema::SchemaVersion;
    use crate::signature::sign_report;

    fn report() -> OutcomeReport {
        OutcomeReport {
            payload_manifest_id: "a1b2c3d4-0000-4000-8000-000000000001".into(),
            track: ManufacturingTrack::WaferLitho(WaferTech::DuvMultiPattern),
            measured_geometry: vec![],
            electrical_test: vec![],
            yield_outcome: YieldSummary {
                units_started: 25,
                units_good: 24,
                dominant_failure_mode: None,
            },
            process_notes: vec![],
            consent: ConsentScope::AggregatedContribution,
            schema_version: SchemaVersion::CURRENT,
            signature: None,
        }
    }

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tpt-fab-aggregate-test-{name}.json"));
        p
    }

    #[test]
    fn write_then_read_roundtrip_with_signature() {
        let mut r = report();
        sign_report(&mut r, "k1", b"key");
        let path = tmp_path("signed");
        JsonOutcomeFile.write_outcome_file(&r, &path).unwrap();

        let keys = |kid: &str| (kid == "k1").then(|| b"key".to_vec());
        let reader =
            JsonOutcomeFile::reader(UnsignedPolicy::RequireSignature, std::rc::Rc::new(keys));
        let back = reader.read_outcome_file(&path).unwrap();
        assert_eq!(back, r);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reader_enforces_signature_policy() {
        let r = report();
        let path = tmp_path("unsigned");
        JsonOutcomeFile.write_outcome_file(&r, &path).unwrap();

        let no_keys = |_: &str| None;
        let strict =
            JsonOutcomeFile::reader(UnsignedPolicy::RequireSignature, std::rc::Rc::new(no_keys));
        assert!(strict.read_outcome_file(&path).is_err());
        let lenient =
            JsonOutcomeFile::reader(UnsignedPolicy::AcceptUnsigned, std::rc::Rc::new(no_keys));
        assert!(lenient.read_outcome_file(&path).is_ok());
        std::fs::remove_file(&path).ok();
    }
}
