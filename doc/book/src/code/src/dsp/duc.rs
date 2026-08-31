//! Spec-driven design of an interpolation chain, and what it costs.

use rhdl_fpga::dsp::cic::compensator::Method;
use rhdl_fpga::dsp::cic::interp_chain::{InterpSpec, Unmet, design};

// ANCHOR: spec
/// The transmit case the up-converter was written for: a 16-bit complex
/// envelope at 1 Msps onto a 125 Msps converter, 200 kHz of signal, out
/// to a 14-bit DAC.
pub fn envelope_to_carrier() -> InterpSpec {
    InterpSpec {
        fs_hz: 125e6,
        // Total interpolation. 125 Msps / 1 Msps.
        interpolate: 125,
        // One-sided: a 400 kHz-wide complex envelope is +/- 200 kHz.
        image_free_bw_hz: 200e3,
        input_width: 16,
        // The DAC. Its own quantisation is the noise floor the chain is
        // measured against, because the chain itself adds none: an
        // interpolator's width taper is lossless.
        output_width: 14,
        // Flatness of the compensated passband, and how far down the
        // worst image in that band must sit.
        max_ripple_db: 0.1,
        min_image_rejection_db: 60.0,
        coeff_width: 16,
        max_stages: 5,
        max_taps: 21,
        max_chain_stages: 2,
        method: Method::LeastSquares,
        // The lowest run-time rate the design must still satisfy, and
        // whether every integer rate in between has to be reachable.
        // `true` forbids splitting -- only a single stage reaches them
        // all. See "Reaching every rate".
        rate_min: 2,
        arbitrary_rate: false,
        // Transmit-side delay is latency, not phase margin, unless the
        // modulator sits inside a loop. See "Delay and control loops".
        max_group_delay_s: 0.0,
        pipelined_combs: true,
    }
}
// ANCHOR_END: spec

// ANCHOR: derive
/// Derive the implementation, and report what it costs.
pub fn derive() -> String {
    match design(envelope_to_carrier()) {
        Ok(d) => {
            let b = d.group_delay;
            format!(
                "split ............. {:?} N={:?} M={:?}\n\
                 images ............ {:.1} dB down (asked >= {:.1})\n\
                 ripple ............ {:.4} dB (asked <= {:.3})\n\
                 compensator ....... {} taps at {:.4} MHz\n\
                 state ............. {} bits uniform, {} as built\n\
                 group delay ....... {:.0} converter samples, largest is the {}\n\
                 rates reachable ... {} of {}",
                d.split(),
                d.depths(),
                d.delays(),
                d.achieved_image_db,
                d.spec.min_image_rejection_db,
                d.achieved_ripple_db,
                d.spec.max_ripple_db,
                d.compensator.taps.len(),
                d.input_rate_hz / 1e6,
                d.register_bits,
                d.built_register_bits,
                b.total(),
                b.dominant().0,
                d.reachable_rates()
                    .iter()
                    .filter(|r| **r >= d.spec.rate_min && **r <= d.spec.interpolate)
                    .count(),
                d.spec.interpolate - d.spec.rate_min + 1,
            )
        }
        Err(e) => format!("unmet: {e:?}"),
    }
}
// ANCHOR_END: derive

// ANCHOR: arbitrary
/// The same spec, but every run-time rate must be reachable.
///
/// `arbitrary_rate` forbids the split, because only a single stage
/// divides by every integer. It is not obviously the more expensive
/// choice — see the chapter.
pub fn every_rate() -> String {
    let spec = InterpSpec {
        arbitrary_rate: true,
        ..envelope_to_carrier()
    };
    match design(spec) {
        Ok(d) => format!(
            "split {:?}, {} state bits as built, rate-weighted cost {:.3e}",
            d.split(),
            d.built_register_bits,
            d.cost
        ),
        Err(Unmet::ImageRejection { best_db, needed_db }) => {
            format!("no single stage rejects enough: best {best_db:.1} against {needed_db:.1}")
        }
        Err(e) => format!("unmet: {e:?}"),
    }
}
// ANCHOR_END: arbitrary

#[cfg(test)]
mod tests {
    use super::*;

    /// **The block the chapter quotes, verbatim.**
    ///
    /// The chapter shows this as a `text` block, which is hand-copied
    /// prose and drifts silently the moment the designer's cost model or
    /// the delay maths changes.
    #[test]
    fn the_derived_block_is_what_the_chapter_says() {
        assert_eq!(
            derive(),
            "split ............. [5, 25] N=[5, 2] M=[2, 1]\n\
             images ............ 66.6 dB down (asked >= 60.0)\n\
             ripple ............ 0.0147 dB (asked <= 0.100)\n\
             compensator ....... 9 taps at 1.0000 MHz\n\
             state ............. 836 bits uniform, 614 as built\n\
             group delay ....... 1864 converter samples, largest is the comb pipeline\n\
             rates reachable ... 73 of 124"
        );
    }

    /// **And the single-stage block.**
    #[test]
    fn the_arbitrary_rate_block_is_what_the_chapter_says() {
        assert_eq!(
            every_rate(),
            "split [125], 351 state bits as built, rate-weighted cost 2.010e10"
        );
    }

    /// **The chapter's central counter-intuitive claim: the single stage
    /// is smaller and more expensive.**
    ///
    /// Asserted rather than left in prose, because it is the one sentence
    /// in the chapter a reader is most likely to disbelieve, and because
    /// a change to the cost model that reversed it would leave the
    /// chapter confidently wrong.
    #[test]
    fn the_single_stage_is_smaller_and_more_expensive() {
        let split = design(envelope_to_carrier()).expect("designable");
        let single = design(InterpSpec {
            arbitrary_rate: true,
            ..envelope_to_carrier()
        })
        .expect("designable");
        assert_eq!(
            single.split(),
            vec![125],
            "arbitrary_rate must forbid a split"
        );
        assert!(
            single.built_register_bits < split.built_register_bits,
            "smaller: {} vs {}",
            single.built_register_bits,
            split.built_register_bits
        );
        assert!(
            single.cost > split.cost,
            "more expensive: {:.4e} vs {:.4e}",
            single.cost,
            split.cost
        );
    }

    /// **The reachability figure the chapter quotes, and that the split
    /// is what costs it.**
    #[test]
    fn a_split_costs_reachability() {
        let d = design(envelope_to_carrier()).expect("designable");
        let reachable = d
            .reachable_rates()
            .iter()
            .filter(|r| **r >= d.spec.rate_min && **r <= d.spec.interpolate)
            .count();
        assert_eq!(reachable, 73);
        assert_eq!(d.spec.interpolate - d.spec.rate_min + 1, 124);
        // A prime above the largest stage factor cannot be reached.
        assert!(!d.reachable_rates().contains(&29));
    }
}
