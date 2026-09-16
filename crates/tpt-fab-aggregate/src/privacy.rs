//! Differential-privacy noise injection with per-field-sensitivity epsilon.
//!
//! Resolved decision (spec Section 6): the DP epsilon **varies by field sensitivity** — yield
//! numbers (business-sensitive) get a tighter (noisier) epsilon than geometry deviations. The
//! defaults here encode that ordering; the policy is fully configurable.
//!
//! Applied as the second protection layer for small cohorts under the minimum-cohort gate
//! (spec 4.4): buckets with fewer than N contributors publish only DP-noised statistics, and
//! only if the fab opted into `AggregatedContribution` at all.

use std::collections::HashMap;

/// Sensitivity class of a field, driving its epsilon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldSensitivity {
    /// Geometry deviations — least business-sensitive, loosest epsilon.
    Geometry,
    /// Yield figures — business-sensitive, tighter epsilon.
    Yield,
    /// Free-form process parameters — tightest epsilon.
    Process,
}

impl FieldSensitivity {
    /// Default epsilon (Laplace scale = 1/epsilon; smaller epsilon = more noise).
    pub fn default_epsilon(self) -> f64 {
        match self {
            FieldSensitivity::Geometry => 2.0,
            FieldSensitivity::Yield => 0.5,
            FieldSensitivity::Process => 0.25,
        }
    }
}

/// Per-field epsilon policy. Falls back to the sensitivity-class default for unlisted fields.
#[derive(Debug, Clone)]
pub struct EpsilonPolicy {
    overrides: HashMap<FieldSensitivity, f64>,
}

impl EpsilonPolicy {
    /// Policy using the sensitivity-class defaults.
    pub fn defaults() -> Self {
        EpsilonPolicy { overrides: HashMap::new() }
    }

    /// Overrides the epsilon for one sensitivity class. Must be positive.
    pub fn set(&mut self, sensitivity: FieldSensitivity, epsilon: f64) {
        assert!(epsilon > 0.0, "epsilon must be positive");
        self.overrides.insert(sensitivity, epsilon);
    }

    /// Effective epsilon for a sensitivity class.
    pub fn epsilon(&self, sensitivity: FieldSensitivity) -> f64 {
        self.overrides.get(&sensitivity).copied().unwrap_or_else(|| sensitivity.default_epsilon())
    }
}

impl Default for EpsilonPolicy {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Deterministic SplitMix64 PRNG — reproducible noise without a `rand` dependency.
///
/// Test and tool outputs must be auditable: the same seed reproduces the same noised
/// publication, so a fab can verify exactly what was (or would be) published.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Seeds the generator.
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    /// Next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Draws one Laplace(`b`) sample where `b = 1 / epsilon`.
///
/// Uses the inverse-CDF method: `X = -b * sign(u) * ln(1 - 2|u|)` with `u ~ U(-0.5, 0.5)`.
pub fn laplace(rng: &mut SplitMix64, epsilon: f64) -> f64 {
    assert!(epsilon > 0.0, "epsilon must be positive");
    let b = 1.0 / epsilon;
    let u = rng.next_f64() - 0.5;
    -b * u.signum() * (1.0 - 2.0 * u.abs()).ln()
}

/// Adds Laplace noise calibrated to `epsilon` for the given field sensitivity.
pub fn noisify(
    rng: &mut SplitMix64,
    value: f64,
    sensitivity: FieldSensitivity,
    policy: &EpsilonPolicy,
) -> f64 {
    value + laplace(rng, policy.epsilon(sensitivity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yield_epsilon_is_tighter_than_geometry() {
        let policy = EpsilonPolicy::defaults();
        assert!(
            policy.epsilon(FieldSensitivity::Yield) < policy.epsilon(FieldSensitivity::Geometry)
        );
        assert!(
            policy.epsilon(FieldSensitivity::Process) < policy.epsilon(FieldSensitivity::Yield)
        );
    }

    #[test]
    fn per_field_override_applies() {
        let mut policy = EpsilonPolicy::defaults();
        policy.set(FieldSensitivity::Yield, 1.5);
        assert_eq!(policy.epsilon(FieldSensitivity::Yield), 1.5);
        assert_eq!(policy.epsilon(FieldSensitivity::Geometry), 2.0); // default untouched
    }

    #[test]
    fn prng_is_reproducible() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn noise_is_centered_and_bounded_by_epsilon() {
        let mut rng = SplitMix64::new(7);
        let policy = EpsilonPolicy::defaults();
        // With epsilon 0.5 (b=2), the sample mean should be near the true value and the
        // overwhelming majority of samples within a few b.
        let b = 1.0 / policy.epsilon(FieldSensitivity::Yield);
        let mut sum = 0.0;
        let mut abs_devs: Vec<f64> = Vec::new();
        let n = 20_000;
        for _ in 0..n {
            let x = noisify(&mut rng, 100.0, FieldSensitivity::Yield, &policy);
            sum += x;
            abs_devs.push((x - 100.0).abs());
        }
        let mean = sum / n as f64;
        assert!((mean - 100.0).abs() < 0.5, "mean {mean} too far from 100");
        // Median absolute deviation of a Laplace(b) is b*ln2 ≈ 0.693b — allow a wide factor
        // so the check is deterministic in practice, not a tail-probability flake.
        abs_devs.sort_by(|a, b| a.total_cmp(b));
        let median_abs = abs_devs[n / 2];
        assert!(
            (0.3 * b..3.0 * b).contains(&median_abs),
            "median |deviation| {median_abs} (b={b}) outside plausible Laplace range"
        );
    }

    #[test]
    fn tighter_epsilon_means_more_noise() {
        let mut rng = SplitMix64::new(9);
        let mut policy = EpsilonPolicy::defaults();
        policy.set(FieldSensitivity::Yield, 0.5);
        // Compare spread of geometry-noise vs yield-noise around the same value.
        let spread = |rng: &mut SplitMix64, policy: &EpsilonPolicy, sens: FieldSensitivity| {
            let mut acc = 0.0;
            for _ in 0..5_000 {
                acc += noisify(rng, 0.0, sens, policy).abs();
            }
            acc / 5_000.0
        };
        let geom = spread(&mut rng, &policy, FieldSensitivity::Geometry);
        let yld = spread(&mut rng, &policy, FieldSensitivity::Yield);
        assert!(geom < yld, "geometry noise {geom} should be smaller than yield noise {yld}");
    }
}
