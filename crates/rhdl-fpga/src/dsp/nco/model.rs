#![warn(missing_docs)]
//! Bit-accurate DDS model, and the spur analysis that sizes the table.
//!
//! This is **not** a widget. It is a plain-Rust model of the
//! phase-to-amplitude datapath, used for two things:
//!
//! 1. **Choosing the architecture.** Phase-truncation spurs move with
//!    the tuning word, so the table width cannot be picked from a
//!    formula alone — it has to be swept. See
//!    [`worst_in_band_sfdr_over_sweep`].
//! 2. **Regression reference.** Once RTL exists, its output should be
//!    compared sample-for-sample against [`DdsModel`].
//!
//! Building the RTL first and characterising afterwards risks
//! discovering the wrong architecture was chosen, which is why this
//! exists before any phase-to-amplitude widget does.
//!
//! # The datapath modelled
//!
//! ```text
//! phase accumulator (PHASE_W)  ─►  truncate to ADDR_W  ─►  quarter-wave
//!                                                          LUT (AMP_W)
//!                                                       ─►  sin, cos
//! ```
//!
//! Quarter-wave symmetry means one table serves both components:
//! `cos(θ) = sin(θ + π/2)`, and π/2 is `2^(ADDR_W-2)` in phase units.
//! The table holds `2^(ADDR_W-2)` entries sampled at bin *midpoints*,
//! which is what makes the quadrant mirror exact rather than
//! off-by-one.

use std::f64::consts::PI;

/// A bit-accurate model of the DDS phase-to-amplitude datapath.
#[derive(Clone, Debug)]
pub struct DdsModel {
    phase_w: u32,
    addr_w: u32,
    amp_w: u32,
    /// Quarter-wave table, `2^(addr_w-2)` entries, midpoint-sampled.
    table: Vec<i32>,
    acc: u64,
}

impl DdsModel {
    /// Build a model.
    ///
    /// `phase_w` is the accumulator width (frequency resolution),
    /// `addr_w` the truncated width addressing the table (spur
    /// performance), `amp_w` the bits per output component.
    ///
    /// # Panics
    /// If `addr_w < 3`, `addr_w > phase_w`, or `phase_w > 63`.
    pub fn new(phase_w: u32, addr_w: u32, amp_w: u32) -> Self {
        assert!(addr_w >= 3, "need at least one quadrant bit plus an index");
        assert!(
            addr_w <= phase_w,
            "cannot address more bits than the accumulator has"
        );
        assert!(phase_w <= 63, "accumulator modelled in u64");
        let quarter = 1usize << (addr_w - 2);
        let scale = ((1i64 << (amp_w - 1)) - 1) as f64;
        let table = (0..quarter)
            .map(|i| {
                // Midpoint sampling: θ = 2π(i + ½)/2^addr_w.
                let theta = 2.0 * PI * (i as f64 + 0.5) / (1u64 << addr_w) as f64;
                (theta.sin() * scale).round() as i32
            })
            .collect();
        Self {
            phase_w,
            addr_w,
            amp_w,
            table,
            acc: 0,
        }
    }

    /// Table size in bits — what the resource budget is spent on.
    pub fn table_bits(&self) -> usize {
        self.table.len() * self.amp_w as usize
    }

    /// Sine of a truncated phase word, via quarter-wave reconstruction.
    fn sin_of(&self, phase_trunc: u64) -> i32 {
        let n = self.table.len() as u64;
        let quadrant = phase_trunc >> (self.addr_w - 2);
        let idx = phase_trunc & (n - 1);
        // Even quadrants read forward, odd quadrants mirror.
        let mag = if quadrant & 1 == 0 {
            self.table[idx as usize]
        } else {
            self.table[(n - 1 - idx) as usize]
        };
        // Upper half-cycle is negative.
        if quadrant >= 2 {
            -mag
        } else {
            mag
        }
    }

    /// Advance one sample, returning `(sin, cos)`.
    pub fn step(&mut self, frequency_word: u64) -> (i32, i32) {
        let phase_mask = if self.phase_w == 64 {
            u64::MAX
        } else {
            (1u64 << self.phase_w) - 1
        };
        let phase = self.acc;
        self.acc = self.acc.wrapping_add(frequency_word) & phase_mask;

        let trunc = phase >> (self.phase_w - self.addr_w);
        let addr_mask = (1u64 << self.addr_w) - 1;
        let quarter_turn = 1u64 << (self.addr_w - 2);
        (
            self.sin_of(trunc),
            self.sin_of((trunc + quarter_turn) & addr_mask),
        )
    }

    /// Produce `n` sine samples for a tuning word, from a zeroed phase.
    pub fn run_sin(&mut self, frequency_word: u64, n: usize) -> Vec<i32> {
        self.acc = 0;
        (0..n).map(|_| self.step(frequency_word).0).collect()
    }
}

/// Classic phase-truncation SFDR estimate, `6.02·P − 3.92` dB.
///
/// A **worst-case, full-Nyquist** figure. It is a screen for obviously
/// undersized tables, not a validation — the in-band figure depends on
/// where spurs land, which depends on the tuning word.
pub fn sfdr_estimate_db(addr_w: u32) -> f64 {
    6.02 * addr_w as f64 - 3.92
}

/// What a spur analysis found.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SpurReport {
    /// Worst spur relative to the carrier, in dB (positive = SFDR).
    pub sfdr_db: f64,
    /// Frequency of the worst spur, Hz.
    pub worst_spur_hz: f64,
    /// Carrier frequency as located in the spectrum, Hz.
    pub carrier_hz: f64,
}

