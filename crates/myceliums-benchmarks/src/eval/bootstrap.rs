//! Paired bootstrap confidence intervals for the difference between two
//! retrieval runs over the same queries.
//!
//! With 51 queries a single aggregate moves by roughly 0.02 when one query
//! flips from a top hit to a miss, and the standard error of MRR is around
//! ±0.06. A bare "hybrid 0.68 vs lexical 0.64" therefore says nothing on its
//! own: the question is whether the difference would survive a different
//! sample of queries. The paired bootstrap answers that without assuming any
//! distribution for per-query scores, which are bounded, discrete and heavily
//! skewed (most reciprocal ranks are 1.0 or 0.0).
//!
//! # Method
//!
//! Both runs score the *same* queries, so the comparison is paired: for each
//! query `i` take the per-query difference `d_i = treatment_i − control_i`.
//! Resample `n` queries with replacement, [`BOOTSTRAP_RESAMPLES`] times, and
//! record the mean difference of each resample. The 95% interval is the 2.5th
//! and 97.5th percentile of those means (percentile method, linear
//! interpolation between order statistics). The point estimate is the observed
//! mean difference, not the bootstrap mean.
//!
//! Pairing matters: per-query scores vary far more *between* queries than
//! between two modes on the *same* query, and pairing cancels that variance. An
//! unpaired interval on the same data would be several times wider.
//!
//! # Determinism
//!
//! The resampling uses a fixed seed and an in-crate SplitMix64 generator rather
//! than an external RNG crate, so the interval for a given pair of runs is
//! bit-identical on every machine and across dependency upgrades — a CI gate
//! that could flip on a re-run would be worse than none.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Number of bootstrap resamples. 10 000 puts the Monte-Carlo error of a 2.5%
/// percentile well below the 4-decimal precision the report prints.
pub const BOOTSTRAP_RESAMPLES: usize = 10_000;

/// Fixed seed for the resampling. Arbitrary but frozen: changing it moves
/// interval endpoints in the fourth decimal and nothing else.
pub const BOOTSTRAP_SEED: u64 = 0x0092_B007_57A9_5EED;

/// Two-sided confidence level of every reported interval.
pub const CONFIDENCE_LEVEL: f64 = 0.95;

/// A difference between two runs on one metric, with its confidence interval.
///
/// `delta` is `treatment − control`: positive means the treatment scored
/// higher.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeltaEstimate {
    /// Observed mean per-query difference.
    pub delta: f64,
    /// Lower bound of the [`CONFIDENCE_LEVEL`] interval.
    pub ci_low: f64,
    /// Upper bound of the [`CONFIDENCE_LEVEL`] interval.
    pub ci_high: f64,
}

impl DeltaEstimate {
    /// The whole interval lies below zero: the treatment is worse on this
    /// sample of queries beyond what resampling noise explains.
    pub fn significantly_negative(&self) -> bool {
        self.ci_high < 0.0
    }

    /// The whole interval lies above zero.
    pub fn significantly_positive(&self) -> bool {
        self.ci_low > 0.0
    }
}

/// Paired bootstrap of the mean difference `treatment − control`.
///
/// `treatment[i]` and `control[i]` must score the same query. Errors on a
/// length mismatch or empty input rather than returning an interval that means
/// nothing.
pub fn paired_bootstrap(
    treatment: &[f64],
    control: &[f64],
    resamples: usize,
    seed: u64,
) -> Result<DeltaEstimate> {
    if treatment.len() != control.len() {
        bail!(
            "paired bootstrap needs one control score per treatment score \
             ({} vs {}) — the two runs did not score the same queries",
            treatment.len(),
            control.len()
        );
    }
    if treatment.is_empty() {
        bail!("paired bootstrap over zero queries has no meaning");
    }
    if resamples == 0 {
        bail!("paired bootstrap needs at least one resample");
    }

    let differences: Vec<f64> = treatment.iter().zip(control).map(|(t, c)| t - c).collect();
    let n = differences.len();
    let observed = differences.iter().sum::<f64>() / n as f64;

    let mut rng = SplitMix64::new(seed);
    let mut means: Vec<f64> = (0..resamples)
        .map(|_| {
            let total: f64 = (0..n).map(|_| differences[rng.below(n)]).sum();
            total / n as f64
        })
        .collect();
    means.sort_by(f64::total_cmp);

    let tail = (1.0 - CONFIDENCE_LEVEL) / 2.0;
    Ok(DeltaEstimate {
        delta: observed,
        ci_low: percentile(&means, tail),
        ci_high: percentile(&means, 1.0 - tail),
    })
}

/// [`paired_bootstrap`] with the frozen [`BOOTSTRAP_RESAMPLES`] and
/// [`BOOTSTRAP_SEED`] — the configuration every report uses.
pub fn paired_bootstrap_default(treatment: &[f64], control: &[f64]) -> Result<DeltaEstimate> {
    paired_bootstrap(treatment, control, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED)
}

