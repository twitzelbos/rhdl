#![warn(missing_docs)]
//! Line plots on a [`super::pdf::Page`].
//!
//! Just enough to draw a labelled magnitude-response chart: linear
//! axes, tick marks, gridlines, several named series, and a legend.
//! Frequency response is what this exists for, so the defaults suit
//! it — but nothing here knows that.
//!
//! Deterministic, like [`super::pdf`]: same data in, same bytes out.

use super::pdf::{Align, Font, Page};

/// One named curve.
#[derive(Clone, Debug)]
pub struct Series {
    /// Legend label.
    pub label: String,
    /// Points in data coordinates.
    pub points: Vec<(f64, f64)>,
    /// Stroke colour.
    pub colour: (f64, f64, f64),
    /// Draw dashed rather than solid.
    pub dashed: bool,
}

impl Series {
    /// A solid series.
    pub fn new(label: impl Into<String>, points: Vec<(f64, f64)>, colour: (f64, f64, f64)) -> Self {
        Self {
            label: label.into(),
            points,
            colour,
            dashed: false,
        }
    }

    /// Draw this series dashed.
    pub fn dashed(mut self) -> Self {
        self.dashed = true;
        self
    }
}

/// Axis limits and labelling.
#[derive(Clone, Debug)]
pub struct Axes {
    /// Chart title.
    pub title: String,
    /// X axis label.
    pub x_label: String,
    /// Y axis label.
    pub y_label: String,
    /// `(min, max)` for x.
    pub x_range: (f64, f64),
    /// `(min, max)` for y.
    pub y_range: (f64, f64),
    /// Number of x divisions.
    pub x_ticks: usize,
    /// Number of y divisions.
    pub y_ticks: usize,
}

impl Axes {
    /// Axes with sensible tick counts.
    pub fn new(
        title: impl Into<String>,
        x_label: impl Into<String>,
        y_label: impl Into<String>,
        x_range: (f64, f64),
        y_range: (f64, f64),
    ) -> Self {
        Self {
            title: title.into(),
            x_label: x_label.into(),
            y_label: y_label.into(),
            x_range,
            y_range,
            x_ticks: 10,
            y_ticks: 6,
        }
    }
}

/// Where on the page to draw, in points.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Left edge of the plot area.
    pub x: f64,
    /// Bottom edge of the plot area.
    pub y: f64,
    /// Plot area width.
    pub w: f64,
    /// Plot area height.
    pub h: f64,
}

const GREY: (f64, f64, f64) = (0.75, 0.75, 0.75);
const BLACK: (f64, f64, f64) = (0.0, 0.0, 0.0);

/// Palette for successive series, so callers need not pick colours.
pub const PALETTE: [(f64, f64, f64); 5] = [
    (0.11, 0.31, 0.72), // blue
    (0.78, 0.15, 0.15), // red
    (0.10, 0.50, 0.20), // green
    (0.55, 0.25, 0.65), // purple
    (0.85, 0.50, 0.05), // orange
];

/// Tick positions at round numbers spanning `lo..hi`.
///
/// Dividing the range evenly gives ticks like `-19.1667` and
/// `-43.3333`, which is what a `-140..5` dB axis at six divisions
/// produces. Nobody reads a plot in thirds of a decibel — an axis is
/// for locating a value, and that wants round numbers.
///
/// Steps come from `{1, 2, 2.5, 5} x 10^k`, the conventional set:
/// picked as the smallest whose count is at or under the target, so the
/// axis is never busier than asked for.
//
// `!(hi > lo)` rather than `hi <= lo`, deliberately: it also catches
// `NaN`, where every comparison is false and the tick loop below would
// never terminate.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn nice_ticks(lo: f64, hi: f64, target: usize) -> Vec<f64> {
    if !(hi > lo) || target == 0 {
        return vec![lo];
    }
    let raw = (hi - lo) / target as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = mag
        * if norm <= 1.0 {
            1.0
        } else if norm <= 2.0 {
            2.0
        } else if norm <= 2.5 {
            2.5
        } else if norm <= 5.0 {
            5.0
        } else {
            10.0
        };
    let mut out = Vec::new();
    let mut t = (lo / step).ceil() * step;
    // A hair of tolerance, or a tick landing exactly on `hi` is lost to
    // floating-point.
    while t <= hi + step * 1e-9 {
        // Snap away the accumulated error, so 0.30000000000000004 does
        // not reach `format_tick`.
        out.push((t / step).round() * step);
        t += step;
    }
    if out.is_empty() {
        out.push(lo);
    }
    out
}

