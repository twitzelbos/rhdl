//! How does pipelining the comb cascade interact with a run-time
//! variable rate?
//!
//! Three answers, and this file is the evidence for each.
//!
//! **Correctness: not at all.** The comb section computes
//! `(1 - z^-M)^N` at the *input* rate, which has no `R` in it, and the
//! pipeline is clocked by accept *events* rather than by cycles — so its
//! depth is `N` accepts whatever the rate. A rate change cannot corrupt
//! values in flight because those values never depended on the rate.
//! `In::restart` clears the new output registers along with the delay
//! lines, in all four widgets.
//!
//! **Latency: yes, and it is the dominant term.** The comb pipeline
//! contributes `N` input samples, which is `N·R` output cycles. Measured
//! below: 7, 10, 16, 28 and 52 cycles at `R` of 1, 2, 4, 8 and 16 for
//! `N = 3` — linear in `R` with slope exactly `N` and a rate-independent
//! intercept. A phase-sensitive transmitter has to recompute its group
//! delay when it changes rate.
//!
//! **And the pipelining matters *most* at small `R`, which is the whole
//! argument for having done it.** The comb section advances one step per
//! accept and accepts arrive every `R` cycles, so at `R = 1` the comb
//! logic runs at the full converter clock. Unpipelined, its critical
//! path would have been `N` chained subtractors *whose timing depended
//! on a run-time input* — and since the rate can drop to one, timing
//! would have had to close for that case regardless, so the combs being
//! "slow" most of the time bought nothing. Pipelining makes the critical
//! path rate-independent, which is the property a synthesis tool can
//! actually be held to.

use rhdl::prelude::*;
use rhdl_fpga::dsp::cic::interp;
use rhdl_fpga::dsp::cic::interpolator::{self, CicInterpolate};

const WI: usize = 10;
const S: usize = 3;
const RMAX: usize = 16;
const M: usize = 1;
const WA: usize = interp::accumulator_width(WI, S, RMAX, M);
const CW: usize = interp::rate_width(RMAX);
type Uut = CicInterpolate<WI, WA, S, RMAX, M, CW>;

fn run(seq: Vec<interpolator::In<WI, CW>>) -> Vec<(i128, bool)> {
    Uut::default()
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .map(|s| (s.output.sample.raw(), s.output.input_ready))
        .collect()
}

fn hold(
    v: i128,
    rate: usize,
    cycles: usize,
    restart_at: Option<usize>,
) -> Vec<interpolator::In<WI, CW>> {
    (0..cycles)
        .map(|n| interpolator::In::<WI, CW> {
            sample: Some(signed::<WI>(v)),
            rate: bits::<CW>(rate as u128),
            restart: restart_at == Some(n),
            downstream_ready: true,
        })
        .collect()
}

/// **The accept cadence is exact at every rate, degenerate ones
/// included.**
///
/// The pipeline is driven by this signal, so if the cadence were
/// disturbed the whole comb section would advance at the wrong rate.
#[test]
fn the_accept_cadence_is_honoured_at_every_rate() {
    for rate in [0usize, 1, 2, 3, 4, 5, 8, 16] {
        let out = run(hold(1, rate, 40, None));
        // Zero and one both mean R = 1.
        let eff = rate.max(1);
        for (n, (_, ready)) in out[1..].iter().enumerate() {
            assert_eq!(*ready, n % eff == 0, "rate {rate}, cycle {n}");
        }
    }
}

/// **The comb pipeline's depth is `N` accepts, so its latency is `N·R`
/// output cycles.**
///
/// Fitted rather than asserted point by point: the first-nonzero-output
/// index must be `N·R + c` with `c` independent of the rate. That is the
/// precise statement of "the pipeline is clocked by accepts, not
/// cycles", and it is what makes the *critical path* rate-independent
/// even though the *latency* is not.
#[test]
fn the_comb_latency_is_n_times_the_rate() {
    let mut intercepts = Vec::new();
    for rate in [1usize, 2, 4, 8, 16] {
        let seq: Vec<interpolator::In<WI, CW>> = (0..40 * rate)
            .map(|n| interpolator::In::<WI, CW> {
                // A single envelope sample, then silence.
                sample: Some(signed::<WI>(if n < rate { 100 } else { 0 })),
                rate: bits::<CW>(rate as u128),
                restart: false,
                downstream_ready: true,
            })
            .collect();
        let first = run(seq)
            .iter()
            .position(|(v, _)| *v != 0)
            .expect("the impulse must reach the output");
        intercepts.push(first as i64 - (S * rate) as i64);
    }
    // Measured: 7, 10, 16, 28, 52 -> intercept 4 throughout.
    assert!(
        intercepts.iter().all(|c| *c == intercepts[0]),
        "the intercept must not depend on the rate: {intercepts:?}"
    );
    assert_eq!(
        intercepts[0], 4,
        "N-1 comb-chain cycles, one register, one reset"
    );
}

/// **A rate change with a restart establishes the new rate's gain**, at
/// every pair tried including a drop to the degenerate rate.
///
/// The documented usage, still correct with values in flight through the
/// comb pipeline — because the restart flushes them.
#[test]
fn a_rate_change_with_a_restart_establishes_the_new_gain() {
    for (a, b) in [(2usize, 8usize), (8, 2), (4, 16), (16, 1), (1, 16)] {
        let mut seq = hold(5, a, 30 * a, None);
        seq.extend(hold(5, b, 40 * b.max(1), Some(0)));
        let out = run(seq);
        let settled = out[out.len() - 2].0;
        let (num, den) = interp::dc_gain_ratio(S, b.max(1), M);
        assert_eq!(
            settled,
            5 * num as i128 / den as i128,
            "rate {a} -> {b}: settled {settled}"
        );
    }
}

/// **Changing the rate part-way through a window is clean.**
///
/// The worst moment: values are in flight in the comb pipeline and the
/// phase counter is mid-count. Nothing the comb section holds depends on
/// the rate, so the only effect is that the counter wraps early.
#[test]
fn a_mid_window_rate_change_is_clean() {
    let seq: Vec<interpolator::In<WI, CW>> = (0..400)
        .map(|n| interpolator::In::<WI, CW> {
            sample: Some(signed::<WI>(7)),
            // Drop from 8 to 4 three cycles into a window.
            rate: bits::<CW>(if n < 35 { 8 } else { 4 }),
            restart: n == 35,
            downstream_ready: true,
        })
        .collect();
    let out = run(seq);
    let (num, den) = interp::dc_gain_ratio(S, 4, M);
    assert_eq!(out[out.len() - 2].0, 7 * num as i128 / den as i128);
}

/// **The rate is not baked into the comb section anywhere.**
///
/// The structural form of the correctness claim: run the same envelope
/// through two different rates and compare the comb section's
/// contribution by checking that the settled output differs *only* by
/// the documented `(R·M)^N / R` gain ratio. If the comb arithmetic had
/// acquired an `R` dependence the ratio would not hold.
#[test]
fn the_comb_section_carries_no_rate_dependence() {
    let level = 6i128;
    let settled_at = |rate: usize| -> i128 {
        let out = run(hold(level, rate, 60 * rate.max(1), Some(0)));
        out[out.len() - 2].0
    };
    for rate in [1usize, 2, 4, 8, 16] {
        let (num, den) = interp::dc_gain_ratio(S, rate.max(1), M);
        assert_eq!(
            settled_at(rate),
            level * num as i128 / den as i128,
            "rate {rate}"
        );
    }
}
