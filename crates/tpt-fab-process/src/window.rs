//! Coarse etch/deposition/thermal process simulation with process-window violation checks.
//!
//! Deliberately simple physics (crate non-goal applies): systematic radial rate variation
//! across a five-site wafer map, linear depth/thickness response, additive thermal budget.
//! The value is the violation bookkeeping — flagging when a process step would run outside
//! its window — not predictive accuracy.

/// An acceptable range for a process result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessWindow {
    /// Lower acceptable bound.
    pub min: f64,
    /// Upper acceptable bound.
    pub max: f64,
}

impl ProcessWindow {
    /// Builds a window.
    pub fn new(min: f64, max: f64) -> Self {
        ProcessWindow { min, max }
    }

    /// Whether a value is inside the window.
    pub fn contains(&self, v: f64) -> bool {
        (self.min..=self.max).contains(&v)
    }

    /// Signed margin to the nearest edge (negative = outside).
    pub fn margin(&self, v: f64) -> f64 {
        if v < self.min {
            v - self.min
        } else if v > self.max {
            v - self.max
        } else {
            0.0
        }
    }
}

/// Sites of the standard five-site wafer map, with relative etch/deposition rate
/// (1.0 = perfect uniformity).
const SITES: [(&str, f64); 5] =
    [("center", 1.0), ("north", 1.02), ("south", 0.98), ("east", 1.03), ("west", 0.97)];

/// One process-window violation.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    /// Which check fired.
    pub kind: ViolationKind,
    /// Site or feature the violation occurred at.
    pub location: String,
    /// Value that violated the window.
    pub value: f64,
    /// Signed distance to the window edge (negative = outside by this much).
    pub margin: f64,
    /// Human-readable summary.
    pub summary: String,
}

/// Violation categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// Etch depth left the window.
    EtchDepth,
    /// Critical dimension after etch left the window.
    EtchCriticalDimension,
    /// Deposited thickness left the window.
    DepositionThickness,
    /// Cumulative thermal budget exceeded.
    ThermalBudget,
}

/// Etch step parameters.
#[derive(Debug, Clone)]
pub struct EtchParams {
    /// Target etch depth, nm.
    pub target_depth_nm: f64,
    /// Within-wafer non-uniformity (fraction, e.g. 0.05 = ±5%).
    pub non_uniformity: f64,
    /// Critical dimension after etch, nm (single value target for the checked feature).
    pub post_etch_cd_nm: f64,
    /// Acceptable depth window, nm.
    pub depth_window: ProcessWindow,
    /// Acceptable CD window, nm.
    pub cd_window: ProcessWindow,
}

/// Deposition step parameters.
#[derive(Debug, Clone)]
pub struct DepositionParams {
    /// Target thickness, nm.
    pub target_thickness_nm: f64,
    /// Within-wafer non-uniformity (fraction).
    pub non_uniformity: f64,
    /// Acceptable thickness window, nm.
    pub thickness_window: ProcessWindow,
}

/// Thermal budget parameters.
#[derive(Debug, Clone)]
pub struct ThermalParams {
    /// Maximum cumulative budget, °C·minutes (front-end-of-line style budget).
    pub budget_c_min: f64,
    /// Process steps as (temperature °C, minutes).
    pub steps: Vec<(f64, f64)>,
}

/// Simulates an etch step across the five-site map; returns any window violations.
///
/// The modeled depth at site `s` is `target * (1 + non_uniformity * site_offset/0.03)` with
/// the site offsets from the map above, so a step at the edge of the window in the center
/// still falls out of the window at the worst site — the class of mistake fabs catch (or
/// miss) today.
pub fn simulate_etch(params: &EtchParams) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (site, rate) in SITES {
        // Rate variation scaled against the map's nominal 1.03 worst case.
        let depth = params.target_depth_nm * (1.0 + params.non_uniformity * (rate - 1.0) / 0.03);
        if !params.depth_window.contains(depth) {
            violations.push(Violation {
                kind: ViolationKind::EtchDepth,
                location: site.into(),
                value: depth,
                margin: params.depth_window.margin(depth),
                summary: format!(
                    "etch depth {depth:.1} nm at {site} outside [{:.1}, {:.1}] nm",
                    params.depth_window.min, params.depth_window.max
                ),
            });
        }
        // CD grows with over-etch (isotropic component): modeled as depth-proportional.
        let cd = params.post_etch_cd_nm * depth / params.target_depth_nm;
        if !params.cd_window.contains(cd) {
            violations.push(Violation {
                kind: ViolationKind::EtchCriticalDimension,
                location: site.into(),
                value: cd,
                margin: params.cd_window.margin(cd),
                summary: format!(
                    "post-etch CD {cd:.1} nm at {site} outside [{:.1}, {:.1}] nm",
                    params.cd_window.min, params.cd_window.max
                ),
            });
        }
    }
    violations
}