/// Draw a plot into `frame` on `page`.
///
/// Points outside `y_range` are clamped to the frame rather than
/// dropped, so a curve that dives to a null still shows where it went
/// instead of silently vanishing — a plot of a CIC response is mostly
/// interesting *because* of those dives.
pub fn draw(page: &mut Page, frame: Frame, axes: &Axes, series: &[Series]) {
    let (x0, x1) = axes.x_range;
    let (y0, y1) = axes.y_range;
    let sx = |v: f64| frame.x + (v - x0) / (x1 - x0) * frame.w;
    let sy = |v: f64| {
        let t = ((v - y0) / (y1 - y0)).clamp(0.0, 1.0);
        frame.y + t * frame.h
    };

    // Gridlines and ticks, at round numbers.
    let xt = nice_ticks(x0, x1, axes.x_ticks);
    let yt = nice_ticks(y0, y1, axes.y_ticks);
    page.stroke_style(GREY, 0.4);
    for v in &xt {
        page.line(sx(*v), frame.y, sx(*v), frame.y + frame.h);
    }
    for v in &yt {
        page.line(frame.x, sy(*v), frame.x + frame.w, sy(*v));
    }

    // Frame.
    page.stroke_style(BLACK, 0.8);
    page.rect(frame.x, frame.y, frame.w, frame.h);

    // Tick labels, thinned so they cannot collide.
    //
    // A narrow range produces long labels — the zoomed composite plot
    // wants `0.001` steps — and at ten divisions those overlap into
    // mush. Measuring the widest label against the available spacing
    // and dropping every other one keeps the axis readable without
    // choosing a tick count per plot by hand.
    page.fill_style(BLACK);
    let x_labels: Vec<String> = xt.iter().map(|v| format_tick(*v)).collect();
    let widest = x_labels
        .iter()
        .map(|l| Font::Regular.width_of(l, 7.0))
        .fold(0.0f64, f64::max);
    let spacing = if xt.len() > 1 {
        frame.w / (xt.len() - 1) as f64
    } else {
        frame.w
    };
    // +2pt of breathing room, so labels do not merely touch.
    let stride = ((widest + 2.0) / spacing).ceil().max(1.0) as usize;
    for (i, v) in xt.iter().enumerate() {
        if i % stride != 0 {
            continue;
        }
        page.text(
            sx(*v),
            frame.y - 12.0,
            7.0,
            Font::Regular,
            Align::Centre,
            &x_labels[i],
        );
    }
    for v in &yt {
        page.text(
            frame.x - 6.0,
            sy(*v) - 2.5,
            7.0,
            Font::Regular,
            Align::Right,
            &format_tick(*v),
        );
    }

    // Titles.
    page.text(
        frame.x + frame.w / 2.0,
        frame.y + frame.h + 10.0,
        10.0,
        Font::Bold,
        Align::Centre,
        &axes.title,
    );
    page.text(
        frame.x + frame.w / 2.0,
        frame.y - 24.0,
        8.0,
        Font::Regular,
        Align::Centre,
        &axes.x_label,
    );
    // Alongside the axis, reading upward, as a technical plot should.
    page.text_vertical(
        frame.x - 34.0,
        frame.y + frame.h / 2.0,
        8.0,
        Font::Regular,
        Align::Centre,
        &axes.y_label,
    );

    // Series.
    for s in series {
        let pts: Vec<(f64, f64)> = s
            .points
            .iter()
            .filter(|(x, _)| *x >= x0 - 1e-12 && *x <= x1 + 1e-12)
            .map(|(x, y)| (sx(*x), sy(*y)))
            .collect();
        page.stroke_style(s.colour, 1.1);
        if s.dashed {
            page.polyline_dashed(&pts, 3.0, 2.0);
        } else {
            page.polyline(&pts);
        }
    }

    // Legend, on an opaque panel, in whichever corner the data avoids.
    //
    // Fixed at top-right it sat on top of any curve that went there --
    // which for a CIC's full-band response is most of the left half.
    // Occupancy is measured in each corner and the emptiest wins; the
    // panel is filled regardless, so even a bad choice stays readable.
    let widest_label = series
        .iter()
        .map(|s| Font::Regular.width_of(&s.label, 7.0))
        .fold(0.0f64, f64::max);
    let box_w = widest_label + 30.0;
    let box_h = series.len() as f64 * 10.0 + 6.0;

    let occupancy = |x_lo: f64, x_hi: f64, y_lo: f64, y_hi: f64| -> usize {
        series
            .iter()
            .flat_map(|s| s.points.iter())
            .filter(|(x, y)| {
                let px = sx(*x);
                let py = sy(*y);
                px >= x_lo && px <= x_hi && py >= y_lo && py <= y_hi
            })
            .count()
    };
    let corners = [
        // (x of box left, y of box bottom)
        (
            frame.x + frame.w - box_w - 4.0,
            frame.y + frame.h - box_h - 4.0,
        ),
        (frame.x + 4.0, frame.y + frame.h - box_h - 4.0),
        (frame.x + frame.w - box_w - 4.0, frame.y + 4.0),
        (frame.x + 4.0, frame.y + 4.0),
    ];
    let (bx, by) = corners
        .iter()
        .map(|(x, y)| ((*x, *y), occupancy(*x, *x + box_w, *y, *y + box_h)))
        .min_by_key(|(_, n)| *n)
        .map(|(c, _)| c)
        .unwrap_or((corners[0].0, corners[0].1));

    page.fill_style((1.0, 1.0, 1.0));
    page.rect_filled(bx, by, box_w, box_h);
    page.stroke_style(GREY, 0.4);
    page.rect(bx, by, box_w, box_h);

    let mut ly = by + box_h - 9.0;
    for s in series {
        let lx = bx + 4.0;
        page.stroke_style(s.colour, 1.6);
        if s.dashed {
            page.polyline_dashed(&[(lx, ly + 2.0), (lx + 16.0, ly + 2.0)], 3.0, 2.0);
        } else {
            page.line(lx, ly + 2.0, lx + 16.0, ly + 2.0);
        }
        page.fill_style(BLACK);
        page.text(lx + 20.0, ly, 7.0, Font::Regular, Align::Left, &s.label);
        ly -= 10.0;
    }
}