/// In-place iterative radix-2 FFT.
pub(crate) fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(
        n.is_power_of_two(),
        "radix-2 FFT needs a power-of-two length"
    );
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr, vi) = (
                    re[i + k + len / 2] * cr - im[i + k + len / 2] * ci,
                    re[i + k + len / 2] * ci + im[i + k + len / 2] * cr,
                );
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// 7-term Blackman-Harris window (~−180 dB sidelobes).
///
/// The window choice is not cosmetic — it sets the **noise floor of the
/// measurement itself**. A 4-term Blackman-Harris has −92 dB sidelobes,
/// and an earlier version of this analysis used one: every
/// configuration then reported ~92 dB SFDR regardless of table width,
/// because the analysis was measuring its own leakage rather than the
/// DDS's spurs. The tell was that a table four bits wider produced an
/// identical figure.
///
/// At −180 dB the window is far below any spur worth finding, so the
/// measurement floor is set by the device under test.
pub(crate) fn blackman_harris(n: usize) -> Vec<f64> {
    const A: [f64; 7] = [
        0.271_051_400_693_42,
        0.433_297_939_234_48,
        0.218_122_999_543_11,
        0.065_925_446_388_03,
        0.010_811_742_098_37,
        0.000_776_584_825_22,
        0.000_013_887_217_35,
    ];
    (0..n)
        .map(|i| {
            let x = 2.0 * PI * i as f64 / n as f64;
            let mut w = A[0];
            for (k, a) in A.iter().enumerate().skip(1) {
                let term = a * (k as f64 * x).cos();
                w += if k % 2 == 1 { -term } else { term };
            }
            w
        })
        .collect()
}

/// Find the worst spur within `[band_lo, band_hi]`, excluding the
/// carrier and its main lobe.
///
/// `band_lo`/`band_hi` are absolute frequencies in Hz. Spurs outside
/// that window are ignored — they are removed by the decimation filter
/// downstream, which is why the in-band figure is the one that matters.
pub fn analyze(samples: &[i32], f_clk: f64, band_lo: f64, band_hi: f64) -> SpurReport {
    let n = samples.len();
    let win = blackman_harris(n);
    let mut re: Vec<f64> = samples
        .iter()
        .zip(&win)
        .map(|(s, w)| *s as f64 * w)
        .collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);

    let half = n / 2;
    let mag: Vec<f64> = (0..half)
        .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
        .collect();
    let bin_hz = f_clk / n as f64;

    // Carrier = largest bin, ignoring DC and its immediate neighbours.
    let carrier = (3..half)
        .max_by(|a, b| mag[*a].total_cmp(&mag[*b]))
        .unwrap();
    // The 7-term window has a ~7-bin main lobe; exclude it with margin
    // so the carrier's own skirt is never mistaken for a spur.
    const LOBE: usize = 12;

    let (mut worst, mut worst_bin) = (0.0f64, carrier);
    for (k, m) in mag.iter().enumerate().take(half).skip(3) {
        if k.abs_diff(carrier) <= LOBE {
            continue;
        }
        let f = k as f64 * bin_hz;
        if f < band_lo || f > band_hi {
            continue;
        }
        if *m > worst {
            worst = *m;
            worst_bin = k;
        }
    }

    let sfdr_db = if worst > 0.0 {
        20.0 * (mag[carrier] / worst).log10()
    } else {
        f64::INFINITY
    };
    SpurReport {
        sfdr_db,
        worst_spur_hz: worst_bin as f64 * bin_hz,
        carrier_hz: carrier as f64 * bin_hz,
    }
}

/// Candidate tuning words for a sizing sweep, chosen adversarially.
///
/// Uniform random sampling of a 48-bit space is close to useless here.
/// Phase-truncation spurs are governed by the *truncated remainder* —
/// the `phase_w - addr_w` low-order bits the table never sees — and the
/// error sequence's period is `2^B / gcd(low, 2^B)`. Short periods
/// concentrate the error into a few strong spurs; long periods spread
/// it into a noise-like floor.
///
/// So the worst cases live at structured values of `low`, and a random
/// sweep will nearly always land in the benign regime and report
/// reassuring numbers.
///
/// This enumerates, for a fixed carrier (the high bits are held so the
/// output stays inside the analysis band):
///
/// - `low = 0` — exactly aligned, no truncation error at all
/// - `low = 2^k` — the shortest periods, and the classic worst cases
/// - small odd values, and `2^(B-1)` neighbourhoods
/// - a spread of pseudo-random values for the benign regime
pub fn adversarial_words(base: u64, phase_w: u32, addr_w: u32, extra_random: usize) -> Vec<u64> {
    let b = phase_w - addr_w;
    let high = (base >> b) << b;
    let span = 1u64 << b;
    let mut lows: Vec<u64> = Vec::new();

    lows.push(0);
    for k in 0..b {
        lows.push(1u64 << k);
        lows.push(span - (1u64 << k));
    }
    for odd in [1u64, 3, 5, 7, 9, 11, 13, 15, 17, 21, 31, 33, 63, 65] {
        lows.push(odd);
        lows.push(span.wrapping_sub(odd));
        lows.push((span / 2).wrapping_add(odd));
        lows.push((span / 2).wrapping_sub(odd));
    }
    for d in 1..=8u64 {
        lows.push(span / d);
        lows.push(span / d + 1);
    }
    // Benign regime, for contrast: a deterministic xorshift spread.
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..extra_random {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        lows.push(x & (span - 1));
    }

    lows.sort_unstable();
    lows.dedup();
    lows.into_iter().map(|l| (high | l).max(1)).collect()
}

/// One entry of a sizing sweep.
#[derive(Copy, Clone, Debug)]
pub struct SweepEntry {
    /// The tuning word tested.
    pub word: u64,
    /// In-band SFDR measured for it, dB.
    pub sfdr_db: f64,
    /// Where the worst in-band spur landed, Hz.
    pub worst_spur_hz: f64,
}

