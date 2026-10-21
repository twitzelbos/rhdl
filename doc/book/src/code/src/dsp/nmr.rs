//! The receive-chain budget for an averaging NMR spectrometer.
//!
//! Every figure the chapter quotes is computed here and pinned by the
//! tests at the bottom, because a receiver budget written by hand drifts
//! the moment a model changes and nothing notices.

use rhdl_fpga::dsp::cic::chain::{self, ChainSpec};
use rhdl_fpga::dsp::cic::compensator::Method;

/// Full-scale sine SNR of an ideal `bits`-bit quantiser, in dB.
pub fn ideal_adc_snr_db(bits: usize) -> f64 {
    6.02 * bits as f64 + 1.76
}

/// SNR improvement from decimating by `r`, in dB.
///
/// Narrowing the noise bandwidth by `r` while keeping the signal is a
/// power ratio of `r`. This is *processing gain*, and it is the reason a
/// 16-bit converter can support a 130 dB measurement.
pub fn processing_gain_db(r: usize) -> f64 {
    10.0 * (r as f64).log10()
}

/// Extra bits needed at the output to carry [`processing_gain_db`].
pub fn processing_gain_bits(r: usize) -> f64 {
    0.5 * (r as f64).log2()
}

/// SNR improvement from coherently averaging `n` transients, in dB.
///
/// Signal amplitude adds as `n`, independent noise as `sqrt(n)`, so the
/// ratio improves as `sqrt(n)` — `10·log10(n)` in dB.
pub fn averaging_gain_db(n: usize) -> f64 {
    10.0 * (n as f64).log10()
}

/// How much an ideal quantiser adds to analog noise of `sigma` LSB rms.
///
/// The quantiser contributes `1/12` LSB² regardless. Above about one LSB
/// of analog noise it is transparent; below a fifth of an LSB it
/// dominates *and stops being random*, which is the failure this whole
/// chapter is about.
pub fn quantiser_penalty_db(sigma_lsb: f64) -> f64 {
    10.0 * ((sigma_lsb * sigma_lsb + 1.0 / 12.0) / (sigma_lsb * sigma_lsb)).log10()
}

/// Averages at which a coherent spur at `sfdr_dbc` overtakes a noise
/// floor starting at `head_db` below full scale.
///
/// Returns a value below one when the spur is already above the noise
/// before any averaging.
pub fn averages_until_spur_limited(sfdr_dbc: f64, head_db: f64) -> f64 {
    10f64.powf((-sfdr_dbc - head_db) / 10.0)
}

// ANCHOR: spec
/// A 20 kHz spectral width from a 125 Msps converter: `R = 2500`, out at
/// 50 ksps.
///
/// `min_snr_db` is the load-bearing field and the one most easily left
/// slack. It is what the pruning schedule is spent against: state a
/// generous requirement and the designer prunes until it is only just
/// met, which is exactly the dynamic range an averaging experiment needs.
pub fn receive_chain(input_width: usize, output_width: usize, min_snr_db: f64) -> ChainSpec {
    ChainSpec {
        fs_hz: 125e6,
        decimate: 2500,
        // One-sided: a 20 kHz-wide complex spectrum is ±10 kHz.
        alias_free_bw_hz: 10e3,
        input_width,
        output_width,
        max_ripple_db: 0.05,
        min_alias_rejection_db: 80.0,
        min_snr_db,
        coeff_width: 18,
        max_stages: 8,
        max_taps: 31,
        max_chain_stages: 2,
        stopband_edge: 1.0,
        min_stopband_db: 0.0,
        method: Method::LeastSquares,
        max_group_delay_s: 0.0,
        pipelined_combs: true,
    }
}
// ANCHOR_END: spec

// ANCHOR: pruning
/// What the pruning budget costs, as a function of what you asked for.
///
/// Same chain, same widths, only `min_snr_db` changing. The register
/// count is the price of the dynamic range.
pub fn pruning_table() -> String {
    let mut s = String::from("  asked   achieved   register bits\n");
    for asked in [0.0f64, 40.0, 80.0, 100.0, 120.0] {
        match chain::design(receive_chain(16, 24, asked)) {
            Ok(d) => s.push_str(&format!(
                "{asked:>7.0} {:>10.1} {:>15}\n",
                d.achieved_snr_db, d.register_bits
            )),
            Err(_) => s.push_str(&format!("{asked:>7.0}      unmet               -\n")),
        }
    }
    s
}
// ANCHOR_END: pruning