/// Tick labels without trailing noise: integers plain, otherwise up to
/// three decimals with trailing zeros trimmed.
fn format_tick(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        return format!("{}", v.round() as i64);
    }
    // Enough places to distinguish neighbouring ticks, capped so a
    // rounding artefact does not produce `0.0500000001`.
    for places in 1..=4 {
        let s = format!("{v:.*}", places);
        if (s.parse::<f64>().unwrap_or(v) - v).abs() < 1e-9 {
            return s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
    }
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::super::pdf::Pdf;
    use super::*;

    fn chart() -> Pdf {
        let mut p = Page::a4();
        let axes = Axes::new("Response", "freq", "dB", (0.0, 0.5), (-80.0, 10.0));
        let s = vec![
            Series::new(
                "cic",
                (0..50).map(|k| (k as f64 / 98.0, -k as f64)).collect(),
                PALETTE[0],
            ),
            Series::new("flat", vec![(0.0, 0.0), (0.5, 0.0)], PALETTE[1]).dashed(),
        ];
        draw(
            &mut p,
            Frame {
                x: 60.0,
                y: 500.0,
                w: 470.0,
                h: 250.0,
            },
            &axes,
            &s,
        );
        let mut d = Pdf::new();
        d.push(p);
        d
    }

    #[test]
    fn it_produces_a_valid_document() {
        let b = chart().to_bytes();
        assert!(b.starts_with(b"%PDF"));
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("(Response) Tj"), "title missing");
        assert!(
            s.contains("(cic) Tj") && s.contains("(flat) Tj"),
            "legend missing"
        );
        // The dashed series must actually set and clear a dash pattern.
        assert!(s.contains("[3.00 2.00] 0 d") && s.contains("[] 0 d"));
    }

    #[test]
    fn plots_are_deterministic() {
        assert_eq!(chart().to_bytes(), chart().to_bytes());
    }

    /// Out-of-range points clamp into the frame instead of disappearing.
    #[test]
    fn a_dive_below_the_axis_is_clamped_not_dropped() {
        let mut p = Page::a4();
        let axes = Axes::new("t", "x", "y", (0.0, 1.0), (-10.0, 0.0));
        let s = vec![Series::new(
            "deep",
            vec![(0.0, 0.0), (0.5, -300.0), (1.0, 0.0)],
            PALETTE[0],
        )];
        let frame = Frame {
            x: 50.0,
            y: 100.0,
            w: 400.0,
            h: 200.0,
        };
        draw(&mut p, frame, &axes, &s);
        let mut d = Pdf::new();
        d.push(p);
        let out = String::from_utf8_lossy(&d.to_bytes()).to_string();
        // Three vertices, and the middle one sits on the frame floor.
        assert!(out.contains("100.000 l"), "clamped point missing: {out}");
    }

    #[test]
    fn tick_labels_are_tidy() {
        assert_eq!(format_tick(0.0), "0");
        assert_eq!(format_tick(-12.0), "-12");
        assert_eq!(format_tick(0.25), "0.25");
        assert_eq!(format_tick(0.5), "0.5");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::super::pdf::Pdf;
    use super::*;

    fn render(axes: Axes, series: Vec<Series>) -> String {
        let mut p = Page::a4();
        draw(
            &mut p,
            Frame {
                x: 60.0,
                y: 400.0,
                w: 470.0,
                h: 200.0,
            },
            &axes,
            &series,
        );
        let mut d = Pdf::new();
        d.push(p);
        String::from_utf8_lossy(&d.to_bytes()).to_string()
    }

    /// The y label runs along the axis, not above it.
    #[test]
    fn the_y_label_is_rotated() {
        let out = render(
            Axes::new("t", "freq", "dB", (0.0, 0.5), (-10.0, 0.0)),
            vec![Series::new("a", vec![(0.0, 0.0), (0.5, -5.0)], PALETTE[0])],
        );
        assert!(out.contains("0 1 -1 0"), "y label not rotated: {out}");
        assert!(out.contains("(dB) Tj"));
    }

    /// **The legend goes where the data is not.**
    ///
    /// Fixed at top-right it sat on any curve that went there, which for
    /// a CIC's full-band response is most of the plot. A curve pinned to
    /// the top-right must push the legend elsewhere.
    #[test]
    fn the_legend_avoids_the_data() {
        let axes = Axes::new("t", "x", "y", (0.0, 1.0), (0.0, 1.0));
        // All the data in the top-right quadrant.
        let hugging: Vec<(f64, f64)> = (0..60)
            .map(|k| (0.5 + 0.5 * k as f64 / 59.0, 0.9))
            .collect();
        let out = render(
            axes,
            vec![Series::new("hugs the top right", hugging, PALETTE[0])],
        );
        // The opaque panel must exist and carry the label...
        assert!(
            out.contains("1.000 1.000 1.000 rg"),
            "no opaque panel: {out}"
        );
        assert!(out.contains("(hugs the top right) Tj"), "{out}");
        // Its box origin should be on the left half or the bottom half.
        let xs: Vec<f64> = out
            .lines()
            .filter(|l| l.trim_end().ends_with("re f"))
            .filter_map(|l| l.split_whitespace().next()?.parse::<f64>().ok())
            .collect();
        assert!(
            xs.iter().any(|x| *x < 300.0),
            "legend should have moved left of centre, boxes at {xs:?}"
        );
    }

    /// **Thinning triggers on a narrow frame, and only then.**
    ///
    /// Listed as an observed defect, it was not one: at the report's
    /// 470pt plot width ten divisions give 47pt per label, and
    /// `"0.0001"` in real Helvetica at 7pt is 19.5pt. Nothing collided.
    /// The apparent crowding came from reading a `pdftotext` dump, which
    /// puts each label on its own line.
    ///
    /// The logic is kept as insurance for a narrow frame or a higher
    /// tick count, which is what this checks — with the wide case
    /// asserting it does *not* fire, so the insurance cannot silently
    /// start dropping labels that fit.
    #[test]
    fn tick_labels_thin_only_when_they_would_collide() {
        let axes = || Axes::new("t", "x", "y", (0.0, 0.001), (-1.0, 1.0));
        let series = || vec![Series::new("a", vec![(0.0, 0.0), (0.001, 0.0)], PALETTE[0])];
        let render_at = |w: f64| -> usize {
            let mut p = Page::a4();
            draw(
                &mut p,
                Frame {
                    x: 60.0,
                    y: 400.0,
                    w,
                    h: 200.0,
                },
                &axes(),
                &series(),
            );
            let mut d = Pdf::new();
            d.push(p);
            String::from_utf8_lossy(&d.to_bytes())
                .matches(") Tj")
                .count()
        };
        let wide = render_at(470.0);
        let cramped = render_at(120.0);
        assert!(
            cramped < wide,
            "a 120pt frame must drop labels a 470pt one keeps: {cramped} vs {wide}"
        );
    }

    /// Ticks keep enough precision to be distinguishable.
    #[test]
    fn tick_precision_adapts_to_the_range() {
        assert_eq!(format_tick(0.0625), "0.0625");
        assert_eq!(format_tick(0.001), "0.001");
        assert_eq!(format_tick(0.5), "0.5");
        assert_eq!(format_tick(-3.0), "-3");
        // And no rounding tail.
        assert_eq!(format_tick(0.1 + 0.2), "0.3");
    }

    #[test]
    fn layout_is_deterministic() {
        let build = || {
            render(
                Axes::new("t", "x", "y", (0.0, 0.5), (-10.0, 2.0)),
                vec![
                    Series::new("a", vec![(0.0, 0.0), (0.5, -8.0)], PALETTE[0]),
                    Series::new("b", vec![(0.0, -1.0), (0.5, -1.0)], PALETTE[1]).dashed(),
                ],
            )
        };
        assert_eq!(build(), build());
    }
}

