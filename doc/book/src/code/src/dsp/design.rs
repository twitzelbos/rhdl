//! Spec-driven design of a decimation chain.

use rhdl_fpga::dsp::cic::chain::{ChainSpec, Unmet, design};
use rhdl_fpga::dsp::cic::compensator::Method;

// ANCHOR: spec
/// A narrowband receive chain: 125 Msps in, a 128 kHz-wide complex
/// channel out, at about 256 ksps.
pub fn narrowband() -> ChainSpec {
    ChainSpec {
        fs_hz: 125e6,
        // Total decimation, across however many stages the designer
        // chooses. An input, because the output rate you want is
        // frequently not an integer divisor of the rate you have.
        decimate: 488,
        // One-sided: a 128 kHz-wide complex channel is +/- 64 kHz.
        alias_free_bw_hz: 64e3,
        input_width: 16,
        output_width: 24,
        // Flatness, rejection and noise. Three separate requirements,
        // bought with three different resources.
        max_ripple_db: 0.1,
        min_alias_rejection_db: 60.0,
        min_snr_db: 80.0,
        coeff_width: 16,
        max_stages: 8,
        max_taps: 31,
        max_chain_stages: 3,
        stopband_edge: 1.0,
        min_stopband_db: 0.0,
        method: Method::LeastSquares,
    }
}
// ANCHOR_END: spec

// ANCHOR: derive
/// Derive the implementation from the requirements.
pub fn derive() -> String {
    match design(narrowband()) {
        Ok(d) => format!("{d}"),
        // A design that cannot meet the spec says which constraint it
        // missed and how close it came, rather than returning something
        // plausible that is quietly wrong.
        Err(e) => format!("unmet: {e:?}"),
    }
}
// ANCHOR_END: derive

// ANCHOR: infeasible
/// Asking for more than a CIC can give.
///
/// Deep rejection across a wide band is the one thing a CIC cannot do,
/// because its nulls and its droop are the same expression: more stages
/// reject better *and* droop more steeply, so chasing rejection with
/// depth makes flatness harder.
pub fn infeasible() -> Unmet {
    design(ChainSpec {
        // Nearly the whole output band, and 90 dB of rejection.
        alias_free_bw_hz: 120e3,
        min_alias_rejection_db: 90.0,
        ..narrowband()
    })
    .expect_err("this combination is not available at any depth")
}
// ANCHOR_END: infeasible

// ANCHOR: antialias
/// The compensator doubling as an anti-alias filter.
///
/// A CIC's own stopband is whatever `sinc^N` happens to give. When that
/// is not enough -- or when something downstream decimates again -- the
/// compensator is the natural place to put the attenuation: it is
/// already there, and already running at the low rate.
///
/// `Method::Remez` rather than least squares, because a stopband
/// requirement is a statement about the *worst case* and least squares
/// minimises the average.
pub fn with_antialias() -> String {
    let spec = ChainSpec {
        stopband_edge: 0.9,
        min_stopband_db: 60.0,
        max_taps: 63,
        method: Method::Remez,
        ..narrowband()
    };
    match design(spec) {
        Ok(d) => format!("{d}"),
        Err(e) => format!("unmet: {e:?}"),
    }
}
// ANCHOR_END: antialias

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_narrowband_chain_designs() {
        let d = design(narrowband()).expect("must design");
        // The book claims a cascade is chosen here; if that stops being
        // true the chapter is wrong and this test says so.
        assert_eq!(d.cics.len(), 2, "expected a cascade, got {:?}", d.split());
        assert_eq!(d.split().iter().product::<usize>(), 488);
        assert!(d.achieved_ripple_db <= 0.1);
        assert!(d.achieved_alias_db >= 60.0);
        assert!(d.achieved_snr_db >= 80.0);
    }

    #[test]
    fn the_report_renders() {
        let text = derive();
        assert!(text.contains("decimate"), "{text}");
        assert!(!text.starts_with("unmet"), "{text}");
    }

    #[test]
    fn the_infeasible_spec_is_refused() {
        // The chapter says this fails; make sure it does, and for a
        // reason the chapter can name.
        let e = infeasible();
        assert!(
            matches!(e, Unmet::AliasRejection { .. } | Unmet::Incompatible { .. }),
            "got {e:?}"
        );
    }

    #[test]
    fn the_antialias_variant_designs() {
        let text = with_antialias();
        assert!(!text.starts_with("unmet"), "{text}");
        assert!(text.contains("achieved stopband"), "{text}");
    }
}

// ANCHOR: macro
// The same requirements, lowered straight to widgets. The design runs
// during compilation; what reaches the linker is a pruned CIC per
// stage cascaded through their framing, plus the derived taps and the
// compensating FIR emitted *beside* the chain -- a compensator need not
// sit right behind the decimator, or be in the FPGA at all.
rhdl::prelude::cic_chain!(
    NarrowbandChain,
    fs = 125e6,
    decimate = 488,
    alias_free_bw = 64e3,
    in_w = 16,
    out_w = 24,
    ripple_db = 0.1,
    alias_db = 60,
    snr_db = 80,
);
// ANCHOR_END: macro

#[cfg(test)]
mod macro_tests {
    use super::*;

    /// The chapter states these numbers; if the designer changes its
    /// mind the chapter is wrong and this says so.
    #[test]
    fn the_macro_derives_what_the_chapter_claims() {
        assert_eq!(narrowband_chain::DECIMATE, 488);
        assert_eq!(narrowband_chain::SPLIT, [8, 61]);
        assert_eq!(narrowband_chain::TAPS.len(), 11);
        assert!(narrowband_chain::RIPPLE_DB <= 0.1);
        assert!(narrowband_chain::ALIAS_REJECTION_DB >= 60.0);
        assert!(narrowband_chain::SNR_DB >= 80.0);
    }

    #[test]
    fn the_emitted_chain_elaborates() -> miette::Result<()> {
        use rhdl::prelude::*;
        // Decimation alone...
        let uut = narrowband_chain::new();
        let _ = uut.descriptor("top".into())?;
        // ...and with the compensator behind it, if that is where you
        // want it.
        let comp = narrowband_chain::compensated();
        let _ = comp.descriptor("top".into())?;
        Ok(())
    }

    /// The chapter quotes both figures; they must stay true.
    #[test]
    fn the_droop_and_ripple_figures_are_as_documented() {
        assert!((narrowband_chain::DROOP_DB - -19.586).abs() < 0.01);
        assert!((narrowband_chain::RIPPLE_DB - 0.0689).abs() < 0.001);
    }
}