// ANCHOR: width
/// Output width against dynamic range and silicon, at a fixed
/// requirement.
///
/// The counter-intuitive column is the last one.
pub fn width_table() -> String {
    let mut s = String::from("  out_w   achieved   register bits\n");
    for out_w in [16usize, 20, 24, 28] {
        match chain::design(receive_chain(16, out_w, 100.0)) {
            Ok(d) => s.push_str(&format!(
                "{out_w:>7} {:>10.1} {:>15}\n",
                d.achieved_snr_db, d.register_bits
            )),
            Err(_) => s.push_str(&format!("{out_w:>7}      unmet               -\n")),
        }
    }
    s
}
// ANCHOR_END: width

#[cfg(test)]
mod tests {
    use super::*;

    /// **The budget figures the chapter quotes.**
    #[test]
    fn the_headline_budget_is_what_the_chapter_says() {
        assert_eq!(format!("{:.1}", ideal_adc_snr_db(14)), "86.0");
        assert_eq!(format!("{:.1}", ideal_adc_snr_db(16)), "98.1");
        assert_eq!(format!("{:.1}", processing_gain_db(2500)), "34.0");
        assert_eq!(format!("{:.2}", processing_gain_bits(2500)), "5.64");
        // In-band, before averaging.
        assert_eq!(
            format!("{:.1}", ideal_adc_snr_db(16) + processing_gain_db(2500)),
            "132.1"
        );
        assert_eq!(
            format!("{:.1}", ideal_adc_snr_db(14) + processing_gain_db(2500)),
            "120.0"
        );
        assert_eq!(format!("{:.1}", averaging_gain_db(4096)), "36.1");
    }

    /// **The ADC operating point: the quantiser is transparent above
    /// about one LSB of analog noise.**
    #[test]
    fn the_quantiser_penalty_is_what_the_chapter_says() {
        assert_eq!(format!("{:.2}", quantiser_penalty_db(0.05)), "15.36");
        assert_eq!(format!("{:.2}", quantiser_penalty_db(0.5)), "1.25");
        assert_eq!(format!("{:.2}", quantiser_penalty_db(1.0)), "0.35");
        assert_eq!(format!("{:.2}", quantiser_penalty_db(2.0)), "0.09");
        // Monotone: more analog noise always makes the quantiser less
        // significant, which is why the *only* reason not to add more is
        // the dynamic range it costs.
        let mut prev = f64::INFINITY;
        for k in 1..40 {
            let p = quantiser_penalty_db(k as f64 * 0.1);
            assert!(p < prev, "penalty must fall with sigma");
            prev = p;
        }
    }

    /// **The NCO variant has to be chosen by averaging depth**, because
    /// a coherent spur gets no help from averaging at all.
    ///
    /// This is the chapter's central claim about the oscillator, and the
    /// numbers are the ones `sin_cos_linear_interp`'s own table reports.
    #[test]
    fn the_nco_choice_follows_the_averaging_depth() {
        let head = ideal_adc_snr_db(16) + processing_gain_db(2500);
        // The default variant is already spur-limited at N = 1.
        assert!(averages_until_spur_limited(-104.3, head) < 1.0);
        // 24-bit is good for a handful.
        let n24 = averages_until_spur_limited(-140.4, head);
        // 6.82 -- the chapter shows it rounded to 7.
        assert_eq!(format!("{n24:.0}"), "7");
        // 28-bit for a serious experiment.
        let n28 = averages_until_spur_limited(-164.5, head);
        assert_eq!(format!("{n28:.0}"), "1754");
        // And a 14-bit front end tolerates 100x more averaging on the
        // same oscillator, because its noise floor starts 12 dB higher.
        let head14 = ideal_adc_snr_db(14) + processing_gain_db(2500);
        let n24_14 = averages_until_spur_limited(-140.4, head14);
        assert!(n24_14 > 15.0 * n24, "{n24_14} vs {n24}");
    }

    /// **Asking for slack SNR spends the whole dynamic range**, and the
    /// register cost of protecting it is modest.
    #[test]
    fn the_pruning_budget_is_spent_against_the_requirement() {
        let slack = chain::design(receive_chain(16, 24, 0.0)).expect("designable");
        let strict = chain::design(receive_chain(16, 24, 120.0)).expect("designable");
        assert!(
            slack.achieved_snr_db < 10.0,
            "a 0 dB requirement must let the designer prune to nothing: {}",
            slack.achieved_snr_db
        );
        assert!(strict.achieved_snr_db > 120.0);
        // The whole difference costs under 40% more register bits.
        let ratio = strict.register_bits as f64 / slack.register_bits as f64;
        assert!(ratio < 1.4, "{ratio}");
        // Same filter either way -- this is a noise decision, not a
        // response decision.
        assert_eq!(slack.split(), strict.split());
        assert_eq!(
            slack.achieved_alias_db.round(),
            strict.achieved_alias_db.round()
        );
    }

