//! Bit-exact validation of both CIC widgets against independent models.
//!
//! "Matches a model" is worth something only if the model was written
//! from the definition rather than from the widget, and only if it is
//! exercised somewhere the two could plausibly diverge. Before this
//! file, the uniform decimator was checked against a model at exactly
//! one configuration with one hand-written stimulus, and the pruned
//! decimator was never checked bit-exactly at all — only statistically,
//! against the uniform widget.
//!
//! Both gaps matter for different reasons.
//!
//! The uniform cascade's correctness rests on two's-complement wrap in
//! the integrators cancelling in the combs. That property is invisible
//! at small amplitudes: the integrators only wrap when the input is
//! near full scale and the accumulator is at Hogenauer's exact bound.
//! A one-bit-too-narrow accumulator produces a plausible-looking signal
//! rather than an obviously broken one, so the stimulus here is
//! deliberately driven to full scale.
//!
//! The pruned cascade adds per-stage truncation *and* per-stage
//! wrapping at different widths. A statistical check against the
//! uniform widget cannot distinguish "truncates as designed" from
//! "truncates one bit off somewhere", because both land inside the
//! error budget. Only a bit-exact model can.

use rhdl::prelude::*;
use rhdl_fpga::cic_pruned;
use rhdl_fpga::core::dff;
use rhdl_fpga::dsp::cic::{
    accumulator_width, counter_width, decimator::CicDecimate, decimator::In, prune,
};

/// Sign-extend the low `w` bits — a register of width `w` holding `v`.
fn wrap(v: i128, w: usize) -> i128 {
    let m = 1i128 << w;
    let x = v & (m - 1);
    if x >= m / 2 { x - m } else { x }
}

/// Arithmetic right shift by the width difference, then truncate.
///
/// What a pruned stage transfer does: the next stage keeps fewer bits
/// and drops the least significant ones.
fn narrow(v: i128, from: usize, to: usize) -> i128 {
    wrap(v >> (from - to), to)
}

/// The uniform CIC, from the definition.
///
/// Every register is `wa` bits and wraps there. Written with explicit
/// wrapping rather than in `i128` so that a too-narrow accumulator
/// shows up as a mismatch instead of being silently papered over by
/// the model's wider arithmetic.
fn uniform_model(x: &[i128], wa: usize, n: usize, r: usize, m: usize) -> Vec<i128> {
    let mut ints = vec![0i128; n];
    let mut combs = vec![vec![0i128; m]; n];
    let mut comb_outs = vec![0i128; n];
    let mut out = Vec::new();
    for (idx, s) in x.iter().enumerate() {
        // Pipelined: stage k reads stage k-1's value from the previous
        // sample. The widget is built that way for fmax; a model that
        // chains combinationally is a different filter.
        let prev = ints.clone();
        for k in 0..n {
            let feed = if k == 0 { *s } else { prev[k - 1] };
            ints[k] = wrap(prev[k] + feed, wa);
        }
        let carry = ints[n - 1];
        if (idx + 1) % r == 0 {
            // Pipelined here too: each comb stage reads the previous
            // stage's value from the previous *comb* cycle. Same
            // reasoning as the integrators -- a chained cascade is
            // `n` subtractors between registers and has to settle in one
            // period whatever rate the registers move at.
            let prev_out = comb_outs.clone();
            for k in 0..n {
                let v = if k == 0 { carry } else { prev_out[k - 1] };
                comb_outs[k] = wrap(v - combs[k][m - 1], wa);
                for j in (1..m).rev() {
                    combs[k][j] = combs[k][j - 1];
                }
                combs[k][0] = v;
            }
            out.push(comb_outs[n - 1]);
        }
    }
    out
}