/// Sweep tuning words and return every result, worst first.
///
/// Returning the whole distribution rather than only the minimum is
/// deliberate: a single worst-case number hides whether the design sits
/// comfortably above target or is one unlucky tuning word away from it.
///
/// `carrier_hz` sets the nominal output; the analysis band is
/// `carrier ± band_hz/2`. Only the low-order bits are varied, so every
/// candidate stays inside that band.
pub fn sizing_sweep(
    phase_w: u32,
    addr_w: u32,
    amp_w: u32,
    f_clk: f64,
    carrier_hz: f64,
    band_hz: f64,
    record: usize,
    extra_random: usize,
) -> Vec<SweepEntry> {
    let base = ((carrier_hz / f_clk) * (1u64 << phase_w) as f64).round() as u64;
    let (band_lo, band_hi) = (carrier_hz - band_hz / 2.0, carrier_hz + band_hz / 2.0);
    let mut model = DdsModel::new(phase_w, addr_w, amp_w);

    let mut out: Vec<SweepEntry> = adversarial_words(base, phase_w, addr_w, extra_random)
        .into_iter()
        .map(|word| {
            let samples = model.run_sin(word, record);
            let rep = analyze(&samples, f_clk, band_lo, band_hi);
            SweepEntry {
                word,
                sfdr_db: rep.sfdr_db,
                worst_spur_hz: rep.worst_spur_hz,
            }
        })
        .collect();
    out.sort_by(|a, b| a.sfdr_db.total_cmp(&b.sfdr_db));
    out
}

/// Worst in-band SFDR over an adversarial sweep.
pub fn worst_in_band_sfdr_over_sweep(
    phase_w: u32,
    addr_w: u32,
    amp_w: u32,
    f_clk: f64,
    carrier_hz: f64,
    band_hz: f64,
    record: usize,
    extra_random: usize,
) -> (f64, u64) {
    let sweep = sizing_sweep(
        phase_w,
        addr_w,
        amp_w,
        f_clk,
        carrier_hz,
        band_hz,
        record,
        extra_random,
    );
    let w = sweep.first().expect("sweep produced no candidates");
    (w.sfdr_db, w.word)
}

/// Exact phase-truncation spur spectrum — no window, no leakage, no
/// blind zone.
///
/// # Why this exists
///
/// A windowed FFT of the output cannot see spurs closer to the carrier
/// than its exclusion zone, and that zone is exactly where the spurs of
/// long-period tuning words live. An earlier version of this module
/// reported such words as clean because their spurs sat under the
/// carrier's skirt.
///
/// This computes the spur spectrum analytically instead, and is exact.
///
/// # The derivation
///
/// With accumulator width `Wp`, table address width `P`, and
/// `B = Wp - P` truncated bits, the discarded remainder is
///
/// ```text
/// r[n] = (n · word) mod 2^B
/// ```
///
/// The phase actually used is short of the ideal by `r[n] / 2^Wp`
/// cycles, so the phase error in radians is
///
/// ```text
/// a[n] = 2π · r[n] / 2^Wp        peak-to-peak 2π/2^P — one truncated LSB
/// ```
///
/// and the output is `sin(ψ[n] − a[n]) ≈ sin ψ[n] − a[n]·cos ψ[n]`. The
/// linearisation is excellent: at `P = 13` the error never exceeds
/// 8 × 10⁻⁴ rad.
///
/// The spurs are therefore the spectrum of `a[n]` translated onto the
/// carrier. `a[n]` is **exactly periodic** with period
/// `T = 2^(B − v)`, where `v` is the number of trailing zeros in
/// `word` — so a DFT over exactly one period is exact, needs no window,
/// and resolves every spur however close to the carrier it lies.
///
/// Spur `k` sits at offset `k · f_clk / T` from the carrier, at
/// `20·log10|A_k|` dBc.
///
/// # Cost
///
/// `T` is `2^(B−v)`, which is small for the cases that matter and
/// enormous as `v → 0`. `max_log2_period` caps it; beyond the cap the
/// spurs are so densely spaced and individually weak that the worst is
/// bounded by the `k = 1` coefficient, which tends to `1/2^P` — i.e.
/// `6.02·P` dBc — at an offset tending to zero. Returns `None` there,
/// and the caller should use that bound.
pub fn exact_spur_spectrum(
    phase_w: u32,
    addr_w: u32,
    word: u64,
    max_log2_period: u32,
) -> Option<Vec<SpurLine>> {
    let b = phase_w - addr_w;
    let v = word.trailing_zeros().min(b);
    let log2_t = b - v;
    if log2_t > max_log2_period {
        return None;
    }
    let t = 1usize << log2_t;

    // One period of the discarded remainder, as phase error in radians.
    let mask = (1u64 << b) - 1;
    let scale = 2.0 * PI / (1u64 << phase_w) as f64;
    let mut re: Vec<f64> = (0..t)
        .map(|n| ((n as u64).wrapping_mul(word) & mask) as f64 * scale)
        .collect();
    let mut im = vec![0.0; t];
    fft(&mut re, &mut im);

    // A_k, normalised.  k = 0 is a static phase offset, not a spur.
    Some(
        (1..t / 2)
            .map(|k| {
                let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() / t as f64;
                SpurLine {
                    harmonic: k as u32,
                    dbc: if mag > 0.0 {
                        20.0 * mag.log10()
                    } else {
                        f64::NEG_INFINITY
                    },
                }
            })
            .collect(),
    )
}

