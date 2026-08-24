#![warn(missing_docs)]
//! A minimal, deterministic PDF writer.
//!
//! Enough of PDF 1.4 to draw a technical report: stroked paths, filled
//! rectangles, and text in the two built-in Helvetica faces. Nothing
//! more, and deliberately nothing more.
//!
//! # Why hand-rolled rather than a crate
//!
//! Two reasons, and the second is the binding one.
//!
//! The dependency cost is real: rendering SVG to PDF pulls in a font
//! database, a TrueType parser and a rasteriser, none of which this
//! library otherwise needs, to draw a few hundred straight lines.
//!
//! **But the decisive reason is determinism.** Committed artifacts in
//! this repository must regenerate byte-identically or `git status`
//! stops meaning anything — see CLAUDE.md §8. Most PDF writers stamp
//! `/CreationDate` and a producer string into every file, so the same
//! input yields a different file every run. This writer emits no
//! timestamp and no identifiers, so the same report is the same bytes
//! forever. That property is worth more here than a general-purpose
//! renderer.
//!
//! # Coordinates
//!
//! PDF's native convention: origin bottom-left, `y` increasing upward,
//! units of 1/72 inch. A4 is 595 x 842. No transform is applied, so
//! what you pass is what PDF sees — one less thing to get inverted.

use std::fmt::Write as _;

/// Which built-in face to draw text in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// Helvetica.
    Regular,
    /// Helvetica-Bold.
    Bold,
}

impl Font {
    fn resource(self) -> &'static str {
        match self {
            Font::Regular => "/F1",
            Font::Bold => "/F2",
        }
    }

    /// Approximate advance width of `text` at `size`, in points.
    ///
    /// Averaged rather than metric-accurate: this is used to centre and
    /// right-align labels, where being a few percent out is invisible,
    /// and carrying the full Helvetica width table would be more code
    /// than the rest of this module.
    pub fn width_of(self, text: &str, size: f64) -> f64 {
        let per = match self {
            Font::Regular => 0.52,
            Font::Bold => 0.56,
        };
        text.chars().count() as f64 * size * per
    }
}

/// Horizontal alignment for [`Page::text`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// `x` is the left edge.
    Left,
    /// `x` is the centre.
    Centre,
    /// `x` is the right edge.
    Right,
}

/// One page of content.
#[derive(Clone, Debug)]
pub struct Page {
    width: f64,
    height: f64,
    ops: String,
}

impl Page {
    /// A blank A4 portrait page.
    pub fn a4() -> Self {
        Self::new(595.0, 842.0)
    }

    /// A blank page of the given size in points.
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            width,
            height,
            ops: String::new(),
        }
    }

    /// Page width in points.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// Page height in points.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// Set stroke colour and line width for subsequent strokes.
    pub fn stroke_style(&mut self, rgb: (f64, f64, f64), width: f64) -> &mut Self {
        let _ = writeln!(
            self.ops,
            "{:.3} {:.3} {:.3} RG {:.3} w",
            rgb.0, rgb.1, rgb.2, width
        );
        self
    }

    /// Set fill colour for subsequent fills and text.
    pub fn fill_style(&mut self, rgb: (f64, f64, f64)) -> &mut Self {
        let _ = writeln!(self.ops, "{:.3} {:.3} {:.3} rg", rgb.0, rgb.1, rgb.2);
        self
    }

    /// Stroke a straight line.
    pub fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) -> &mut Self {
        let _ = writeln!(self.ops, "{x0:.3} {y0:.3} m {x1:.3} {y1:.3} l S");
        self
    }

    /// Stroke a connected path through `pts`.
    ///
    /// Fewer than two points draws nothing, which is the useful
    /// behaviour for a plot series that happened to be filtered empty.
    pub fn polyline(&mut self, pts: &[(f64, f64)]) -> &mut Self {
        if pts.len() < 2 {
            return self;
        }
        let _ = writeln!(self.ops, "{:.3} {:.3} m", pts[0].0, pts[0].1);
        for p in &pts[1..] {
            let _ = writeln!(self.ops, "{:.3} {:.3} l", p.0, p.1);
        }
        let _ = writeln!(self.ops, "S");
        self
    }

    /// Stroke a dashed connected path.
    pub fn polyline_dashed(&mut self, pts: &[(f64, f64)], on: f64, off: f64) -> &mut Self {
        let _ = writeln!(self.ops, "[{on:.2} {off:.2}] 0 d");
        self.polyline(pts);
        let _ = writeln!(self.ops, "[] 0 d");
        self
    }

    /// Stroke a rectangle outline.
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        let _ = writeln!(self.ops, "{x:.3} {y:.3} {w:.3} {h:.3} re S");
        self
    }

    /// Fill a rectangle with the current fill colour.
    pub fn rect_filled(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        let _ = writeln!(self.ops, "{x:.3} {y:.3} {w:.3} {h:.3} re f");
        self
    }

    /// Draw text. `y` is the baseline.
    pub fn text(
        &mut self,
        x: f64,
        y: f64,
        size: f64,
        font: Font,
        align: Align,
        text: &str,
    ) -> &mut Self {
        let w = font.width_of(text, size);
        let x = match align {
            Align::Left => x,
            Align::Centre => x - w / 2.0,
            Align::Right => x - w,
        };
        let _ = writeln!(
            self.ops,
            "BT {} {size:.2} Tf {x:.3} {y:.3} Td ({}) Tj ET",
            font.resource(),
            escape(text)
        );
        self
    }
}