/// The `q`-quantile of an ascending slice, interpolating linearly between the
/// two nearest order statistics (Hyndman–Fan type 7, the R and NumPy default).
fn percentile(sorted: &[f64], q: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let position = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let below = position.floor() as usize;
    let above = position.ceil() as usize;
    let fraction = position - below as f64;
    sorted[below] + (sorted[above] - sorted[below]) * fraction
}

/// SplitMix64 (Steele, Lea & Flood 2014): a tiny, well-mixed 64-bit generator.
///
/// Not cryptographic and not meant to be — it only has to be fast, of decent
/// statistical quality for resampling, and identical everywhere.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform index in `0..n` by multiply-shift (Lemire), whose bias for
    /// `n` in the tens is far below anything a percentile could show.
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_runs_have_a_zero_width_interval_at_zero() {
        let scores = [1.0, 0.5, 0.0, 0.25, 1.0];
        let estimate = paired_bootstrap_default(&scores, &scores).unwrap();
        assert_eq!(estimate.delta, 0.0);
        assert_eq!(estimate.ci_low, 0.0);
        assert_eq!(estimate.ci_high, 0.0);
        assert!(!estimate.significantly_negative());
        assert!(!estimate.significantly_positive());
    }

    #[test]
    fn a_uniform_shift_is_recovered_exactly() {
        // Every query improves by the same amount, so every resample does too.
        let control = [0.1, 0.2, 0.3, 0.4];
        let treatment: Vec<f64> = control.iter().map(|c| c + 0.25).collect();
        let estimate = paired_bootstrap_default(&treatment, &control).unwrap();
        for bound in [estimate.delta, estimate.ci_low, estimate.ci_high] {
            assert!((bound - 0.25).abs() < 1e-12, "{bound}");
        }
        assert!(estimate.significantly_positive());
    }

    #[test]
    fn the_interval_brackets_the_observed_difference() {
        let control = [1.0, 0.0, 0.5, 1.0, 0.0, 0.33, 1.0, 0.25, 0.0, 1.0];
        let treatment = [1.0, 1.0, 0.5, 0.5, 0.0, 1.0, 1.0, 0.25, 1.0, 1.0];
        let estimate = paired_bootstrap_default(&treatment, &control).unwrap();
        assert!(estimate.ci_low <= estimate.delta && estimate.delta <= estimate.ci_high);
        assert!(estimate.ci_low < estimate.ci_high);
    }

    #[test]
    fn a_single_query_flip_is_not_significant() {
        // One query of twenty drops from a hit to a miss: a real difference on
        // this sample, but a resample that omits that query shows none, so the
        // interval must reach zero. This is the false alarm the CI gate exists
        // to avoid.
        let control = vec![1.0; 20];
        let mut treatment = control.clone();
        treatment[7] = 0.0;
        let estimate = paired_bootstrap_default(&treatment, &control).unwrap();
        assert!(estimate.delta < 0.0);
        assert!(!estimate.significantly_negative(), "{estimate:?}");
    }

    #[test]
    fn many_consistent_drops_are_significant() {
        let control = vec![1.0; 50];
        let mut treatment = control.clone();
        for slot in treatment.iter_mut().take(10) {
            *slot = 0.0;
        }
        let estimate = paired_bootstrap_default(&treatment, &control).unwrap();
        assert!((estimate.delta + 0.2).abs() < 1e-12);
        assert!(estimate.significantly_negative(), "{estimate:?}");
    }

    #[test]
    fn the_same_seed_gives_bit_identical_intervals() {
        let control = [0.9, 0.1, 0.4, 0.7, 0.3, 0.0, 1.0];
        let treatment = [0.8, 0.6, 0.4, 0.2, 0.9, 0.5, 1.0];
        let first = paired_bootstrap_default(&treatment, &control).unwrap();
        let second = paired_bootstrap_default(&treatment, &control).unwrap();
        assert_eq!(first, second);
        let other_seed = paired_bootstrap(&treatment, &control, BOOTSTRAP_RESAMPLES, 7).unwrap();
        assert_eq!(
            first.delta, other_seed.delta,
            "the point estimate is not random"
        );
    }

    #[test]
    fn mismatched_or_empty_inputs_are_errors() {
        assert!(paired_bootstrap_default(&[1.0], &[1.0, 0.0]).is_err());
        assert!(paired_bootstrap_default(&[], &[]).is_err());
        assert!(paired_bootstrap(&[1.0], &[0.0], 0, 1).is_err());
    }

    #[test]
    fn percentile_interpolates_between_order_statistics() {
        let sorted = [0.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&sorted, 0.0), 0.0);
        assert_eq!(percentile(&sorted, 1.0), 4.0);
        assert_eq!(percentile(&sorted, 0.5), 2.0);
        assert!((percentile(&sorted, 0.125) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn resampled_indices_cover_the_range_uniformly() {
        let mut rng = SplitMix64::new(BOOTSTRAP_SEED);
        let mut counts = [0usize; 5];
        for _ in 0..50_000 {
            counts[rng.below(5)] += 1;
        }
        for count in counts {
            // Expected 10 000 each; ±5% is ~11 standard deviations.
            assert!((9_500..=10_500).contains(&count), "{counts:?}");
        }
    }
}