/// The pruned CIC, from the definition plus Hogenauer's §V schedule.
///
/// Each stage has its own width and wraps at it, and each transfer
/// between stages discards the bits the next stage does not keep. A
/// pruned register does not hold the value — it holds the value
/// divided by two to the power of its discarded bits — which is why
/// the input is narrowed into stage one rather than simply extended.
#[allow(clippy::too_many_arguments)]
fn pruned_model(x: &[i128], wi: usize, n: usize, r: usize, m: usize, bo: usize) -> Vec<i128> {
    let full = accumulator_width(wi, n, r, m);
    // w[0..n) are the integrators, w[n..2n) the combs.
    let w: Vec<usize> = (1..=2 * n)
        .map(|j| prune::stage_width(j, wi, n, r, m, bo))
        .collect();

    let mut ints = vec![0i128; n];
    let mut combs = vec![vec![0i128; m]; n];
    let mut comb_outs = vec![0i128; n];
    let mut out = Vec::new();
    for (idx, s) in x.iter().enumerate() {
        let prev = ints.clone();
        // The sample arrives at weight one; stage one's LSB weighs
        // 2^(full - w[0]).
        ints[0] = wrap(prev[0] + narrow(*s, full, w[0]), w[0]);
        for k in 1..n {
            let feed = narrow(prev[k - 1], w[k - 1], w[k]);
            ints[k] = wrap(prev[k] + feed, w[k]);
        }
        let carry = ints[n - 1];
        if (idx + 1) % r == 0 {
            // Pipelined, as in `uniform_model`: each comb stage reads
            // the previous stage's *registered* output, narrowed to its
            // own width.
            let prev_out = comb_outs.clone();
            let first = narrow(carry, w[n - 1], w[n]);
            for k in 0..n {
                let wj = w[n + k];
                let vin = if k == 0 {
                    first
                } else {
                    narrow(prev_out[k - 1], w[n + k - 1], wj)
                };
                comb_outs[k] = wrap(vin - combs[k][m - 1], wj);
                for j in (1..m).rev() {
                    combs[k][j] = combs[k][j - 1];
                }
                combs[k][0] = vin;
            }
            out.push(comb_outs[n - 1]);
        }
    }
    out
}

/// Deterministic stimulus that reaches full scale.
///
/// Full scale is the point: the integrators only wrap near it, and
/// wrap cancellation is the property the accumulator bound exists to
/// protect. A gentle stimulus tests the arithmetic and misses the
/// invariant.
fn stimulus_bits(n: usize, wi: usize, seed: u64) -> Vec<i128> {
    let fs = (1i128 << (wi - 1)) - 1;
    let mut s = seed | 1;
    (0..n)
        .map(|k| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            match k % 8 {
                // Sustained full scale, both signs: the worst case for
                // integrator growth.
                0 | 1 => fs,
                2 | 3 => -fs,
                // Alternating extremes.
                4 => {
                    if (s >> 40) & 1 == 0 {
                        fs
                    } else {
                        -fs
                    }
                }
                // Uniform random over the whole range.
                _ => ((s >> 33) as i128 % (2 * fs + 1)) - fs,
            }
        })
        .collect()
}