/// Simulates a deposition step across the five-site map.
pub fn simulate_deposition(params: &DepositionParams) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (site, rate) in SITES {
        let thickness =
            params.target_thickness_nm * (1.0 + params.non_uniformity * (rate - 1.0) / 0.03);
        if !params.thickness_window.contains(thickness) {
            violations.push(Violation {
                kind: ViolationKind::DepositionThickness,
                location: site.into(),
                value: thickness,
                margin: params.thickness_window.margin(thickness),
                summary: format!(
                    "deposited {thickness:.1} nm at {site} outside [{:.1}, {:.1}] nm",
                    params.thickness_window.min, params.thickness_window.max
                ),
            });
        }
    }
    violations
}

/// Checks a thermal sequence against the cumulative budget.
pub fn simulate_thermal(params: &ThermalParams) -> Vec<Violation> {
    let total: f64 = params.steps.iter().map(|(t, m)| t * m).sum();
    if total > params.budget_c_min {
        vec![Violation {
            kind: ViolationKind::ThermalBudget,
            location: "wafer".into(),
            value: total,
            margin: total - params.budget_c_min,
            summary: format!(
                "cumulative thermal budget {total:.0} °C·min exceeds {0:.0} °C·min",
                params.budget_c_min
            ),
        }]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal_etch_is_clean() {
        let params = EtchParams {
            target_depth_nm: 300.0,
            non_uniformity: 0.03,
            post_etch_cd_nm: 130.0,
            depth_window: ProcessWindow::new(285.0, 315.0),
            cd_window: ProcessWindow::new(125.0, 135.0),
        };
        assert_eq!(simulate_etch(&params), Vec::new());
    }

    #[test]
    fn over_etching_violates_depth_and_cd() {
        let params = EtchParams {
            target_depth_nm: 340.0, // window caps at 315
            non_uniformity: 0.03,
            post_etch_cd_nm: 140.0,
            depth_window: ProcessWindow::new(285.0, 315.0),
            cd_window: ProcessWindow::new(125.0, 135.0),
        };
        let v = simulate_etch(&params);
        assert!(v.iter().any(|x| x.kind == ViolationKind::EtchDepth));
        assert!(v.iter().any(|x| x.kind == ViolationKind::EtchCriticalDimension));
        assert!(v.iter().all(|x| x.margin > 0.0), "violations report how far outside");
    }

    #[test]
    fn uniformity_can_push_edge_sites_out_while_center_stays_in() {
        // Center depth exactly on target; a large non-uniformity drives the fast site out.
        let params = EtchParams {
            target_depth_nm: 315.0,
            non_uniformity: 0.10,
            post_etch_cd_nm: 130.0,
            depth_window: ProcessWindow::new(285.0, 315.0),
            cd_window: ProcessWindow::new(125.0, 135.0),
        };
        let v = simulate_etch(&params);
        assert!(v.iter().any(|x| x.location == "east" && x.kind == ViolationKind::EtchDepth));
        assert!(!v.iter().any(|x| x.location == "center"));
    }

    #[test]
    fn deposition_and_thermal_checks() {
        let dep = DepositionParams {
            target_thickness_nm: 500.0,
            non_uniformity: 0.06,
            thickness_window: ProcessWindow::new(490.0, 510.0),
        };
        let v = simulate_deposition(&dep);
        assert!(v.iter().any(|x| x.kind == ViolationKind::DepositionThickness));

        let thermal =
            ThermalParams { budget_c_min: 45_000.0, steps: vec![(450.0, 60.0), (400.0, 60.0)] };
        let v = simulate_thermal(&thermal);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, ViolationKind::ThermalBudget);
    }
}
