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
use crate::dsp::cic::{accumulator_width, chain, compensator, dc_gain, prune, response};

/// What to report on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CicReport {
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

/// Render the report.
///
/// Returns `None` if the compensator cannot be designed for this
/// configuration — an even tap count, or a passband that reaches a CIC
/// null, where the required gain is unbounded.
pub fn cic_report(cfg: CicReport) -> Option<Pdf> {
    let spec = compensator::Spec {
        cics: vec![compensator::CicShape {
            decimate: cfg.rate,
            stages: cfg.stages,
            delay: cfg.delay,
        }],
        passband: cfg.passband,
        taps: cfg.taps,
        stopband_edge: 1.0,
        min_stopband_db: 0.0,
        max_ripple_db: 0.1,
        method: compensator::Method::LeastSquares,
    };
    let design = compensator::design(spec)?;
    let quant = compensator::quantise(&design, cfg.coeff_width);
    let mut doc = Pdf::new();
    doc.push(page_one(&cfg));
    doc.push(page_two(&cfg, &design, &quant));
    Some(doc)
}

fn page_one(cfg: &CicReport) -> Page {
    let (n, r, m) = (cfg.stages, cfg.rate, cfg.delay);
    let mut p = Page::a4();
    p.fill_style((0.0, 0.0, 0.0));
    p.text(
        297.5,
        800.0,
        16.0,
        Font::Bold,
        Align::Centre,
        "CIC Decimator - Frequency Response",
    );
    p.text(
        297.5,
        784.0,
        9.0,
        Font::Regular,
        Align::Centre,
        &format!(
            "N = {n} stages, R = {r}, M = {m}, input width {} bits",
            cfg.w_in
        ),
    );

    // Full input band: the nulls are the point.
    let axes = Axes::new(
        "Magnitude, full input band - nulls land on every alias",
        "frequency / input sample rate",
        "dB",
        (0.0, 0.5),
        (-120.0, 5.0),
    );
    let folds: Vec<(f64, f64)> = (1..=(r / 2))
        .flat_map(|k| {
            let f = k as f64 / r as f64;
            [(f, -120.0), (f, 5.0), (f, -120.0)]
        })
        .collect();
    let s = vec![
        Series::new("|H(f)|", response::curve_input(1200, n, r, m), PALETTE[0]),
        Series::new("k/R (alias centres)", folds, (0.80, 0.80, 0.85)).dashed(),
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

    // Decimated band: the droop is the price.
    let edge = response::passband_edge_out(cfg.passband);
    let band: Vec<(f64, f64)> = (0..600)
        .map(|k| {
            let u = 0.5 * k as f64 / 599.0;
            (u, 20.0 * response::magnitude_out(u, n, r, m).log10())
        })
        .collect();
    let axes = Axes::new(
        "Magnitude across the decimated band - the price",
        "frequency / output sample rate (Nyquist = 0.5)",
        "dB",
        (0.0, 0.5),
        (-20.0, 2.0),
    );
    let s = vec![
        Series::new("|H(u)|", band, PALETTE[0]),
        Series::new(
            format!("passband edge u = {edge:.3}"),
            vec![(edge, -20.0), (edge, 2.0)],
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

    let widths: Vec<usize> = (1..=2 * n)
        .map(|j| prune::stage_width(j, cfg.w_in, n, r, m, cfg.b_out))
        .collect();
    let full_w = accumulator_width(cfg.w_in, n, r, m);
    let lines = [
        format!("DC gain (R*M)^N .................. {}", dc_gain(n, r, m)),
        format!("Accumulator width required ....... {full_w} bits"),
        format!(
            "Passband droop at u = {edge:.3} ........ {:.2} dB",
            response::passband_droop_db(cfg.passband, n, r, m)
        ),
        format!(
            "Worst alias in the passband ....... {:.1} dB",
            response::worst_alias_db(cfg.passband, n, r, m)
        ),
        format!(
            "First null at f = 1/(R*M) ........ {:.5}",
            1.0 / (r * m) as f64
        ),
        String::new(),
        format!("Pruned stage widths (b_out = {}) .. {widths:?}", cfg.b_out),
        format!(
            "Register bits: {} pruned vs {} uniform",
            widths.iter().sum::<usize>(),
            full_w * 2 * n
        ),
    ];
    p.fill_style((0.0, 0.0, 0.0));
    p.text(60.0, 235.0, 10.0, Font::Bold, Align::Left, "Figures");
    let mut y = 220.0;
    for l in &lines {
        p.text(60.0, y, 8.0, Font::Regular, Align::Left, l);
        y -= 11.0;
    }
    p.text(
        60.0,
        y - 12.0,
        7.5,
        Font::Regular,
        Align::Left,
        "Droop is inherent to the sinc^N shape, not a design error: the nulls that reject the",
    );
    p.text(
        60.0,
        y - 21.0,
        7.5,
        Font::Regular,
        Align::Left,
        "aliases and the droop across the passband are the same expression. See page 2.",
    );
    p
}

fn page_two(cfg: &CicReport, design: &compensator::Design, quant: &compensator::Quantised) -> Page {
    let (n, r, m) = (cfg.stages, cfg.rate, cfg.delay);
    let mut p = Page::a4();
    p.fill_style((0.0, 0.0, 0.0));
    p.text(
        297.5,
        800.0,
        16.0,
        Font::Bold,
        Align::Centre,
        "CIC Compensation Filter",
    );
    p.text(
        297.5,
        784.0,
        9.0,
        Font::Regular,
        Align::Centre,
        &format!(
            "{}-tap symmetric FIR at the output rate, {}-bit coefficients",
            cfg.taps, cfg.coeff_width
        ),
    );

    let edge = response::passband_edge_out(cfg.passband);
    let scale = (1u64 << quant.shift) as f64;
    let real: Vec<f64> = quant.taps.iter().map(|t| *t as f64 / scale).collect();

    let at = |k: usize| edge * k as f64 / 599.0;
    let cic: Vec<(f64, f64)> = (0..600)
        .map(|k| {
            (
                at(k),
                20.0 * response::magnitude_out(at(k), n, r, m).log10(),
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
        .map(|k| (at(k), compensator::composite_db(&real, &design.spec, at(k))))
        .collect();

    let axes = Axes::new(
        "CIC, compensator, and the two together",
        "frequency / output sample rate",
        "dB",
        (0.0, edge),
        (-12.0, 12.0),
    );
    let s = vec![
        Series::new("CIC alone", cic, PALETTE[0]),
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
        "Composite, zoomed - what is left after compensation",
        "frequency / output sample rate",
        "dB",
        (0.0, edge),
        (-0.3, 0.3),
    );
    let s = vec![
        Series::new("composite (quantised taps)", both, PALETTE[2]),
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

    let droop = response::passband_droop_db(cfg.passband, n, r, m);
    let lines = [
        format!("Uncompensated droop, DC to edge ... {droop:.2} dB"),
        format!(
            "Ripple, ideal taps ............... {:.4} dB",
            design.ripple_db
        ),
        format!(
            "Ripple, {}-bit taps ............... {:.4} dB",
            cfg.coeff_width, quant.ripple_db
        ),
        format!(
            "Improvement ...................... {:.0}x",
            droop.abs() / quant.ripple_db.max(1e-9)
        ),
        String::new(),
        format!(
            "Peak gain asked of the filter .... {:.2}x",
            design.peak_gain
        ),
        format!("Coefficient fractional bits ...... {}", quant.shift),
        format!("DC gain of quantised filter ...... {:.6}", quant.dc_gain),
        format!("Multipliers (folded, symmetric) .. {}", cfg.taps / 2 + 1),
        String::new(),
        format!("Taps: {:?}", quant.taps),
    ];
    p.fill_style((0.0, 0.0, 0.0));
    p.text(60.0, 235.0, 10.0, Font::Bold, Align::Left, "Figures");
    let mut y = 220.0;
    for l in &lines {
        p.text(60.0, y, 8.0, Font::Regular, Align::Left, l);
        y -= 11.0;
    }
    p.text(
        60.0,
        y - 12.0,
        7.5,
        Font::Regular,
        Align::Left,
        "Compensation shapes the passband only. Alias rejection is unchanged - if the aliases",
    );
    p.text(
        60.0,
        y - 21.0,
        7.5,
        Font::Regular,
        Align::Left,
        "are too large the answer is more CIC stages or a narrower passband, not more taps.",
    );
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_configuration_renders() {
        let d = cic_report(CicReport::default()).expect("must design");
        assert_eq!(d.len(), 2, "two pages");
        let b = d.to_bytes();
        assert!(b.starts_with(b"%PDF"));
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("(CIC Decimator - Frequency Response) Tj"));
        assert!(s.contains("(CIC Compensation Filter) Tj"));
    }

    #[test]
    fn a_derived_chain_renders() {
        let d = chain::design(chain::ChainSpec::default()).expect("must design");
        let doc = chain_report(&d);
        assert_eq!(doc.len(), 2, "two pages");
        let s = String::from_utf8_lossy(&doc.to_bytes()).to_string();
        assert!(s.starts_with("%PDF"));
        for key in [
            "Decimation Chain - Derived Design",
            "Compensation and Result",
            "stage 1",
            "stage 2",
            "alias rejection",
            "alternative",
        ] {
            assert!(s.contains(&format!("({key}")), "missing {key}");
        }
    }

    #[test]
    fn chain_reports_are_deterministic() {
        let d = chain::design(chain::ChainSpec::default()).unwrap();
        assert_eq!(chain_report(&d).to_bytes(), chain_report(&d).to_bytes());
    }

    /// A single-stage design renders too — the cascade case must not be
    /// the only one that works.
    #[test]
    fn a_single_stage_chain_renders() {
        let d = chain::design(chain::ChainSpec {
            allow_cascade: false,
            ..chain::ChainSpec::default()
        })
        .expect("single stage must design");
        assert_eq!(d.cics.len(), 1);
        let s = String::from_utf8_lossy(&chain_report(&d).to_bytes()).to_string();
        assert!(s.contains("(stage 1"), "no stage 1");
        assert!(!s.contains("(stage 2"), "should not claim a second stage");
    }

    #[test]
    fn reports_are_deterministic() {
        let a = cic_report(CicReport::default()).unwrap().to_bytes();
        let b = cic_report(CicReport::default()).unwrap().to_bytes();
        assert_eq!(a, b, "a committed report must regenerate byte-identically");
    }

    #[test]
    fn a_different_configuration_gives_a_different_report() {
        let a = cic_report(CicReport::default()).unwrap().to_bytes();
        let cfg = CicReport {
            stages: 5,
            ..CicReport::default()
        };
        let b = cic_report(cfg).unwrap().to_bytes();
        assert_ne!(a, b, "the report must depend on the configuration");
    }

    #[test]
    fn an_undesignable_spec_is_reported_rather_than_panicking() {
        let cfg = CicReport {
            taps: 16, // even: no centre tap
            ..CicReport::default()
        };
        assert!(cic_report(cfg).is_none());
    }
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
    let mut doc = Pdf::new();
    doc.push(chain_page_one(d));
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

fn chain_page_one(d: &chain::ChainDesign) -> Page {
    let mut p = Page::a4();
    p.fill_style((0.0, 0.0, 0.0));
    p.text(
        297.5,
        800.0,
        16.0,
        Font::Bold,
        Align::Centre,
        "Decimation Chain - Derived Design",
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
