#![warn(missing_docs)]
//! Report generation: a PDF describing a filter design.
//!
//! [`cic_report`] renders what a CIC does to a signal and what its
//! compensator does about it — the response, the droop, the alias
//! floor, the designed taps, and the composite. Two A4 pages.
//!
//! The builder is here rather than in the example that writes the file
//! for two reasons: so a user can generate the report for *their*
//! configuration rather than the one the example hard-codes, and so it
//! can be tested. A report generator that only runs inside an example
//! is a report generator nobody checks.
//!
//! Deterministic — see [`super::pdf`] — so a committed report can be
//! diffed and a change means something.

use super::pdf::{Align, Font, Page, Pdf};
use super::plot::{Axes, Frame, PALETTE, Series, draw};
use crate::dsp::cic::{accumulator_width, chain, compensator, prune, response};

/// What to report on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CicReport {
    /// Converter sample rate, in Hz — for the rate figures.
    pub fs_hz: f64,
    /// Input sample width, for the accumulator figure.
    pub w_in: usize,
    /// CIC stages.
    pub stages: usize,
    /// Decimation factor.
    pub rate: usize,
    /// Differential delay.
    pub delay: usize,
    /// Passband as a fraction of the decimated Nyquist.
    pub passband: f64,
    /// Compensator length, in taps.
    pub taps: usize,
    /// Coefficient width for the quantised design.
    pub coeff_width: usize,
    /// Pruning budget, for the stage-width figures.
    pub b_out: usize,
}

impl Default for CicReport {
    fn default() -> Self {
        Self {
            fs_hz: 125e6,
            w_in: 16,
            stages: 4,
            rate: 32,
            delay: 1,
            passband: 0.8,
            taps: 15,
            coeff_width: 16,
            b_out: 20,
        }
    }
}

/// Render a report for a CIC you specified by hand.
///
/// # Why this exists alongside [`chain_report`]
///
/// Two different questions. [`chain_report`] answers "here are my
/// requirements, what should I build" — its input is a design the
/// library derived. This one answers "I have chosen these parameters,
/// what do they do", which is the question you ask while exploring, or
/// when matching an existing implementation you did not choose.
///
/// **Both render through the same page builders.** An earlier version
/// had two parallel sets, about 140 lines each, which is two things to
/// keep in step and two places for a plot to be improved in only one
/// of them. This one synthesises the single-stage [`chain::ChainDesign`]
/// the given parameters describe and hands it to the same renderer, so
/// the two reports cannot drift.
///
/// Returns `None` if the compensator cannot be designed for these
/// parameters — an even tap count, or a passband reaching a CIC null.
pub fn cic_report(cfg: CicReport) -> Option<Pdf> {
    Some(render(&as_design(cfg)?, "Specified Parameters"))
}

/// The single-stage design the given parameters describe.
///
/// Separate from the rendering so a caller can inspect what their
/// parameters amount to without producing a PDF — and so the
/// synthesis is testable on its own.
pub fn as_design(cfg: CicReport) -> Option<chain::ChainDesign> {
    let (n, r, m) = (cfg.stages, cfg.rate, cfg.delay);
    let spec = compensator::Spec {
        cics: vec![compensator::CicShape {
            decimate: r,
            stages: n,
            delay: m,
        }],
        passband: cfg.passband,
        taps: cfg.taps,
        stopband_edge: 1.0,
        min_stopband_db: 0.0,
        max_ripple_db: 0.1,
        method: compensator::Method::LeastSquares,
    };
    let quant = compensator::quantise(&compensator::design(spec)?, cfg.coeff_width);

    let full = accumulator_width(cfg.w_in, n, r, m);
    let widths: Vec<usize> = (1..=2 * n)
        .map(|j| prune::stage_width(j, cfg.w_in, n, r, m, cfg.b_out))
        .collect();
    let register_bits: usize = widths.iter().sum();
    let stage = chain::CicStage {
        decimate: r,
        stages: n,
        delay: m,
        input_rate_hz: cfg.fs_hz,
        input_width: cfg.w_in,
        accumulator_width: full,
        prune_budget: cfg.b_out,
        stage_widths: widths,
    };

    // The requirements this configuration happens to meet, rather than
    // requirements it was asked to meet: the caller chose parameters,
    // so the "asked for" column is filled from what was achieved. A
    // report claiming the parameters met a spec nobody stated would be
    // inventing the spec.
    let ripple = quant.ripple_db;
    let alias = -response::worst_alias_db(cfg.passband, n, r, m);
    let snr = chain::snr_db(cfg.w_in, cfg.w_in, n, r, m, cfg.b_out);

    Some(chain::ChainDesign {
        spec: chain::ChainSpec {
            fs_hz: cfg.fs_hz,
            decimate: r,
            alias_free_bw_hz: cfg.passband * (cfg.fs_hz / r as f64) / 2.0,
            input_width: cfg.w_in,
            output_width: cfg.w_in,
            max_ripple_db: ripple,
            min_alias_rejection_db: alias,
            min_snr_db: snr,
            coeff_width: cfg.coeff_width,
            max_stages: n,
            max_taps: cfg.taps,
            max_chain_stages: 1,
            stopband_edge: 1.0,
            min_stopband_db: 0.0,
            method: compensator::Method::LeastSquares,
        },
        cics: vec![stage],
        multipliers: quant.taps.len() / 2 + 1,
        compensator: quant,
        passband: cfg.passband,
        output_rate_hz: cfg.fs_hz / r as f64,
        achieved_ripple_db: ripple,
        achieved_alias_db: alias,
        achieved_snr_db: snr,
        achieved_stopband_db: f64::INFINITY,
        register_bits,
        cost: register_bits as f64,
        alternative: None,
    })
}

