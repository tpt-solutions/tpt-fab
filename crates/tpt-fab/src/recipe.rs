//! Recipe data model with versioning and drift detection.
//!
//! A [`Recipe`] is an append-only chain of immutable [`RecipeVersion`]s. Drift detection
//! compares a measured equipment parameter set against a recipe version's expected parameters,
//! parameter by parameter, against per-parameter tolerances, and classifies each as in-spec,
//! tolerance (warn), or out-of-spec (fail).

use crate::error::FabError;
use crate::secs::crc32;
use std::collections::BTreeMap;
use std::fmt;
use std::time::SystemTime;

/// A typed parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValue {
    /// Numeric parameter (pressures, temperatures, times, RF powers...).
    Numeric(f64),
    /// Textual parameter (gas names, modes...).
    Text(String),
    /// Boolean parameter.
    Flag(bool),
}

impl fmt::Display for ParameterValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParameterValue::Numeric(v) => write!(f, "{v}"),
            ParameterValue::Text(s) => write!(f, "{s}"),
            ParameterValue::Flag(b) => write!(f, "{b}"),
        }
    }
}

/// Tolerance applied to a numeric parameter during drift detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tolerance {
    /// Allowed absolute deviation, e.g. `±0.5 °C`.
    Absolute(f64),
    /// Allowed relative deviation as a fraction of the expected value, e.g. `±2%` = 0.02.
    Relative(f64),
}

impl Tolerance {
    /// Deviation status for a measured value against an expected value.
    pub fn classify(&self, expected: f64, measured: f64) -> DriftStatus {
        let allowed = match self {
            Tolerance::Absolute(a) => *a,
            Tolerance::Relative(r) => expected.abs() * r,
        };
        let dev = (measured - expected).abs();
        if dev <= allowed {
            DriftStatus::InSpec
        } else if dev <= allowed * 2.0 {
            DriftStatus::Tolerance
        } else {
            DriftStatus::OutOfSpec
        }
    }
}

/// Per-parameter drift status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftStatus {
    /// Within tolerance.
    InSpec,
    /// Outside tolerance but within 2x — flagged for review.
    Tolerance,
    /// Beyond 2x tolerance — recipe must not run.
    OutOfSpec,
}

/// One drifted parameter finding.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftFinding {
    /// Parameter name.
    pub parameter: String,
    /// Expected value from the recipe version.
    pub expected: ParameterValue,
    /// Measured value from the equipment.
    pub measured: ParameterValue,
    /// Classification.
    pub status: DriftStatus,
}

/// Result of a drift check.
#[derive(Debug, Clone)]
pub struct DriftCheck {
    /// Recipe that was checked.
    pub recipe_id: String,
    /// Recipe version that was checked.
    pub recipe_version: u32,
    /// Per-parameter findings.
    pub findings: Vec<DriftFinding>,
}

impl DriftCheck {
    /// Overall status: worst finding across parameters.
    pub fn status(&self) -> DriftStatus {
        self.findings
            .iter()
            .map(|f| f.status)
            .max_by_key(|s| match s {
                DriftStatus::InSpec => 0,
                DriftStatus::Tolerance => 1,
                DriftStatus::OutOfSpec => 2,
            })
            .unwrap_or(DriftStatus::InSpec)
    }

    /// Whether the recipe is safe to run (no OutOfSpec findings).
    pub fn runnable(&self) -> bool {
        self.status() != DriftStatus::OutOfSpec
    }
}

/// One immutable recipe version.
#[derive(Debug, Clone)]
pub struct RecipeVersion {
    /// Monotonically increasing version number (1-based).
    pub number: u32,
    /// Expected parameter set.
    pub parameters: BTreeMap<String, ParameterValue>,
    /// Per-parameter tolerances; missing entries default to ±2% relative.
    pub tolerances: BTreeMap<String, Tolerance>,
    /// Raw recipe body as transferred over S7 (SECS process program).
    pub body: Vec<u8>,
    /// CRC-32 of `body`, computed at construction.
    pub body_crc32: u32,
    /// Who created this version.
    pub author: String,
    /// When this version was created.
    pub created: SystemTime,
}

