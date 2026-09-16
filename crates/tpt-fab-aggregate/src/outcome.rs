//! The `OutcomeReport` schema — the one shared file format for both manufacturing tracks.
//!
//! Everything is keyed against the same semantic IDs the outbound payload used
//! (`FabricLink.id`, footprint IDs, net names, recipe/lot IDs) rather than raw coordinates —
//! that is what makes the exchange usable for calibration rather than just a pile of
//! unstructured data (spec 4.1).

use crate::error::AggregateError;
use crate::jobj;
use crate::json::Json;
use crate::schema::SchemaVersion;

/// A semantic identifier from the outbound payload (net name, footprint ID, `FabricLink.id`,
/// recipe/lot ID). Never a raw coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SemanticId(pub String);

impl SemanticId {
    /// Validates and wraps a semantic ID (non-empty).
    pub fn new(s: impl Into<String>) -> Result<Self, AggregateError> {
        let s = s.into();
        if s.is_empty() {
            return Err(AggregateError::Schema("empty semantic id".into()));
        }
        Ok(SemanticId(s))
    }
}

/// Wafer-track patterning technologies (mirrors `tpt-fab-litho`'s enum; duplicated here so
/// this crate stays dependency-free for the PCB track to share).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaferTech {
    /// DUV multi-patterning.
    DuvMultiPattern,
    /// EUV.
    Euv,
    /// Nanoimprint.
    Nil,
    /// E-beam.
    Ebeam,
}

impl WaferTech {
    /// Stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            WaferTech::DuvMultiPattern => "duv-multi-pattern",
            WaferTech::Euv => "euv",
            WaferTech::Nil => "nil",
            WaferTech::Ebeam => "ebeam",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "duv-multi-pattern" => WaferTech::DuvMultiPattern,
            "euv" => WaferTech::Euv,
            "nil" => WaferTech::Nil,
            "ebeam" => WaferTech::Ebeam,
            _ => return None,
        })
    }
}

/// Which manufacturing track a report came from — one shared schema for both (spec 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManufacturingTrack {
    /// RFC-001 PCB track (`tpt-silicon-cam`).
    Pcb,
    /// Wafer track, tagged with the patterning technology.
    WaferLitho(WaferTech),
}

impl ManufacturingTrack {
    /// Wire representation: `"pcb"` or `"wafer-litho:<tech>"`.
    pub fn as_str(self) -> String {
        match self {
            ManufacturingTrack::Pcb => "pcb".into(),
            ManufacturingTrack::WaferLitho(t) => format!("wafer-litho:{}", t.as_str()),
        }
    }

    /// Parses the wire representation.
    pub fn parse(s: &str) -> Result<Self, AggregateError> {
        match s {
            "pcb" => Ok(ManufacturingTrack::Pcb),
            rest if rest.starts_with("wafer-litho:") => {
                let tech = WaferTech::from_str(rest.trim_start_matches("wafer-litho:"))
                    .ok_or_else(|| {
                        AggregateError::Schema(format!("unknown wafer tech in '{s}'"))
                    })?;
                Ok(ManufacturingTrack::WaferLitho(tech))
            }
            other => Err(AggregateError::Schema(format!("unknown track '{other}'"))),
        }
    }
}

/// As-built vs. as-designed geometry for one feature, keyed by semantic ID.
#[derive(Debug, Clone, PartialEq)]
pub struct GeometryDeviation {
    /// Feature semantic ID (footprint ID, net name, ...).
    pub feature_id: SemanticId,
    /// As-designed dimension, µm.
    pub designed_um: f64,
    /// As-measured dimension, µm.
    pub measured_um: f64,
}

/// An electrical measurement tied to a `FabricLink.id` (spec 4.1 example: an 85 Ω trace that
/// measured 87 Ω is a directly usable calibration point for the `tpt-silicon-si-pi` solver).
#[derive(Debug, Clone, PartialEq)]
pub struct ElectricalMeasurement {
    /// FabricLink semantic ID.
    pub link_id: SemanticId,
    /// Quantity, e.g. `impedance`, `skew`, `ir_drop`.
    pub quantity: String,
    /// Designed value.
    pub designed: f64,
    /// Measured value.
    pub measured: f64,
    /// Unit (`ohm`, `ps`, `mV`, ...).
    pub unit: String,
}

/// Yield outcome summary (statistical, not per-die — per-die data never leaves the fab).
#[derive(Debug, Clone, PartialEq)]
pub struct YieldSummary {
    /// Dies (or boards) started.
    pub units_started: u64,
    /// Units passing all tests.
    pub units_good: u64,
    /// Dominant failure mode if the fab chose to share it.
    pub dominant_failure_mode: Option<String>,
}

