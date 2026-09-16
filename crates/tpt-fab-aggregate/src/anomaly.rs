//! Anomaly / anti-poisoning detection (spec 4.6).
//!
//! A shared fine-tuning signal is only as good as its worst contributor — and once
//! contributing unlocks tooling (spec 4.7), it also unlocks an incentive to game the system.
//! Before any report feeds a model (`tpt-ai`'s yield heatmap, `tpt-silicon-si-pi`'s solver
//! calibration, `tpt-fab-process`'s OPC/etch corrections), it is screened against the
//! existing aggregate distribution and flagged for review when far outside it.

use crate::error::AggregateError;
use crate::outcome::OutcomeReport;

/// A running distribution tracker (mean / MAD-based, robust to the outliers it flags).
#[derive(Debug, Clone, Default)]
pub struct Distribution {
    values: Vec<f64>,
}

impl Distribution {
    /// Empty distribution.
    pub fn new() -> Self {
        Self::default()
    }

    /// Observes a value.
    pub fn observe(&mut self, v: f64) {
        if v.is_finite() {
            self.values.push(v);
        }
    }

    /// Number of observations.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn median_of(&self, sorted: &[f64]) -> f64 {
        let n = sorted.len();
        if n == 0 {
            return 0.0;
        }
        if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        }
    }

    /// Median.
    pub fn median(&self) -> f64 {
        let mut sorted = self.values.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        self.median_of(&sorted)
    }

    /// Median absolute deviation (MAD).
    pub fn mad(&self) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        let med = self.median();
        let mut devs: Vec<f64> = self.values.iter().map(|v| (v - med).abs()).collect();
        devs.sort_by(|a, b| a.total_cmp(b));
        self.median_of(&devs)
    }

    /// Robust z-score of `v` against this distribution (MAD-scaled).
    ///
    /// Degenerate history (all observations identical, MAD = 0) is handled explicitly: any
    /// deviation from the constant median is infinitely surprising, so the score is
    /// infinite — while an identical value scores 0.
    pub fn robust_z(&self, v: f64) -> f64 {
        let med = self.median();
        let mad = self.mad();
        if mad == 0.0 {
            if v == med {
                0.0
            } else if v > med {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            }
        } else {
            (v - med) / (1.4826 * mad)
        }
    }
}

/// Screening result for one report.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenResult {
    /// Whether the report was flagged for human review.
    pub flagged: bool,
    /// Field-level findings (name, robust z-score).
    pub findings: Vec<(String, f64)>,
}

/// Anti-poisoning screener with per-metric distributions.
#[derive(Debug)]
pub struct AnomalyDetector {
    yield_dist: Distribution,
    geometry_dist: Distribution,
    electrical_dist: Distribution,
    /// Flag when |robust z| exceeds this (in MADs).
    pub z_threshold: f64,
    /// Do not screen (only accumulate) until at least this many observations exist.
    pub min_history: usize,
}

impl AnomalyDetector {
    /// Detector with the default threshold (4 MADs) and minimum history (8 observations).
    pub fn new() -> Self {
        AnomalyDetector {
            yield_dist: Distribution::new(),
            geometry_dist: Distribution::new(),
            electrical_dist: Distribution::new(),
            z_threshold: 4.0,
            min_history: 8,
        }
    }

    /// Screens a report against accumulated history. **Call this before letting a report
    /// feed any model.** Flagged reports go to review; they are never silently absorbed.
    /// The report's metrics are added to history either way (history reflects what arrived,
    /// not what was accepted).
    pub fn screen(&mut self, report: &OutcomeReport) -> ScreenResult {
        let yield_rate = report.yield_outcome.yield_rate();
        let geom_dev = mean_abs_geometry_deviation(report);
        let elec_ratio = mean_electrical_ratio(report);

        let mut findings = Vec::new();
        if self.yield_dist.len() >= self.min_history {
            let z = self.yield_dist.robust_z(yield_rate);
            if z.abs() > self.z_threshold {
                findings.push(("yield_rate".into(), z));
            }
        }
        if let Some(g) = geom_dev.filter(|_| self.geometry_dist.len() >= self.min_history) {
            let z = self.geometry_dist.robust_z(g);
            if z.abs() > self.z_threshold {
                findings.push(("geometry_deviation_um".into(), z));
            }
        }
        if let Some(e) = elec_ratio.filter(|_| self.electrical_dist.len() >= self.min_history) {
            let z = self.electrical_dist.robust_z(e);
            if z.abs() > self.z_threshold {
                findings.push(("electrical_ratio".into(), z));
            }
        }

        // Accumulate history.
        self.yield_dist.observe(yield_rate);
        if let Some(g) = geom_dev {
            self.geometry_dist.observe(g);
        }
        if let Some(e) = elec_ratio {
            self.electrical_dist.observe(e);
        }

        ScreenResult { flagged: !findings.is_empty(), findings }
    }
}

