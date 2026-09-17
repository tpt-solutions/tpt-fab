# `tpt-fab-data` release runbook

How to take aggregated outcomes from this repo's pipeline to a published release in the
dedicated **`tpt-fab-data`** repository — the resolved home for open dataset artifacts
(spec Section 6 decision: keeps large/versioned dataset files out of this code repo's git
history). Nothing here is automated: every step ends in a file that a human deliberately
moves, consistent with the whole loop's file-exchange philosophy.

## Preconditions

- An `AggregationEngine` with ingested, signature-verified, anomaly-screened contributions
  (manual intake or `tpt-fab-intake`-processed), and at least one bucket at or above the
  N ≥ 5 minimum cohort.
- Attribution names only for fabs that opted into public credit. **Attribution is opt-in;
  absence of an opt-in means no name in the release**, even if their data (with
  `AggregatedContribution` consent) is in the aggregate.

## 1. Produce the publication(s)

```rust,ignore
let bucket = BucketKey { node_nm: 130, process: "sky130-mpw-7".into(), tech: "duv-multi-pattern".into() };
if let Some(p) = engine.publish(&bucket, seed) {
    // p.dp_protected == true → below the cohort gate: DP-noised statistics only.
    // p.dp_protected == false → plain aggregate, publishable as-is.
}
```

Rules of thumb:

- Include DP-protected publications only if the bucket's contributors opted into
  *small-cohort contribution*; they must keep `"dp_protected": true` visible in the artifact
  so downstream users know the noise is there.
- Never publish a bucket whose cohort contains a contributor whose consent was not
  `AggregatedContribution` — the engine enforces this at ingestion; do not work around it.

## 2. Assemble the release artifact

```rust,ignore
let release = DatasetRelease {
    title: "sky130-mpw outcomes, 2026-Q3".into(),
    publications: vec![/* step-1 publications */],
    attributions: vec![/* opted-in fabs only */],
};
release.write_to(Path::new("sky130-mpw-2026-q3.json"))?;
```

The artifact is aggregate-only by construction: bucket summaries, cohort counts, the
`dp_protected` flag, and attribution names. There are no `payload_manifest_id`,
`feature_id`, or `link_id` fields — the aggregate tests assert their absence.

## 3. Verify before publishing

```sh
# No raw-data fields anywhere in the artifact:
grep -E "payload_manifest_id|feature_id|link_id|measured_um|\"measured\"" release.json && echo "FAIL: raw fields present"
# Deterministic bytes (the artifact must be reproducible from the engine):
sha256sum release.json   # compare against a regeneration from the same seed
```

## 4. Land it in `tpt-fab-data`

Layout (append-only; published artifacts are never rewritten):

```
tpt-fab-data/
├── README.md            # purpose, license, how to cite
├── LICENSE              # CC-BY-4.0 (attribution = the opted-in fabs)
└── releases/
    └── 2026-Q3/
        ├── sky130-mpw-2026-q3.json
        └── SHA256SUMS
```

- One directory per release cycle (quarterly or per shuttle batch, whichever cadence the
  data supports — do not pad).
- PR-based: one reviewer confirms step 3's checks and the opt-in attribution list before
  merge.
- Tag the merge commit (`release/2026-Q3`) so citations resolve permanently.

## 5. Update the code repo

Check off the release in `TODO.md` (Phase 4, "Open aggregated dataset release") with the
release tag, so the code repo's history points at the data repo's artifacts without
containing them.
