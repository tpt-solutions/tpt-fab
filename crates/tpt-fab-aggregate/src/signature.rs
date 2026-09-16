//! Signature verification on receipt (spec 4.8).
//!
//! The signature authenticates the file's **origin, not its transport**: once a fab has
//! already decided to send a file by whatever out-of-band means they chose, this lets the
//! receiver verify it genuinely came from that fab and was not altered in transit or storage.
//! Key distribution is deliberately out of scope here (it is an operational relationship, not
//! a protocol).

use crate::error::AggregateError;
use crate::hash::{hex, hmac_sha256, unhex};
use crate::json::Json;
use crate::outcome::{OutcomeReport, Signature};

/// Canonical signing payload: the report's canonical JSON with the `signature` field removed.
fn signing_bytes(report: &OutcomeReport) -> String {
    let mut value = report.to_json();
    if let Json::Obj(entries) = &mut value {
        entries.retain(|(k, _)| k != "signature");
    }
    value.serialize()
}

/// Signs a report in place: computes the HMAC over the canonical signing bytes and attaches
/// the signature.
pub fn sign_report(report: &mut OutcomeReport, key_id: &str, key: &[u8]) {
    let mac = hmac_sha256(key, signing_bytes(report).as_bytes());
    report.signature = Some(Signature {
        algorithm: "hmac-sha256".into(),
        key_id: key_id.to_string(),
        mac_hex: hex(&mac),
    });
}

/// What to do when a report arrives without a signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsignedPolicy {
    /// Reject unsigned files outright.
    RequireSignature,
    /// Accept unsigned files (e.g. early pilots where the fab has no key yet).
    AcceptUnsigned,
}

/// Verifies a report's signature against the named key. Returns the report unchanged on
/// success (typed pass-through so callers can chain).
pub fn verify_report(
    report: &OutcomeReport,
    key_of: &dyn Fn(&str) -> Option<Vec<u8>>,
    unsigned_policy: UnsignedPolicy,
) -> Result<(), AggregateError> {
    let Some(sig) = &report.signature else {
        return match unsigned_policy {
            UnsignedPolicy::AcceptUnsigned => Ok(()),
            UnsignedPolicy::RequireSignature => {
                Err(AggregateError::Signature("file has no signature".into()))
            }
        };
    };
    if sig.algorithm != "hmac-sha256" {
        return Err(AggregateError::Signature(format!(
            "unsupported algorithm '{}'",
            sig.algorithm
        )));
    }
    let Some(key) = key_of(&sig.key_id) else {
        return Err(AggregateError::Signature(format!("unknown key id '{}'", sig.key_id)));
    };
    let expected = hmac_sha256(&key, signing_bytes(report).as_bytes());
    let claimed = unhex(&sig.mac_hex)?;
    if claimed.len() != expected.len() || !claimed.iter().zip(expected.iter()).all(|(a, b)| a == b)
    {
        return Err(AggregateError::Signature(
            "signature does not match file contents — origin or content mismatch".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{
        ConsentScope, GeometryDeviation, ManufacturingTrack, SemanticId, WaferTech, YieldSummary,
    };
    use crate::schema::SchemaVersion;

    fn unsigned_report() -> OutcomeReport {
        OutcomeReport {
            payload_manifest_id: "0f0e0d0c-1111-4222-8333-444455556666".into(),
            track: ManufacturingTrack::WaferLitho(WaferTech::Ebeam),
            measured_geometry: vec![GeometryDeviation {
                feature_id: SemanticId::new("via-1").unwrap(),
                designed_um: 50.0,
                measured_um: 51.2,
            }],
            electrical_test: vec![],
            yield_outcome: YieldSummary {
                units_started: 10,
                units_good: 9,
                dominant_failure_mode: None,
            },
            process_notes: vec![],
            consent: ConsentScope::PrivateBilateral,
            schema_version: SchemaVersion::CURRENT,
            signature: None,
        }
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let mut report = unsigned_report();
        sign_report(&mut report, "fab-alpha", b"secret-key");
        assert!(report.signature.is_some());
        let keys = |kid: &str| (kid == "fab-alpha").then(|| b"secret-key".to_vec());
        verify_report(&report, &keys, UnsignedPolicy::RequireSignature).unwrap();
    }

    #[test]
    fn tampered_content_fails_verification() {
        let mut report = unsigned_report();
        sign_report(&mut report, "fab-alpha", b"secret-key");
        // Tamper after signing.
        report.yield_outcome.units_good += 500;
        let keys = |kid: &str| (kid == "fab-alpha").then(|| b"secret-key".to_vec());
        assert!(matches!(
            verify_report(&report, &keys, UnsignedPolicy::AcceptUnsigned),
            Err(AggregateError::Signature(_))
        ));
    }

    #[test]
    fn unknown_key_and_wrong_key_fail() {
        let mut report = unsigned_report();
        sign_report(&mut report, "fab-alpha", b"secret-key");
        let no_keys = |_: &str| None;
        assert!(verify_report(&report, &no_keys, UnsignedPolicy::AcceptUnsigned).is_err());
        let other_keys = |kid: &str| (kid == "fab-alpha").then(|| b"other-key".to_vec());
        assert!(verify_report(&report, &other_keys, UnsignedPolicy::AcceptUnsigned).is_err());
    }

    #[test]
    fn unsigned_policy_is_honored() {
        let report = unsigned_report();
        let no_keys = |_: &str| None;
        assert!(verify_report(&report, &no_keys, UnsignedPolicy::AcceptUnsigned).is_ok());
        assert!(verify_report(&report, &no_keys, UnsignedPolicy::RequireSignature).is_err());
    }
}