/// Phase-to-amplitude architecture under evaluation.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PhaseToAmp {
    /// Plain lookup: truncate phase to `addr_w` and read the table.
    Lut { addr_w: u32 },
    /// Coarse table plus first-order rotation by the fine remainder —
    /// the §8.7 hybrid.
    ///
    /// ```text
    /// cos(θ+δ) ≈ cos θ − sin θ·δ
    /// sin(θ+δ) ≈ sin θ + cos θ·δ
    /// ```
    ///
    /// One dual-port table supplies `sin θ` and `cos θ`; two multipliers
    /// apply `δ`. The residual is second order in the coarse step, so
    /// accuracy improves far faster per coarse bit than enlarging the
    /// table does — and unlike dither it costs no noise floor, which
    /// matters when the instrument is sensitivity-limited.
    Hybrid { coarse_w: u32, fine_w: u32 },
    /// The hybrid with **finite-precision arithmetic**, as it will
    /// actually be built.
    ///
    /// [`PhaseToAmp::Hybrid`] performs the rotation in `f64`, so it
    /// reports the architecture's ceiling rather than an implementation.
    /// Real hardware quantises the table, the product and the sum, and
    /// each imposes its own floor. Sizing against the `f64` figure
    /// would repeat the mistake this module keeps catching: believing a
    /// model that flatters the design.
    ///
    /// - `amp_w` — bits per table entry
    /// - `prod_w` — bits retained after the `cos θ · δ` multiply
    /// - `out_w` — bits of the final sum (the DAC-facing width)
    HybridQ {
        coarse_w: u32,
        fine_w: u32,
        amp_w: u32,
        prod_w: u32,
        out_w: u32,
    },
    /// Coarse table plus **CORDIC micro-rotation** of the fine
    /// remainder — the other thing "hybrid" can mean.
    ///
    /// Starts the CORDIC at iteration `i0 = coarse_w - 2`, chosen so
    /// `atan(2^-i0)` just covers the largest fine remainder. Because
    /// the iterations all have large `i`, the CORDIC gain
    /// `∏ sqrt(1 + 2^-2i)` is within a few LSB of unity, so no gain
    /// compensation stage is needed — which is the main attraction of
    /// micro-rotation over a full CORDIC.
    ///
    /// Trades the linear variant's 2 multipliers for `2·stages` adders
    /// and shifts. Worth it only on parts where DSP slices are scarce
    /// or already committed.
    LutCordic {
        coarse_w: u32,
        fine_w: u32,
        stages: u32,
    },
}

impl PhaseToAmp {
    /// Bits of phase the architecture actually consumes.
    fn phase_bits_used(self) -> u32 {
        match self {
            PhaseToAmp::Lut { addr_w } => addr_w,
            PhaseToAmp::Hybrid { coarse_w, fine_w } => coarse_w + fine_w,
            PhaseToAmp::HybridQ {
                coarse_w, fine_w, ..
            } => coarse_w + fine_w,
            PhaseToAmp::LutCordic {
                coarse_w, fine_w, ..
            } => coarse_w + fine_w,
        }
    }

    /// Sine error for a full-precision phase, in units of full scale.
    ///
    /// Returns `actual − ideal`, computed in f64 so the analysis
    /// isolates *architectural* error from amplitude quantisation.
    fn sin_error(self, phase: u64, phase_w: u32) -> f64 {
        let full = (1u64 << phase_w) as f64;
        let ideal = (2.0 * PI * phase as f64 / full).sin();
        let actual = match self {
            PhaseToAmp::Lut { addr_w } => {
                let t = phase >> (phase_w - addr_w);
                // Midpoint-sampled table, as built by `DdsModel`.
                (2.0 * PI * (t as f64 + 0.5) / (1u64 << addr_w) as f64).sin()
            }
            PhaseToAmp::Hybrid { coarse_w, fine_w } => {
                let used = coarse_w + fine_w;
                let t = phase >> (phase_w - used);
                let coarse = t >> fine_w;
                let fine = t & ((1u64 << fine_w) - 1);
                // Coarse angle at bin midpoint, fine offset in radians.
                let theta = 2.0 * PI * (coarse as f64 + 0.5) / (1u64 << coarse_w) as f64;
                let step = 2.0 * PI / (1u64 << coarse_w) as f64;
                let delta = (fine as f64 / (1u64 << fine_w) as f64 - 0.5) * step;
                theta.sin() + theta.cos() * delta
            }
            PhaseToAmp::HybridQ {
                coarse_w,
                fine_w,
                amp_w,
                prod_w,
                out_w,
            } => {
                let used = coarse_w + fine_w;
                let t = phase >> (phase_w - used);
                let coarse = t >> fine_w;
                let fine = t & ((1u64 << fine_w) - 1);
                let theta = 2.0 * PI * (coarse as f64 + 0.5) / (1u64 << coarse_w) as f64;
                let step = 2.0 * PI / (1u64 << coarse_w) as f64;
                let delta = (fine as f64 / (1u64 << fine_w) as f64 - 0.5) * step;

                // Table entries: quantised to amp_w.
                let qa = ((1i64 << (amp_w - 1)) - 1) as f64;
                let sin_t = (theta.sin() * qa).round() / qa;
                let cos_t = (theta.cos() * qa).round() / qa;
                // Product: quantised to prod_w.
                let qp = ((1i64 << (prod_w - 1)) - 1) as f64;
                let prod = ((cos_t * delta) * qp).round() / qp;
                // Sum: quantised to the output width.
                let qo = ((1i64 << (out_w - 1)) - 1) as f64;
                ((sin_t + prod) * qo).round() / qo
            }
            PhaseToAmp::LutCordic {
                coarse_w,
                fine_w,
                stages,
            } => {
                let used = coarse_w + fine_w;
                let t = phase >> (phase_w - used);
                let coarse = t >> fine_w;
                let fine = t & ((1u64 << fine_w) - 1);
                let theta = 2.0 * PI * (coarse as f64 + 0.5) / (1u64 << coarse_w) as f64;
                let step = 2.0 * PI / (1u64 << coarse_w) as f64;
                let mut z = (fine as f64 / (1u64 << fine_w) as f64 - 0.5) * step;

                // Micro-rotation: start where atan(2^-i) just covers the
                // largest possible remainder.
                let i0 = coarse_w.saturating_sub(2);
                let (mut x, mut y) = (theta.cos(), theta.sin());
                let mut gain = 1.0f64;
                for i in i0..(i0 + stages) {
                    let p = 2f64.powi(-(i as i32));
                    let d = if z >= 0.0 { 1.0 } else { -1.0 };
                    let (nx, ny) = (x - d * y * p, y + d * x * p);
                    x = nx;
                    y = ny;
                    z -= d * p.atan();
                    gain *= (1.0 + p * p).sqrt();
                }
                y / gain
            }
        };
        actual - ideal
    }
}

