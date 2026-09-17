//! The intake ledger: an append-record plus screening-history persistence file.
//!
//! The ledger is what makes the queue *semi*-automated and its decisions reviewable: every
//! file that passed through gets a decision record (file, disposition, reason, timestamp),
//! and the anomaly-screening distributions persist across runs so later files are screened
//! against everything seen before, not just this batch. Plain JSON, written locally next to
//! the dispositions — no egress, in keeping with the rest of this stack.

use std::path::Path;

use tpt_fab_aggregate::json::Json;
use tpt_fab_aggregate::{jobj, AggregateError};

/// One processed file's record.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRecord {
    /// File name as delivered (basename, not the full path).
    pub file: String,
    /// Disposition applied.
    pub disposition: String,
    /// Why — surfaced for the human reviewer.
    pub reason: String,
    /// Unix seconds at decision time.
    pub at_unix: u64,
    /// Consent scope carried by the report (recorded so downstream humans know what they
    /// may legally do with the data; empty when the file never parsed).
    pub consent: String,
}

/// Persisted queue state: decisions plus the anomaly-screening history.
#[derive(Debug, Default, Clone)]
pub struct IntakeLedger {
    /// Decision records, oldest first.
    pub decisions: Vec<DecisionRecord>,
    /// Screening history: yield-rate observations.
    pub history_yield: Vec<f64>,
    /// Screening history: mean geometry-deviation (µm) observations.
    pub history_geometry: Vec<f64>,
    /// Screening history: electrical measured/designed ratio observations.
    pub history_electrical: Vec<f64>,
}

impl IntakeLedger {
    /// Loads a ledger; a missing file is an empty ledger (first run).
    pub fn load(path: &Path) -> Result<IntakeLedger, AggregateError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(IntakeLedger::default())
            }
            Err(e) => return Err(AggregateError::Io(e)),
        };
        let v = Json::parse(&text)?;
        let mut ledger = IntakeLedger::default();
        if let Some(items) = v.get("decisions").and_then(Json::as_arr) {
            for d in items {
                ledger.decisions.push(DecisionRecord {
                    file: d.get("file").and_then(Json::as_str).unwrap_or_default().into(),
                    disposition: d
                        .get("disposition")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .into(),
                    reason: d.get("reason").and_then(Json::as_str).unwrap_or_default().into(),
                    at_unix: d.get("at_unix").and_then(Json::as_f64).unwrap_or(0.0) as u64,
                    consent: d.get("consent").and_then(Json::as_str).unwrap_or_default().into(),
                });
            }
        }
        if let Some(h) = v.get("history") {
            let pull = |key: &str| -> Vec<f64> {
                h.get(key)
                    .and_then(Json::as_arr)
                    .map(|a| a.iter().filter_map(Json::as_f64).collect())
                    .unwrap_or_default()
            };
            ledger.history_yield = pull("yield");
            ledger.history_geometry = pull("geometry");
            ledger.history_electrical = pull("electrical");
        }
        Ok(ledger)
    }

    /// Appends a decision record.
    pub fn record(&mut self, record: DecisionRecord) {
        self.decisions.push(record);
    }

    /// Appends one report's screening metrics to the persisted history.
    pub fn observe_metrics(
        &mut self,
        yield_rate: f64,
        geometry: Option<f64>,
        electrical: Option<f64>,
    ) {
        self.history_yield.push(yield_rate);
        if let Some(g) = geometry {
            self.history_geometry.push(g);
        }
        if let Some(e) = electrical {
            self.history_electrical.push(e);
        }
    }

    /// Serializes the ledger (deterministic JSON).
    pub fn to_json(&self) -> Json {
        jobj! {
            "decisions" => Json::Arr(
                self.decisions.iter().map(|d| jobj! {
                    "file" => Json::Str(d.file.clone()),
                    "disposition" => Json::Str(d.disposition.clone()),
                    "reason" => Json::Str(d.reason.clone()),
                    "at_unix" => Json::Num(d.at_unix as f64),
                    "consent" => Json::Str(d.consent.clone()),
                }).collect()
            ),
            "history" => jobj! {
                "yield" => Json::Arr(self.history_yield.iter().map(|v| Json::Num(*v)).collect()),
                "geometry" => Json::Arr(self.history_geometry.iter().map(|v| Json::Num(*v)).collect()),
                "electrical" => Json::Arr(self.history_electrical.iter().map(|v| Json::Num(*v)).collect()),
            },
        }
    }

    /// Writes the ledger atomically enough for a single-operator tool: write to a `.tmp`
    /// sibling, then rename over the target.
    pub fn save(&self, path: &Path) -> Result<(), AggregateError> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_json().serialize())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Unix seconds now (ledger timestamps; no external time crate).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
