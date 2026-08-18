#![warn(missing_docs)]
//! The NCO's **numeric contract** — one place for the coupled numbers
//! that decide what actually comes out of the instrument.
//!
//! Before this module those numbers lived in three unconnected places:
//! a `const F_CLK` local to a single test, a runtime parameter in
//! [`super::model`], and prose comments. Every headline claim in
//! `dsp::nco` is conditional on them, so scattering them meant a clock
//! change would silently invalidate the physics while the build stayed
//! green. Here, changing one breaks the build.
//!
//! | constant | value | decides |
//! |---|---|---|
//! | [`F_SAMPLE_HZ`] | 125 MHz | Hz per LSB of the frequency word |
//! | [`PHASE_W`] | 48 | frequency resolution, `F_SAMPLE_HZ / 2^48` |
//! | [`TOTAL_W`] | 22 | phase-truncation spur floor |
//! | [`AMP_W`] | 18 | internal amplitude width |
//! | [`DAC_W`] | 14 | what leaves the board |
//!
//! # Why the hardware stays unitless
//!
//! The widgets take a dimensionless phase increment, because division
//! does not belong in a datapath. The conversion lives here, as `const
//! fn`, so it is evaluated by rustc and costs nothing in emitted RTL.
//!
//! # Why µHz rather than `f64`
//!
//! `const fn` cannot do floating point on stable, and the resolution
//! ([`resolution_microhertz`]) is well under 1 Hz — so Hz as an integer
//! would quantise the very thing the 48-bit accumulator exists to
//! provide. Microhertz keeps the whole tuning range in `u128` with
//! room to spare.
//!
//! # The measured facts these constants encode
//!
//! - **14 bits at the DAC costs about 3 dB**, not the collapse one
//!   might expect: −111.9 dBc against −115.3 at 18 bits. Quantisation
//!   error spreads over all of Nyquist, so a 1 MHz analysis band sees a
//!   small fraction of it and the worst *discrete* spur sits far below
//!   total noise power. The DAC is not the bottleneck.
//! - **A wider output buys nothing on its own.** At 24 bits out the
//!   worst spur is −115.3 dBc — identical to 18 bits. −115 dBc is ≈19.1
//!   effective bits, so anything below bit ~19 is packaging.
//! - **Accuracy is set by the phase split, not the amplitude width.**
//!   Growing coarse/fine from 10/12 to 13/15 moves the floor −115.3 →
//!   −152.6 dBc, about **12.4 dB per coarse bit** — two bits of
//!   accuracy per coarse bit, which is the signature of an
//!   interpolation exact to second order. Raising [`AMP_W`] alone does
//!   nothing.
//!
//! So [`AMP_W`] = 18 is not a target, it is headroom: four bits above
//! what [`DAC_W`] can express, so downstream gain scaling and complex
//! modulation do not quantise hard.

pub use super::sin_cos_linear_interp::{AMP_W, TOTAL_W};

/// Sample clock, Hz. The Red Pitaya 125-14's 125 MHz.
pub const F_SAMPLE_HZ: u64 = 125_000_000;

/// Phase accumulator width. Sets frequency resolution.
pub const PHASE_W: usize = 48;

/// DAC width. The `-14` in Red Pitaya 125-14.
pub const DAC_W: usize = 14;

/// Microhertz per Hz, so the conversions read without magic numbers.
const UHZ_PER_HZ: u128 = 1_000_000;

/// Frequency in µHz → tuning word, rounded to nearest.
///
/// `word = round(f / F_SAMPLE_HZ · 2^PHASE_W)`
///
/// Rounded rather than truncated: truncation biases every commanded
/// frequency low, and a systematic offset is worse than a symmetric one
/// when phase is being accumulated over an entire experiment.
pub const fn tuning_word(microhertz: u128) -> u128 {
    let denom = F_SAMPLE_HZ as u128 * UHZ_PER_HZ;
    let num = microhertz * (1u128 << PHASE_W);
    (num + denom / 2) / denom
}

/// Tuning word → frequency in µHz. The inverse of [`tuning_word`],
/// to within one [`resolution_microhertz`].
pub const fn frequency_microhertz(word: u128) -> u128 {
    let denom = 1u128 << PHASE_W;
    let num = word * F_SAMPLE_HZ as u128 * UHZ_PER_HZ;
    (num + denom / 2) / denom
}