impl YieldSummary {
    /// Good/started ratio in `[0, 1]`; 0 when nothing started.
    pub fn yield_rate(&self) -> f64 {
        if self.units_started == 0 {
            0.0
        } else {
            self.units_good as f64 / self.units_started as f64
        }
    }
}

/// A process-parameter deviation note.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessDeviation {
    /// Parameter name.
    pub parameter: String,
    /// Target value.
    pub target: f64,
    /// Actual value.
    pub actual: f64,
    /// Unit.
    pub unit: String,
}

/// Machine-readable consent scope travelling *inside* the report (spec 4.3), enforced
/// structurally by the ingestion pipeline rather than by policy documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentScope {
    /// Raw data, shared only between this fab and `tpt-solutions`.
    PrivateBilateral,
    /// Fab-side aggregation only; feeds the shared public model.
    AggregatedContribution,
    /// Outcome tooling used locally, nothing transmitted.
    NoSharing,
}

impl ConsentScope {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            ConsentScope::PrivateBilateral => "private-bilateral",
            ConsentScope::AggregatedContribution => "aggregated-contribution",
            ConsentScope::NoSharing => "no-sharing",
        }
    }

    /// Parses the wire name.
    pub fn parse(s: &str) -> Result<Self, AggregateError> {
        match s {
            "private-bilateral" => Ok(ConsentScope::PrivateBilateral),
            "aggregated-contribution" => Ok(ConsentScope::AggregatedContribution),
            "no-sharing" => Ok(ConsentScope::NoSharing),
            other => Err(AggregateError::Schema(format!("unknown consent scope '{other}'"))),
        }
    }

    /// Whether this scope permits inclusion in the shared/public aggregate at all.
    pub fn permits_public_aggregation(self) -> bool {
        matches!(self, ConsentScope::AggregatedContribution)
    }
}

/// File-origin signature (spec 4.8): authenticates the file's origin, not its transport.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    /// Algorithm, currently only `hmac-sha256`.
    pub algorithm: String,
    /// Key identifier both sides understand (key distribution is out of band).
    pub key_id: String,
    /// HMAC bytes, hex-encoded.
    pub mac_hex: String,
}

/// The complete outcome report (spec 4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct OutcomeReport {
    /// Ties back to the exact SMP/recipe payload that was sent out (UUID string).
    pub payload_manifest_id: String,
    /// Manufacturing track — one shared schema for PCB and wafer.
    pub track: ManufacturingTrack,
    /// As-built vs. as-designed geometry, keyed by ID.
    pub measured_geometry: Vec<GeometryDeviation>,
    /// Electrical test results, keyed to `FabricLink.id`.
    pub electrical_test: Vec<ElectricalMeasurement>,
    /// Yield summary.
    pub yield_outcome: YieldSummary,
    /// Process deviation notes.
    pub process_notes: Vec<ProcessDeviation>,
    /// Consent scope, machine-readable, enforced in the pipeline.
    pub consent: ConsentScope,
    /// Schema version of this report.
    pub schema_version: SchemaVersion,
    /// Origin signature, if the fab signed the file.
    pub signature: Option<Signature>,
}

