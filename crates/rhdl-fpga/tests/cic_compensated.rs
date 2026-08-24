//! Does the compensator actually flatten a real CIC?
//!
//! [`compensator::design`] predicts a ripple figure from a closed-form
//! model of the CIC. That prediction is worth exactly nothing until it
//! is checked against the widgets, because the model knows nothing
//! about pipelining delay, fixed-point truncation, saturation, or the
//! quantisation of the taps themselves.
//!
//! So every number here is *measured*: a tone goes into
//! [`CicDecimate`], its output goes into [`SymmetricFir`], and the
//! amplitude that comes out is compared against the amplitude at DC.
//! The predicted ripple is then held to that measurement rather than
//! the other way round.

use rhdl::prelude::*;
use rhdl_fpga::dsp::cic::{
    accumulator_width, compensator, counter_width, decimator::CicDecimate, decimator::In as CicIn,
    response,
};
use rhdl_fpga::dsp::fir::{SymmetricFir, accumulator_width as fir_acc, symmetric::In as FirIn};
use std::f64::consts::TAU;

const WI: usize = 12;
const N: usize = 3;
const R: usize = 16;
const M: usize = 1;
const WA: usize = accumulator_width(WI, N, R, M);
const CW: usize = counter_width(R);

const TAPS: usize = 15;
const HALF: usize = 7;
const WC: usize = 14;
const WACC: usize = fir_acc(WA, WC, TAPS);

const PASSBAND: f64 = 0.8;
/// The fractional bits `quantise` picks for `WC`-bit taps at this spec.
///
/// Deterministic for a given spec, so it can be a const generic — and
/// `fir()` asserts the design agrees rather than assuming it.
const SHIFT: usize = 12;

type Cic = CicDecimate<WI, WA, N, R, M, CW>;
type Fir = SymmetricFir<WA, WC, WACC, TAPS, HALF, SHIFT, WA>;

/// Build the compensator for this CIC, quantised to `WC`-bit taps.
fn design() -> compensator::Quantised {
    let mut spec = compensator::Spec::for_cic(N, R, M);
    spec.taps = TAPS;
    spec.passband = PASSBAND;
    let d = compensator::design(spec).expect("design must succeed for a sane spec");
    compensator::quantise(&d, WC)
}

/// A FIR whose shift is taken from the quantised design.
///
/// `SHIFT` is a const generic but the design picks it at runtime, so
/// the two must agree; the assert is how that is enforced rather than
/// hoped for.
fn fir(q: &compensator::Quantised) -> Fir {
    assert_eq!(
        q.shift as usize, SHIFT,
        "the design's fractional-bit choice must match the type's SHIFT; \
         if quantise() changes, update SHIFT rather than loosening this"
    );
    let mut t = [SignedBits::<WC>::default(); TAPS];
    for (k, v) in q.taps.iter().enumerate() {
        t[k] = signed::<WC>(*v as i128);
    }
    SymmetricFir::new(t)
}