macro_rules! feed {
    ($wi:tt, $x:expr) => {{
        let mut v: Vec<In<$wi>> = $x
            .iter()
            .map(|s| In::<$wi> {
                sample: Some(signed::<$wi>(*s)),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        v.extend(std::iter::repeat_n(
            In::<$wi> {
                sample: None,
                restart: false,
                downstream_ready: true,
            },
            4,
        ));
        v
    }};
}

/// One configuration: uniform widget vs uniform model, pruned widget
/// vs pruned model, both bit-exact, over several seeds.
macro_rules! case {
    ($name:ident, $wi:tt, $n:tt, $r:tt, $m:tt, $bo:tt) => {
        mod $name {
            use super::*;
            cic_pruned!(Pruned, w_in = $wi, n = $n, r = $r, m = $m, b_out = $bo);

            const FULL: usize = accumulator_width($wi, $n, $r, $m);
            const CW: usize = counter_width($r);
            type Uniform = CicDecimate<$wi, FULL, $n, $r, $m, CW>;

            #[test]
            fn uniform_is_bit_exact_against_the_model() {
                for seed in [1u64, 7, 12345, 0xdead_beef] {
                    let x = stimulus_bits(40 * $r, $wi, seed);
                    let got: Vec<i128> = Uniform::default()
                        .run(feed!($wi, x).into_iter().with_reset(1).clock_pos_edge(100))
                        .synchronous_sample()
                        .filter_map(|s| s.output.sample.map(|v| v.raw()))
                        .collect();
                    let want = uniform_model(&x, FULL, $n, $r, $m);
                    assert_eq!(got.len(), want.len(), "seed {seed}: output count");
                    assert_eq!(got, want, "seed {seed}: uniform widget vs model");
                }
            }

            #[test]
            fn pruned_is_bit_exact_against_the_model() {
                for seed in [1u64, 7, 12345, 0xdead_beef] {
                    let x = stimulus_bits(40 * $r, $wi, seed);
                    let got: Vec<i128> = Pruned::default()
                        .run(feed!($wi, x).into_iter().with_reset(1).clock_pos_edge(100))
                        .synchronous_sample()
                        .filter_map(|s| s.output.sample.map(|v| v.raw()))
                        .collect();
                    let want = pruned_model(&x, $wi, $n, $r, $m, $bo);
                    assert_eq!(got.len(), want.len(), "seed {seed}: output count");
                    assert_eq!(got, want, "seed {seed}: pruned widget vs model");
                }
            }
        }
    };
}

case!(n2_r4, 8, 2, 4, 1, 4);
case!(n2_r16_m2, 10, 2, 16, 2, 6);
case!(n3_r8, 12, 3, 8, 1, 6);
case!(n4_r16, 12, 4, 16, 1, 10);
case!(n5_r8, 10, 5, 8, 1, 8);

/// The model must be able to fail.
///
/// A model that agrees with the widget because both are wrong, or
/// because the comparison is insensitive, proves nothing. Perturbing
/// one sample must change the output — and perturbing the *schedule*
/// must break the pruned comparison.
#[test]
fn the_comparison_is_sensitive() {
    let wi = 12;
    let (n, r, m) = (3usize, 8usize, 1usize);
    let full = accumulator_width(wi, n, r, m);
    let x = stimulus_bits(40 * r, wi, 1);
    let base = uniform_model(&x, full, n, r, m);

    let mut y = x.clone();
    y[5] += 1;
    assert_ne!(
        base,
        uniform_model(&y, full, n, r, m),
        "one LSB must matter"
    );

    // A too-narrow accumulator must corrupt the result, not degrade
    // it gracefully -- the property `Default`'s assert protects.
    //
    // This needs sustained full-scale DC, not the varying stimulus
    // above, and the difference is the whole point of the accumulator
    // bound. Hogenauer's width is a *worst-case* bound: it is set by
    // the largest value the cascade can produce, `full_scale *
    // (R*M)^N`. A signal that changes sign never integrates that far,
    // so a one-bit-narrow accumulator carries it perfectly well and a
    // test built on such a signal reports everything is fine.
    //
    // Which is exactly why the width is asserted at construction
    // rather than left to testing: the failure is invisible until the
    // day someone feeds the filter a large DC offset.
    let fs = (1i128 << (wi - 1)) - 1;
    let dc = vec![fs; 40 * r];
    let dc_ok = uniform_model(&dc, full, n, r, m);
    assert_eq!(
        *dc_ok.last().unwrap(),
        fs * (r * m).pow(n as u32) as i128,
        "at the exact bound the worst case is still exact"
    );
    assert_ne!(
        dc_ok,
        uniform_model(&dc, full - 1, n, r, m),
        "one bit narrower must break wrap cancellation at the worst case"
    );

    // And the pruned model must actually depend on its schedule.
    let a = pruned_model(&x, wi, n, r, m, 6);
    let b = pruned_model(&x, wi, n, r, m, 10);
    assert_ne!(a, b, "the pruning budget must change the output");
}
