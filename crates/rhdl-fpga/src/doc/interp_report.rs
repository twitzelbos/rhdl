#![warn(missing_docs)]
//! Report generation: a PDF describing an **interpolation** chain.
//!
//! The transmit counterpart of [`super::report`]. Where that one plots
//! aliases folding into a band, this one plots images radiating out of
//! it — and the two questions a transmit designer actually asks are
//! *how far down are the images* and *how flat is the band*.
//!
//! Two A4 pages:
//!
//! - **Chain and images.** The composite response across the whole
//!   converter band with every image band marked, so the answer to "how
//!   far down" is a picture rather than a number. Then the stages, their
//!   rates, and their widths.
//! - **Compensation and result.** The droop, the pre-compensator, and
//!   the composite; the asked-for versus achieved figures; and the taps.
//!
//! # The report says what the compensator cannot do
//!
//! Prominently, on page two. A reader who sees images 45 dB down and
//! wants 60 will reach for more compensator taps, because that is what
//! works on the receive side. It cannot work here: a pre-compensator
//! runs at the envelope rate, so its response is periodic and the image
//! at `k + u` sees exactly the gain the signal at `u` sees. The knobs
//! are `N`, `R` and bandwidth.
//!
//! Putting that in the report rather than only in the rustdoc is
//! deliberate. The report is what gets printed and argued over, and this
//! is the one thing about a transmit chain that transposing receive
//! intuition gets wrong.
//!
//! Deterministic — see [`super::pdf`] — so a committed report can be
//! diffed and a change means something.

use super::pdf::{Align, Font, Page, Pdf};
// Shared with `super::report` rather than copied: two tap wrappers is
// two things to keep in step, and the one in `report` already carries
// the reasoning about separators and the right page edge.
use super::plot::{Axes, Frame, PALETTE, Series, draw};
use super::report::wrap_values;
use crate::dsp::cic::{compensator, interp, interp_chain, response};

/// A hand-specified interpolator to report on.
///
/// For the "I have chosen these parameters, what do they do" question.
/// [`interp_chain_report`] answers the other one — "here are my
/// requirements, what should I build".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InterpReport {
    /// Converter (output) sample rate, in Hz.
    pub fs_hz: f64,
    /// Envelope width, per component.
    pub w_in: usize,
    /// CIC stages.
    pub stages: usize,
    /// Interpolation factor.
    pub rate: usize,
    /// Differential delay.
    pub delay: usize,
    /// Signal bandwidth as a fraction of the envelope Nyquist.
    pub passband: f64,
    /// Compensator length, in taps.
    pub taps: usize,
    /// Coefficient width.
    pub coeff_width: usize,
    /// Converter width.
    pub output_width: usize,
}

impl Default for InterpReport {
    /// The worked configuration in [`crate::dsp::cic::interp`]: 1 Msps
    /// envelope onto a 125 Msps converter.
    fn default() -> Self {
        Self {
            fs_hz: 125e6,
            w_in: 16,
            stages: 3,
            rate: 125,
            delay: 1,
            passband: 0.4,
            taps: 15,
            coeff_width: 16,
            output_width: 14,
        }
    }
}

/// Render a report for an interpolator you specified by hand.
///
/// Synthesises the single-stage [`interp_chain::InterpDesign`] the given
/// parameters describe and hands it to the same renderer
/// [`interp_chain_report`] uses, so the two reports cannot drift.
///
/// Returns `None` if the compensator cannot be designed for these
/// parameters, which in practice means **an even tap count**: a
/// symmetric compensator needs a centre tap.
///
/// The other documented failure — a passband reaching a CIC null — is
/// not reachable here. [`InterpReport::passband`] is a fraction of the
/// envelope Nyquist, so it tops out at `u = 0.5`, and the nearest null
/// is at `u = 1`. A caller would have to pass a passband above one,
/// which describes a band wider than the rate carrying it.
pub fn interp_report(cfg: InterpReport) -> Option<Pdf> {
    Some(render(&as_design(cfg)?, "Specified Parameters"))
}

