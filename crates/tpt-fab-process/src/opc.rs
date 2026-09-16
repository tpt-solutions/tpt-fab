//! OPC suggestion engine, keyed to the active patterning backend.
//!
//! Rule-based and deliberately coarse (see the crate-level non-goal): dense lines undersize,
//! line ends pull back, corners round, vias shrink. The engine translates those published
//! first-order effects into per-feature mask bias suggestions. The illumination parameters
//! (wavelength, NA) come from the [`PatterningTech`] of the installed backend — the same
//! layer analyzed against a different backend produces different suggestions, which is the
//! point of keying OPC to the backend.

use tpt_fab_litho::PatterningTech;

/// Feature shapes the engine understands (from a layout extraction upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureKind {
    /// A line segment.
    Line,
    /// A space between lines.
    Space,
    /// A line end (pullback-sensitive).
    LineEnd,
    /// A corner (rounding-sensitive).
    Corner,
    /// A via or contact hole.
    Via,
    /// An isolated island pad.
    Island,
}

/// One extracted layout feature.
#[derive(Debug, Clone)]
pub struct DrawFeature {
    /// Semantic feature ID (matches the IDs later used in outcome reports).
    pub id: String,
    /// Shape kind.
    pub kind: FeatureKind,
    /// Drawn width, nm.
    pub width_nm: f64,
    /// Pitch to the nearest parallel neighbor, nm (`None` = isolated).
    pub pitch_nm: Option<f64>,
}

/// Illumination parameters resolved from the backend technology.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Illumination {
    /// Exposure wavelength, nm.
    pub wavelength_nm: f64,
    /// Numerical aperture.
    pub na: f64,
}

impl Illumination {
    /// Resolves the published reference illumination for a technology.
    pub fn for_tech(tech: PatterningTech) -> Option<Illumination> {
        match tech {
            // Generic ArF immersion characteristics (DuvProfile::GenericArFi).
            PatterningTech::DuvMultiPattern => {
                Some(Illumination { wavelength_nm: 193.0, na: 1.35 })
            }
            PatterningTech::Euv => Some(Illumination { wavelength_nm: 13.5, na: 0.33 }),
            // NIL replicates a template: there is no aerial image to correct.
            PatterningTech::Nil => None,
            // E-beam has no optical proximity effect; corrections are proximity-effect
            // corrections handled at write time, not mask OPC.
            PatterningTech::Ebeam => None,
        }
    }

    /// k1 factor for a pitch: `pitch * NA / wavelength`. Printable above ~0.25-0.3.
    pub fn k1(&self, pitch_nm: f64) -> f64 {
        pitch_nm * self.na / self.wavelength_nm
    }
}

/// One bias suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct OpcSuggestion {
    /// Feature the suggestion applies to.
    pub feature_id: String,
    /// Suggested mask bias in nm (positive = draw larger).
    pub suggested_bias_nm: f64,
    /// Why — surfaced so a human can review (and eventually calibrate) the rule.
    pub rationale: String,
}

/// Full OPC report for one layer.
#[derive(Debug, Clone)]
pub struct OpcReport {
    /// Technology the suggestions are keyed to.
    pub tech: PatterningTech,
    /// Resolved illumination (`None` for NIL/E-beam, see [`Illumination::for_tech`]).
    pub illumination: Option<Illumination>,
    /// Suggestions, one per feature the engine has a rule for.
    pub suggestions: Vec<OpcSuggestion>,
    /// Notes (e.g. why the engine produced no suggestions).
    pub notes: Vec<String>,
}

/// OPC engine bound to a backend technology.
#[derive(Debug, Clone, Copy)]
pub struct OpcEngine {
    tech: PatterningTech,
    /// Reference dense pitch (nm) at which undersizing peaks.
    ref_dense_pitch_nm: f64,
    /// Maximum line bias applied at the tightest printable pitch.
    max_line_bias_nm: f64,
    /// Line-end pullback compensation.
    line_end_bias_nm: f64,
    /// Corner serif bias.
    corner_bias_nm: f64,
    /// Via size bias.
    via_bias_nm: f64,
}