/// Render a derived chain — the output of [`crate::dsp::cic::chain::design`].
///
/// Two pages. The first is what the cascade does to the signal: the
/// combined response across the full input band, where the nulls of
/// every stage are visible at once, and across the decimated band,
/// where the droop the compensator has to undo lives. The second is the
/// compensator and the composite, with the derived figures and the
/// alternative the designer rejected.
///
/// This is the report to reach for when the parameters were *derived*.
/// [`cic_report`] renders a single CIC from parameters you chose
/// yourself, which is the right thing when you are exploring rather
/// than specifying.
pub fn chain_report(d: &chain::ChainDesign) -> Pdf {
    render(d, "Derived Design")
}

/// Render a design under a given provenance label.
///
/// The heading says where the numbers came from, because that changes
/// how the reader should treat them: a *derived* design met
/// requirements someone stated, while *specified parameters* met
/// nothing in particular — the report's "asked for" column is then
/// just the achieved value restated. Labelling a hand-chosen
/// configuration "Derived Design" claims an authority it does not have.
pub fn render(d: &chain::ChainDesign, provenance: &str) -> Pdf {
    let mut doc = Pdf::new();
    doc.push(chain_page_one(d, provenance));
    doc.push(chain_page_two(d));
    doc
}

/// The cascade's shapes, for the combined-response evaluator.
fn shapes(d: &chain::ChainDesign) -> Vec<compensator::CicShape> {
    d.cics
        .iter()
        .map(|c| compensator::CicShape {
            decimate: c.decimate,
            stages: c.stages,
            delay: c.delay,
        })
        .collect()
}