impl Default for AnomalyDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn mean_abs_geometry_deviation(report: &OutcomeReport) -> Option<f64> {
    if report.measured_geometry.is_empty() {
        return None;
    }
    let sum: f64 =
        report.measured_geometry.iter().map(|g| (g.measured_um - g.designed_um).abs()).sum();
    Some(sum / report.measured_geometry.len() as f64)
}

fn mean_electrical_ratio(report: &OutcomeReport) -> Option<f64> {
    let ratios: Vec<f64> = report
        .electrical_test
        .iter()
        .filter(|e| e.designed != 0.0)
        .map(|e| e.measured / e.designed)
        .collect();
    if ratios.is_empty() {
        None
    } else {
        Some(ratios.iter().sum::<f64>() / ratios.len() as f64)
    }
}

/// Validates basic sanity on a parsed report before anything else runs with it.
pub fn validate_report_basics(report: &OutcomeReport) -> Result<(), AggregateError> {
    if report.payload_manifest_id.is_empty() {
        return Err(AggregateError::InvalidInput("empty payload_manifest_id".into()));
    }
    let y = &report.yield_outcome;
    if y.units_good > y.units_started {
        return Err(AggregateError::InvalidInput(format!(
            "yield claims {} good of {} started — impossible",
            y.units_good, y.units_started
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{
        ConsentScope, ElectricalMeasurement, GeometryDeviation, ManufacturingTrack, SemanticId,
        YieldSummary,
    };
    use crate::schema::SchemaVersion;

    fn report(yield_rate: f64, geom_dev: f64, elec_ratio: f64) -> OutcomeReport {
        OutcomeReport {
            payload_manifest_id: "m".into(),
            track: ManufacturingTrack::Pcb,
            measured_geometry: vec![GeometryDeviation {
                feature_id: SemanticId::new("f").unwrap(),
                designed_um: 10.0,
                measured_um: 10.0 + geom_dev,
            }],
            electrical_test: vec![ElectricalMeasurement {
                link_id: SemanticId::new("l").unwrap(),
                quantity: "impedance".into(),
                designed: 50.0,
                measured: 50.0 * elec_ratio,
                unit: "ohm".into(),
            }],
            yield_outcome: YieldSummary {
                units_started: 1000,
                units_good: (yield_rate * 1000.0) as u64,
                dominant_failure_mode: None,
            },
            process_notes: vec![],
            consent: ConsentScope::AggregatedContribution,
            schema_version: SchemaVersion::CURRENT,
            signature: None,
        }
    }

    #[test]
    fn impossible_yield_is_rejected() {
        let mut r = report(0.5, 0.1, 1.0);
        r.yield_outcome.units_good = r.yield_outcome.units_started + 1;
        assert!(validate_report_basics(&r).is_err());
        assert!(validate_report_basics(&report(0.5, 0.1, 1.0)).is_ok());
    }

    #[test]
    fn normal_stream_not_flagged() {
        let mut det = AnomalyDetector::new();
        // Build history with mild variation.
        for i in 0..20 {
            let y = 0.90 + 0.01 * ((i % 5) as f64 - 2.0);
            let res = det.screen(&report(y, 0.1, 1.0));
            assert!(!res.flagged, "normal report flagged: {res:?}");
        }
    }

    #[test]
    fn poisoned_outlier_is_flagged_for_review() {
        let mut det = AnomalyDetector::new();
        for _ in 0..12 {
            det.screen(&report(0.9, 0.1, 1.0));
        }
        // A "too good to be true" yield plus wild geometry deviation.
        let res = det.screen(&report(0.995, 5.0, 1.0));
        assert!(res.flagged);
        let names: Vec<&str> = res.findings.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"geometry_deviation_um"), "{names:?}");
    }

    #[test]
    fn min_history_prevents_premature_flagging() {
        let mut det = AnomalyDetector::new();
        det.min_history = 30;
        // Even an extreme outlier is not screened until history exists.
        let res = det.screen(&report(0.999, 9.0, 2.0));
        assert!(!res.flagged);
    }
}