impl RecipeVersion {
    /// Creates a new version, computing the body checksum.
    pub fn new(
        number: u32,
        parameters: BTreeMap<String, ParameterValue>,
        tolerances: BTreeMap<String, Tolerance>,
        body: Vec<u8>,
        author: impl Into<String>,
    ) -> Self {
        let body_crc32 = crc32(&body);
        RecipeVersion {
            number,
            parameters,
            tolerances,
            body,
            body_crc32,
            author: author.into(),
            created: SystemTime::now(),
        }
    }
}

/// A recipe: identity plus its append-only version chain.
#[derive(Debug, Clone)]
pub struct Recipe {
    /// Recipe ID (PPID convention: short identifier).
    pub id: String,
    /// Human-readable description.
    pub description: String,
    versions: Vec<RecipeVersion>,
}

impl Recipe {
    /// Creates a recipe with version 1.
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        parameters: BTreeMap<String, ParameterValue>,
        tolerances: BTreeMap<String, Tolerance>,
        body: Vec<u8>,
    ) -> Self {
        Recipe {
            id: id.into(),
            description: description.into(),
            versions: vec![RecipeVersion::new(1, parameters, tolerances, body, "system")],
        }
    }

    /// Appends a new version (number = previous + 1); returns its number.
    pub fn add_version(&mut self, mut v: RecipeVersion) -> u32 {
        v.number = self.versions.last().map(|x| x.number + 1).unwrap_or(1);
        let number = v.number;
        self.versions.push(v);
        number
    }

    /// Current (latest) version.
    pub fn latest(&self) -> &RecipeVersion {
        self.versions.last().expect("recipe always has version 1")
    }

    /// Fetches a specific version.
    pub fn version(&self, number: u32) -> Result<&RecipeVersion, FabError> {
        self.versions
            .iter()
            .find(|v| v.number == number)
            .ok_or_else(|| FabError::Recipe(format!("{} has no version {number}", self.id)))
    }

    /// All version numbers.
    pub fn version_numbers(&self) -> Vec<u32> {
        self.versions.iter().map(|v| v.number).collect()
    }
}

/// Store of recipes on the equipment/host side.
#[derive(Debug, Default)]
pub struct RecipeStore {
    recipes: BTreeMap<String, Recipe>,
}

impl RecipeStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a recipe (id must be unique).
    pub fn add(&mut self, recipe: Recipe) -> Result<(), FabError> {
        let id = recipe.id.clone();
        if self.recipes.insert(id.clone(), recipe).is_some() {
            return Err(FabError::Recipe(format!("duplicate recipe {id}")));
        }
        Ok(())
    }

    /// Fetches a recipe.
    pub fn get(&self, id: &str) -> Result<&Recipe, FabError> {
        self.recipes.get(id).ok_or_else(|| FabError::Recipe(format!("unknown recipe {id}")))
    }

    /// Fetches a recipe mutably.
    pub fn get_mut(&mut self, id: &str) -> Result<&mut Recipe, FabError> {
        self.recipes.get_mut(id).ok_or_else(|| FabError::Recipe(format!("unknown recipe {id}")))
    }

    /// All recipe IDs.
    pub fn ids(&self) -> Vec<String> {
        self.recipes.keys().cloned().collect()
    }
}