impl OutcomeReport {
    /// Serializes to canonical JSON (deterministic field order, insertion-ordered objects).
    pub fn to_json(&self) -> Json {
        let mut obj: Vec<(String, Json)> = Vec::new();
        obj.push(("payload_manifest_id".into(), Json::Str(self.payload_manifest_id.clone())));
        obj.push(("track".into(), Json::Str(self.track.as_str())));
        obj.push((
            "measured_geometry".into(),
            Json::Arr(
                self.measured_geometry
                    .iter()
                    .map(|g| {
                        jobj! {
                            "feature_id" => Json::Str(g.feature_id.0.clone()),
                            "designed_um" => Json::Num(g.designed_um),
                            "measured_um" => Json::Num(g.measured_um),
                        }
                    })
                    .collect(),
            ),
        ));
        obj.push((
            "electrical_test".into(),
            Json::Arr(
                self.electrical_test
                    .iter()
                    .map(|e| {
                        jobj! {
                            "link_id" => Json::Str(e.link_id.0.clone()),
                            "quantity" => Json::Str(e.quantity.clone()),
                            "designed" => Json::Num(e.designed),
                            "measured" => Json::Num(e.measured),
                            "unit" => Json::Str(e.unit.clone()),
                        }
                    })
                    .collect(),
            ),
        ));
        obj.push((
            "yield_outcome".into(),
            jobj! {
                "units_started" => Json::Num(self.yield_outcome.units_started as f64),
                "units_good" => Json::Num(self.yield_outcome.units_good as f64),
                "dominant_failure_mode" => self
                    .yield_outcome
                    .dominant_failure_mode
                    .clone()
                    .map(Json::Str)
                    .unwrap_or(Json::Null),
            },
        ));
        obj.push((
            "process_notes".into(),
            Json::Arr(
                self.process_notes
                    .iter()
                    .map(|p| {
                        jobj! {
                            "parameter" => Json::Str(p.parameter.clone()),
                            "target" => Json::Num(p.target),
                            "actual" => Json::Num(p.actual),
                            "unit" => Json::Str(p.unit.clone()),
                        }
                    })
                    .collect(),
            ),
        ));
        obj.push(("consent".into(), Json::Str(self.consent.as_str().into())));
        obj.push(("schema_version".into(), Json::Str(self.schema_version.to_string())));
        if let Some(sig) = &self.signature {
            obj.push((
                "signature".into(),
                jobj! {
                    "algorithm" => Json::Str(sig.algorithm.clone()),
                    "key_id" => Json::Str(sig.key_id.clone()),
                    "mac_hex" => Json::Str(sig.mac_hex.clone()),
                },
            ));
        }
        Json::Obj(obj)
    }

    /// Serializes to a canonical JSON string.
    pub fn to_canonical_json(&self) -> String {
        self.to_json().serialize()
    }

    /// Parses a report from JSON previously produced by [`OutcomeReport::to_json`], after
    /// schema-version validation/migration.
    pub fn from_json(value: &Json) -> Result<OutcomeReport, AggregateError> {
        let value = crate::schema::ensure_current(value.clone())?;
        let need = |key: &str| -> Result<&Json, AggregateError> {
            value.get(key).ok_or_else(|| AggregateError::Schema(format!("missing field '{key}'")))
        };
        let manifest = need("payload_manifest_id")?
            .as_str()
            .ok_or_else(|| AggregateError::Schema("payload_manifest_id not a string".into()))?
            .to_string();
        let track = ManufacturingTrack::parse(
            need("track")?
                .as_str()
                .ok_or_else(|| AggregateError::Schema("track not a string".into()))?,
        )?;
        let mut measured_geometry = Vec::new();
        for g in need("measured_geometry")?
            .as_arr()
            .ok_or_else(|| AggregateError::Schema("measured_geometry not an array".into()))?
        {
            measured_geometry.push(GeometryDeviation {
                feature_id: SemanticId::new(
                    g.get("feature_id").and_then(Json::as_str).ok_or_else(|| {
                        AggregateError::Schema("geometry missing feature_id".into())
                    })?,
                )?,
                designed_um: g.get("designed_um").and_then(Json::as_f64).unwrap_or(0.0),
                measured_um: g.get("measured_um").and_then(Json::as_f64).unwrap_or(0.0),
            });
        }
        let mut electrical_test = Vec::new();
        for e in need("electrical_test")?
            .as_arr()
            .ok_or_else(|| AggregateError::Schema("electrical_test not an array".into()))?
        {
            electrical_test.push(ElectricalMeasurement {
                link_id: SemanticId::new(
                    e.get("link_id").and_then(Json::as_str).ok_or_else(|| {
                        AggregateError::Schema("electrical missing link_id".into())
                    })?,
                )?,
                quantity: e.get("quantity").and_then(Json::as_str).unwrap_or_default().to_string(),
                designed: e.get("designed").and_then(Json::as_f64).unwrap_or(0.0),
                measured: e.get("measured").and_then(Json::as_f64).unwrap_or(0.0),
                unit: e.get("unit").and_then(Json::as_str).unwrap_or_default().to_string(),
            });
        }
        let y = need("yield_outcome")?;
        let yield_outcome = YieldSummary {
            units_started: y.get("units_started").and_then(Json::as_f64).unwrap_or(0.0) as u64,
            units_good: y.get("units_good").and_then(Json::as_f64).unwrap_or(0.0) as u64,
            dominant_failure_mode: y
                .get("dominant_failure_mode")
                .and_then(Json::as_str)
                .map(str::to_string),
        };
        let mut process_notes = Vec::new();
        if let Some(notes) = need("process_notes")?.as_arr() {
            for p in notes {
                process_notes.push(ProcessDeviation {
                    parameter: p
                        .get("parameter")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    target: p.get("target").and_then(Json::as_f64).unwrap_or(0.0),
                    actual: p.get("actual").and_then(Json::as_f64).unwrap_or(0.0),
                    unit: p.get("unit").and_then(Json::as_str).unwrap_or_default().to_string(),
                });
            }
        }
        let consent = ConsentScope::parse(
            need("consent")?
                .as_str()
                .ok_or_else(|| AggregateError::Schema("consent not a string".into()))?,
        )?;
        let signature = value.get("signature").map(|s| {
            Result::<Signature, AggregateError>::Ok(Signature {
                algorithm: s
                    .get("algorithm")
                    .and_then(Json::as_str)
                    .ok_or_else(|| AggregateError::Schema("signature missing algorithm".into()))?
                    .to_string(),
                key_id: s
                    .get("key_id")
                    .and_then(Json::as_str)
                    .ok_or_else(|| AggregateError::Schema("signature missing key_id".into()))?
                    .to_string(),
                mac_hex: s
                    .get("mac_hex")
                    .and_then(Json::as_str)
                    .ok_or_else(|| AggregateError::Schema("signature missing mac_hex".into()))?
                    .to_string(),
            })
        });
        let signature = signature.transpose()?;
        Ok(OutcomeReport {
            payload_manifest_id: manifest,
            track,
            measured_geometry,
            electrical_test,
            yield_outcome,
            process_notes,
            consent,
            schema_version: SchemaVersion::CURRENT,
            signature,
        })
    }
}