/// The smallest representable frequency step, µHz.
///
/// This — not the accumulator width in the abstract — is what has to be
/// small against the narrowest line the instrument must resolve.
pub const fn resolution_microhertz() -> u128 {
    frequency_microhertz(1)
}

/// Phase in millidegrees → phase word, rounded to nearest.
///
/// A full turn is `2^PHASE_W`, so 360 000 millidegrees maps onto the
/// whole accumulator range.
pub const fn phase_word(millidegrees: u128) -> u128 {
    let denom = 360_000u128;
    let num = (millidegrees % denom) * (1u128 << PHASE_W);
    ((num + denom / 2) / denom) % (1u128 << PHASE_W)
}

// ---------------------------------------------------------------------
// The claims, as build-time checks rather than prose.
// ---------------------------------------------------------------------

/// The narrowest line the instrument must resolve, µHz.
///
/// **Assumption — confirm against the application.** 0.1 Hz.
pub const NARROWEST_LINEWIDTH_UHZ: u128 = 100_000;

const _: () = assert!(
    resolution_microhertz() * 100 < NARROWEST_LINEWIDTH_UHZ,
    "frequency resolution is not a small fraction of the narrowest \
     linewidth: either PHASE_W is too small for this F_SAMPLE_HZ, or \
     the linewidth assumption needs revisiting"
);

const _: () = assert!(
    AMP_W > DAC_W,
    "AMP_W is headroom above the DAC so downstream gain scaling and \
     complex modulation do not quantise hard; at or below DAC_W there \
     is no headroom left"
);

const _: () = assert!(
    TOTAL_W < PHASE_W,
    "phase-to-amplitude consumes the TOP TOTAL_W bits of the \
     accumulator; if TOTAL_W reached PHASE_W there would be no \
     truncation and the spur analysis would not apply"
);

/// Bits discarded when the accumulator's phase enters
/// phase-to-amplitude.
///
/// **This truncation is the phase truncation the whole spur analysis is
/// about.** The composite must take the *top* [`TOTAL_W`] bits — taking
/// the bottom produces something that still looks like a waveform and
/// is entirely wrong.
pub const PHASE_TRUNCATION_BITS: usize = PHASE_W - TOTAL_W;

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip is exact to within one resolution step.
    #[test]
    fn tuning_word_round_trips() {
        for hz in [1u128, 1_000, 10_000_000, 12_345_678, 25_000_000] {
            let uhz = hz * UHZ_PER_HZ;
            let back = frequency_microhertz(tuning_word(uhz));
            let err = back.abs_diff(uhz);
            assert!(
                err <= resolution_microhertz(),
                "{hz} Hz round-tripped to {back} µHz, error {err} µHz \
                 exceeds one step of {} µHz",
                resolution_microhertz()
            );
        }
    }

    /// The resolution is what the docs claim, and it is what makes the
    /// 0.1 Hz stepping grid representable at all.
    #[test]
    fn resolution_is_sub_microhertz_scale() {
        let r = resolution_microhertz();
        assert!(r < 1, "expected sub-µHz resolution at 48 bits, got {r} µHz");
        // 0.1 Hz steps must be exactly representable to well under a step.
        let a = tuning_word(12_000_000 * UHZ_PER_HZ);
        let b = tuning_word(12_000_000 * UHZ_PER_HZ + 100_000);
        assert!(b > a, "a 0.1 Hz step must change the tuning word");
    }

    /// A phase word covers exactly one turn.
    #[test]
    fn phase_word_spans_one_turn() {
        assert_eq!(phase_word(0), 0);
        assert_eq!(
            phase_word(360_000),
            0,
            "a full turn is indistinguishable from none"
        );
        assert_eq!(phase_word(90_000), 1u128 << (PHASE_W - 2), "quarter turn");
        assert_eq!(phase_word(180_000), 1u128 << (PHASE_W - 1), "half turn");
    }

    /// The truncation the composite must perform, stated numerically so
    /// a width change is visible here rather than only in behaviour.
    #[test]
    fn phase_truncation_is_as_expected() {
        assert_eq!(PHASE_TRUNCATION_BITS, 26);
        assert_eq!(PHASE_W, 48);
        assert_eq!(TOTAL_W, 22);
    }
}