    /// **The worked chain the chapter quotes.**
    #[test]
    fn the_worked_chain_is_what_the_chapter_says() {
        let d = chain::design(receive_chain(16, 24, 100.0)).expect("designable");
        assert_eq!(d.split(), vec![100, 25]);
        assert_eq!(
            d.cics.iter().map(|c| c.stages).collect::<Vec<_>>(),
            vec![2, 6]
        );
        assert_eq!(format!("{:.1}", d.achieved_snr_db), "107.0");
        assert_eq!(format!("{:.1}", d.achieved_alias_db), "83.7");
        assert_eq!(format!("{:.3}", d.achieved_ripple_db), "0.031");
        assert_eq!(d.register_bits, 330);
    }

    /// **The receive band really is full, so compensation is indicated.**
    ///
    /// The contrast the chapter draws with a narrowband transmit pulse,
    /// where the droop is microdecibels and a compensator is pure cost.
    #[test]
    fn the_receive_band_is_full_and_droops() {
        use rhdl_fpga::dsp::cic::response::passband_droop_db;
        // 2 * 10 kHz * 2500 / 125 MHz.
        let passband = 2.0 * 10e3 * 2500.0 / 125e6;
        assert_eq!(format!("{passband:.1}"), "0.4");
        assert_eq!(
            format!("{:.1}", passband_droop_db(passband, 2, 2500, 1)),
            "-1.2"
        );
        assert_eq!(
            format!("{:.1}", passband_droop_db(passband, 6, 2500, 1)),
            "-3.5"
        );
    }

    /// **The 14-bit front end tolerates ~109 averages on the 24-bit
    /// oscillator**, the figure the chapter quotes.
    #[test]
    fn the_fourteen_bit_figure_is_what_the_chapter_says() {
        let head14 = ideal_adc_snr_db(14) + processing_gain_db(2500);
        let n = averages_until_spur_limited(-140.4, head14);
        assert_eq!(format!("{n:.0}"), "109");
    }

    /// **A wider output word is both better and cheaper.**
    ///
    /// The counter-intuitive one, and the reason the chapter says to size
    /// the output from the processing gain rather than from the ADC.
    #[test]
    fn a_wider_output_costs_fewer_registers() {
        let narrow = chain::design(receive_chain(16, 16, 100.0)).expect("designable");
        let wide = chain::design(receive_chain(16, 28, 100.0)).expect("designable");
        assert!(
            wide.achieved_snr_db > narrow.achieved_snr_db,
            "wider must not be worse"
        );
        assert!(
            wide.register_bits < narrow.register_bits,
            "wider should be cheaper: {} vs {}",
            wide.register_bits,
            narrow.register_bits
        );
        // The deltas the chapter quotes.
        assert_eq!(
            format!("{:.1}", wide.achieved_snr_db - narrow.achieved_snr_db),
            "7.6"
        );
        assert_eq!(narrow.register_bits - wide.register_bits, 171);
    }
}

/// SNR set by sampling aperture jitter, in dB.
///
/// `-20·log10(2π·f_in·σ_t)`. Depends on the **input** frequency, not the
/// sample rate, so it is a direct-sampling constraint. Broadband and
/// random, so unlike an oscillator spur it does average down — but it
/// caps the single transient, and no digital design can lift it.
pub fn jitter_snr_db(f_in_hz: f64, sigma_t_s: f64) -> f64 {
    -20.0 * (std::f64::consts::TAU * f_in_hz * sigma_t_s).log10()
}

/// Jitter, in seconds rms, that puts the jitter floor exactly at a
/// `bits`-bit converter's own SNR at `f_in_hz`.
pub fn jitter_budget_s(bits: usize, f_in_hz: f64) -> f64 {
    10f64.powf(-ideal_adc_snr_db(bits) / 20.0) / (std::f64::consts::TAU * f_in_hz)
}

#[cfg(test)]
mod jitter_tests {
    use super::*;

    /// **The jitter table the chapter quotes.**
    #[test]
    fn the_jitter_table_is_what_the_chapter_says() {
        for (sigma, f, want) in [
            (10e-12f64, 1e6f64, "84"),
            (10e-12, 10e6, "64"),
            (10e-12, 50e6, "50"),
            (10e-12, 100e6, "44"),
            (1e-12, 10e6, "84"),
            (100e-15, 100e6, "84"),
            (10e-15, 50e6, "110"),
        ] {
            assert_eq!(
                format!("{:.0}", jitter_snr_db(f, sigma)),
                want,
                "sigma={sigma} f={f}"
            );
        }
    }