/// The single-stage design the given parameters describe.
///
/// Separate from the rendering so a caller can inspect what their
/// parameters amount to without producing a PDF.
pub fn as_design(cfg: InterpReport) -> Option<interp_chain::InterpDesign> {
    let (n, r, m) = (cfg.stages, cfg.rate, cfg.delay);
    let shapes = vec![compensator::CicShape {
        decimate: r,
        stages: n,
        delay: m,
    }];
    let cdesign = compensator::design(compensator::Spec {
        cics: shapes.clone(),
        passband: cfg.passband,
        taps: cfg.taps,
        stopband_edge: 1.0,
        // A pre-compensator's stopband buys no image rejection, so
        // asking for one would spend taps on nothing. See the module
        // docs.
        min_stopband_db: 0.0,
        max_ripple_db: 100.0,
        method: compensator::Method::LeastSquares,
    })?;
    let quantised = compensator::quantise(&cdesign, cfg.coeff_width);

    let input_rate_hz = cfg.fs_hz / r as f64;
    let (image_db, at_u) = interp_chain::cascade_image_db(&shapes, cfg.passband, r);
    let scale = 2f64.powi(quantised.shift as i32);
    let real: Vec<f64> = quantised.taps.iter().map(|x| *x as f64 / scale).collect();
    let (any_input, in_band, l1, peak) =
        interp_chain::compensator_headroom_of(cfg.w_in, &real, cfg.passband);
    let widths: Vec<usize> = (1..=2 * n)
        .map(|j| interp::stage_width(j, cfg.w_in, n, r, m))
        .collect();

    // The spec this configuration would have satisfied, so the report's
    // "asked for" column is honest about being a restatement.
    let spec = interp_chain::InterpSpec {
        fs_hz: cfg.fs_hz,
        interpolate: r,
        image_free_bw_hz: cfg.passband * cfg.fs_hz / (2.0 * r as f64),
        input_width: cfg.w_in,
        output_width: cfg.output_width,
        max_ripple_db: quantised.ripple_db,
        min_image_rejection_db: image_db,
        coeff_width: cfg.coeff_width,
        max_stages: n,
        max_taps: cfg.taps,
        max_chain_stages: 1,
        method: compensator::Method::LeastSquares,
        // A hand-specified single stage reaches every rate up to its
        // factor, so the range is the full one and nothing is
        // restricted.
        rate_min: 2,
        arbitrary_rate: true,
    };

    Some(interp_chain::InterpDesign {
        spec,
        cics: vec![interp_chain::InterpStage {
            interpolate: r,
            stages: n,
            delay: m,
            input_rate_hz,
            output_rate_hz: cfg.fs_hz,
            input_width: cfg.w_in,
            accumulator_width: interp::accumulator_width(cfg.w_in, n, r, m),
            stage_widths: widths,
            uniform_state_bits: interp::uniform_state_bits(cfg.w_in, n, r, m),
            tapered_state_bits: interp::tapered_state_bits(cfg.w_in, n, r, m),
            built_state_bits: interp::implemented_state_bits(cfg.w_in, n, r, m),
            built_widths: (1..=2 * n)
                .map(|j| interp::implemented_stage_width(j, cfg.w_in, n, r, m))
                .collect(),
        }],
        compensator: quantised.clone(),
        passband: cfg.passband,
        input_rate_hz,
        achieved_ripple_db: quantised.ripple_db,
        achieved_image_db: image_db,
        // Hand-specified parameters describe one rate, so that rate is
        // trivially the worst one.
        worst_image_rate: r,
        worst_image_hz: at_u * input_rate_hz,
        dac_snr_db: 6.02 * cfg.output_width as f64 + 1.76,
        cost: 0.0,
        register_bits: interp::uniform_state_bits(cfg.w_in, n, r, m),
        tapered_register_bits: interp::tapered_state_bits(cfg.w_in, n, r, m),
        built_register_bits: interp::implemented_state_bits(cfg.w_in, n, r, m),
        mid_width_any_input: any_input,
        mid_width_in_band: in_band,
        compensator_l1: l1,
        compensator_peak: peak,
        alternative: None,
    })
}

/// Render a report for a design [`interp_chain::design`] derived.
pub fn interp_chain_report(d: &interp_chain::InterpDesign) -> Pdf {
    render(d, "Derived Design")
}