impl OpcEngine {
    /// Engine with published-heuristic default biases.
    pub fn new(tech: PatterningTech) -> Self {
        OpcEngine {
            tech,
            ref_dense_pitch_nm: 200.0,
            max_line_bias_nm: 6.0,
            line_end_bias_nm: 12.0,
            corner_bias_nm: 8.0,
            via_bias_nm: 4.0,
        }
    }

    /// Overrides the line-end pullback compensation (e.g. from calibrated outcome data).
    pub fn with_biases(
        mut self,
        ref_dense_pitch_nm: f64,
        max_line_bias_nm: f64,
        line_end_bias_nm: f64,
        corner_bias_nm: f64,
        via_bias_nm: f64,
    ) -> Self {
        self.ref_dense_pitch_nm = ref_dense_pitch_nm;
        self.max_line_bias_nm = max_line_bias_nm;
        self.line_end_bias_nm = line_end_bias_nm;
        self.corner_bias_nm = corner_bias_nm;
        self.via_bias_nm = via_bias_nm;
        self
    }

    /// Analyzes a layer and produces suggestions.
    pub fn analyze(&self, features: &[DrawFeature]) -> OpcReport {
        let illumination = Illumination::for_tech(self.tech);
        let mut suggestions = Vec::new();
        let mut notes = Vec::new();

        let Some(illum) = illumination else {
            notes.push(match self.tech {
                PatterningTech::Nil => "NIL replicates the template 1:1; OPC is not applicable. \
                    Template QC (defect density, separation force) is the equivalent control."
                    .into(),
                PatterningTech::Ebeam => {
                    "E-beam has no optical proximity effect; shape corrections \
                    are proximity-effect corrections applied at write time by the backend."
                        .into()
                }
                _ => unreachable!("no illumination only for NIL/E-beam"),
            });
            return OpcReport { tech: self.tech, illumination, suggestions, notes };
        };

        for f in features {
            match f.kind {
                FeatureKind::Line | FeatureKind::Space => {
                    let Some(pitch) = f.pitch_nm else {
                        suggestions.push(OpcSuggestion {
                            feature_id: f.id.clone(),
                            suggested_bias_nm: 0.5 * self.max_line_bias_nm,
                            rationale: "isolated feature: moderate assist bias".into(),
                        });
                        continue;
                    };
                    let k1 = illum.k1(pitch);
                    if k1 < 0.35 {
                        // Tight pitch: strongest undersizing, scale bias by how tight.
                        let tightness = (0.35 - k1) / 0.35;
                        let bias = self.max_line_bias_nm * tightness;
                        let dir = if f.kind == FeatureKind::Line { bias } else { -bias };
                        suggestions.push(OpcSuggestion {
                            feature_id: f.id.clone(),
                            suggested_bias_nm: dir,
                            rationale: format!(
                                "dense pitch {pitch:.0} nm (k1={k1:.2}): line prints thin, \
                                 line bias +{bias:.1} nm / space bias −{bias:.1} nm"
                            ),
                        });
                    }
                }
                FeatureKind::LineEnd => {
                    suggestions.push(OpcSuggestion {
                        feature_id: f.id.clone(),
                        suggested_bias_nm: self.line_end_bias_nm,
                        rationale: format!(
                            "line-end pullback: extend end by {:.1} nm (DRC cannot see this)",
                            self.line_end_bias_nm
                        ),
                    });
                }
                FeatureKind::Corner => {
                    suggestions.push(OpcSuggestion {
                        feature_id: f.id.clone(),
                        suggested_bias_nm: self.corner_bias_nm,
                        rationale: format!(
                            "corner rounding: add {:.1} nm serif mass (DRC cannot see this)",
                            self.corner_bias_nm
                        ),
                    });
                }
                FeatureKind::Via => {
                    suggestions.push(OpcSuggestion {
                        feature_id: f.id.clone(),
                        suggested_bias_nm: self.via_bias_nm,
                        rationale: format!(
                            "via undersizing: bias hole +{:.1} nm",
                            self.via_bias_nm
                        ),
                    });
                }
                FeatureKind::Island => {
                    suggestions.push(OpcSuggestion {
                        feature_id: f.id.clone(),
                        suggested_bias_nm: 0.5 * self.max_line_bias_nm,
                        rationale: "island pad: moderate bias for isolated undersizing".into(),
                    });
                }
            }
        }
        OpcReport { tech: self.tech, illumination, suggestions, notes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer() -> Vec<DrawFeature> {
        vec![
            DrawFeature {
                id: "dense-line".into(),
                kind: FeatureKind::Line,
                width_nm: 130.0,
                pitch_nm: Some(160.0),
            },
            DrawFeature {
                id: "iso-line".into(),
                kind: FeatureKind::Line,
                width_nm: 130.0,
                pitch_nm: None,
            },
            DrawFeature {
                id: "end-1".into(),
                kind: FeatureKind::LineEnd,
                width_nm: 130.0,
                pitch_nm: None,
            },
            DrawFeature {
                id: "corner-1".into(),
                kind: FeatureKind::Corner,
                width_nm: 130.0,
                pitch_nm: None,
            },
            DrawFeature {
                id: "via-1".into(),
                kind: FeatureKind::Via,
                width_nm: 150.0,
                pitch_nm: None,
            },
        ]
    }

    #[test]
    fn dense_features_get_pitch_dependent_bias() {
        let engine = OpcEngine::new(PatterningTech::DuvMultiPattern);
        // 160 nm pitch at 193i is k1≈1.12 — comfortably printable, no suggestion.
        let relaxed = vec![DrawFeature {
            id: "relaxed".into(),
            kind: FeatureKind::Line,
            width_nm: 130.0,
            pitch_nm: Some(160.0),
        }];
        assert!(!engine.analyze(&relaxed).suggestions.iter().any(|s| s.feature_id == "relaxed"));
        // A genuinely tight pitch (k1 < 0.35) gets a positive line bias.
        let tight = vec![DrawFeature {
            id: "tight".into(),
            kind: FeatureKind::Line,
            width_nm: 20.0,
            pitch_nm: Some(45.0),
        }];
        let report = engine.analyze(&tight);
        let s = report
            .suggestions
            .iter()
            .find(|s| s.feature_id == "tight")
            .expect("tight pitch must be analyzed");
        assert!(s.suggested_bias_nm > 0.0, "tight line bias should be positive");
        assert!(s.rationale.contains("k1"));
    }

    #[test]
    fn line_end_and_corner_always_flagged() {
        let engine = OpcEngine::new(PatterningTech::Euv);
        let report = engine.analyze(&layer());
        let end = report
            .suggestions
            .iter()
            .find(|s| s.feature_id == "end-1")
            .expect("line end must be flagged");
        assert!(end.suggested_bias_nm > 0.0);
        assert!(end.rationale.contains("DRC cannot see this"));
        let corner = report
            .suggestions
            .iter()
            .find(|s| s.feature_id == "corner-1")
            .expect("corner must be flagged");
        assert!(corner.suggested_bias_nm > 0.0);
    }

    #[test]
    fn nil_and_ebeam_produce_no_optical_suggestions() {
        for tech in [PatterningTech::Nil, PatterningTech::Ebeam] {
            let report = OpcEngine::new(tech).analyze(&layer());
            assert!(report.suggestions.is_empty(), "{tech:?} should have no OPC suggestions");
            assert!(!report.notes.is_empty(), "{tech:?} should explain why");
        }
    }

    #[test]
    fn suggestions_track_the_active_backends_optics() {
        // 30 nm pitch: DUV immersion k1 = 30*1.35/193 ≈ 0.21 — below the printable OPC
        // threshold, so the DUV-keyed engine flags it. EUV k1 = 30*0.33/13.5 ≈ 0.73 —
        // comfortable, so the EUV-keyed engine says nothing. Same geometry, different
        // backend, different suggestions: OPC is keyed to the backend.
        let tight = vec![DrawFeature {
            id: "tight".into(),
            kind: FeatureKind::Line,
            width_nm: 15.0,
            pitch_nm: Some(30.0),
        }];
        let duv = OpcEngine::new(PatterningTech::DuvMultiPattern).analyze(&tight);
        assert!(duv.suggestions.iter().any(|s| s.feature_id == "tight"));
        let euv = OpcEngine::new(PatterningTech::Euv).analyze(&tight);
        assert!(!euv.suggestions.iter().any(|s| s.feature_id == "tight"));
    }
}
