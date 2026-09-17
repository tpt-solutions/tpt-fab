# AGENTS.md — tpt-fab

Rust Cargo workspace, standalone repo (`tpt-solutions/tpt-fab`) — **no shared Cargo workspace**
with `tpt-silicon` / `tpt-protocol` / `tpt-telos`. Source of truth: `spec.txt` (RFC-002);
build progress: `TODO.md`.

## Build & verify (CI gates, in this order)
- `cargo fmt --all -- --check` — format gate (`rustfmt.toml`: edition 2021, max_width 100,
  `use_small_heuristics = "Max"`).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — warnings denied
  (workspace `[workspace.lints.clippy] all = "warn"`; CI promotes to deny).
- `cargo test --workspace` — all tests.

CI sets `RUSTFLAGS=-D warnings`, so keep the build warning-clean locally too.

## Crate layout & dependency direction
```
tpt-fab-litho      (leaf — PatterningBackend trait + DUV/EUV/NIL/E-beam backends)
tpt-fab-aggregate  (leaf — outcome file exchange; ZERO dependencies, ZERO network code)
tpt-fab            → depends on tpt-fab-litho (SECS/GEM core, recipe/lot/APC, simulator)
tpt-fab-process    → depends on tpt-fab-litho + tpt-fab-aggregate (OPC/process sim, sky130 ingestion)
tpt-fab-intake     → depends on tpt-fab-aggregate (receiver-side semi-automated intake queue CLI; local files only, no egress)
```
- `tpt-fab-aggregate` has a hard acceptance criterion: **no network-capable dependency in its
  Cargo.toml, no egress code path at all.** It is currently dependency-free (`std` only); keep it
  that way. Its whole trust model is "read the code, verify there is no send step."
- `tpt-fab` core must not match on lithography technology specifics — everything technology-
  specific lives behind the `PatterningBackend` trait in `tpt-fab-litho`. Swapping the backend
  must not change recipe/lot/APC logic (Phase 2 milestone; there is a test asserting this).
- Vendor-proprietary SECS/GEM extensions are excluded from core. The only sanctioned seam is
  `tpt_fab::adapter::VendorAdapter` — opt-in, registered explicitly at runtime, default none.

## Per-crate docs & metadata (house convention, mirror `tpt-telos`/`tpt-protocol`)
- Every crate has its own `README.md` (positioning, usage, module map, design notes) and its
  own `CHANGELOG.md` in the crate directory (Keep a Changelog / SemVer format). Entries must
  match what is **actually published on crates.io** — all four crates are unpublished, so each
  changelog has only an `## [Unreleased]` section. After `cargo publish`, promote the
  `Unreleased` entry to a dated `[x.y.z]` section.
- Every crate manifest carries `keywords` (max 5) and `categories` (crates.io slugs, max 5)
  plus `readme = "README.md"` so `cargo publish` bundles the README.
- Runnable examples live in each crate's `examples/` directory; CI's clippy `--all-targets`
  gate compiles them. Keep README code snippets consistent with the compiled examples.

## Cross-cutting dependencies (spec)
`tpt-telos` and `tpt-ai` are cross-cutting dependencies per the spec, but neither is published to
crates.io and this repo is workspace-standalone, so they appear here as integration seams only
(e.g. anomaly-detected outcome reports are the feedstock for the `tpt-ai` yield heatmap). Do not
add path dependencies outside this repo.

## Domain conventions
- SEMI standards this repo implements: E4 (HSMS), E5 (SECS-II), E30 (GEM), E39/E40/E90/E116
  (GEM300). Message names follow the standard (`S1F13`, `S2F41`, …); keep stream/function numbers
  exactly as published.
- Everything outcome-related is keyed to semantic IDs (`FabricLink.id`, footprint IDs, net names,
  recipe/lot IDs), never raw coordinates.
- Every lithography backend is documented **unvalidated-against-real-hardware** until a design
  partner adopts it — keep that flag in each backend's docs when touching them.
- `tpt-fab-process` docs must keep the explicit non-goal: this is not a TCAD replacement; the
  target is "catches what mature-node/emerging fabs currently catch with nothing."
- Minimum cohort size for aggregate publication is N = 5 (resolved decision; do not regress it to
  "open question" wording). Differential-privacy epsilon varies by field sensitivity
  (yield tighter than geometry).

## File-based exchange invariants (load-bearing)
- `OutcomeFileWriter` writes to a local path and returns. No thread, no timer, no scheduler may
  send anything anywhere. If you find yourself writing a socket in this repo, stop.
- `ConsentScope` travels inside the `OutcomeReport`; ingestion enforces it structurally.
- `SchemaVersion` on both outbound payloads and outcome reports: reject or best-effort migrate
  unknown minor versions — never silently misinterpret a changed field.
- Signatures authenticate file origin, not transport (HMAC-SHA256, implemented in-crate).