fn chain_page_one(d: &chain::ChainDesign, provenance: &str) -> Page {
    let mut p = Page::a4();
    p.fill_style((0.0, 0.0, 0.0));
    p.text(
        297.5,
        800.0,
        16.0,
        Font::Bold,
        Align::Centre,
        &format!("Decimation Chain - {provenance}"),
    );
    p.text(
        297.5,
        784.0,
        9.0,
        Font::Regular,
        Align::Centre,
        &format!(
            "{:.3} MHz / {} = {:.4} kHz, split {:?}, {:.3} kHz alias-free",
            d.spec.fs_hz / 1e6,
            d.spec.decimate,
            d.output_rate_hz / 1e3,
            d.split(),
            d.spec.alias_free_bw_hz / 1e3
        ),
    );

    let sh = shapes(d);
    let total = d.spec.decimate;

    // Full input band: every stage's nulls, and where each stage folds.
    let curve: Vec<(f64, f64)> = (0..1400)
        .map(|k| {
            // Input-rate frequency, expressed as the output-rate `u`
            // the cascade evaluator expects.
            let f = 0.5 * k as f64 / 1399.0;
            let u = f * total as f64;
            let a = compensator::cascade_magnitude(&sh, u);
            (f, if a <= 1e-15 { -140.0 } else { 20.0 * a.log10() })
        })
        .collect();
    let axes = Axes::new(
        "Combined magnitude, full input band",
        "frequency / converter rate",
        "dB",
        (0.0, 0.5),
        (-140.0, 5.0),
    );
    let mut s = vec![Series::new("|H1 x H2|", curve, PALETTE[0])];
    // Where the *first* stage folds: the aliases it alone must reject.
    let first = d.cics[0].decimate;
    let folds: Vec<(f64, f64)> = (1..=(first / 2))
        .flat_map(|k| {
            let f = k as f64 / first as f64;
            [(f, -140.0), (f, 5.0), (f, -140.0)]
        })
        .collect();
    s.push(Series::new("stage-1 alias centres", folds, (0.80, 0.80, 0.85)).dashed());
    draw(
        &mut p,
        Frame {
            x: 60.0,
            y: 545.0,
            w: 470.0,
            h: 200.0,
        },
        &axes,
        &s,
    );

    // Decimated band: the droop.
    let edge = response::passband_edge_out(d.passband);
    let band: Vec<(f64, f64)> = (0..600)
        .map(|k| {
            let u = 0.5 * k as f64 / 599.0;
            let a = compensator::cascade_magnitude(&sh, u);
            (u, if a <= 1e-15 { -60.0 } else { 20.0 * a.log10() })
        })
        .collect();
    let axes = Axes::new(
        "Combined magnitude across the decimated band",
        "frequency / output rate (Nyquist = 0.5)",
        "dB",
        (0.0, 0.5),
        (-30.0, 2.0),
    );
    let s = vec![
        Series::new("|H1 x H2|", band, PALETTE[0]),
        Series::new(
            format!("alias-free edge u = {edge:.3}"),
            vec![(edge, -30.0), (edge, 2.0)],
            PALETTE[1],
        )
        .dashed(),
    ];
    draw(
        &mut p,
        Frame {
            x: 60.0,
            y: 290.0,
            w: 470.0,
            h: 200.0,
        },
        &axes,
        &s,
    );

    p.fill_style((0.0, 0.0, 0.0));
    p.text(60.0, 235.0, 10.0, Font::Bold, Align::Left, "Stages");
    let mut y = 220.0;
    for (k, c) in d.cics.iter().enumerate() {
        for line in [
            format!(
                "stage {}: /{}  N={} M={}  at {:.3} MHz",
                k + 1,
                c.decimate,
                c.stages,
                c.delay,
                c.input_rate_hz / 1e6
            ),
            format!(
                "   accumulator {} bits, prune budget {}, widths {:?}",
                c.accumulator_width, c.prune_budget, c.stage_widths
            ),
            format!(
                "   {} register bits, in {} bits out {} bits",
                c.register_bits(),
                c.input_width,
                c.output_width()
            ),
        ] {
            p.text(60.0, y, 8.0, Font::Regular, Align::Left, &line);
            y -= 11.0;
        }
        y -= 4.0;
    }
    p.text(
        60.0,
        y - 6.0,
        7.5,
        Font::Regular,
        Align::Left,
        "A stage's nulls must reject the aliases of its own decimation: once energy has folded",
    );
    p.text(
        60.0,
        y - 15.0,
        7.5,
        Font::Regular,
        Align::Left,
        "into the band no later stage can remove it, so every stage carries the full requirement.",
    );
    p
}

