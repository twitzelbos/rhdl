#![warn(missing_docs)]
//! Digital up-conversion — a complex envelope onto a carrier.
//!
//! The transmit mirror of [`crate::dsp::ddc`]. A baseband envelope
//! arrives at a rate the host can generate — a megasample per second or
//! less — and has to leave at the converter's rate, modulated onto a
//! carrier, with the images the upsampling creates suppressed.
//!
//! ```text
//!   env @ f_lo --> [ interpolate x R ] --> @ f_hi --> x carrier --> out
//! ```
//!
//! Three widgets:
//!
//! - [`EnvelopeUpsampler`] — the shared front end. Splits the complex
//!   envelope, interpolates each arm with an identical CIC, recombines.
//! - [`IqDuc`] — upsampler plus oscillator plus a full complex mixer.
//!   Emits `Iq`, for a quadrature DAC or an external I/Q modulator.
//!   Four multiplies.
//! - [`RealDuc`] — the same, with
//!   [`crate::dsp::mixer::RealPartMixer`]. Emits `Real`, for a single
//!   DAC. Two multiplies.
//!
//! # Which one
//!
//! [`RealDuc`] if a single converter carries the signal, which is the
//! usual case and the cheaper one. [`IqDuc`] if the passband is formed
//! outside the FPGA — a quadrature modulator, or an I/Q pair going to a
//! transceiver — where both components have to leave the chip.
//!
//! They are separate widgets rather than one with a flag because the
//! multiplier count differs and [`crate::dsp::mixer`] records why that
//! makes them separate widgets: `if`/`else` in a kernel lowers to a mux
//! whose *both* arms evaluate, so a flag would emit four multiplies
//! either way and the saving would exist only in the documentation.
//!
//! # What is shared with the down-converter, and what is not
//!
//! Shared: the oscillator is the same [`crate::dsp::nco`] composite, the
//! framing is the same [`crate::dsp::sync::SyncMark`], both arms of the
//! rate change are the same type for the same reason, and neither chain
//! normalises the CIC's gain.
//!
//! Not shared, and the differences are all consequences of direction:
//!
//! | | down-converter | up-converter |
//! |---|---|---|
//! | mixing happens | first | last |
//! | rate change | decimate, after mixing | interpolate, before mixing |
//! | `ready` upstream | pass-through | a real once-per-`R` request |
//! | output cadence | one cycle in `R` | every cycle |
//! | pruning | Hogenauer §V applies | it does not — see [`crate::dsp::cic::interp`] |
//! | width tapering | costs noise | costs nothing |
//!
//! The `ready` row is the one that changes how a chain is assembled. A
//! down-converter is pushed: samples arrive from a converter and the
//! chain keeps up or reports an overrun. An up-converter *pulls* — it
//! asks for an envelope sample once every `R` cycles — so whatever
//! generates the envelope has to answer that request, and a host DMA
//! feeding it needs a FIFO in between rather than a fixed schedule.
//!
//! # A worked sizing: 1 Msps envelope onto a 125 Msps carrier
//!
//! The case these widgets were written for, with every parameter
//! derived rather than guessed. A 16-bit complex envelope at one
//! megasample per second, three CIC stages, unit differential delay,
//! out to a 14-bit DAC at 125 megasamples.
//!
//! ```text
//!   W       = 16      envelope width per component
//!   S       = 3       CIC stages
//!   M       = 1       differential delay
//!   R_MAX   = 125     125 Msps / 1 Msps
//!   WA      = 30      = W + interp::gain_bits(3, 125, 1) = 16 + 14
//!   CW      = 7       = interp::rate_width(125)
//!   OW      = 14      the DAC
//!   PROD_W  = 49      = WA + AMP_W + 1 = 30 + 18 + 1
//!   DROP    = 35      = PROD_W - OW
//! ```
//!
//! The gain a caller has to undo is `(R·M)^N / R = 125^2 = 15625`, which
//! [`crate::dsp::cic::interp::dc_gain_ratio`] reports as the exact ratio
//! `1953125/125`.
//!
//! `R_MAX = 125` sizes the widths, and any smaller rate then works
//! unchanged — 500 ksps at `R = 250` would not, and would need a
//! rebuild. Mark the first sample at each new rate; see below.
//!
//! **What the taper saves.** The uniform-width interpolator in this
//! configuration spends 270 bits of state per arm. Tapered to each
//! stage's own growth bound the exact figure is 180 — widths
//! `17, 18, 19, 18, 24, 30` — and a generated
//! [`crate::cic_interp_tapered!`] widget spends **181**, because it
//! lifts the non-monotonic fourth stage to the running maximum so that
//! every inter-stage transfer is a widening. Either way a 33% saving,
//! and *losslessly*: an interpolator's taper injects no error at all, so
//! the tapered widget is **bit-identical** to
//! [`crate::dsp::cic::interpolator`]'s uniform form rather than merely
//! close to it. [`crate::dsp::cic::interp`] carries the argument and
//! `tests/cic_interp_tapered.rs` asserts the equality.
//! `the_worked_sizing_is_what_the_docs_say` pins these numbers.
//!
//! # The rate is a run-time input
//!
//! Set once at build time by `R_MAX`, chosen at run time up to that.
//! [`crate::dsp::cic::interpolator`] carries the analysis; two
//! consequences reach a caller of this module:
//!
//! - **The gain moves with the rate**, and nothing here normalises it.
//!   [`crate::dsp::cic::interp::dc_gain_ratio`] reports the factor.
//! - **A rate change wants a mark with it.** Changing the rate alone
//!   leaves the output at the old rate's amplitude, because the comb
//!   section feeds the integrators the `N`-th difference of the
//!   envelope and that is zero for a steady one. Marking the first
//!   sample at the new rate clears the cascade so the new gain
//!   establishes itself.

pub mod iq;
pub mod real;
pub mod upsampler;

pub use iq::IqDuc;
pub use real::RealDuc;
pub use upsampler::EnvelopeUpsampler;
