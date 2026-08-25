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

/// Point size the figures block is set in.
const FIGURE_SIZE: f64 = 8.0;

/// Width available to the figures block, in points.
///
/// A4 is 595 wide and the block starts at x=60, so this leaves the same
/// 60 points on the right.
const TEXT_WIDTH: f64 = 475.0;

/// A labelled list of numbers, broken across as many lines as it needs.
///
/// A tap list is the one figure a reader copies out of the report, and a
/// long one used to run off the right edge of the page with the last few
/// coefficients simply absent — the report looked complete and was not.
/// An equiripple design with a stopband requirement routinely needs
/// forty-plus taps, so this stopped being a corner case.
///
/// Continuation lines are indented under the first value rather than to
/// the margin, so the block still reads as one field.
fn wrap_values<T: std::fmt::Display>(
    label: &str,
    values: &[T],
    max_width: f64,
    size: f64,
) -> Vec<String> {
    let f = Font::Regular;
    let indent = " ".repeat(label.chars().count());
    let mut out = Vec::new();
    let mut line = format!("{label}[");
    let mut first = true;
    for v in values {
        let piece = if first {
            v.to_string()
        } else {
            format!(", {v}")
        };
        if !first && f.width_of(&format!("{line}{piece}"), size) > max_width {
            // The separator goes at the end of the broken line, not the
            // start of the next: a list split as `..., 3975` / `-17439,
            // ...` has no comma between those two values at all, and
            // copying it out of the report silently merges them.
            line.push(',');
            out.push(line);
            line = format!("{indent} {v}");
        } else {
            line.push_str(&piece);
        }
        first = false;
    }
    line.push(']');
    out.push(line);
    out
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
    // The three curves over `0 .. x_hi`, sampled uniformly.
    let curves = |x_hi: f64| {
        let at = |k: usize| x_hi * k as f64 / 599.0;
        let mut cic = Vec::with_capacity(600);
        let mut fir = Vec::with_capacity(600);
        let mut both = Vec::with_capacity(600);
        for k in 0..600 {
            let f = at(k);
            let c = compensator::cascade_magnitude(&sh, f);
            let h = compensator::fir_amplitude(&real, f).abs();
            cic.push((f, 20.0 * c.log10()));
            fir.push((f, 20.0 * h.log10()));
            both.push((f, 20.0 * (c * h).log10()));
        }
        (cic, fir, both)
    };

    // A requested stopband is the entire reason an equiripple design
    // differs from a droop-inverting one, and it lives *above* the
    // passband. Plotting only `0 .. edge` therefore renders a Remez
    // design and a least-squares design as the same picture, with the
    // achieved attenuation appearing nowhere but a line of text. When a
    // stopband was asked for, this plot spans the full output Nyquist so
    // that the transition, the floor, and whether the floor was actually
    // met are all visible. The zoomed plot below still covers the
    // passband, so nothing is lost by widening this one.
    let band = response::passband_edge_out(d.spec.stopband_edge);
    // An empty stopband is not a stopband: with `stopband_edge` at
    // Nyquist there is no band above it to measure, `stopband_db`
    // reports infinity, and the marker rendered as "compensator inf dB".
    // A finite achieved level is required for the same reason.
    let stop = (d.spec.min_stopband_db > 0.0 && band < 0.5 && d.achieved_stopband_db.is_finite())
        .then_some((band, d.spec.min_stopband_db));
    let x_hi = if stop.is_some() { 0.5 } else { edge };
    // Round the floor down to a decade tick so the deepest line drawn is
    // not sitting on the frame. It has to clear the *achieved* level as
    // well as the asked-for one: a design that overshoots badly -- asked
    // 40 dB, got 90 -- would otherwise have its achieved line clamped
    // onto the frame floor, which reads as "off the bottom of the plot"
    // rather than as the 90 dB it is.
    let y_lo = match stop {
        Some((_, db)) => {
            let deepest = db.max(d.achieved_stopband_db.min(200.0));
            -(((deepest + 25.0) / 10.0).ceil() * 10.0)
        }
        None => -15.0,
    };

    let (cic, fir, wide) = curves(x_hi);
    let axes = Axes::new(
        if stop.is_some() {
            "Cascade, compensator, and the two together - full output band"
        } else {
            "Cascade, compensator, and the two together"
        },
        "frequency / output rate",
        "dB",
        (0.0, x_hi),
        (y_lo, 15.0),
    );
    let mut s = vec![
        Series::new("cascade", cic, PALETTE[0]),
        Series::new("compensator", fir, PALETTE[4]),
        Series::new("composite", wide, PALETTE[2]),
        Series::new("flat", vec![(0.0, 0.0), (x_hi, 0.0)], (0.6, 0.6, 0.6)).dashed(),
    ];
    if let Some((band, db)) = stop {
        // Both levels describe the composite, so the achieved line is
        // drawn in the composite's colour and sits on the green curve's
        // stopband peaks. It used to describe the compensator alone,
        // which needed the line drawn in *that* curve's colour and
        // explicitly labelled, because a reader matching it to the
        // composite would have read an error of 30 dB. Measuring what
        // the requirement is actually about removed the need to explain
        // which curve the number belonged to.
        s.push(
            Series::new(
                "stopband edge",
                vec![(band, y_lo), (band, 15.0)],
                (0.45, 0.45, 0.45),
            )
            .dashed(),
        );
        s.push(
            Series::new(
                format!("asked >= {db:.0} dB"),
                vec![(band, -db), (x_hi, -db)],
                (0.45, 0.45, 0.45),
            )
            .dashed(),
        );
        s.push(
            Series::new(
                format!("composite {:.1} dB", d.achieved_stopband_db),
                vec![
                    (band, -d.achieved_stopband_db),
                    (x_hi, -d.achieved_stopband_db),
                ],
                PALETTE[2],
            )
            .dashed(),
        );
    }
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
    let (_, _, both) = curves(edge);
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
        if d.achieved_stopband_db.is_finite() {
            lines.push(format!(
                "stopband .......... {:.1} dB above {:.2} Nyquist (asked >= {:.1})",
                d.achieved_stopband_db, d.spec.stopband_edge, d.spec.min_stopband_db
            ));
            // No disambiguating note any more: the figure is the
            // composite's, which is the curve the reader is looking at.
        } else {
            // `stopband_db` reports infinity when the edge leaves no
            // band above it. Printing that with `{:.1}` gave
            // "stopband .......... inf dB", which reads as a spectacular
            // result rather than as a requirement that measured nothing.
            lines.push(format!(
                "stopband .......... no band above {:.2} Nyquist to measure (asked >= {:.1})",
                d.spec.stopband_edge, d.spec.min_stopband_db
            ));
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "register bits ..... {} (rate-weighted cost {:.1})",
        d.register_bits, d.cost
    ));
    lines.extend(wrap_values(
        "taps .............. ",
        &d.compensator.taps,
        TEXT_WIDTH,
        FIGURE_SIZE,
    ));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A design whose stopband figures can be set without re-running the
    /// search, because what is under test is the *rendering* decision.
    fn design_with_stopband(asked: f64, achieved: f64) -> chain::ChainDesign {
        let mut d = as_design(CicReport::default()).expect("the default must design");
        d.spec.stopband_edge = 0.75;
        d.spec.min_stopband_db = asked;
        d.achieved_stopband_db = achieved;
        d
    }

    fn rendered(d: &chain::ChainDesign) -> String {
        String::from_utf8_lossy(&render(d, "Test").to_bytes()).to_string()
    }

    /// The stopband markers appear when a stopband was asked for.
    ///
    /// This is the whole point of the change: without them an equiripple
    /// design and a least-squares one render identically, and the only
    /// evidence of the attenuation is a line of text.
    #[test]
    fn a_requested_stopband_is_drawn() {
        let out = rendered(&design_with_stopband(60.0, 66.0));
        assert!(out.contains("stopband edge"), "no stopband edge marker");
        assert!(out.contains("asked >= 60 dB"), "no requested level");
        assert!(
            out.contains("composite 66.0 dB"),
            "no achieved level, or it does not name the composite"
        );
    }

    /// And not otherwise. The default asks for no attenuation, so the
    /// markers would be three meaningless lines and a wider axis.
    #[test]
    fn an_unrequested_stopband_is_not_drawn() {
        let d = as_design(CicReport::default()).expect("the default must design");
        assert_eq!(d.spec.min_stopband_db, 0.0, "premise of the test");
        let out = rendered(&d);
        assert!(!out.contains("stopband edge"));
        assert!(
            !out.contains("full output band"),
            "axis widened for nothing"
        );
    }

    /// The achieved level must be inside the frame even when it far
    /// overshoots what was asked, or it clamps onto the frame floor and
    /// reads as "off the bottom of the plot".
    #[test]
    fn a_wildly_overshot_stopband_still_fits_the_frame() {
        let out = rendered(&design_with_stopband(40.0, 90.0));
        assert!(out.contains("composite 90.0 dB"));
        // The y axis is labelled in round steps; asking for 40 but
        // achieving 90 must push the floor past -90.
        assert!(
            out.contains("-120") || out.contains("-100"),
            "axis did not extend to hold the achieved level"
        );
    }

    /// An infinite achieved attenuation means there was no stopband to
    /// measure, so there is nothing to mark.
    ///
    /// `stopband_db` returns infinity when `stopband_edge` sits at
    /// Nyquist, which `chain::design` accepts — it satisfies any
    /// requested level. The marker used to render "compensator inf dB".
    #[test]
    fn an_empty_stopband_is_not_drawn() {
        let out = rendered(&design_with_stopband(60.0, f64::INFINITY));
        assert!(!out.contains("inf"), "an infinity reached the page");
        assert!(!out.contains("NaN"), "NaN reached the page");
        assert!(!out.contains("stopband edge"), "marked an empty band");
    }

    /// A stopband edge at Nyquist leaves no band, whatever the achieved
    /// figure says.
    #[test]
    fn a_stopband_edge_at_nyquist_is_not_drawn() {
        let mut d = design_with_stopband(60.0, 66.0);
        d.spec.stopband_edge = 1.0;
        let out = rendered(&d);
        assert!(!out.contains("stopband edge"));
    }

    /// A tap list short enough to fit is left on one line.
    #[test]
    fn a_short_list_is_not_wrapped() {
        let out = wrap_values("taps ... ", &[1, -2, 3], TEXT_WIDTH, FIGURE_SIZE);
        assert_eq!(out, vec!["taps ... [1, -2, 3]".to_string()]);
    }

    /// A long one is wrapped, and nothing is lost doing it.
    ///
    /// The bug this guards is silent truncation: the list used to be
    /// formatted with `{:?}` and simply ran off the right edge of the
    /// page, so the report looked complete with its last coefficients
    /// missing.
    #[test]
    fn a_long_list_wraps_without_losing_values() {
        let values: Vec<i32> = (0..47).map(|i| (i * 977) % 30011 - 15000).collect();
        let out = wrap_values("taps .............. ", &values, TEXT_WIDTH, FIGURE_SIZE);
        assert!(out.len() > 1, "a 47-tap list should not fit on one line");
        for line in &out {
            assert!(
                Font::Regular.width_of(line, FIGURE_SIZE) <= TEXT_WIDTH,
                "line overflows the text width: {line:?}"
            );
        }
        // Every value survives, in order.
        let joined = out.join(" ");
        let recovered: Vec<i32> = joined
            .trim_start_matches(|c: char| c != '[')
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|t| t.trim().parse().expect("a value"))
            .collect();
        assert_eq!(recovered, values);
    }

    /// Continuation lines line up under the first value, so the block
    /// still reads as one field rather than as free text.
    #[test]
    fn wrapped_lines_are_indented_under_the_first_value() {
        let values: Vec<i32> = (0..60).collect();
        let label = "taps .............. ";
        let out = wrap_values(label, &values, TEXT_WIDTH, FIGURE_SIZE);
        assert!(out.len() > 1);
        for line in &out[1..] {
            let lead = line.len() - line.trim_start().len();
            assert_eq!(
                lead,
                label.len() + 1,
                "continuation not aligned under the first value: {line:?}"
            );
        }
    }

    /// An empty list is not a crash.
    #[test]
    fn an_empty_list_renders_as_empty_brackets() {
        let out = wrap_values("taps ... ", &[0i32; 0], TEXT_WIDTH, FIGURE_SIZE);
        assert_eq!(out, vec!["taps ... []".to_string()]);
    }
}