/// Drift-detection logic: compares `measured` against a recipe version.
///
/// Parameters present in both maps are compared per tolerance (default: relative 2%).
/// Parameters present only in one map produce findings with [`DriftStatus::OutOfSpec`]
/// (missing measurement) or are ignored (extra measurements are informational, not drift).
pub fn detect_drift(
    recipe_id: &str,
    recipe_version: &RecipeVersion,
    measured: &BTreeMap<String, ParameterValue>,
) -> DriftCheck {
    let mut findings = Vec::new();
    for (name, expected) in &recipe_version.parameters {
        let Some(value) = measured.get(name) else {
            findings.push(DriftFinding {
                parameter: name.clone(),
                expected: expected.clone(),
                measured: ParameterValue::Text("<missing>".into()),
                status: DriftStatus::OutOfSpec,
            });
            continue;
        };
        let status = match (expected, value) {
            (ParameterValue::Numeric(e), ParameterValue::Numeric(m)) => recipe_version
                .tolerances
                .get(name)
                .copied()
                .unwrap_or(Tolerance::Relative(0.02))
                .classify(*e, *m),
            (a, b) if a == b => DriftStatus::InSpec,
            _ => DriftStatus::OutOfSpec,
        };
        findings.push(DriftFinding {
            parameter: name.clone(),
            expected: expected.clone(),
            measured: value.clone(),
            status,
        });
    }
    DriftCheck { recipe_id: recipe_id.to_string(), recipe_version: recipe_version.number, findings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BTreeMap<String, ParameterValue> {
        BTreeMap::from([
            ("chamber_pressure_mtorr".into(), ParameterValue::Numeric(50.0)),
            ("rf_power_w".into(), ParameterValue::Numeric(300.0)),
            ("process_time_s".into(), ParameterValue::Numeric(60.0)),
        ])
    }

    fn recipe() -> Recipe {
        let tolerances = BTreeMap::from([
            ("chamber_pressure_mtorr".into(), Tolerance::Absolute(2.0)),
            ("rf_power_w".into(), Tolerance::Relative(0.05)),
        ]);
        Recipe::new("etch-std", "standard poly etch", params(), tolerances, b"recipe-body".to_vec())
    }

    #[test]
    fn versioning_is_append_only() {
        let mut r = recipe();
        assert_eq!(r.version_numbers(), vec![1]);
        let v2 = RecipeVersion::new(
            0, // ignored, reassigned
            params(),
            BTreeMap::new(),
            b"body-v2".to_vec(),
            "process-eng",
        );
        assert_eq!(r.add_version(v2), 2);
        assert_eq!(r.version_numbers(), vec![1, 2]);
        assert_eq!(r.version(1).unwrap().body, b"recipe-body");
        assert_eq!(r.latest().body, b"body-v2");
        assert!(r.version(3).is_err());
    }

    #[test]
    fn body_checksum_detects_tampering() {
        let r = recipe();
        let v = r.latest();
        assert_eq!(v.body_crc32, crc32(b"recipe-body"));
        assert_ne!(v.body_crc32, crc32(b"recipe-bodv"));
    }

    #[test]
    fn drift_classifies_absolute_and_relative() {
        let r = recipe();
        let measured = BTreeMap::from([
            // Absolute tol ±2: dev 1.5 -> in spec.
            ("chamber_pressure_mtorr".into(), ParameterValue::Numeric(51.5)),
            // Relative tol 5%: 5% of 300 = 15; dev 20 -> out of spec (15 < 20 <= 30 → tolerance).
            ("rf_power_w".into(), ParameterValue::Numeric(320.0)),
            ("process_time_s".into(), ParameterValue::Numeric(60.5)),
        ]);
        let check = detect_drift("etch-std", r.latest(), &measured);
        assert_eq!(check.status(), DriftStatus::Tolerance);
        assert!(check.runnable());
        assert_eq!(
            check.findings.iter().find(|f| f.parameter == "rf_power_w").unwrap().status,
            DriftStatus::Tolerance
        );
    }

    #[test]
    fn drift_hard_fails_on_out_of_spec_and_missing() {
        let r = recipe();
        let measured = BTreeMap::from([
            ("chamber_pressure_mtorr".into(), ParameterValue::Numeric(80.0)), // way out
            ("rf_power_w".into(), ParameterValue::Numeric(300.0)),
        ]);
        let check = detect_drift("etch-std", r.latest(), &measured);
        assert_eq!(check.status(), DriftStatus::OutOfSpec);
        assert!(!check.runnable());
        // Missing process_time_s also flags.
        assert!(check
            .findings
            .iter()
            .any(|f| f.parameter == "process_time_s" && f.status == DriftStatus::OutOfSpec));
    }

    #[test]
    fn store_enforces_unique_ids() {
        let mut store = RecipeStore::new();
        store.add(recipe()).unwrap();
        assert!(store.add(recipe()).is_err());
        assert_eq!(store.ids(), vec!["etch-std"]);
        assert!(store.get("etch-std").is_ok());
        assert!(store.get("nope").is_err());
    }
}