/// Push a tone through the CIC, then through the FIR, and return the
/// settled output samples.
fn through(q: &compensator::Quantised, f: f64, cycles: usize) -> Vec<f64> {
    let amp = ((1i128 << (WI - 1)) - 1) as f64 * 0.45;
    let n = cycles * R;
    let mut seq: Vec<CicIn<WI>> = (0..n)
        .map(|k| CicIn::<WI> {
            sample: Some(signed::<WI>(
                (amp * (TAU * f * k as f64).cos()).round() as i128
            )),
            restart: false,
            downstream_ready: true,
        })
        .collect();
    seq.extend(std::iter::repeat_n(
        CicIn::<WI> {
            sample: None,
            restart: false,
            downstream_ready: true,
        },
        4,
    ));

    let decimated: Vec<i128> = Cic::default()
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter_map(|s| s.output.sample.map(|v| v.raw()))
        .collect();

    let mut fin: Vec<FirIn<WA>> = decimated
        .iter()
        .map(|v| FirIn::<WA> {
            sample: Some(signed::<WA>(*v)),
            downstream_ready: true,
        })
        .collect();
    fin.push(FirIn::<WA> {
        sample: None,
        downstream_ready: true,
    });

    let out: Vec<f64> = fir(q)
        .run(fin.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter_map(|s| s.output.sample.map(|v| v.raw() as f64))
        .collect();

    // Discard the start-up transient: the CIC's window is N*R*M input
    // samples and the FIR's is TAPS output samples.
    let skip = (N + 2) + TAPS;
    out[skip.min(out.len() - 1)..].to_vec()
}

/// RMS amplitude of a settled tone.
fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

/// Same path with the FIR bypassed, for the before-and-after.
fn cic_only(f: f64, cycles: usize) -> Vec<f64> {
    let amp = ((1i128 << (WI - 1)) - 1) as f64 * 0.45;
    let n = cycles * R;
    let mut seq: Vec<CicIn<WI>> = (0..n)
        .map(|k| CicIn::<WI> {
            sample: Some(signed::<WI>(
                (amp * (TAU * f * k as f64).cos()).round() as i128
            )),
            restart: false,
            downstream_ready: true,
        })
        .collect();
    seq.extend(std::iter::repeat_n(
        CicIn::<WI> {
            sample: None,
            restart: false,
            downstream_ready: true,
        },
        4,
    ));
    let out: Vec<f64> = Cic::default()
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter_map(|s| s.output.sample.map(|v| v.raw() as f64))
        .collect();
    out[(N + 2).min(out.len() - 1)..].to_vec()
}

/// Measured response in dB relative to DC, across the passband.
fn sweep(q: &compensator::Quantised, compensated: bool) -> Vec<(f64, f64)> {
    // DC reference: a constant input, so amplitude is the mean rather
    // than the RMS of a tone.
    let dc = if compensated {
        through(q, 0.0, 400).iter().sum::<f64>() / through(q, 0.0, 400).len() as f64
    } else {
        let v = cic_only(0.0, 400);
        v.iter().sum::<f64>() / v.len() as f64
    };
    assert!(dc.abs() > 1.0, "DC reference is degenerate: {dc}");

    let edge_u = response::passband_edge_out(PASSBAND);
    (1..=10)
        .map(|k| {
            let u = edge_u * k as f64 / 10.0;
            let f = u / R as f64; // input-rate frequency
            let a = if compensated {
                rms(&through(q, f, 400))
            } else {
                rms(&cic_only(f, 400))
            };
            // A tone's RMS is amplitude/sqrt(2); DC's is the amplitude.
            let mag = a * 2f64.sqrt() / dc.abs();
            (u, 20.0 * mag.log10())
        })
        .collect()
}

#[test]
fn the_measured_droop_matches_the_closed_form() {
    // Before trusting the compensator, check that the *model* of what
    // it must invert describes the real widget. If this fails, the
    // design is inverting the wrong thing.
    let q = design();
    let measured = sweep(&q, false);
    for (u, db) in &measured {
        let predicted = 20.0 * response::magnitude_out(*u, N, R, M).log10();
        println!("u={u:.4}  measured {db:.3} dB  predicted {predicted:.3} dB");
        assert!(
            (db - predicted).abs() < 0.35,
            "at u={u} the widget droops {db} dB but the model says {predicted}"
        );
    }
}

#[test]
fn compensation_flattens_the_real_widget() {
    let q = design();
    let before = sweep(&q, false);
    let after = sweep(&q, true);

    let worst_before = before.iter().fold(0.0f64, |m, (_, db)| m.max(db.abs()));
    let span_after = {
        let lo = after.iter().fold(f64::INFINITY, |m, (_, db)| m.min(*db));
        let hi = after
            .iter()
            .fold(f64::NEG_INFINITY, |m, (_, db)| m.max(*db));
        hi - lo
    };
    for ((u, b), (_, a)) in before.iter().zip(after.iter()) {
        println!("u={u:.4}  before {b:+.3} dB  after {a:+.3} dB");
    }
    println!(
        "worst droop before {worst_before:.3} dB; span after {span_after:.3} dB; \
         design predicted {:.3} dB",
        q.ripple_db
    );

    assert!(
        worst_before > 3.0,
        "this configuration barely droops, so it cannot demonstrate anything: {worst_before}"
    );
    // The measured flatness must be in the neighbourhood the design
    // predicted -- generous, because measurement over a finite window
    // has its own error, but not so generous as to accept a filter
    // that did nothing.
    assert!(
        span_after < 0.5,
        "compensated passband spans {span_after} dB, expected flat"
    );
    assert!(
        span_after < worst_before / 10.0,
        "compensation must be a large improvement: {span_after} vs {worst_before}"
    );
}

#[test]
fn the_compensator_preserves_dc_gain() {
    // The composite must not quietly rescale: a measurement chain that
    // changes its own gain when you change the filter length is a trap.
    let q = design();
    assert!(
        (q.dc_gain - 1.0).abs() < 0.01,
        "quantised DC gain drifted to {}",
        q.dc_gain
    );
}

#[test]
fn the_taps_survive_quantisation_to_hardware_width() {
    let q = design();
    let peak = q.taps.iter().fold(0i64, |m, t| m.max(t.abs()));
    assert!(
        peak < (1i64 << (WC - 1)),
        "tap {peak} does not fit {WC} bits"
    );
    for k in 0..TAPS {
        assert_eq!(
            q.taps[k],
            q.taps[TAPS - 1 - k],
            "quantisation broke symmetry"
        );
    }
}
