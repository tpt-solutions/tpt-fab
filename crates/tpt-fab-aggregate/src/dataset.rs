//! Open aggregated dataset release (spec 4.9).
//!
//! The aggregated outcome dataset is a natural open release (with attribution to contributing
//! fabs who opted in — never raw data), keeping the project consistent with its own licensing
//! philosophy. Resolved decision (spec Section 6): releases are published to a dedicated
//! **`tpt-fab-data`** repository, keeping large/versioned dataset artifacts out of this code
//! repo's git history. This module emits exactly the artifact that repo would contain —
//! attribution-only, aggregate-only records.

use crate::aggregate::{AggregatePublication, BucketKey};
use crate::json::Json;
use crate::schema::SchemaVersion;
use std::path::Path;

/// A single attribution entry (a fab that opted into public credit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    /// Public contributor name (as the fab wants it credited).
    pub name: String,
    /// Buckets the fab contributed to.
    pub buckets: Vec<BucketKey>,
}

/// A complete dataset release artifact for the `tpt-fab-data` repo.
#[derive(Debug, Clone)]
pub struct DatasetRelease {
    /// Release title (e.g. `"sky130-mpw outcomes, 2026-Q3"`).
    pub title: String,
    /// Publications included (aggregate-only).
    pub publications: Vec<AggregatePublication>,
    /// Attributions to opted-in contributors.
    pub attributions: Vec<Attribution>,
}

impl DatasetRelease {
    /// Serializes the release artifact.
    pub fn to_json(&self) -> Json {
        crate::jobj! {
            "title" => Json::Str(self.title.clone()),
            "dataset_release" => Json::Num(1.0),
            "schema_version" => Json::Str(SchemaVersion::CURRENT.to_string()),
            "publications" => Json::Arr(self.publications.iter().map(|p| p.to_json()).collect()),
            "attributions" => Json::Arr(
                self.attributions
                    .iter()
                    .map(|a| crate::jobj! {
                        "name" => Json::Str(a.name.clone()),
                        "buckets" => Json::Arr(
                            a.buckets.iter().map(|b| crate::jobj! {
                                "node_nm" => Json::Num(b.node_nm as f64),
                                "process" => Json::Str(b.process.clone()),
                                "tech" => Json::Str(b.tech.clone()),
                            }).collect()
                        ),
                    })
                    .collect(),
            ),
            "license" => Json::Str("CC-BY-4.0".into()),
        }
    }

    /// Writes the release artifact to `path` (a file in the `tpt-fab-data` repo). File-out
    /// only — the release is published by whoever moves this file, deliberately, by hand.
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_json().serialize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publication() -> AggregatePublication {
        AggregatePublication {
            bucket: BucketKey {
                node_nm: 130,
                process: "sky130-mpw".into(),
                tech: "duv-multi-pattern".into(),
            },
            n_contributors: 6,
            mean_yield_rate: 0.92,
            mean_abs_geometry_deviation_um: 0.11,
            mean_electrical_ratio: 1.02,
            dp_protected: false,
        }
    }

    #[test]
    fn release_contains_attribution_not_raw_data() {
        let release = DatasetRelease {
            title: "test release".into(),
            publications: vec![publication()],
            attributions: vec![Attribution {
                name: "Fab Alpha (pilot)".into(),
                buckets: vec![BucketKey {
                    node_nm: 130,
                    process: "sky130-mpw".into(),
                    tech: "duv-multi-pattern".into(),
                }],
            }],
        };
        let text = release.to_json().serialize();
        assert!(text.contains("Fab Alpha (pilot)"));
        assert!(text.contains("\"mean_yield_rate\":0.92"));
        // No per-report fields anywhere: no manifest ids, no semantic ids.
        assert!(!text.contains("payload_manifest_id"));
        assert!(!text.contains("feature_id"));
        assert!(!text.contains("link_id"));
    }

    #[test]
    fn release_writes_deterministic_file() {
        let release = DatasetRelease {
            title: "r".into(),
            publications: vec![publication()],
            attributions: vec![],
        };
        let mut p = std::env::temp_dir();
        p.push("tpt-fab-aggregate-test-release.json");
        release.write_to(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, release.to_json().serialize());
        std::fs::remove_file(&p).ok();
    }
}