fn chain_page_two(d: &chain::ChainDesign) -> Page {
    let mut p = Page::a4();
    p.fill_style((0.0, 0.0, 0.0));
    p.text(
        297.5,
        800.0,
        16.0,
        Font::Bold,
        Align::Centre,
        "Compensation and Result",
    );
    p.text(
        297.5,
        784.0,
        9.0,
        Font::Regular,
        Align::Centre,
        &format!(
            "{} taps, {}-bit coefficients, {} multipliers",
            d.compensator.taps.len(),
            d.compensator.coeff_width,
            d.multipliers
        ),
    );

    let sh = shapes(d);
    let edge = response::passband_edge_out(d.passband);
    let scale = 2f64.powi(d.compensator.shift as i32);
    let real: Vec<f64> = d
        .compensator
        .taps
        .iter()
        .map(|t| *t as f64 / scale)
        .collect();
    let at = |k: usize| edge * k as f64 / 599.0;

    let cic: Vec<(f64, f64)> = (0..600)
        .map(|k| {
            (
                at(k),
                20.0 * compensator::cascade_magnitude(&sh, at(k)).log10(),
            )
        })
        .collect();
    let fir: Vec<(f64, f64)> = (0..600)
        .map(|k| {
            (
                at(k),
                20.0 * compensator::fir_amplitude(&real, at(k)).abs().log10(),
            )
        })
        .collect();
    let both: Vec<(f64, f64)> = (0..600)
        .map(|k| {
            let m = compensator::cascade_magnitude(&sh, at(k))
                * compensator::fir_amplitude(&real, at(k)).abs();
            (at(k), 20.0 * m.log10())
        })
        .collect();

    let axes = Axes::new(
        "Cascade, compensator, and the two together",
        "frequency / output rate",
        "dB",
        (0.0, edge),
        (-15.0, 15.0),
    );
    let s = vec![
        Series::new("cascade", cic, PALETTE[0]),
        Series::new("compensator", fir, PALETTE[4]),
        Series::new("composite", both.clone(), PALETTE[2]),
        Series::new("flat", vec![(0.0, 0.0), (edge, 0.0)], (0.6, 0.6, 0.6)).dashed(),
    ];
    draw(
        &mut p,
        Frame {
            x: 60.0,
            y: 545.0,
            w: 470.0,
            h: 200.0,
        },
        &axes,
        &s,
    );

    let axes = Axes::new(
        "Composite, zoomed - what is left",
        "frequency / output rate",
        "dB",
        (0.0, edge),
        (-0.3, 0.3),
    );
    let s = vec![
        Series::new("composite", both, PALETTE[2]),
        Series::new("flat", vec![(0.0, 0.0), (edge, 0.0)], (0.6, 0.6, 0.6)).dashed(),
    ];
    draw(
        &mut p,
        Frame {
            x: 60.0,
            y: 290.0,
            w: 470.0,
            h: 200.0,
        },
        &axes,
        &s,
    );

    let mut lines = vec![
        format!(
            "ripple ............ {:.4} dB  (asked <= {:.3})",
            d.achieved_ripple_db, d.spec.max_ripple_db
        ),
        format!(
            "alias rejection ... {:.1} dB   (asked >= {:.1})",
            d.achieved_alias_db, d.spec.min_alias_rejection_db
        ),
        format!(
            "output SNR ........ {:.1} dB   (asked >= {:.1})",
            d.achieved_snr_db, d.spec.min_snr_db
        ),
    ];
    if d.spec.min_stopband_db > 0.0 {
        lines.push(format!(
            "stopband .......... {:.1} dB above {:.2} Nyquist (asked >= {:.1})",
            d.achieved_stopband_db, d.spec.stopband_edge, d.spec.min_stopband_db
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "register bits ..... {} (rate-weighted cost {:.1})",
        d.register_bits, d.cost
    ));
    lines.push(format!("taps .............. {:?}", d.compensator.taps));
    lines.push(format!(
        "DC gain ........... {:.6} (exact by construction)",
        d.compensator.dc_gain
    ));
    match &d.alternative {
        None => lines.push("alternative ....... none considered".to_string()),
        Some(a) => lines.push(format!(
            "alternative ....... {:?}, cost {:.1}, {} bits - {}",
            a.split,
            a.cost.unwrap_or(f64::NAN),
            a.register_bits.unwrap_or(0),
            a.why
        )),
    }

    p.fill_style((0.0, 0.0, 0.0));
    p.text(60.0, 235.0, 10.0, Font::Bold, Align::Left, "Achieved");
    let mut y = 220.0;
    for l in &lines {
        p.text(60.0, y, 8.0, Font::Regular, Align::Left, l);
        y -= 11.0;
    }
    p.text(
        60.0,
        y - 8.0,
        7.5,
        Font::Regular,
        Align::Left,
        "Rate-weighted cost is a proxy, not an area figure: flops cost the same however slowly",
    );
    p.text(
        60.0,
        y - 17.0,
        7.5,
        Font::Regular,
        Align::Left,
        "they are clocked. It captures that a wide adder at the converter rate is the hard part.",
    );
    p
}