/// Render a design under a given provenance label.
///
/// The heading says where the numbers came from, because that changes
/// how the reader should treat them — same reasoning as
/// [`super::report::render`].
pub fn render(d: &interp_chain::InterpDesign, provenance: &str) -> Pdf {
    let mut doc = Pdf::new();
    doc.push(page_one(d, provenance));
    doc.push(page_two(d));
    doc
}

/// The taps as reals, for evaluating what the hardware does.
fn real_taps(d: &interp_chain::InterpDesign) -> Vec<f64> {
    let scale = 2f64.powi(d.compensator.shift as i32);
    d.compensator
        .taps
        .iter()
        .map(|t| *t as f64 / scale)
        .collect()
}

fn page_one(d: &interp_chain::InterpDesign, provenance: &str) -> Page {
    let mut p = Page::a4();
    p.fill_style((0.0, 0.0, 0.0));
    p.text(
        297.5,
        800.0,
        16.0,
        Font::Bold,
        Align::Centre,
        &format!("Interpolation Chain - {provenance}"),
    );
    p.text(
        297.5,
        784.0,
        9.0,
        Font::Regular,
        Align::Centre,
        &format!(
            "{:.4} kHz x {} = {:.3} MHz, split {:?}, {:.3} kHz signal",
            d.input_rate_hz / 1e3,
            d.spec.interpolate,
            d.spec.fs_hz / 1e6,
            d.split(),
            d.spec.image_free_bw_hz / 1e3
        ),
    );

    let order = d.evaluation_shapes();
    let total = d.spec.interpolate as f64;
    let edge = response::passband_edge_out(d.passband);

    // ---- the whole converter band, with the images marked ----
    //
    // The transmit picture. `g` is the output-rate frequency the DAC
    // sees; the cascade evaluator wants envelope-rate `u = g * R`.
    let curve: Vec<(f64, f64)> = (0..1400)
        .map(|k| {
            let g = 0.5 * k as f64 / 1399.0;
            let a = compensator::cascade_magnitude(&order, g * total);
            (g, if a <= 1e-15 { -160.0 } else { 20.0 * a.log10() })
        })
        .collect();
    let axes = Axes::new(
        "Composite magnitude across the converter band",
        "frequency / converter rate",
        "dB",
        (0.0, 0.5),
        (-160.0, 5.0),
    );
    let mut s = vec![Series::new("cascade", curve, PALETTE[0])];
    // Every image band: the signal repeats at each multiple of the
    // envelope rate, which is `g = k / R`.
    let bands: Vec<(f64, f64)> = (1..=(d.spec.interpolate / 2))
        .flat_map(|k| {
            let lo = (k as f64 - edge) / total;
            let hi = (k as f64 + edge) / total;
            [
                (lo, -160.0),
                (lo, 5.0),
                (hi, 5.0),
                (hi, -160.0),
                (hi, -160.0),
            ]
        })
        .collect();
    s.push(Series::new("image bands", bands, (0.80, 0.80, 0.85)).dashed());
    s.push(
        Series::new(
            format!("worst image {:.1} dB down", d.achieved_image_db),
            vec![(0.0, -d.achieved_image_db), (0.5, -d.achieved_image_db)],
            PALETTE[1],
        )
        .dashed(),
    );
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

    // ---- the first image, up close ----
    //
    // Where the requirement is actually met or missed. The full-band
    // plot shows the shape; this shows the margin.
    let g_hi = (1.5 / total).min(0.5);
    let near: Vec<(f64, f64)> = (0..800)
        .map(|k| {
            let g = g_hi * k as f64 / 799.0;
            let a = compensator::cascade_magnitude(&order, g * total);
            (g, if a <= 1e-15 { -160.0 } else { 20.0 * a.log10() })
        })
        .collect();
    let y_lo = -(((d.achieved_image_db + 25.0) / 10.0).ceil() * 10.0);
    let axes = Axes::new(
        "The signal and its first image",
        "frequency / converter rate",
        "dB",
        (0.0, g_hi),
        (y_lo, 5.0),
    );
    let s = vec![
        Series::new("cascade", near, PALETTE[0]),
        Series::new(
            format!("signal edge {:.4}", edge / total),
            vec![(edge / total, y_lo), (edge / total, 5.0)],
            PALETTE[3],
        )
        .dashed(),
        Series::new(
            format!("asked >= {:.0} dB", d.spec.min_image_rejection_db),
            vec![
                (0.0, -d.spec.min_image_rejection_db),
                (g_hi, -d.spec.min_image_rejection_db),
            ],
            (0.45, 0.45, 0.45),
        )
        .dashed(),
        Series::new(
            format!("achieved {:.1} dB", d.achieved_image_db),
            vec![(0.0, -d.achieved_image_db), (g_hi, -d.achieved_image_db)],
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

    // ---- the stages ----
    p.fill_style((0.0, 0.0, 0.0));
    p.text(60.0, 235.0, 10.0, Font::Bold, Align::Left, "Stages");
    let mut y = 220.0;
    for (k, c) in d.cics.iter().enumerate() {
        for line in [
            format!(
                "stage {}: x{}  N={} M={}  combs at {:.4} MHz, integrators at {:.3} MHz",
                k + 1,
                c.interpolate,
                c.stages,
                c.delay,
                c.input_rate_hz / 1e6,
                c.output_rate_hz / 1e6
            ),
            format!(
                "   in {} bits, uniform accumulator {} bits, tapered widths {:?}",
                c.input_width, c.accumulator_width, c.stage_widths
            ),
            format!(
                "   {} register bits uniform, {} as built (lossless), bound {}",
                c.uniform_state_bits, c.built_state_bits, c.tapered_state_bits
            ),
            format!("   widths as built {:?}", c.built_widths),
        ] {
            p.text(60.0, y, 8.0, Font::Regular, Align::Left, &line);
            y -= 11.0;
        }
        y -= 4.0;
    }
    for line in [
        "The combs run at the stage's input rate and the integrators at its output rate, so an",
        "integrator bit costs R times what a comb bit costs. Depth belongs early and rate late.",
        "The tapered widths are lossless: each stage sized to its own growth bound holds its value",
        "exactly, so a tapered interpolator is bit-identical to a uniform one.",
    ] {
        p.text(60.0, y - 6.0, 7.5, Font::Regular, Align::Left, line);
        y -= 9.0;
    }
    p
}

fn page_two(d: &interp_chain::InterpDesign) -> Page {
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
            "{} taps, {}-bit coefficients, at the {:.4} MHz envelope rate",
            d.compensator.taps.len(),
            d.compensator.coeff_width,
            d.input_rate_hz / 1e6
        ),
    );

    let order = d.evaluation_shapes();
    let taps = real_taps(d);
    let edge = response::passband_edge_out(d.passband);

    // ---- the passband: droop, compensator, composite ----
    let mut cic = Vec::with_capacity(600);
    let mut fir = Vec::with_capacity(600);
    let mut both = Vec::with_capacity(600);
    for k in 0..600 {
        let u = edge * k as f64 / 599.0;
        let c = compensator::cascade_magnitude(&order, u);
        let h = compensator::fir_amplitude(&taps, u).abs();
        cic.push((u, 20.0 * c.log10()));
        fir.push((u, 20.0 * h.log10()));
        both.push((u, 20.0 * (c * h).log10()));
    }
    let droop = response::passband_droop_db(
        d.passband,
        d.cics[0].stages,
        d.cics[0].interpolate,
        d.cics[0].delay,
    );
    let y_lo = (droop - 3.0).min(-3.0);
    let axes = Axes::new(
        "Droop, pre-compensator, and the two together - the signal band",
        "frequency / envelope rate (Nyquist = 0.5)",
        "dB",
        (0.0, edge),
        (y_lo, 15.0),
    );
    let s = vec![
        Series::new("cascade", cic, PALETTE[0]),
        Series::new("compensator", fir, PALETTE[4]),
        Series::new("composite", both, PALETTE[2]),
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

    // ---- the compensator, over a whole period ----
    //
    // Two periods of `u`, which is the plot that makes the module's
    // central point visible: the compensator's gain repeats, so it lifts
    // each image exactly as much as it lifts the signal.
    let mut wide = Vec::with_capacity(800);
    for k in 0..800 {
        let u = 2.0 * k as f64 / 799.0;
        let h = compensator::fir_amplitude(&taps, u).abs();
        wide.push((u, 20.0 * h.log10()));
    }
    let axes = Axes::new(
        "The compensator repeats, so it lifts every image as much as the signal",
        "frequency / envelope rate",
        "dB",
        (0.0, 2.0),
        (-20.0, 20.0),
    );
    let s = vec![
        Series::new("compensator", wide, PALETTE[4]),
        Series::new(
            "signal band",
            vec![(0.0, -20.0), (0.0, 20.0), (edge, 20.0), (edge, -20.0)],
            PALETTE[3],
        )
        .dashed(),
        Series::new(
            "first image band",
            vec![
                (1.0 - edge, -20.0),
                (1.0 - edge, 20.0),
                (1.0 + edge, 20.0),
                (1.0 + edge, -20.0),
            ],
            (0.80, 0.80, 0.85),
        )
        .dashed(),
    ];
    draw(
        &mut p,
        Frame {
            x: 60.0,
            y: 300.0,
            w: 470.0,
            h: 180.0,
        },
        &axes,
        &s,
    );

    // ---- the figures ----
    p.fill_style((0.0, 0.0, 0.0));
    p.text(60.0, 250.0, 10.0, Font::Bold, Align::Left, "Figures");
    let mut y = 235.0;
    let mut lines: Vec<String> = vec![
        format!(
            "images ............... {:.1} dB down  (asked >= {:.1})",
            d.achieved_image_db, d.spec.min_image_rejection_db
        ),
        format!("   worst image at .... {:.4} MHz", d.worst_image_hz / 1e6),
        format!(
            "passband ripple ...... {:.4} dB  (asked <= {:.3})",
            d.achieved_ripple_db, d.spec.max_ripple_db
        ),
        format!("uncompensated droop .. {:.3} dB at the band edge", droop),
        format!(
            "converter floor ...... {:.1} dB at {} bits (the chain adds none)",
            d.dac_snr_db, d.spec.output_width
        ),
        format!(
            "register bits ........ {} uniform, {} as built, {} at the exact bound",
            d.register_bits, d.built_register_bits, d.tapered_register_bits
        ),
        format!(
            "compensator DC gain .. {:.4}, shift {}",
            d.compensator.dc_gain, d.compensator.shift
        ),
        format!(
            "compensator headroom . {} bits for any input, {} for in-band only",
            d.mid_width_any_input, d.mid_width_in_band
        ),
        format!(
            "   norms ............. l1 {:.4}, passband peak {:.4}",
            d.compensator_l1, d.compensator_peak
        ),
        format!(
            "rates reachable ...... {} of {} in {}..={}{}",
            d.reachable_rates()
                .iter()
                .filter(|r| **r >= d.spec.rate_min && **r <= d.spec.interpolate)
                .count(),
            d.spec.interpolate - d.spec.rate_min + 1,
            d.spec.rate_min,
            d.spec.interpolate,
            if d.cics.len() > 1 {
                "  (a split restricts them)"
            } else {
                "  (single stage: all of them)"
            }
        ),
        format!("worst image at rate .. {}", d.worst_image_rate),
        format!(
            "adder depth .......... {} deep in the combs, 1 in the integrators",
            d.cics.iter().map(|c| c.stages).max().unwrap_or(0)
        ),
    ];
    if let Some(a) = &d.alternative {
        lines.push(format!(
            "runner-up ............ split {:?} N={:?} M={:?}, {} register bits",
            a.split, a.stages, a.delays, a.register_bits
        ));
    }
    for line in &lines {
        p.text(60.0, y, 8.0, Font::Regular, Align::Left, line);
        y -= 11.0;
    }
    y -= 6.0;
    for line in wrap_values("taps ................. ", &d.compensator.taps, 475.0, 8.0) {
        p.text(60.0, y, 8.0, Font::Regular, Align::Left, &line);
        y -= 11.0;
    }

    // ---- headroom, which is easy to under-read ----
    y -= 8.0;
    p.text(
        60.0,
        y,
        9.0,
        Font::Bold,
        Align::Left,
        "Build to the any-input headroom unless the envelope is band-limited.",
    );
    y -= 12.0;
    for line in [
        "The in-band figure is the peak gain an in-band sinusoid sees. The any-input figure is the",
        "sum of the taps' magnitudes, which is the bound for an arbitrary bounded input -- and a",
        "transmit envelope is not band-limited: switch-on, a burst boundary and a modulation change",
        "are all steps, and a step is exactly the input that reaches it. Choosing the narrower",
        "figure for a burst transmitter is how a compensator saturates on the first sample.",
    ] {
        p.text(60.0, y, 7.5, Font::Regular, Align::Left, line);
        y -= 9.5;
    }

    // ---- the thing a receive-trained reader will get wrong ----
    y -= 10.0;
    p.text(
        60.0,
        y,
        9.0,
        Font::Bold,
        Align::Left,
        "More taps will not improve image rejection.",
    );
    y -= 12.0;
    for line in [
        "The compensator runs before the rate change, at the envelope rate, so its response is",
        "periodic and the image at k+u sees exactly the gain the signal at u sees -- the plot above",
        "shows it. The image-to-signal ratio is the cascade's alone. This is the one place where",
        "transposing receive-side intuition is wrong: there, the compensator sits after the fold and",
        "its stopband is part of the alias budget.",
        "",
        "The knobs are the CIC depth N, the interpolation factor R, and the signal bandwidth. A",
        "compensator at the converter rate would work, because it is no longer periodic in u, and",
        "costs a FIR running at the full clock.",
    ] {
        p.text(60.0, y, 7.5, Font::Regular, Align::Left, line);
        y -= 9.5;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default hand-specified configuration renders.
    #[test]
    fn the_default_configuration_renders() {
        let doc = interp_report(InterpReport::default()).expect("designable");
        let bytes = doc.to_bytes();
        assert!(bytes.starts_with(b"%PDF"), "must be a PDF");
        assert!(
            bytes.len() > 10_000,
            "two pages of plots, got {}",
            bytes.len()
        );
    }

    /// And so does a derived design, through the same renderer.
    #[test]
    fn a_derived_design_renders() {
        let d = interp_chain::design(interp_chain::InterpSpec::default()).expect("designable");
        let bytes = interp_chain_report(&d).to_bytes();
        assert!(bytes.starts_with(b"%PDF"));
        assert!(bytes.len() > 10_000);
    }

    /// The headroom figures reach the page.
    ///
    /// Not just computed — a figure a reader needs and cannot see is not
    /// reported. Checked by rendering and looking for the label, which is
    /// crude and is exactly the failure it catches: a field added to the
    /// design and never wired into a page.
    #[test]
    fn the_headroom_figures_are_on_the_page() {
        let d = as_design(InterpReport::default()).expect("designable");
        assert!(d.mid_width_any_input >= d.mid_width_in_band);
        let bytes = interp_report(InterpReport::default()).unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("compensator headroom"),
            "the headroom line must be rendered"
        );
        assert!(
            text.contains("adder depth"),
            "the adder-depth line must be rendered"
        );
    }

    /// **The report is deterministic.**
    ///
    /// The property that makes committing it worthwhile: a diff means a
    /// design changed, not that the clock moved.
    #[test]
    fn the_report_is_byte_identical_across_runs() {
        let a = interp_report(InterpReport::default()).unwrap().to_bytes();
        let b = interp_report(InterpReport::default()).unwrap().to_bytes();
        assert_eq!(a, b);
    }

    /// The synthesised design describes the parameters it was given.
    #[test]
    fn the_synthesised_design_matches_its_parameters() {
        let cfg = InterpReport::default();
        let d = as_design(cfg).expect("designable");
        assert_eq!(d.cics.len(), 1);
        assert_eq!(d.cics[0].interpolate, cfg.rate);
        assert_eq!(d.cics[0].stages, cfg.stages);
        assert_eq!(d.input_rate_hz, cfg.fs_hz / cfg.rate as f64);
        assert_eq!(d.passband, cfg.passband);
        // The "asked for" figures are restatements, which is what
        // "Specified Parameters" in the heading warns the reader about.
        assert_eq!(d.spec.min_image_rejection_db, d.achieved_image_db);
        assert_eq!(d.spec.max_ripple_db, d.achieved_ripple_db);
    }

    /// **The two report paths agree on the numbers they share.**
    ///
    /// `as_design` synthesises a design by hand and `interp_chain`
    /// derives one; pointed at the same *shape* they must compute the
    /// same image rejection, because they call the same evaluator. If
    /// they diverge, one of them is scaling the cascade differently —
    /// the error `interp_chain`'s ordering helper exists to prevent.
    ///
    /// The derived search minimises cost, so it has to be *forced* to
    /// the shape being compared. At this configuration each CIC stage
    /// buys 12.62 dB of image rejection, so asking for 37 dB admits
    /// `N = 3` and excludes `N = 2` — which is how the requirement below
    /// is chosen rather than guessed.
    #[test]
    fn the_two_paths_agree_on_image_rejection() {
        let cfg = InterpReport::default();
        let hand = as_design(cfg).expect("designable");
        let spec = interp_chain::InterpSpec {
            fs_hz: cfg.fs_hz,
            interpolate: cfg.rate,
            image_free_bw_hz: cfg.passband * cfg.fs_hz / (2.0 * cfg.rate as f64),
            input_width: cfg.w_in,
            output_width: cfg.output_width,
            max_ripple_db: 1.0,
            // Admits N = 3 and excludes N = 2; see the doc comment.
            min_image_rejection_db: 37.0,
            coeff_width: cfg.coeff_width,
            max_stages: cfg.stages,
            max_taps: cfg.taps,
            max_chain_stages: 1,
            method: compensator::Method::LeastSquares,
            rate_min: 2,
            arbitrary_rate: true,
        };
        let derived = interp_chain::design(spec).expect("designable");
        assert_eq!(derived.split(), vec![cfg.rate]);
        assert_eq!(
            derived.depths(),
            vec![cfg.stages],
            "forced to the same depth"
        );
        assert!(
            (derived.achieved_image_db - hand.achieved_image_db).abs() < 1e-9,
            "derived {:.6} vs hand {:.6}",
            derived.achieved_image_db,
            hand.achieved_image_db
        );
    }

    /// **Each CIC stage buys the same number of dB.**
    ///
    /// A `sinc^N` response is the first-order response to the `N`, so
    /// image rejection is linear in depth — 12.62 dB per stage at the
    /// default configuration. Worth pinning because it is the number a
    /// reader of the report uses to decide how much depth to buy, and
    /// because a non-linear result would mean the cascade evaluation was
    /// wrong.
    #[test]
    fn image_rejection_is_linear_in_the_stage_count() {
        let cfg = InterpReport::default();
        let per_stage: Vec<f64> = (1..=5)
            .map(|n| {
                let sh = vec![compensator::CicShape {
                    decimate: cfg.rate,
                    stages: n,
                    delay: cfg.delay,
                }];
                interp_chain::cascade_image_db(&sh, cfg.passband, cfg.rate).0
            })
            .collect();
        for (k, db) in per_stage.iter().enumerate() {
            let expected = 12.62 * (k + 1) as f64;
            assert!(
                (db - expected).abs() < 0.02,
                "N={}: {db:.3} dB, expected about {expected:.2}",
                k + 1
            );
        }
    }

    /// **An even tap count does not render**, which is the reachable
    /// `None`.
    ///
    /// A symmetric compensator needs a centre tap. The other documented
    /// failure — a passband reaching a null — is not reachable through
    /// this struct; see [`interp_report`].
    #[test]
    fn an_even_tap_count_does_not_render() {
        let cfg = InterpReport {
            taps: 14,
            ..InterpReport::default()
        };
        assert!(interp_report(cfg).is_none());
        // And the odd count either side does.
        for taps in [13usize, 15] {
            assert!(
                interp_report(InterpReport {
                    taps,
                    ..InterpReport::default()
                })
                .is_some(),
                "{taps} taps should design"
            );
        }
    }

    /// The tap wrapper keeps every value and every separator.
    #[test]
    fn the_tap_wrapper_loses_nothing() {
        let values: Vec<i64> = (0..40).map(|k| (k * 7919) % 1000 - 500).collect();
        let lines = wrap_values("taps ", &values, 200.0, 8.0);
        assert!(lines.len() > 1, "a long list must wrap");
        let joined = lines.join("").replace("taps ", "").replace(' ', "");
        let inner = joined.trim_start_matches('[').trim_end_matches(']');
        let back: Vec<i64> = inner.split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(back, values);
    }
}