/// Exact spur spectrum for any [`PhaseToAmp`] architecture.
///
/// Generalises [`exact_spur_spectrum`]: rather than assuming the error
/// is the truncation sawtooth, it evaluates the architecture's actual
/// error at every phase in one period and transforms that. Exact for
/// any architecture, with no window and no blind zone.
pub fn exact_spur_spectrum_for(
    arch: PhaseToAmp,
    phase_w: u32,
    word: u64,
    max_log2_period: u32,
) -> Option<Vec<SpurLine>> {
    exact_spur_spectrum_offset(arch, phase_w, word, 0, max_log2_period)
}

/// [`exact_spur_spectrum_for`] with a static phase offset applied.
///
/// A phase offset is added before the coarse/fine split, so it changes
/// which `(θ, δ)` pairs occur together and could in principle change
/// the spur spectrum. For an **odd** tuning word it cannot: `W` is
/// invertible mod `2^B`, so the offset is exactly a time shift and
/// magnitudes are preserved. For `v > 0` that argument fails, which is
/// why this exists rather than being assumed away.
pub fn exact_spur_spectrum_offset(
    arch: PhaseToAmp,
    phase_w: u32,
    word: u64,
    phase_offset: u64,
    max_log2_period: u32,
) -> Option<Vec<SpurLine>> {
    // NOTE the period.  The OUTPUT error carries carrier modulation, so
    // it repeats only over the full phase sequence, `2^(phase_w - v)` —
    // not over the truncated remainder's `2^(B - v)`.  An earlier
    // version used the latter and reported −164 dBc identically for
    // every table width, which is how the mistake announced itself:
    // a four-bit-wider table cannot give an identical answer.
    //
    // `exact_spur_spectrum` may legitimately use the shorter period
    // because it transforms the PHASE error, where the carrier
    // modulation is handled analytically as sideband translation.
    let v = word.trailing_zeros().min(phase_w);
    let log2_t = phase_w - v;
    if log2_t > max_log2_period {
        return None;
    }
    let t = 1usize << log2_t;

    let phase_mask = if phase_w >= 64 {
        u64::MAX
    } else {
        (1u64 << phase_w) - 1
    };
    let mut re: Vec<f64> = (0..t)
        .map(|n| {
            let phase = ((n as u64).wrapping_mul(word).wrapping_add(phase_offset)) & phase_mask;
            arch.sin_error(phase, phase_w)
        })
        .collect();
    let mut im = vec![0.0; t];
    fft(&mut re, &mut im);

    Some(
        (1..t / 2)
            .map(|k| {
                let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() / t as f64;
                SpurLine {
                    harmonic: k as u32,
                    // A real sequence splits a spur of amplitude A into
                    // two coefficients of A/2, so A = 2·mag.  The
                    // carrier has amplitude 1 in these units, so the
                    // ratio is 2·mag.  Writing mag/2 here understated
                    // every spur by 12 dB and was caught only by
                    // cross-checking against the phase-domain analyser.
                    dbc: if mag > 0.0 {
                        20.0 * (2.0 * mag).log10()
                    } else {
                        f64::NEG_INFINITY
                    },
                }
            })
            .collect(),
    )
}

/// Worst exact spur in band for any architecture.
pub fn worst_exact_spur_for(
    arch: PhaseToAmp,
    phase_w: u32,
    word: u64,
    f_clk: f64,
    band_hz: f64,
    max_log2_period: u32,
) -> Option<(f64, f64)> {
    let v = word.trailing_zeros().min(phase_w);
    let t = 1u64 << (phase_w - v);
    let f_fund = f_clk / t as f64;
    // The output-error spectrum is in ABSOLUTE frequency, so the band
    // test is against the carrier's absolute position — not against
    // zero.  Getting this wrong searches near DC and finds nothing,
    // reporting absurd figures like −400 dBc.
    let carrier_hz = word as f64 / (1u64 << phase_w) as f64 * f_clk;
    let lines = exact_spur_spectrum_for(arch, phase_w, word, max_log2_period)?;
    lines
        .iter()
        .map(|l| (l.dbc, l.harmonic as f64 * f_fund))
        .filter(|(_, f_abs)| (*f_abs - carrier_hz).abs() <= band_hz / 2.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(dbc, f_abs)| (dbc, f_abs - carrier_hz))
}

/// One spur line from [`exact_spur_spectrum`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SpurLine {
    /// Harmonic index of the error fundamental.
    pub harmonic: u32,
    /// Level relative to the carrier, dB (negative).
    pub dbc: f64,
}