/// Escape a string for a PDF literal string.
///
/// Also drops anything outside printable ASCII rather than emitting it
/// raw: without a font encoding for it the glyph would be wrong
/// anyway, and a stray byte can desynchronise a parser. `-` stands in
/// for the minus sign so that "-3 dB" survives a copy-paste from a
/// document that used U+2212.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            '\u{2212}' | '\u{2013}' | '\u{2014}' => out.push('-'),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            _ => {}
        }
    }
    out
}

/// A document: a sequence of pages.
#[derive(Clone, Debug, Default)]
pub struct Pdf {
    pages: Vec<Page>,
}

impl Pdf {
    /// An empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a page.
    pub fn push(&mut self, page: Page) {
        self.pages.push(page);
    }

    /// How many pages.
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// Is the document empty?
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Serialise to PDF bytes.
    ///
    /// Object numbering: 1 catalog, 2 page tree, 3 and 4 the two fonts,
    /// then two objects per page (the page and its content stream).
    /// Deterministic: no dates, no producer string, no identifiers.
    pub fn to_bytes(&self) -> Vec<u8> {
        let n_pages = self.pages.len().max(1);
        let mut out: Vec<u8> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();

        out.extend_from_slice(b"%PDF-1.4\n");

        let page_obj = |i: usize| 5 + 2 * i;
        let content_obj = |i: usize| 6 + 2 * i;

        let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: String| {
            offsets.push(out.len());
            out.extend_from_slice(body.as_bytes());
        };

        // 1: catalog
        obj(
            &mut out,
            &mut offsets,
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
        );
        // 2: page tree
        let kids: Vec<String> = (0..n_pages)
            .map(|i| format!("{} 0 R", page_obj(i)))
            .collect();
        obj(
            &mut out,
            &mut offsets,
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
                kids.join(" "),
                n_pages
            ),
        );
        // 3, 4: fonts
        obj(
            &mut out,
            &mut offsets,
            "3 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n".to_string(),
        );
        obj(
            &mut out,
            &mut offsets,
            "4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>\nendobj\n".to_string(),
        );

        // A document with no pages is still a valid document; give it
        // one blank page rather than emitting a broken page tree.
        let blank = Page::a4();
        for i in 0..n_pages {
            let p = self.pages.get(i).unwrap_or(&blank);
            obj(
                &mut out,
                &mut offsets,
                format!(
                    "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] \
                     /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>\nendobj\n",
                    page_obj(i),
                    p.width,
                    p.height,
                    content_obj(i)
                ),
            );
            obj(
                &mut out,
                &mut offsets,
                format!(
                    "{} 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
                    content_obj(i),
                    p.ops.len(),
                    p.ops
                ),
            );
        }