/// Generates a random UUIDv4-shaped manifest ID from the crate PRNG (deterministic given the
/// same seed; the ID's only job is to tie the report back to an outbound payload).
pub fn new_manifest_id(seed: u64) -> String {
    let mut rng = crate::privacy::SplitMix64::new(seed);
    let mut b = [0u8; 16];
    for chunk in b.chunks_mut(8) {
        chunk.copy_from_slice(&rng.next_u64().to_be_bytes()[..chunk.len()]);
    }
    b[6] = (b[6] & 0x0F) | 0x40; // version 4
    b[8] = (b[8] & 0x3F) | 0x80; // variant 10
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> OutcomeReport {
        OutcomeReport {
            payload_manifest_id: new_manifest_id(1),
            track: ManufacturingTrack::WaferLitho(WaferTech::Euv),
            measured_geometry: vec![GeometryDeviation {
                feature_id: SemanticId::new("fp-0603-r1").unwrap(),
                designed_um: 100.0,
                measured_um: 101.5,
            }],
            electrical_test: vec![ElectricalMeasurement {
                link_id: SemanticId::new("FL-usb-dp").unwrap(),
                quantity: "impedance".into(),
                designed: 85.0,
                measured: 87.0,
                unit: "ohm".into(),
            }],
            yield_outcome: YieldSummary {
                units_started: 500,
                units_good: 461,
                dominant_failure_mode: Some("open via".into()),
            },
            process_notes: vec![ProcessDeviation {
                parameter: "etch_time_s".into(),
                target: 60.0,
                actual: 62.5,
                unit: "s".into(),
            }],
            consent: ConsentScope::AggregatedContribution,
            schema_version: SchemaVersion::CURRENT,
            signature: None,
        }
    }

    #[test]
    fn json_roundtrip_preserves_everything() {
        let report = sample();
        let text = report.to_canonical_json();
        let parsed = Json::parse(&text).unwrap();
        let back = OutcomeReport::from_json(&parsed).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn semantic_ids_reject_empty() {
        assert!(SemanticId::new("").is_err());
        assert!(SemanticId::new("net-vbus").is_ok());
    }

    #[test]
    fn track_wire_format() {
        assert_eq!(ManufacturingTrack::Pcb.as_str(), "pcb");
        assert_eq!(
            ManufacturingTrack::parse("wafer-litho:nil").unwrap(),
            ManufacturingTrack::WaferLitho(WaferTech::Nil)
        );
        assert!(ManufacturingTrack::parse("quantum").is_err());
    }

    #[test]
    fn manifest_id_is_uuid_shaped() {
        let id = new_manifest_id(42);
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4"); // version nibble
        let bits = id.as_bytes()[19] as char;
        assert!(matches!(bits, '8' | '9' | 'a' | 'b'), "variant nibble, got {bits}");
        assert_eq!(new_manifest_id(42), id); // deterministic per seed
    }
}