/// Worst exact spur within a band around the carrier, and its offset.
///
/// Returns `(dbc, offset_hz)`. Spurs outside `± band_hz/2` are ignored:
/// the decimation filter downstream removes them.
pub fn worst_exact_spur_in_band(
    phase_w: u32,
    addr_w: u32,
    word: u64,
    f_clk: f64,
    band_hz: f64,
    max_log2_period: u32,
) -> Option<(f64, f64)> {
    let b = phase_w - addr_w;
    let v = word.trailing_zeros().min(b);
    let t = 1u64 << (b - v);
    let f_fundamental = f_clk / t as f64;

    let lines = exact_spur_spectrum(phase_w, addr_w, word, max_log2_period)?;
    let half_band = band_hz / 2.0;
    lines
        .iter()
        .map(|l| (l.dbc, l.harmonic as f64 * f_fundamental))
        .filter(|(_, off)| *off <= half_band)
        .max_by(|a, b| a.0.total_cmp(&b.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const F_CLK: f64 = 125.0e6;

    /// The quarter-wave reconstruction must produce a real sine, not
    /// something merely periodic. Compare against `f64::sin` within a
    /// couple of LSB.
    #[test]
    fn quarter_wave_reconstruction_is_a_sine() {
        let phase_w = 32;
        let addr_w = 12;
        let amp_w = 16;
        let model = DdsModel::new(phase_w, addr_w, amp_w);
        let scale = ((1i64 << (amp_w - 1)) - 1) as f64;

        for trunc in 0..(1u64 << addr_w) {
            let got = model.sin_of(trunc) as f64;
            let theta = 2.0 * PI * (trunc as f64 + 0.5) / (1u64 << addr_w) as f64;
            let want = theta.sin() * scale;
            assert!(
                (got - want).abs() <= 2.0,
                "phase {trunc}: got {got}, want {want}"
            );
        }
    }

    /// `cos` must lead `sin` by exactly a quarter turn — the property
    /// that lets one table serve complex modulation.
    #[test]
    fn cos_leads_sin_by_a_quarter_turn() {
        let mut a = DdsModel::new(32, 12, 16);
        let mut b = DdsModel::new(32, 12, 16);
        let word = 0x0123_4567u64;
        let quarter = 1u64 << (32 - 2);

        // Run `b` a quarter turn ahead of `a`.
        b.acc = quarter;
        for _ in 0..64 {
            let (_sin_a, cos_a) = a.step(word);
            let (sin_b, _cos_b) = b.step(word);
            assert_eq!(cos_a, sin_b, "cos(θ) must equal sin(θ + π/2)");
        }
    }

    /// **The model validates itself against theory.**
    ///
    /// Measured SFDR must track `6.02·P − 3.92` as the table grows. If
    /// it does not, the model — or the analysis — is wrong, and every
    /// number it produces afterwards is worthless.
    #[test]
    fn measured_sfdr_tracks_the_truncation_formula() {
        // Full Nyquist band, so this is the figure the formula predicts.
        let record = 1 << 14;
        // A tuning word with plenty of low-order structure, so
        // truncation actually bites.
        let word = 0x0ACE_1357u64;

        for addr_w in [8u32, 10, 12] {
            let mut model = DdsModel::new(32, addr_w, 16);
            let samples = model.run_sin(word, record);
            let rep = analyze(&samples, F_CLK, 0.0, F_CLK / 2.0);
            let predicted = sfdr_estimate_db(addr_w);
            assert!(
                (rep.sfdr_db - predicted).abs() < 12.0,
                "addr_w={addr_w}: measured {:.1} dB, formula predicts {predicted:.1} dB",
                rep.sfdr_db
            );
            // And it must improve with width, monotonically enough to
            // be useful as a sizing tool.
            assert!(
                rep.sfdr_db > sfdr_estimate_db(addr_w - 2) - 12.0,
                "addr_w={addr_w} should beat a table two bits smaller"
            );
        }
    }

    /// Wider amplitude alone does not fix a narrow phase table — the
    /// floor is set by phase truncation. Guards against sizing `AMP_W`
    /// when `ADDR_W` is the actual constraint.
    #[test]
    fn amplitude_width_does_not_rescue_a_narrow_phase_table() {
        let record = 1 << 14;
        let word = 0x0ACE_1357u64;
        let narrow = {
            let mut m = DdsModel::new(32, 8, 16);
            analyze(&m.run_sin(word, record), F_CLK, 0.0, F_CLK / 2.0).sfdr_db
        };
        let narrow_wide_amp = {
            let mut m = DdsModel::new(32, 8, 24);
            analyze(&m.run_sin(word, record), F_CLK, 0.0, F_CLK / 2.0).sfdr_db
        };
        assert!(
            (narrow - narrow_wide_amp).abs() < 6.0,
            "tripling amplitude bits should not materially change a \
             phase-truncation-limited figure: {narrow:.1} vs {narrow_wide_amp:.1}"
        );
    }

    /// **The general analyser must reproduce the validated one.**
    ///
    /// `exact_spur_spectrum` (phase-domain, validated against the
    /// windowed measurement) and `exact_spur_spectrum_for` with
    /// `PhaseToAmp::Lut` model the same thing by different routes. They
    /// must agree.
    ///
    /// This is the check that should have been written before trusting
    /// any hybrid number: the general path acquired three separate bugs
    /// — wrong period, wrong band reference, and an unvalidated
    /// normalisation — and each produced plausible-looking output.
    #[test]
    fn general_analyser_agrees_with_the_validated_one() {
        let phase_w = 26u32;
        let addr_w = 10u32;
        let f_clk = 125.0e6;
        // A word whose spur lands well inside the band.
        let word = (1u64 << 20) | (1u64 << 9);

        let a = worst_exact_spur_in_band(phase_w, addr_w, word, f_clk, 40.0e6, 26)
            .expect("phase-domain analysis");
        let b = worst_exact_spur_for(PhaseToAmp::Lut { addr_w }, phase_w, word, f_clk, 40.0e6, 26)
            .expect("output-domain analysis");

        assert!(
            (a.0 - b.0).abs() < 3.0,
            "the two analyses must agree on level: phase-domain {:.1} dBc, \
             output-domain {:.1} dBc",
            a.0,
            b.0
        );
    }

    /// The table budget: quarter-wave symmetry, one table for both
    /// components.
    #[test]
    fn table_fits_one_block_ram() {
        // 70 dB target -> 13 address bits, 16-bit amplitude.
        let m = DdsModel::new(48, 13, 16);
        assert_eq!(m.table_bits(), 2048 * 16, "2^(13-2) entries x 16 bits");
        assert!(
            m.table_bits() <= 36 * 1024,
            "must fit a single BRAM36: {} bits",
            m.table_bits()
        );
    }
}

#[cfg(test)]
mod sweep_report {
    use super::*;

    const F_CLK: f64 = 125.0e6;
    const BAND: f64 = 1.0e6;

    /// Large adversarial sweep across table widths and carriers.
    ///
    /// Run with:
    /// `cargo test -p rhdl-fpga --lib dsp::nco::model::sweep_report::large_sweep -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn large_sweep() {
        let record = 1 << 16;
        println!("\n  carrier    P   words   worst    p10     median   best");
        for carrier in [1.0e6, 10.0e6, 21.0e6, 45.0e6] {
            for addr_w in [9u32, 10, 11, 12, 13] {
                let sweep = sizing_sweep(48, addr_w, 16, F_CLK, carrier, BAND, record, 400);
                let n = sweep.len();
                let finite: Vec<f64> = sweep
                    .iter()
                    .map(|e| e.sfdr_db)
                    .filter(|v| v.is_finite())
                    .collect();
                let worst = finite.first().copied().unwrap_or(f64::NAN);
                let p10 = finite[finite.len() / 10];
                let median = finite[finite.len() / 2];
                let best = finite[finite.len() - 1];
                println!(
                    "  {:>5.1} MHz  {:>2}   {:>5}  {:>6.1}  {:>6.1}  {:>6.1}  {:>6.1}",
                    carrier / 1e6,
                    addr_w,
                    n,
                    worst,
                    p10,
                    median,
                    best
                );
            }
        }
    }

    /// **Linear interpolation vs CORDIC micro-rotation.**
    ///
    /// Same coarse table, same fine remainder — only the rotator
    /// differs. This decides whether a parameterised fine stage is
    /// worth building at all.
    #[test]
    #[ignore]
    fn linear_vs_cordic() {
        let phase_w = 26u32;
        let word = (1u64 << 18) | (1u64 << 5);
        println!("\n  coarse fine  rotator            worst dBc   cost");
        for (coarse, fine) in [(8u32, 10u32), (10, 12)] {
            let lin = PhaseToAmp::Hybrid {
                coarse_w: coarse,
                fine_w: fine,
            };
            if let Some(l) = exact_spur_spectrum_for(lin, phase_w, word, 26) {
                let w = l.iter().map(|x| x.dbc).fold(f64::NEG_INFINITY, f64::max);
                println!("  {coarse:>6} {fine:>4}  linear (Taylor)    {w:>9.1}   2 mult");
            }
            for stages in [2u32, 3, 4, 6, 8] {
                let cor = PhaseToAmp::LutCordic {
                    coarse_w: coarse,
                    fine_w: fine,
                    stages,
                };
                if let Some(l) = exact_spur_spectrum_for(cor, phase_w, word, 26) {
                    let w = l.iter().map(|x| x.dbc).fold(f64::NEG_INFINITY, f64::max);
                    println!(
                        "  {coarse:>6} {fine:>4}  cordic {stages} stages    {w:>9.1}   {} add",
                        2 * stages
                    );
                }
            }
            println!();
        }
    }

    /// **What does finite-precision arithmetic actually cost?**
    ///
    /// The `f64` hybrid reports the architecture's ceiling. This is what
    /// the implementation can reach.
    #[test]
    #[ignore]
    fn arithmetic_precision_cost() {
        let phase_w = 26u32;
        let word = (1u64 << 18) | (1u64 << 5);
        let ideal = PhaseToAmp::Hybrid {
            coarse_w: 10,
            fine_w: 12,
        };
        let ceiling = exact_spur_spectrum_for(ideal, phase_w, word, 26)
            .map(|l| l.iter().map(|x| x.dbc).fold(f64::NEG_INFINITY, f64::max))
            .unwrap();
        println!("\n  coarse=10 fine=12, f64 rotation (architecture ceiling): {ceiling:.1} dBc");
        println!();
        println!("  amp  prod  out    worst dBc   cost vs ceiling");
        for (amp, prod, out) in [
            (12u32, 12u32, 12u32),
            (14, 14, 14),
            (16, 16, 16),
            (18, 18, 18),
            (18, 20, 20),
            (20, 22, 22),
            (24, 24, 24),
        ] {
            let arch = PhaseToAmp::HybridQ {
                coarse_w: 10,
                fine_w: 12,
                amp_w: amp,
                prod_w: prod,
                out_w: out,
            };
            if let Some(lines) = exact_spur_spectrum_for(arch, phase_w, word, 26) {
                let worst = lines
                    .iter()
                    .map(|x| x.dbc)
                    .fold(f64::NEG_INFINITY, f64::max);
                println!(
                    "  {amp:>3}  {prod:>4}  {out:>3}   {worst:>9.1}   {:>+8.1} dB",
                    worst - ceiling
                );
            }
        }
    }

    /// **Do static phase offsets change the spur spectrum?**
    ///
    /// Theory says no for an odd tuning word — the offset is a pure
    /// time shift there. For `v > 0` the argument fails, and this grid
    /// has words with `v` up to 19, so it is checked rather than
    /// assumed.
    #[test]
    #[ignore]
    fn phase_offset_effect_on_spurs() {
        let phase_w = 26u32;
        let arch = PhaseToAmp::Hybrid {
            coarse_w: 8,
            fine_w: 6,
        };
        println!("\n   word v   offset          worst dBc   spread");
        for v in [0u32, 3, 7, 11] {
            let word = (1u64 << 18) | (1u64 << v);
            let mut levels = Vec::new();
            for k in 0..8u64 {
                // Offsets spanning a full turn, deliberately including
                // values not aligned to the fine LSB.
                let off = k.wrapping_mul(0x0123_4567) & ((1u64 << phase_w) - 1);
                if let Some(lines) = exact_spur_spectrum_offset(arch, phase_w, word, off, 26) {
                    let worst = lines
                        .iter()
                        .map(|l| l.dbc)
                        .fold(f64::NEG_INFINITY, f64::max);
                    levels.push(worst);
                }
            }
            let lo = levels.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = levels.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            println!(
                "  {:>6} {v:>2}   8 offsets      {hi:>8.1}   {:>6.2} dB",
                word.trailing_zeros(),
                hi - lo
            );
        }
    }

    /// **Plain LUT vs LUT+rotation hybrid, exact analysis.**
    ///
    /// Worst-case in-band spur for a tuning word whose spur lands close
    /// to the carrier (the hard case), across architectures, with the
    /// quarter-wave table size for each.
    #[test]
    #[ignore]
    fn architecture_comparison() {
        // Reduced accumulator so the FULL phase period is tractable:
        // 2^(phase_w - v) points.  Spur levels depend on the architecture
        // and the used/truncated bit split, not on absolute accumulator
        // width, so this is representative.
        let phase_w = 30u32;
        let carrier = 10.0e6;
        let base = ((carrier / F_CLK) * (1u64 << phase_w) as f64).round() as u64;

        println!("\n  architecture                 table (entries x 16b)   worst dBc   mults");
        for addr_w in [10u32, 11, 12, 13, 14] {
            let arch = PhaseToAmp::Lut { addr_w };
            let b = phase_w - addr_w;
            let word = ((base >> b) << b) | (1u64 << 7);
            if let Some((dbc, _)) = worst_exact_spur_for(arch, phase_w, word, F_CLK, 1.0e6, 23) {
                let entries = 1usize << (addr_w - 2);
                println!(
                    "  LUT  P={addr_w:<2}                     {entries:>6}  ({:>5.1} Kbit)      {dbc:>7.1}     0",
                    entries as f64 * 16.0 / 1024.0
                );
            }
        }
        println!();
        for (coarse_w, fine_w) in [
            (6u32, 6u32),
            (8, 8),
            (8, 10),
            (10, 10),
            (10, 12),
            (11, 12),
            (12, 12),
            (12, 14),
        ] {
            let arch = PhaseToAmp::Hybrid { coarse_w, fine_w };
            let b = phase_w - (coarse_w + fine_w);
            let word = ((base >> b) << b) | (1u64 << 7);
            if let Some((dbc, _)) = worst_exact_spur_for(arch, phase_w, word, F_CLK, 1.0e6, 23) {
                let entries = 1usize << (coarse_w.saturating_sub(2));
                println!(
                    "  Hybrid coarse={coarse_w:<2} fine={fine_w:<2}       {entries:>6}  ({:>5.1} Kbit)      {dbc:>7.1}     2",
                    entries as f64 * 16.0 / 1024.0
                );
            }
        }
    }

    /// **Exact vs windowed.** Where the windowed analyser can see, the
    /// two must agree; where it cannot, the exact method reveals what
    /// was hidden.
    #[test]
    #[ignore]
    fn exact_versus_windowed() {
        let addr_w = 11u32;
        let b = 48 - addr_w;
        let carrier = 10.0e6;
        let base = ((carrier / F_CLK) * (1u64 << 48) as f64).round() as u64;
        let high = (base >> b) << b;
        let record = 1 << 22;

        println!(
            "\n  P={addr_w}   formula predicts {:.1} dBc",
            -sfdr_estimate_db(addr_w)
        );
        println!("  v    offset (Hz)      EXACT dBc    windowed SFDR    verdict");
        for v in [12u32, 14, 16, 18, 20, 22, 24, 26, 28] {
            let word = high | (1u64 << v);
            let exact = worst_exact_spur_in_band(48, addr_w, word, F_CLK, 1.0e6, 26);
            let mut m = DdsModel::new(48, addr_w, 16);
            let samples = m.run_sin(word, record);
            let win = analyze(&samples, F_CLK, carrier - 500e3, carrier + 500e3);
            match exact {
                Some((dbc, off)) => {
                    let agree = ((-dbc) - win.sfdr_db).abs() < 3.0;
                    println!(
                        "  {v:>2}   {off:>11.0}   {dbc:>10.1}    {:>10.1}      {}",
                        win.sfdr_db,
                        if agree { "agree" } else { "WINDOWED WRONG" }
                    );
                }
                None => println!("  {v:>2}   period too long — analytic bound applies"),
            }
        }
    }

    /// **Close-in spur probe.**
    ///
    /// The standard analysis excludes bins around the carrier to avoid
    /// the window main lobe, which blinds it to spurs closer than that
    /// exclusion. For truncation words with LONG periods the spur sits
    /// very close to the carrier — a few hundred Hz — and is therefore
    /// invisible at a short record.
    ///
    /// This uses a much longer record so a bin is ~30 Hz and the
    /// exclusion covers only a few hundred Hz, exposing what the
    /// standard sweep misses.
    #[test]
    #[ignore]
    fn close_in_spurs() {
        let record = 1 << 22; // bin ~30 Hz
        let addr_w = 11u32;
        let b = 48 - addr_w;
        let carrier = 10.0e6;
        let base = ((carrier / F_CLK) * (1u64 << 48) as f64).round() as u64;
        let high = (base >> b) << b;

        println!(
            "\n  record 2^22, bin {:.1} Hz, band +/-500 kHz",
            F_CLK / (1u64 << 22) as f64
        );
        println!("  v    spur offset (theory)   measured SFDR (dB)   worst spur offset (Hz)");
        for v in [16u32, 18, 20, 22, 24, 26, 28] {
            let low = 1u64 << v;
            let word = high | low;
            let mut m = DdsModel::new(48, addr_w, 16);
            let samples = m.run_sin(word, record);
            let rep = analyze(&samples, F_CLK, carrier - 500e3, carrier + 500e3);
            let period = 1u64 << (b - v);
            println!(
                "  {v:>2}   {:>14.0} Hz   {:>16.1}   {:>+12.0}",
                F_CLK / period as f64,
                rep.sfdr_db,
                rep.worst_spur_hz - rep.carrier_hz
            );
        }
    }

    /// What do the worst tuning words look like? Structure here tells
    /// us whether the adversarial selection is finding the real cases.
    #[test]
    #[ignore]
    fn worst_word_structure() {
        let record = 1 << 16;
        let addr_w = 11u32;
        let b = 48 - addr_w;
        let sweep = sizing_sweep(48, addr_w, 16, F_CLK, 10.0e6, BAND, record, 400);
        println!("\n  worst 12 tuning words at P={addr_w} (B={b} truncated bits)");
        println!("  SFDR dB   spur MHz    low bits (hex)   low/2^B");
        for e in sweep.iter().take(12) {
            let low = e.word & ((1u64 << b) - 1);
            println!(
                "  {:>7.1}   {:>8.4}    {:>14X}   {:.6}",
                e.sfdr_db,
                e.worst_spur_hz / 1e6,
                low,
                low as f64 / (1u64 << b) as f64
            );
        }
    }
}