    /// **The budget the chapter states: better than 1 ps at 10 MHz for 16
    /// bits, about 100 fs at 100 MHz, ~3 ps at 10 MHz for 14 bits.**
    #[test]
    fn the_jitter_budget_is_what_the_chapter_says() {
        // 199 fs. An earlier draft of the chapter said "better than
        // 1 ps", which is wrong by 5x -- and 5x on a jitter spec is the
        // difference between an ordinary oscillator and an expensive one.
        let b16_10m = jitter_budget_s(16, 10e6);
        assert_eq!(format!("{:.0}", b16_10m * 1e15), "199");
        let b16_100m = jitter_budget_s(16, 100e6);
        assert_eq!(format!("{:.1}", b16_100m * 1e15), "19.9");
        let b14_10m = jitter_budget_s(14, 10e6);
        assert_eq!(format!("{:.0}", b14_10m * 1e15), "794");
        // And it scales inversely with input frequency, which is the
        // property that makes direct sampling hard.
        assert!((jitter_budget_s(16, 10e6) / jitter_budget_s(16, 100e6) - 10.0).abs() < 1e-6);
    }
}

/// Level of an oscillator-spur *ghost* relative to the noise floor, in dB.
///
/// The mixer multiplies, so `out = in × ideal + in × err`: a spur adds a
/// displaced **ghost of the signal** at `−SFDR` relative to the signal
/// that cast it, not a fixed-level tone. So the ghost is visible when the
/// achieved SNR of the strongest peak exceeds the SFDR.
///
/// `peak_dbfs` is negative (dB below full scale), `sigma_lsb` the analog
/// noise, `bits` the converter, `r` the decimation, `n` the averages.
pub fn ghost_above_noise_db(
    bits: usize,
    sigma_lsb: f64,
    peak_dbfs: f64,
    r: usize,
    n: usize,
    sfdr_dbc: f64,
) -> f64 {
    // Full scale above the analog noise, in dB.
    let fs_to_noise = 20.0 * (2f64.powi(bits as i32 - 1) / sigma_lsb).log10();
    let peak_snr = fs_to_noise + peak_dbfs + processing_gain_db(r) + averaging_gain_db(n);
    peak_snr + sfdr_dbc
}

#[cfg(test)]
mod ghost_tests {
    use super::*;

    /// **The ghost table the chapter quotes.**
    ///
    /// 16-bit, `σ = 1` LSB, `R = 2500`, SFDR −96.3 dBc.
    #[test]
    fn the_ghost_table_is_what_the_chapter_says() {
        for (peak, n, want) in [
            (0.0f64, 1usize, "28.0"),
            (-20.0, 1, "8.0"),
            (-40.0, 1, "-12.0"),
            (-60.0, 1, "-32.0"),
            (0.0, 256, "52.1"),
            (-40.0, 256, "12.1"),
            (-60.0, 256, "-7.9"),
        ] {
            assert_eq!(
                format!("{:.1}", ghost_above_noise_db(16, 1.0, peak, 2500, n, -96.3)),
                want,
                "peak={peak} n={n}"
            );
        }
    }

    /// **The chapter's conclusion: the oscillator choice depends on
    /// headroom and averaging depth, not on the converter alone.**
    #[test]
    fn headroom_and_averaging_decide_the_oscillator() {
        // The boundary is exact and memorable: on the 9 Kbit table, a
        // 16-bit chain at sigma = 1 LSB and R = 2500 puts the ghost
        // precisely at the noise floor when the strongest peak is 40 dB
        // below full scale and 16 transients are averaged.
        let boundary = ghost_above_noise_db(16, 1.0, -40.0, 2500, 16, -96.3);
        assert_eq!(format!("{boundary:.1}"), "0.0");
        // Either parameter relaxed by one step buries it.
        assert!(ghost_above_noise_db(16, 1.0, -50.0, 2500, 16, -96.3) < -9.0);
        assert!(ghost_above_noise_db(16, 1.0, -40.0, 2500, 4, -96.3) < -5.0);
        // Demanding use: it does not.
        let demanding = ghost_above_noise_db(16, 1.0, 0.0, 2500, 256, -96.3);
        assert!(demanding > 40.0, "got {demanding:.1} dB");
        // And the bigger table fixes the demanding case.
        let with28 = ghost_above_noise_db(16, 1.0, 0.0, 2500, 256, -164.5);
        assert!(
            with28 < 0.0,
            "SinCosLinearInterp28 should bury it, got {with28:.1}"
        );
    }
}
