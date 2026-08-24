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

    // Gridlines and ticks.
    page.stroke_style(GREY, 0.4);
    for i in 0..=axes.x_ticks {
        let v = x0 + (x1 - x0) * i as f64 / axes.x_ticks as f64;
        page.line(sx(v), frame.y, sx(v), frame.y + frame.h);
    }
    for i in 0..=axes.y_ticks {
        let v = y0 + (y1 - y0) * i as f64 / axes.y_ticks as f64;
        page.line(frame.x, sy(v), frame.x + frame.w, sy(v));
    }

    // Frame.
    page.stroke_style(BLACK, 0.8);
    page.rect(frame.x, frame.y, frame.w, frame.h);

    // Tick labels.
    page.fill_style(BLACK);
    for i in 0..=axes.x_ticks {
        let v = x0 + (x1 - x0) * i as f64 / axes.x_ticks as f64;
        page.text(
            sx(v),
            frame.y - 12.0,
            7.0,
            Font::Regular,
            Align::Centre,
            &format_tick(v),
        );
    }
    for i in 0..=axes.y_ticks {
        let v = y0 + (y1 - y0) * i as f64 / axes.y_ticks as f64;
        page.text(
            frame.x - 6.0,
            sy(v) - 2.5,
            7.0,
            Font::Regular,
            Align::Right,
            &format_tick(v),
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
    // No text rotation, so the y label sits above the axis rather than
    // along it. Less pretty; one fewer PDF operator to get wrong.
    page.text(
        frame.x,
        frame.y + frame.h + 10.0,
        8.0,
        Font::Regular,
        Align::Left,
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

    // Legend, top-right inside the frame.
    let mut ly = frame.y + frame.h - 12.0;
    for s in series {
        let lx = frame.x + frame.w - 90.0;
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
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
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