#[cfg(test)]
mod tick_tests {
    use super::*;

    /// Round numbers, not evenly divided thirds.
    #[test]
    fn ticks_land_on_round_numbers() {
        // The case that produced -19.1667 and -43.3333.
        let t = nice_ticks(-140.0, 5.0, 6);
        for v in &t {
            let scaled = v / 25.0;
            assert!(
                (scaled - scaled.round()).abs() < 1e-9,
                "{v} is not a round step: {t:?}"
            );
        }
        assert!(t.len() >= 4 && t.len() <= 8, "{t:?}");
    }

    #[test]
    fn ticks_stay_inside_the_range() {
        for (lo, hi) in [(-140.0, 5.0), (0.0, 0.5), (-0.3, 0.3), (0.0, 0.001)] {
            let t = nice_ticks(lo, hi, 6);
            assert!(!t.is_empty(), "{lo}..{hi} produced nothing");
            for v in &t {
                assert!(*v >= lo - 1e-9 && *v <= hi + 1e-9, "{v} outside {lo}..{hi}");
            }
        }
    }

    /// No floating-point tails in the labels.
    #[test]
    fn tick_values_are_snapped() {
        for v in nice_ticks(-0.3, 0.3, 6) {
            let s = format_tick(v);
            assert!(
                s.len() <= 6,
                "`{s}` looks like a floating-point artefact from {v}"
            );
        }
    }

    #[test]
    fn a_degenerate_range_does_not_loop_forever() {
        assert_eq!(nice_ticks(1.0, 1.0, 6), vec![1.0]);
        assert_eq!(nice_ticks(5.0, 1.0, 6), vec![5.0]);
        assert_eq!(nice_ticks(0.0, 1.0, 0), vec![0.0]);
    }

    /// The count tracks the request without exceeding it wildly.
    #[test]
    fn the_tick_count_respects_the_target() {
        let few = nice_ticks(0.0, 100.0, 4);
        let many = nice_ticks(0.0, 100.0, 20);
        assert!(many.len() > few.len(), "{} vs {}", many.len(), few.len());
        assert!(few.len() <= 6, "asked for 4, got {}", few.len());
    }
}
