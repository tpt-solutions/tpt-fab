//! Schema versioning for outcome reports (spec 4.5).
//!
//! Fabs and `tpt-solutions` do not upgrade in lockstep, so the ingestion pipeline either
//! migrates a recognized older minor version or rejects an unrecognized one — it **never**
//! silently misinterprets a field whose meaning changed between versions.

use crate::error::AggregateError;
use crate::json::Json;

/// Semantic version of the outcome-report schema. Only `major.minor` are meaningful here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaVersion {
    /// Major version — incompatible changes.
    pub major: u32,
    /// Minor version — backward-compatible additions; older minors get migrated.
    pub minor: u32,
}

impl SchemaVersion {
    /// The version this crate reads and writes.
    pub const CURRENT: SchemaVersion = SchemaVersion { major: 0, minor: 2 };

    /// Builds a version.
    pub const fn new(major: u32, minor: u32) -> Self {
        SchemaVersion { major, minor }
    }

    /// Parses `"0.2"`-style strings.
    pub fn parse(s: &str) -> Result<SchemaVersion, AggregateError> {
        let (maj, min) = s
            .split_once('.')
            .ok_or_else(|| AggregateError::Schema(format!("bad schema version '{s}'")))?;
        Ok(SchemaVersion {
            major: maj
                .parse()
                .map_err(|_| AggregateError::Schema(format!("bad major in '{s}'")))?,
            minor: min
                .parse()
                .map_err(|_| AggregateError::Schema(format!("bad minor in '{s}'")))?,
        })
    }
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Validates the report's schema version and best-effort migrates it to
/// [`SchemaVersion::CURRENT`].
///
/// * Same version → returned unchanged.
/// * Recognized older minor (0.1) → migrated (see `migrate_from_0_1`).
/// * Anything else → [`AggregateError::UnsupportedSchemaVersion`].
pub fn ensure_current(mut report: Json) -> Result<Json, AggregateError> {
    let found = report
        .get("schema_version")
        .and_then(Json::as_str)
        .ok_or_else(|| AggregateError::Schema("missing schema_version".into()))?;
    let version = SchemaVersion::parse(found)?;
    match version {
        v if v == SchemaVersion::CURRENT => Ok(report),
        SchemaVersion { major: 0, minor: 1 } => {
            report = migrate_from_0_1(report)?;
            Ok(report)
        }
        other => Err(AggregateError::UnsupportedSchemaVersion { found: other.to_string() }),
    }
}

/// 0.1 → 0.2 migration: in 0.1, `process_notes` was a single free-text string; in 0.2 it is
/// an array of structured `{parameter, target, actual, unit}` entries. The legacy string is
/// preserved as a single unstructured note (empty parameter name, unit `-`).
fn migrate_from_0_1(mut report: Json) -> Result<Json, AggregateError> {
    if let Json::Obj(entries) = &mut report {
        if let Some((_, old)) = entries.iter_mut().find(|(k, _)| k == "process_notes") {
            if let Json::Str(text) = old {
                let migrated = Json::Arr(vec![Json::Obj(vec![
                    ("parameter".into(), Json::Str(String::new())),
                    ("target".into(), Json::Num(0.0)),
                    ("actual".into(), Json::Num(0.0)),
                    ("unit".into(), Json::Str("-".into())),
                    ("legacy_note".into(), Json::Str(text.clone())),
                ])]);
                *old = migrated;
            }
        }
        // Refresh the version marker last.
        for (k, v) in entries.iter_mut() {
            if k == "schema_version" {
                *v = Json::Str(SchemaVersion::CURRENT.to_string());
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobj;

    #[test]
    fn parse_and_display() {
        let v = SchemaVersion::parse("0.10").unwrap();
        assert_eq!(v, SchemaVersion { major: 0, minor: 10 });
        assert_eq!(v.to_string(), "0.10");
        assert!(SchemaVersion::parse("zero.one").is_err());
        assert!(SchemaVersion::parse("01").is_err());
    }

    #[test]
    fn current_passes_through() {
        let report = jobj! {
            "schema_version" => Json::Str("0.2".into()),
            "yield_outcome" => Json::Null,
        };
        let out = ensure_current(report).unwrap();
        assert_eq!(out.get("schema_version").unwrap().as_str(), Some("0.2"));
    }

    #[test]
    fn v0_1_migrates_notes_to_array() {
        let report = jobj! {
            "schema_version" => Json::Str("0.1".into()),
            "process_notes" => Json::Str("chamber ran hot".into()),
        };
        let out = ensure_current(report).unwrap();
        assert_eq!(out.get("schema_version").unwrap().as_str(), Some("0.2"));
        let notes = out.get("process_notes").unwrap().as_arr().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].get("legacy_note").unwrap().as_str(), Some("chamber ran hot"));
    }

    #[test]
    fn unknown_future_version_is_rejected() {
        let report = jobj! {
            "schema_version" => Json::Str("0.3".into()),
        };
        let err = ensure_current(report).unwrap_err();
        assert!(matches!(err, AggregateError::UnsupportedSchemaVersion { .. }));
        // And so are major bumps — never guessed at.
        let report = jobj! {
            "schema_version" => Json::Str("1.0".into()),
        };
        assert!(ensure_current(report).is_err());
    }
}