        // Cross-reference table.
        let xref_at = out.len();
        let count = offsets.len() + 1; // +1 for the free object 0
        let mut xref = format!("xref\n0 {count}\n0000000000 65535 f \n");
        for off in &offsets {
            let _ = writeln!(xref, "{off:010} 00000 n ");
        }
        let _ = write!(
            xref,
            "trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
        );
        out.extend_from_slice(xref.as_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Pdf {
        let mut p = Page::a4();
        p.stroke_style((0.0, 0.0, 0.0), 1.0)
            .line(50.0, 50.0, 500.0, 700.0)
            .polyline(&[(10.0, 10.0), (20.0, 30.0), (40.0, 25.0)])
            .fill_style((0.2, 0.4, 0.9))
            .rect_filled(100.0, 100.0, 50.0, 20.0)
            .text(300.0, 800.0, 14.0, Font::Bold, Align::Centre, "Title (x)");
        let mut d = Pdf::new();
        d.push(p);
        d
    }

    #[test]
    fn it_looks_like_a_pdf() {
        let b = sample().to_bytes();
        assert!(b.starts_with(b"%PDF-1.4"), "missing header");
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("/Type /Catalog"));
        assert!(s.contains("/Type /Pages"));
        assert!(s.contains("/Type /Page "));
        assert!(s.trim_end().ends_with("%%EOF"));
    }

    /// The xref offsets must actually point at their objects.
    ///
    /// This is the one thing in a PDF that a reader will reject outright
    /// and that no amount of visual inspection catches.
    #[test]
    fn the_xref_offsets_are_correct() {
        let b = sample().to_bytes();
        let s = String::from_utf8_lossy(&b);
        let xref_start = s.find("xref\n").expect("no xref");
        let body = &s[xref_start..];
        let mut lines = body.lines();
        assert_eq!(lines.next().unwrap(), "xref");
        let header = lines.next().unwrap();
        let count: usize = header.split_whitespace().nth(1).unwrap().parse().unwrap();
        // Skip the free entry, then check each offset lands on "N 0 obj".
        let _free = lines.next().unwrap();
        for i in 1..count {
            let entry = lines.next().unwrap();
            let off: usize = entry.split_whitespace().next().unwrap().parse().unwrap();
            let at = &s[off..];
            assert!(
                at.starts_with(&format!("{i} 0 obj")),
                "xref entry {i} points at {:?}, not an object header",
                &at[..at.len().min(20)]
            );
        }
        // And startxref must point at the xref table.
        let sx = s.rfind("startxref\n").unwrap();
        let val: usize = s[sx + 10..].lines().next().unwrap().trim().parse().unwrap();
        assert_eq!(val, xref_start, "startxref does not point at xref");
    }

    #[test]
    fn the_declared_stream_length_matches() {
        let b = sample().to_bytes();
        let s = String::from_utf8_lossy(&b);
        let i = s.find("/Length ").unwrap();
        let declared: usize = s[i + 8..]
            .split(' ')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let start = s.find("stream\n").unwrap() + 7;
        let end = s.find("endstream").unwrap();
        assert_eq!(declared, end - start, "/Length disagrees with the stream");
    }

    /// The property the whole hand-rolled writer exists for.
    #[test]
    fn the_same_document_is_the_same_bytes() {
        assert_eq!(sample().to_bytes(), sample().to_bytes());
    }

    #[test]
    fn text_is_escaped_and_sanitised() {
        let mut p = Page::a4();
        p.text(
            0.0,
            0.0,
            10.0,
            Font::Regular,
            Align::Left,
            "a(b)c\\d\u{2212}3 \u{4e2d}",
        );
        let mut d = Pdf::new();
        d.push(p);
        let s = String::from_utf8_lossy(&d.to_bytes()).to_string();
        assert!(s.contains("(a\\(b\\)c\\\\d-3 ) Tj"), "got: {s}");
    }

    #[test]
    fn an_empty_document_still_has_a_page() {
        let s = String::from_utf8_lossy(&Pdf::new().to_bytes()).to_string();
        assert!(s.contains("/Count 1"));
        assert!(s.contains("/Type /Page "));
    }

    #[test]
    fn multiple_pages_are_numbered_consistently() {
        let mut d = Pdf::new();
        d.push(Page::a4());
        d.push(Page::a4());
        d.push(Page::new(842.0, 595.0));
        assert_eq!(d.len(), 3);
        let s = String::from_utf8_lossy(&d.to_bytes()).to_string();
        assert!(s.contains("/Count 3"));
        assert!(
            s.contains("/Kids [5 0 R 7 0 R 9 0 R]"),
            "got kids wrong: {s}"
        );
        // The landscape page must keep its own MediaBox.
        assert!(s.contains("/MediaBox [0 0 842.00 595.00]"));
    }
}
